//! 决策备忘：将场景压缩成可扫一眼的清单

pub fn print_decision_cheat_sheet() {
    println!("\n  ┌─ Mutex / 锁：场景与泛化策略 ───────────────────────────────");
    println!("  │ 多线程共享小块可变状态     → Arc<Mutex<T>>；缩短临界区");
    println!("  │ 读多写少（配置/路由表）    → RwLock；或 ArcSwap / 快照指针");
    println!("  │ 纯计数、无复杂不变式      → Atomic*，避免 Mutex<u64>");
    println!("  │ 多资源嵌套                → 全局固定加锁顺序，或合并为单锁");
    println!("  │ 持锁时不能阻塞等另一锁    → try_lock + 退避/上限；或 actor");
    println!("  │ 工作线程 panic 后恢复     → PoisonError::into_inner + 日志/重置");
    println!("  │ 不变式只能单线程维护      → channel 串行状态机（见 channels 背压）");
    println!("  │ 标准库 Mutex 功能边界     → 超时/公平性 → parking_lot 或运行时封装");
    println!("  └──────────────────────────────────────────────────────────────");
}
