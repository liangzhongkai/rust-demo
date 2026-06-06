//! # 泛化：从 HFT/Web3 场景到通用文件 I/O 策略
//!
//! | 问题类型           | 标志特征                     | 首选套路                          |
//! |--------------------|------------------------------|-----------------------------------|
//! | 1. 零拷贝读        | 大文件、随机/顺序扫          | mmap / &[u8] 视图 + 定长 stride   |
//! | 2. 流式处理        | 日志/文本/未知大小           | BufReader + 行/帧迭代             |
//! | 3. 原子发布        | 配置/快照读者无锁            | write tmp → fsync → rename        |
//! | 4. 追加日志        | 审计、WAL、event log         | append-only + fsync 策略          |
//! | 5. 分片归档        | 单文件过大、按时间/upload      | 目录分区 + sidecar offset         |
//! | 6. 崩溃恢复        | 状态机、订单、indexer        | checkpoint + WAL replay           |
//! | 7. 热更新          | 盘中调参、多链 RPC           | mtime/inotify + 版本号防回滚      |
//! | 8. 可观测性        | 「写盘慢/丢数据」            | bytes/fsync/latency/ENOSPC 指标   |

#![allow(dead_code)]

// ============================================================================
// 策略 1：零拷贝 / 定长 stride 读
// ============================================================================
pub mod zero_copy_read {
    pub fn stride_records(data: &[u8], stride: usize) -> usize {
        data.len() / stride
    }

    pub fn get_record<'a>(data: &'a [u8], stride: usize, i: usize) -> Option<&'a [u8]> {
        let off = i.checked_mul(stride)?;
        data.get(off..off + stride)
    }

    pub fn demonstrate() {
        println!("## 策略 1：零拷贝定长读");
        let tape: Vec<u8> = (0..4u32).flat_map(|n| n.to_le_bytes()).collect();
        println!("记录数 = {}", stride_records(&tape, 4));
        println!("第 2 条 = {:?}", get_record(&tape, 4, 2));
        println!("HFT: mmap_tick_tape | Web3: chain_snapshot 数据区\n");
    }
}

// ============================================================================
// 策略 2：流式 BufReader
// ============================================================================
pub mod streaming_reader {
    use std::io::{self, BufRead, BufReader, Cursor};
    pub fn count_nonempty_lines(data: &[u8]) -> io::Result<usize> {
        let r = BufReader::new(Cursor::new(data));
        Ok(r.lines().filter_map(|l| l.ok()).filter(|l| !l.is_empty()).count())
    }

    pub fn demonstrate() {
        println!("## 策略 2：流式 BufReader");
        let log = b"ev1\n\nev2\n";
        println!("非空行 = {}", count_nonempty_lines(log).unwrap());
        println!("HFT: spill seg 回放 | Web3: indexer 行级重放\n");
    }
}

// ============================================================================
// 策略 3：write-rename 原子发布
// ============================================================================
pub mod atomic_publish {
    use std::fs::{self, File};
    use std::io;
    use std::path::Path;
    pub fn publish(path: &Path, content: &str) -> io::Result<()> {
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, content)?;
        File::open(&tmp)?.sync_all()?;
        fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn demonstrate() {
        println!("## 策略 3：原子发布");
        let dir = std::env::temp_dir().join("file-io-strategy-atomic");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("cfg.json");
        publish(&p, r#"{"ok":true}"#).unwrap();
        println!("发布完成 size = {} 字节", fs::metadata(p).unwrap().len());
        println!("HFT: ref_data_snapshot | Web3: multichain_config / manifest\n");
    }
}

// ============================================================================
// 策略 4：append-only + fsync 策略
// ============================================================================
pub mod append_only_journal {
    #[derive(Debug, Clone, Copy)]
    pub enum FsyncMode {
        Always,
        Every(u32),
    }

    pub struct Journal {
        pub entries: u32,
        pub fsyncs: u32,
        mode: FsyncMode,
        since: u32,
    }

    impl Journal {
        pub fn new(mode: FsyncMode) -> Self {
            Self {
                entries: 0,
                fsyncs: 0,
                mode,
                since: 0,
            }
        }

        pub fn append(&mut self) {
            self.entries += 1;
            self.since += 1;
            match self.mode {
                FsyncMode::Always => self.sync(),
                FsyncMode::Every(n) if self.since >= n => self.sync(),
                _ => {}
            }
        }

        fn sync(&mut self) {
            self.fsyncs += 1;
            self.since = 0;
        }
    }

    pub fn demonstrate() {
        println!("## 策略 4：append-only 日志");
        let mut j = Journal::new(FsyncMode::Every(5));
        for _ in 0..12 {
            j.append();
        }
        println!("12 条 → fsync {} 次", j.fsyncs);
        println!("HFT: order_journal_aof | Web3: indexer_event_log\n");
    }
}

// ============================================================================
// 策略 5：分片 + sidecar offset
// ============================================================================
pub mod partitioned_files {
    use std::path::PathBuf;

    pub struct PartitionLayout {
        pub root: PathBuf,
    }

    impl PartitionLayout {
        pub fn tick_path(&self, date: &str, symbol: &str) -> PathBuf {
            self.root.join(date).join(format!("{symbol}.bin"))
        }

        pub fn offset_path(&self, symbol: &str) -> PathBuf {
            self.root.join(format!("{symbol}.offset"))
        }
    }

    pub fn demonstrate() {
        println!("## 策略 5：分片 + offset");
        let layout = PartitionLayout {
            root: PathBuf::from("/data/ticks"),
        };
        println!("tick = {:?}", layout.tick_path("2025-06-06", "ES"));
        println!("offset = {:?}", layout.offset_path("ES"));
        println!("HFT: partitioned_archive | Web3: 按 block 范围分目录\n");
    }
}

// ============================================================================
// 策略 6：checkpoint + WAL 恢复
// ============================================================================
pub mod checkpoint_wal {
    #[derive(Debug, Default)]
    pub struct State {
        pub value: i64,
    }

    pub fn replay(wal: &[&str], mut st: State) -> State {
        for line in wal {
            let delta: i64 = line.parse().unwrap();
            st.value += delta;
        }
        st
    }

    pub fn demonstrate() {
        println!("## 策略 6：checkpoint + WAL");
        let st = replay(&["10", "-3", "7"], State::default());
        println!("重放后 value = {}", st.value);
        println!("HFT: wal_checkpoint | Web3: snapshot 后增量追块\n");
    }
}

// ============================================================================
// 策略 7：热更新 + 版本防回滚
// ============================================================================
pub mod hot_reload_versioned {
    pub fn accept_new(old_ver: u64, new_ver: u64) -> bool {
        new_ver > old_ver
    }

    pub fn demonstrate() {
        println!("## 策略 7：版本化热更新");
        println!("v2→v3 接受 = {}", accept_new(2, 3));
        println!("v3→v2 拒绝 = {}", accept_new(3, 2));
        println!("HFT: config_hot_reload | Web3: reorg_safe_cursor\n");
    }
}

// ============================================================================
// 策略 8：可观测性指标
// ============================================================================
pub mod io_metrics {
    #[derive(Debug, Default)]
    pub struct Metrics {
        pub bytes_written: u64,
        pub fsync_latency_us: u64,
        pub enospc_errors: u64,
        pub reload_count: u64,
    }

    impl Metrics {
        pub fn record_write(&mut self, n: usize, fsync_us: u64) {
            self.bytes_written += n as u64;
            self.fsync_latency_us = fsync_us;
        }
    }

    pub fn demonstrate() {
        println!("## 策略 8：I/O 可观测性");
        let mut m = Metrics::default();
        m.record_write(4096, 120);
        println!(
            "bytes={} last_fsync_us={}",
            m.bytes_written, m.fsync_latency_us
        );
        println!("必盯：fsync P99、spill 速率、ENOSPC、reorg 回滚条数\n");
    }
}

// ============================================================================
// 反例：错误做法速查
// ============================================================================
pub mod anti_patterns {
    pub fn list() -> [&'static str; 4] {
        [
            "大快照 read_to_end → OOM",
            "配置原地 truncate 写 → 半写 crash",
            "async worker 里 sync fsync → 阻塞",
            "无 ENOSPC 处理 → 静默丢 WAL",
        ]
    }

    pub fn demonstrate() {
        println!("## 反例速查");
        for (i, a) in list().iter().enumerate() {
            println!("  {}. {}", i + 1, a);
        }
        println!();
    }
}

pub fn demonstrate() {
    zero_copy_read::demonstrate();
    streaming_reader::demonstrate();
    atomic_publish::demonstrate();
    append_only_journal::demonstrate();
    partitioned_files::demonstrate();
    checkpoint_wal::demonstrate();
    hot_reload_versioned::demonstrate();
    io_metrics::demonstrate();
    anti_patterns::demonstrate();
}
