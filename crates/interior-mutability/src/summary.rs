//! 决策备忘：将场景压缩成可扫一眼的清单

pub fn print_decision_cheat_sheet() {
    println!("\n  ┌─ Interior Mutability：场景与泛化策略 ─────────────────────");
    println!("  │ Copy 标量、单线程 seq/flags     → Cell");
    println!("  │ 单线程 &self 回调改结构         → RefCell（borrow 要短）");
    println!("  │ 多线程计数/闸门/nonce           → Atomic* + fetch_* / CAS");
    println!("  │ 读多写少配置/ABI/元数据         → RwLock 或 ArcSwap 快照");
    println!("  │ 多线程队列/复杂不变式           → Mutex；临界区最小化");
    println!("  │ 一次性/延迟 init                → OnceLock");
    println!("  │ 不必共享内存                    → channel 串行状态机");
    println!("  │ 多 key 争用同一把锁             → 分片 Mutex 或单 writer");
    println!("  │ RefCell 切忌跨线程              → 用 Mutex/RwLock 替代");
    println!("  │ Atomic 切忌 load+store 复合     → fetch_add / compare_exchange");
    println!("  └──────────────────────────────────────────────────────────────");
}
