//! # 文件 I/O 底层机制
//!
//! 这一节回答四个根本问题，后面 HFT/Web3 章节都建立在这之上：
//!
//! 1. **顺序 vs 随机**：`Read` 流式消费 vs `seek` 随机定位
//! 2. **缓冲**：为什么生产代码几乎总是 `BufReader` / `BufWriter`
//! 3. **持久化语义**：`write` ≠ 落盘；`sync_all` / `rename` 决定崩溃后看到什么
//! 4. **分层**：字节 I/O、framing、业务状态必须拆开；文件只是 transport

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Cursor, Read, Seek, SeekFrom};

// ============================================================================
// 1. 顺序读 vs 随机定位
// ============================================================================
pub mod sequential_vs_random {

    /// 顺序：适合日志回放、流式解析、tail -f
    pub fn sequential_sum(data: &[u8]) -> u64 {
        data.iter().map(|&b| b as u64).sum()
    }

    /// 随机：适合 mmap 索引表、checkpoint 跳转、分片归档 seek
    pub fn random_read_nth(data: &[u8], record_size: usize, n: usize) -> Option<u8> {
        let off = n.checked_mul(record_size)?;
        data.get(off).copied()
    }

    pub fn demonstrate() {
        println!("## 1. 顺序读 vs 随机定位");
        let tape = b"ABCDE";
        println!("顺序 checksum = {}", sequential_sum(tape));
        println!(
            "随机第 3 条(定长1) = {:?}",
            random_read_nth(tape, 1, 2).map(|b| b as char)
        );
        println!("HFT：mmap 定长 tick 表 | Web3：按 block 偏移 seek 快照\n");
    }
}

// ============================================================================
// 2. 缓冲 I/O
// ============================================================================
pub mod buffering {
    use super::*;

    pub fn unbuffered_byte_reads(data: &[u8]) -> usize {
        // 模拟：每次 read(1) 都是一次逻辑 syscall
        let mut r = Cursor::new(data);
        let mut buf = [0u8; 1];
        let mut syscalls = 0;
        while r.read(&mut buf).unwrap_or(0) > 0 {
            syscalls += 1;
        }
        syscalls
    }

    pub fn buffered_line_reads(data: &[u8]) -> usize {
        let mut r = BufReader::new(Cursor::new(data));
        let mut lines = 0;
        let mut line = String::new();
        while r.read_line(&mut line).unwrap_or(0) > 0 {
            lines += 1;
            line.clear();
        }
        lines
    }

    pub fn demonstrate() {
        println!("## 2. 缓冲 I/O");
        let log = b"tick\nfill\ncancel\n";
        println!("逐字节 read 次数 = {}", unbuffered_byte_reads(log));
        println!("BufReader 行数 = {}", buffered_line_reads(log));
        println!("规则：热路径用定长帧 + 大缓冲；文本日志用 BufReader\n");
    }
}

// ============================================================================
// 3. 持久化语义：write / flush / sync / rename
// ============================================================================
pub mod durability_semantics {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum FsyncPolicy {
        /// 每条记录 fsync —— 最安全，延迟最高
        Always,
        /// 每 N 条批量 fsync —— HFT 订单日志常见折中
        EveryN(u32),
        /// 只靠 OS page cache —— 崩溃可能丢最近写入
        OsDefault,
    }

    pub struct JournalWriter {
        pub records: usize,
        pub fsync_calls: u32,
        policy: FsyncPolicy,
        since_fsync: u32,
    }

    impl JournalWriter {
        pub fn new(policy: FsyncPolicy) -> Self {
            Self {
                records: 0,
                fsync_calls: 0,
                policy,
                since_fsync: 0,
            }
        }

        pub fn append(&mut self, _payload: &[u8]) {
            self.records += 1;
            self.since_fsync += 1;
            match self.policy {
                FsyncPolicy::Always => self.fsync(),
                FsyncPolicy::EveryN(n) if self.since_fsync >= n => self.fsync(),
                _ => {}
            }
        }

        fn fsync(&mut self) {
            self.fsync_calls += 1;
            self.since_fsync = 0;
        }
    }

    pub fn demonstrate() {
        println!("## 3. 持久化语义");
        let mut j = JournalWriter::new(FsyncPolicy::EveryN(3));
        for _ in 0..7 {
            j.append(b"order");
        }
        println!(
            "7 条记录, EveryN(3) → fsync {} 次",
            j.fsync_calls
        );
        println!("崩溃窗口：OsDefault 丢最近写；Always 最安全但慢");
        println!("原子替换：write tmp → fsync → rename（读者只见完整文件）\n");
    }
}

// ============================================================================
// 4. 分层：transport → framing → parse → state
// ============================================================================
pub mod layered_io {
    use super::*;

    #[derive(Debug)]
    pub struct Tick {
        pub seq: u64,
        pub px: i64,
    }

    pub fn read_loop(mut r: impl Read) -> Vec<Tick> {
        let mut buf = Vec::new();
        r.read_to_end(&mut buf).unwrap();
        buf.chunks_exact(16)
            .map(|chunk| Tick {
                seq: u64::from_le_bytes(chunk[0..8].try_into().unwrap()),
                px: i64::from_le_bytes(chunk[8..16].try_into().unwrap()),
            })
            .collect()
    }

    pub fn demonstrate() {
        println!("## 4. 分层 I/O");
        let mut wire = Vec::new();
        for (seq, px) in [(1u64, 100i64), (2u64, 101i64)] {
            wire.extend_from_slice(&seq.to_le_bytes());
            wire.extend_from_slice(&px.to_le_bytes());
        }
        let ticks = read_loop(Cursor::new(&wire));
        println!("解析 tick 数 = {}", ticks.len());
        println!("文件层只负责字节；定长 framing 在上层；业务状态再上一层\n");
    }
}

// ============================================================================
// 5. Seek 与偏移管理
// ============================================================================
pub mod seek_cursor {
    use super::*;

    pub fn tail_from_offset(data: &[u8], offset: u64) -> &[u8] {
        let mut c = Cursor::new(data);
        c.seek(SeekFrom::Start(offset)).unwrap();
        let pos = c.position() as usize;
        &data[pos..]
    }

    pub fn demonstrate() {
        println!("## 5. Seek 与游标");
        let log = b"AAAA BBBB CCCC";
        let tail = tail_from_offset(log, 5);
        println!("offset=5 之后 = {:?}", std::str::from_utf8(tail).unwrap());
        println!("HFT：crash 后从 checkpoint offset 续读 | Web3：快照 + 增量日志\n");
    }
}

pub fn demonstrate() {
    sequential_vs_random::demonstrate();
    buffering::demonstrate();
    durability_semantics::demonstrate();
    layered_io::demonstrate();
    seek_cursor::demonstrate();
}
