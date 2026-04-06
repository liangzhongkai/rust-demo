//! ## 场景：读多写少的配置 / 特性开关 / 路由表快照
//!
//! 服务启动后 **大量请求只读** 配置；运维 **偶尔** 热更新。`Mutex<T>` 会让只读路径也互斥，浪费并发度。
//!
//! ### 权衡
//! - **`RwLock<T>`**：多读者并行；写者独占。适合读 ≫ 写。
//! - **注意**：Rust 标准库 `RwLock` 的实现偏 **读者优先或实现相关**，写者可能 **饥饿**；极端写负载要测或换 `parking_lot` 等。
//! - **替代**：`ArcSwap` / 原子指针换整表（几乎无锁读），写时复制新 `Arc` —— 适合大配置、读极热。
//!
//! ### 泛化策略
//! - 区分 **读路径延迟敏感** 与 **写频率**；读热、写极少 → `RwLock` 或 copy-on-write 快照。
//! - 若配置含 `!Sync` 字段或需复杂不变式，仍可能回到 `Mutex` 或单线程刷新通道。

use std::sync::{Arc, RwLock};
use std::thread;

pub fn demonstrate() {
    let cfg = Arc::new(RwLock::new(String::from("mode=stable")));

    let mut readers = vec![];
    for id in 0..6 {
        let c = cfg.clone();
        readers.push(thread::spawn(move || {
            let n = (0..200)
                .filter(|_| c.read().map(|g| g.contains("stable")).unwrap_or(false))
                .count();
            println!("    reader {id}: 完成 {n} 次只读检查");
        }));
    }

    let c = cfg.clone();
    let writer = thread::spawn(move || {
        thread::sleep(std::time::Duration::from_millis(2));
        let mut w = c.write().unwrap();
        *w = String::from("mode=canary");
        println!("    writer: 热更新为 canary");
    });

    for h in readers {
        h.join().unwrap();
    }
    writer.join().unwrap();

    println!("    最终配置: {}", cfg.read().unwrap());
}
