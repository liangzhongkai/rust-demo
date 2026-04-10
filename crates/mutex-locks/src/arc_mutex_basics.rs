//! ## 场景：进程内共享状态（会话表、连接计数、简易限流计数器）
//!
//! 生产里常见「多线程读写同一块内存」：例如网关统计当前连接数、缓存命中计数。
//! `Arc<Mutex<T>>` 是最直接的模型：引用计数共享所有权 + 互斥访问。
//!
//! ### 权衡
//! - **优点**：语义简单，类型系统保证 `T` 不会数据竞争。
//! - **成本**：临界区内的任何工作都会拉长锁持有时间 → 争用上升、延迟尾部分布变差。
//!
//! ### 泛化策略
//! - 把锁里放 **小、快** 的状态；重活移到锁外（只拷出 id / 快照）。
//! - 若更新模式允许，考虑 **分片**（按 key 哈希到多把锁）或 **无锁计数**（`Atomic*`）替代整表锁。

use std::sync::{Arc, Mutex};
use std::thread;

pub fn demonstrate() {
    let total = Arc::new(Mutex::new(0_u64));
    let mut handles = vec![];

    for i in 0..4 {
        let t = total.clone();
        handles.push(thread::spawn(move || {
            for _ in 0..1000 {
                let mut g = t.lock().unwrap();
                *g += 1;
            }
            println!("    worker {i} 完成 1000 次自增");
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    println!("    最终计数 = {}（期望 4000）", *total.lock().unwrap());
}
