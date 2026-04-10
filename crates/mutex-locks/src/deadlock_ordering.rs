//! ## 场景：两个账户转账、两阶段提交、嵌套资源（表 A + 表 B）
//!
//! 若线程 1 先锁 `ma` 再锁 `mb`，线程 2 先锁 `mb` 再锁 `ma`，在 **阻塞 `lock()`** 下可能永久死锁。
//!
//! ### 权衡
//! - **全局锁序**（对资源唯一编号，始终按 id 升序加锁）：实现成本中等，能消灭这类循环等待。
//! - **`try_lock` + 退避**：适合低冲突、可重试场景；高冲突下可能活锁，需上限与抖动。
//!
//! ### 泛化策略
//! - 多锁：要么 **单把粗锁**（简单但吞吐差），要么 **固定顺序**，要么 **把两资源合并为一个事务对象**（一把锁包两层数据）。
//! - 分布式系统对应：**全局顺序的锁服务** 或 **两阶段提交 / Saga** 的显式状态机，本质都是避免循环等待。

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub fn demonstrate() {
    let ma = Arc::new(Mutex::new(100_u64));
    let mb = Arc::new(Mutex::new(200_u64));

    // 反模式（注释）：若此处两个线程都用 lock() 且顺序相反，可能死锁。
    // 正例：两线程都按 (ma, mb) 顺序加锁。
    let a1 = ma.clone();
    let b1 = mb.clone();
    let h1 = thread::spawn(move || transfer_ordered(&a1, &b1, 10));

    let a2 = ma.clone();
    let b2 = mb.clone();
    let h2 = thread::spawn(move || transfer_ordered(&a2, &b2, 5));

    h1.join().unwrap();
    h2.join().unwrap();

    let ga = ma.lock().unwrap();
    let gb = mb.lock().unwrap();
    println!("    有序加锁后: A={}, B={}（总和应保持 300）", *ga, *gb);
}

/// 始终先锁 `from` 再锁 `to`，两线程参数顺序一致 → 避免循环等待。
fn transfer_ordered(from: &Mutex<u64>, to: &Mutex<u64>, amt: u64) {
    let mut a = from.lock().unwrap();
    thread::sleep(Duration::from_millis(1));
    let mut b = to.lock().unwrap();
    *a = a.saturating_sub(amt);
    *b += amt;
}
