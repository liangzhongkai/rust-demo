//! # 文件 I/O 常见陷阱与诊断
//!
//! 生产事故里反复出现的 8 个文件陷阱：
//! - 现象（监控/日志里看到什么）
//! - 根因（内核/应用层面发生了什么）
//! - 修法（一行改法 + 风格预防）

#![allow(dead_code)]

// ============================================================================
// 陷阱 1：整文件 read_to_string / read_to_end 导致 OOM
// ============================================================================
pub mod read_entire_file_oom {
    use std::fs;
    use std::io::{self, BufRead, BufReader, Cursor};
    use std::path::Path;
    pub fn bad_load(path: &Path) -> io::Result<String> {
        // ❌ 10GB 日志/快照直接进内存
        fs::read_to_string(path)
    }

    pub fn good_line_count(data: &[u8]) -> usize {
        // ✅ 流式处理，内存恒定
        BufReader::new(Cursor::new(data))
            .lines()
            .count()
    }

    pub fn demonstrate() {
        println!("## 陷阱 1：整文件读入 OOM");
        let data = b"a\nb\nc\n";
        println!("流式行数 = {}", good_line_count(data));
        println!("现象：RSS 随文件大小线性涨 → OOM kill");
        println!("规则：大文件用 BufReader / mmap；只在确认小文件时 read_to_end\n");
    }
}

// ============================================================================
// 陷阱 2：无缓冲逐字节 read
// ============================================================================
pub mod unbuffered_reads {
    use std::io::{self, BufRead, BufReader, Cursor, Read};
    pub fn bad_count(mut r: impl Read) -> io::Result<usize> {
        let mut n = 0;
        let mut b = [0u8; 1];
        while r.read(&mut b)? > 0 {
            n += 1;
        }
        Ok(n)
    }

    pub fn good_count(data: &[u8]) -> usize {
        BufReader::new(Cursor::new(data)).fill_buf().unwrap().len()
    }

    pub fn demonstrate() {
        println!("## 陷阱 2：无缓冲逐字节 read");
        let data = b"hello";
        println!("逐字节逻辑次数 = {}", bad_count(Cursor::new(data)).unwrap());
        println!("BufReader 一次缓冲 = {} 字节", good_count(data));
        println!("规则：文本/日志用 BufReader；二进制定长帧用 chunks_exact\n");
    }
}

// ============================================================================
// 陷阱 3：直接覆写配置文件 —— 读者看到半写内容
// ============================================================================
pub mod partial_overwrite {
    use std::fs;
    use std::io;
    use std::path::Path;
    pub fn bad_save(path: &Path, content: &str) -> io::Result<()> {
        // ❌ truncate 后慢慢 write，崩溃时文件为空或半截
        fs::write(path, content)
    }

    pub fn good_save(path: &Path, content: &str) -> io::Result<()> {
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, content)?;
        fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn demonstrate() {
        println!("## 陷阱 3：直接覆写配置");
        println!("❌ 写一半 crash → 读者 parse 失败或限额变 0");
        println!("✅ tmp → fsync → rename 原子替换");
        println!("规则：所有「读者无锁」的配置/快照都用 write-rename\n");
    }
}

// ============================================================================
// 陷阱 4：误以为 write() 已落盘
// ============================================================================
pub mod write_not_durable {
    use std::io::{self, Write};
    pub fn bad_journal(mut f: impl Write, record: &str) -> io::Result<()> {
        writeln!(f, "{record}")?; // ❌ 只在 page cache
        Ok(())
    }

    pub fn good_journal(mut f: impl Write, record: &str) -> io::Result<()> {
        writeln!(f, "{record}")?;
        f.flush()?; // 仍需 file.sync_data() 才持久
        Ok(())
    }

    pub fn demonstrate() {
        println!("## 陷阱 4：write ≠ 落盘");
        println!("现象：进程 crash 后「明明写了」的订单/事件消失");
        println!("根因：数据在 page cache，未 fsync");
        println!("规则：量化 fsync 策略；监管/资金路径 PerRecord\n");
    }
}

// ============================================================================
// 陷阱 5：在 async runtime 线程做阻塞文件 I/O
// ============================================================================
pub mod blocking_in_async {
    pub fn bad_pattern() -> &'static str {
        "tokio::fs 却用 std::fs::read_to_string 堵 worker → 延迟尖刺"
    }

    pub fn good_pattern() -> &'static str {
        "spawn_blocking / 专用 I/O 线程池 / rio-io_uring"
    }

    pub fn demonstrate() {
        println!("## 陷阱 5：async 里阻塞 I/O");
        println!("❌ {}", bad_pattern());
        println!("✅ {}", good_pattern());
        println!("规则：热路径与 I/O 线程分离；indexer 批量写用 channel\n");
    }
}

// ============================================================================
// 陷阱 6：多进程/多线程写同一文件无锁
// ============================================================================
pub mod concurrent_writers {
    use std::fs::OpenOptions;
    use std::io::{self, Write};
    use std::path::Path;
    pub fn bad_append(path: &Path, line: &str) -> io::Result<()> {
        // ❌ 两个进程同时 append 可能交叉行
        let mut f = OpenOptions::new().append(true).open(path)?;
        writeln!(f, "{line}")?;
        Ok(())
    }

    pub fn good_pattern() -> &'static str {
        "单 writer 进程 + flock/O_EXCL sidecar 或消息队列串行化"
    }

    pub fn demonstrate() {
        println!("## 陷阱 6：并发写同一文件");
        println!("现象：日志行被截断拼接、JSON 不可解析");
        println!("✅ {}", good_pattern());
        println!("规则：append-only 也要单写者；多消费者用复制流\n");
    }
}

// ============================================================================
// 陷阱 7：路径拼接未规范化 —— 目录穿越
// ============================================================================
pub mod path_traversal {
    use std::path::{Component, Path, PathBuf};
    pub fn bad_join(base: &Path, user_input: &str) -> PathBuf {
        // ❌ "../../etc/passwd" 可能逃逸
        base.join(user_input)
    }

    pub fn good_join(base: &Path, user_input: &str) -> Option<PathBuf> {
        let p = base.join(user_input);
        let canonical = p.components().fold(PathBuf::new(), |mut acc, c| {
            match c {
                Component::ParentDir => {
                    acc.pop();
                }
                Component::Normal(s) => acc.push(s),
                Component::RootDir => acc.push("/"),
                _ => {}
            }
            acc
        });
        if canonical.starts_with(base) {
            Some(canonical)
        } else {
            None
        }
    }

    pub fn demonstrate() {
        println!("## 陷阱 7：路径穿越");
        let base = Path::new("/data/indexer");
        let evil = "../../etc/passwd";
        println!("bad_join = {:?}", bad_join(base, evil));
        println!("good_join = {:?}", good_join(base, evil));
        println!("规则：用户输入路径必须 canonicalize 后校验前缀\n");
    }
}

// ============================================================================
// 陷阱 8：忽略 ENOSPC / 磁盘满
// ============================================================================
pub mod ignore_disk_full {
    use std::io;
    #[derive(Debug)]
    pub enum WriteResult {
        Ok,
        DiskFull,
        Other,
    }

    pub fn classify(err: &io::Error) -> WriteResult {
        match err.raw_os_error() {
            Some(28) => WriteResult::DiskFull, // ENOSPC on Linux
            _ => WriteResult::Other,
        }
    }

    pub fn good_handler(res: WriteResult) -> &'static str {
        match res {
            WriteResult::DiskFull => "停写 + 告警 + 只读降级 + 清理旧分片",
            WriteResult::Ok => "继续",
            WriteResult::Other => "重试/上报",
        }
    }

    pub fn demonstrate() {
        println!("## 陷阱 8：忽略磁盘满");
        let err = io::Error::from_raw_os_error(28);
        println!("ENOSPC 应对 = {}", good_handler(classify(&err)));
        println!("现象：静默丢日志、indexer 卡死、WAL 半条");
        println!("规则：df/iostat 告警；写失败必须显式处理；分片归档便于清理\n");
    }
}

pub fn demonstrate() {
    read_entire_file_oom::demonstrate();
    unbuffered_reads::demonstrate();
    partial_overwrite::demonstrate();
    write_not_durable::demonstrate();
    blocking_in_async::demonstrate();
    concurrent_writers::demonstrate();
    path_traversal::demonstrate();
    ignore_disk_full::demonstrate();
}
