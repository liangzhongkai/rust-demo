//! # HFT 生产场景下的文件 I/O
//!
//! 高频交易的文件硬约束：
//! - **延迟**：热路径禁止阻塞 fsync；回放用 mmap 零拷贝
//! - **正确**：订单日志必须可恢复；配置热更新不能读到半写文件
//! - **吞吐**：分片归档、批量 fsync、预分配避免碎片
//!
//! 下面 7 个场景对应行情回放、订单日志、风控配置、灾备恢复等真实写法。

#![allow(dead_code)]

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub type SeqNum = u64;
pub type Px = i64;

// ============================================================================
// 场景 1：mmap 行情 tape 零拷贝回放
// ============================================================================
/// **生产问题**：回测/复盘要扫亿级定长 tick，不能 `read_to_end` 再 parse。
///
/// **套路**：文件映射为 `&[u8]`，按 record stride 切片，热循环无堆分配。
pub mod mmap_tick_tape {
    use super::*;

    const RECORD: usize = 16;

    #[derive(Debug, Clone, Copy)]
    pub struct Tick {
        pub seq: SeqNum,
        pub px: Px,
    }

    pub struct TapeView<'a> {
        data: &'a [u8],
    }

    impl<'a> TapeView<'a> {
        pub fn open(data: &'a [u8]) -> io::Result<Self> {
            Ok(Self { data })
        }

        pub fn len(&self) -> usize {
            self.data.len() / RECORD
        }

        pub fn get(&self, i: usize) -> Option<Tick> {
            let off = i.checked_mul(RECORD)?;
            let chunk = self.data.get(off..off + RECORD)?;
            Some(Tick {
                seq: u64::from_le_bytes(chunk[0..8].try_into().ok()?),
                px: i64::from_le_bytes(chunk[8..16].try_into().ok()?),
            })
        }

        pub fn iter(&self) -> impl Iterator<Item = Tick> + '_ {
            (0..self.len()).filter_map(|i| self.get(i))
        }
    }

    pub fn demonstrate() {
        println!("## 场景 1：mmap 行情 tape 零拷贝回放");
        let mut raw = Vec::new();
        for i in 0..3 {
            raw.extend_from_slice(&(i as u64).to_le_bytes());
            raw.extend_from_slice(&(100 + i as i64).to_le_bytes());
        }
        let tape = TapeView::open(&raw).unwrap();
        let last = tape.get(2).unwrap();
        println!("记录数={} 最后 seq={} px={}", tape.len(), last.seq, last.px);
        println!("关键：真实环境用 memmap2 + madvise(Sequential)；解析层只拿 &[u8] 视图\n");
    }
}

// ============================================================================
// 场景 2：订单 AOF 日志 —— fsync 策略
// ============================================================================
/// **生产问题**：每笔订单都要落盘，但每条 fsync 在 NVMe 上也要 ~10µs+。
///
/// **套路**：append-only + 可配置 `EveryN` / group commit；崩溃窗口可量化。
pub mod order_journal_aof {
    use super::*;

    #[derive(Debug, Clone, Copy)]
    pub enum Durability {
        PerRecord,
        Batch(u32),
    }

    pub struct AofJournal {
        writer: BufWriter<File>,
        pub records: u64,
        pub fsyncs: u32,
        policy: Durability,
        pending: u32,
    }

    impl AofJournal {
        pub fn open(path: &Path, policy: Durability) -> io::Result<Self> {
            let f = OpenOptions::new().create(true).append(true).open(path)?;
            Ok(Self {
                writer: BufWriter::new(f),
                records: 0,
                fsyncs: 0,
                policy,
                pending: 0,
            })
        }

        pub fn append_order(&mut self, cl_ord_id: &str, px: Px) -> io::Result<()> {
            writeln!(self.writer, "{cl_ord_id},{px}")?;
            self.records += 1;
            self.pending += 1;
            match self.policy {
                Durability::PerRecord => self.flush_and_sync()?,
                Durability::Batch(n) if self.pending >= n => self.flush_and_sync()?,
                _ => {}
            }
            Ok(())
        }

        fn flush_and_sync(&mut self) -> io::Result<()> {
            self.writer.flush()?;
            self.writer.get_ref().sync_data()?;
            self.fsyncs += 1;
            self.pending = 0;
            Ok(())
        }
    }

    pub fn demonstrate() {
        println!("## 场景 2：订单 AOF 日志");
        let dir = std::env::temp_dir().join("file-io-hft-aof");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("orders.aof");
        let mut j = AofJournal::open(&path, Durability::Batch(2)).unwrap();
        j.append_order("A1", 100).unwrap();
        j.append_order("A2", 101).unwrap();
        j.append_order("A3", 102).unwrap();
        println!(
            "3 笔订单, Batch(2) → records={} fsyncs={}",
            j.records, j.fsyncs
        );
        println!("关键：量化崩溃窗口 = 未 fsync 的 batch；监管场景用 PerRecord\n");
    }
}

// ============================================================================
// 场景 3：内存 ring 满时 spill 到分片文件
// ============================================================================
/// **生产问题**：突发行情把 in-memory ring 打满，不能直接丢 tick。
///
/// **套路**：批量 spill 到 `.seg` 文件，消费者按序回放 seg + ring。
pub mod ring_spill_segments {
    use super::*;

    pub struct SpillWriter {
        dir: PathBuf,
        seg_id: u64,
        pub spilled_bytes: u64,
    }

    impl SpillWriter {
        pub fn new(dir: PathBuf) -> Self {
            Self {
                dir,
                seg_id: 0,
                spilled_bytes: 0,
            }
        }

        pub fn spill(&mut self, batch: &[u8]) -> io::Result<()> {
            let path = self.dir.join(format!("spill_{:06}.seg", self.seg_id));
            let mut f = File::create(&path)?;
            f.write_all(batch)?;
            self.spilled_bytes += batch.len() as u64;
            self.seg_id += 1;
            Ok(())
        }

        pub fn replay_order(&self) -> Vec<String> {
            let mut names: Vec<_> = fs::read_dir(&self.dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with("spill_"))
                .collect();
            names.sort();
            names
        }
    }

    pub fn demonstrate() {
        println!("## 场景 3：ring spill 分片");
        let dir = std::env::temp_dir().join("file-io-hft-spill");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut w = SpillWriter::new(dir.clone());
        w.spill(b"tick_batch_1").unwrap();
        w.spill(b"tick_batch_2").unwrap();
        println!("spill 顺序 = {:?}", w.replay_order());
        println!("关键：seg 只追加；ring 水位恢复后停写；metric 盯 spill 速率\n");
    }
}

// ============================================================================
// 场景 4：参考数据快照 —— 原子读
// ============================================================================
/// **生产问题**：盘中更新 symbol 列表，读者不能读到写了一半的 JSON/CSV。
///
/// **套路**：写 `ref.json.tmp` → fsync → `rename` 覆盖；读者只 `open` 正式名。
pub mod ref_data_snapshot {
    use super::*;

    pub fn publish_atomic(dir: &Path, content: &str) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        let final_path = dir.join("ref_data.json");
        let tmp_path = dir.join("ref_data.json.tmp");
        {
            let mut f = File::create(&tmp_path)?;
            f.write_all(content.as_bytes())?;
            f.sync_all()?;
        }
        fs::rename(tmp_path, final_path)?;
        Ok(())
    }

    pub fn load(dir: &Path) -> io::Result<String> {
        let p = dir.join("ref_data.json");
        let mut s = String::new();
        File::open(p)?.read_to_string(&mut s)?;
        Ok(s)
    }

    pub fn demonstrate() {
        println!("## 场景 4：参考数据原子快照");
        let dir = std::env::temp_dir().join("file-io-hft-ref");
        let _ = fs::remove_dir_all(&dir);
        publish_atomic(&dir, r#"{"symbols":["AAPL","MSFT"]}"#).unwrap();
        let data = load(&dir).unwrap();
        println!("加载 = {}", data);
        println!("关键：读者无锁；写者单写；Linux rename 同目录原子\n");
    }
}

// ============================================================================
// 场景 5：WAL + checkpoint 崩溃恢复
// ============================================================================
/// **生产问题**：进程 crash 后要从最近 checkpoint + WAL 重放恢复持仓/seq。
///
/// **套路**：周期性写 checkpoint 文件；WAL 只追加；启动时 `checkpoint + replay`。
pub mod wal_checkpoint {
    use super::*;

    #[derive(Debug, Default)]
    pub struct State {
        pub position: i64,
        pub last_seq: SeqNum,
    }

    pub struct Wal {
        path: PathBuf,
    }

    impl Wal {
        pub fn new(path: PathBuf) -> Self {
            Self { path }
        }

        pub fn append(&self, seq: SeqNum, delta: i64) -> io::Result<()> {
            let mut f = OpenOptions::new().create(true).append(true).open(&self.path)?;
            writeln!(f, "{seq},{delta}")?;
            Ok(())
        }

        pub fn replay(&self, mut st: State) -> io::Result<State> {
            let content = fs::read_to_string(&self.path).unwrap_or_default();
            for line in content.lines() {
                let mut it = line.split(',');
                let seq: SeqNum = it.next().unwrap().parse().unwrap();
                let delta: i64 = it.next().unwrap().parse().unwrap();
                st.last_seq = seq;
                st.position += delta;
            }
            Ok(st)
        }
    }

    pub fn write_checkpoint(path: &Path, st: &State) -> io::Result<()> {
        let tmp = path.with_extension("tmp");
        let mut f = File::create(&tmp)?;
        writeln!(f, "{},{}", st.position, st.last_seq)?;
        f.sync_all()?;
        fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn demonstrate() {
        println!("## 场景 5：WAL + checkpoint");
        let dir = std::env::temp_dir().join("file-io-hft-wal");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let wal = Wal::new(dir.join("wal.log"));
        wal.append(1, 10).unwrap();
        wal.append(2, -3).unwrap();
        let st = wal.replay(State::default()).unwrap();
        write_checkpoint(&dir.join("ckpt.bin"), &st).unwrap();
        println!("恢复 position={} last_seq={}", st.position, st.last_seq);
        println!("关键：checkpoint 后 truncate WAL；fsync 顺序：数据 → checkpoint\n");
    }
}

// ============================================================================
// 场景 6：风控配置热更新
// ============================================================================
/// **生产问题**：风控限额盘中调整，不能重启进程；也不能读到半写配置。
///
/// **套路**：mtime 轮询 / inotify + 原子替换；版本号单调递增拒绝回滚。
pub mod config_hot_reload {
    use super::*;
    use std::time::SystemTime;

    #[derive(Debug, Clone)]
    pub struct RiskConfig {
        pub version: u64,
        pub max_notional: i64,
    }

    pub struct Reloader {
        path: PathBuf,
        last_mtime: Option<SystemTime>,
        pub cfg: RiskConfig,
    }

    impl Reloader {
        pub fn new(path: PathBuf, cfg: RiskConfig) -> Self {
            Self {
                path,
                last_mtime: None,
                cfg,
            }
        }

        pub fn maybe_reload(&mut self) -> io::Result<bool> {
            let meta = fs::metadata(&self.path)?;
            let mtime = meta.modified()?;
            if self.last_mtime == Some(mtime) {
                return Ok(false);
            }
            self.last_mtime = Some(mtime);
            let text = fs::read_to_string(&self.path)?;
            let mut it = text.trim().split(',');
            let version: u64 = it.next().unwrap().parse().unwrap();
            let max_notional: i64 = it.next().unwrap().parse().unwrap();
            if version < self.cfg.version {
                return Ok(false); // 拒绝回滚
            }
            self.cfg = RiskConfig { version, max_notional };
            Ok(true)
        }
    }

    pub fn demonstrate() {
        println!("## 场景 6：风控配置热更新");
        let dir = std::env::temp_dir().join("file-io-hft-risk");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("risk.cfg");
        fs::write(&path, "1,1000000").unwrap();
        let mut r = Reloader::new(path.clone(), RiskConfig { version: 1, max_notional: 1_000_000 });
        fs::write(&path, "2,2000000").unwrap();
        assert!(r.maybe_reload().unwrap());
        println!("新版本 v{} max={}", r.cfg.version, r.cfg.max_notional);
        println!("关键：版本号防回滚；reload 失败保留旧 cfg；metric 记录 reload 次数\n");
    }
}

// ============================================================================
// 场景 7：按日期/合约分区的 tick 归档 + tail 续读
// ============================================================================
/// **生产问题**：合规要求保留原始 tick；单文件过大无法高效 tail/上传。
///
/// **套路**：`{date}/{symbol}.bin` 分区；sidecar `.offset` 记录续读位置。
pub mod partitioned_archive {
    use super::*;

    pub fn append_tick(dir: &Path, date: &str, symbol: &str, payload: &[u8]) -> io::Result<()> {
        let d = dir.join(date);
        fs::create_dir_all(&d)?;
        let path = d.join(format!("{symbol}.bin"));
        let mut f = OpenOptions::new().create(true).append(true).open(path)?;
        f.write_all(payload)?;
        Ok(())
    }

    pub fn save_offset(dir: &Path, symbol: &str, off: u64) -> io::Result<()> {
        fs::write(dir.join(format!("{symbol}.offset")), off.to_string())?;
        Ok(())
    }

    pub fn tail_read(path: &Path, offset: u64) -> io::Result<Vec<u8>> {
        let mut f = File::open(path)?;
        f.seek(SeekFrom::Start(offset))?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        Ok(buf)
    }

    pub fn demonstrate() {
        println!("## 场景 7：分区 tick 归档");
        let dir = std::env::temp_dir().join("file-io-hft-archive");
        let _ = fs::remove_dir_all(&dir);
        append_tick(&dir, "2025-06-06", "ES", b"tick1").unwrap();
        append_tick(&dir, "2025-06-06", "ES", b"tick2").unwrap();
        save_offset(&dir, "ES", 5).unwrap();
        let path = dir.join("2025-06-06/ES.bin");
        let tail = tail_read(&path, 5).unwrap();
        println!("offset=5 之后 = {:?}", std::str::from_utf8(&tail).unwrap());
        println!("关键：按日滚动；对象存储上传整目录；offset 与 bin 分开存\n");
    }
}

pub fn demonstrate() {
    mmap_tick_tape::demonstrate();
    order_journal_aof::demonstrate();
    ring_spill_segments::demonstrate();
    ref_data_snapshot::demonstrate();
    wal_checkpoint::demonstrate();
    config_hot_reload::demonstrate();
    partitioned_archive::demonstrate();
}
