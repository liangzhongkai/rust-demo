//! 场景：多线程更新共享指标（QPS 计数、连接数、小型缓存）
//!
//! **权衡**
//! - `Arc<Mutex<T>>`：正确但锁竞争大；临界区要极短，避免在锁内做 I/O。
//! - 高并发计数可考虑 `Atomic*` / 分片计数器；复杂不变式仍要锁或单 writer 线程。
//! - **大量阻塞 I/O**：纯 OS 线程模型下一线程一连接会耗尽线程栈；这类问题通常迁到 async 运行时或专用线程池。

use std::sync::{Arc, Mutex};
use std::thread;

pub fn demonstrate_mutex_counter() {
    let counter = Arc::new(Mutex::new(0i64));
    let n = 8;
    let per_thread = 10_000u32;

    let handles: Vec<_> = (0..n)
        .map(|_| {
            let c = Arc::clone(&counter);
            thread::spawn(move || {
                for _ in 0..per_thread {
                    let mut g = c.lock().unwrap();
                    *g += 1;
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    let total = *counter.lock().unwrap();
    let expected = (n as i64) * (per_thread as i64);
    println!("  Arc<Mutex<>> 累加: {total} (期望 {expected})");
    println!("  → 泛化：共享可变状态 = 最小化锁粒度 + 明确不变式；能消息化则消息化。");
}
