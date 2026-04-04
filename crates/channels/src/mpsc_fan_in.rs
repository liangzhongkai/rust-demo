//! 场景：多源事件汇入单处理线程（日志聚合、分片采集、多连接读端统一排队）
//!
//! **权衡**
//! - `mpsc` 只保证**送达顺序与并发安全**，不保证全局时序（跨发送者的相对顺序依赖调度）。
//! - 每个额外 `Sender` 都是一次 `clone`；用完须 `drop` 所有 `Sender`，否则接收端永远等不到结束。
//! - **泛化**：N 写 1 读 → `mpsc`；需要广播或多读 → 不能靠单条 `mpsc`，改用扇出、`broadcast` 类 crate 或共享状态 + 通知。

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

pub fn demonstrate() {
    let (tx, rx) = mpsc::channel();

    for shard in 0..3 {
        let tx = tx.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(10 * (shard as u64 + 1)));
            let _ = tx.send(format!("shard{shard}: row"));
        });
    }
    drop(tx);

    for msg in rx {
        println!("  {msg}");
    }
    println!("  （所有 Sender 已 drop，迭代自然结束 — 生产中常用于「优雅等到上游全部结束」）");
}
