//! # 互斥锁（mutex-locks）：生产向场景与权衡
//!
//! 用标准库 `std::sync` 演示常见模式，并把具体问题抽象成可复用的应对策略。
//! 运行：`cargo run -p mutex-locks`

mod arc_mutex_basics;
mod deadlock_ordering;
mod granularity;
mod mutex_vs_channel;
mod poison_and_recovery;
mod rwlock_hot_read;
mod summary;
mod try_lock_backoff;

fn main() {
    println!("=== Mutex / RwLock：场景、权衡与泛化策略 ===\n");

    println!("--- 1. Arc<Mutex<T>>：共享计数 / 会话表类状态 ---");
    arc_mutex_basics::demonstrate();

    println!("\n--- 2. 毒化锁：持锁路径 panic 后的恢复语义 ---");
    poison_and_recovery::demonstrate();

    println!("\n--- 3. 多锁与顺序：转账式「两资源」与死锁规避 ---");
    deadlock_ordering::demonstrate();

    println!("\n--- 4. RwLock：读多写少的配置热读 ---");
    rwlock_hot_read::demonstrate();

    println!("\n--- 5. 锁粒度：粗锁包多字段 vs 分锁（结构性争用） ---");
    granularity::demonstrate();

    println!("\n--- 6. try_lock：非阻塞获取与退避（避免嵌套阻塞） ---");
    try_lock_backoff::demonstrate();

    println!("\n--- 7. Mutex vs Channel：共享内存与消息传递的边界 ---");
    mutex_vs_channel::demonstrate();

    summary::print_decision_cheat_sheet();
}
