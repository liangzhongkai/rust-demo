//! 场景：请求排队 / 背压（网关转发、任务队列、日志落盘 worker）
//!
//! **权衡**
//! - **无界** `mpsc::channel`：发送永不阻塞，瞬时突发会把内存撑满 → 适合“确定流量小”或另有全局限流。
//! - **有界** `sync_channel(n)`：队列满时 `send` 阻塞 → 天然背压，保护下游；容量要按延迟与吞吐调参。
//! - **共享内存**（Mutex）vs **消息**：优先消息传递减少锁竞争；高频小消息注意 channel 开销与批量化。

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

pub fn demonstrate_bounded_backpressure() {
    let (tx, rx) = mpsc::sync_channel::<i32>(2);
    let worker = thread::spawn(move || {
        for job in rx {
            thread::sleep(Duration::from_millis(5)); // 模拟慢消费
            let _ = job;
        }
    });

    for i in 0..6 {
        match tx.send(i) {
            Ok(()) => println!("  投递 job {i} ok"),
            Err(_) => break,
        }
    }
    drop(tx);
    worker.join().unwrap();

    println!("  → 泛化：有界队列 = 背压；无界 = 简单但易 OOM。生产常配合超时、丢弃策略或优先级队列。");
}
