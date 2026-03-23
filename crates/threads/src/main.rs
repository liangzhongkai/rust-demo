//! # 线程：生产向场景与权衡
//!
//! 本 crate 用 `std::thread` 演示常见模式，并把具体问题抽象成可复用的应对策略。
//! 运行：`cargo run -p threads`

mod channels;
mod naming;
mod scoped;
mod shared_state;
mod spawn_join;
mod summary;

fn main() {
    println!("=== Threads：场景、权衡与泛化策略 ===\n");

    println!("--- 1. spawn + join：CPU 分块并行（报表/批处理） ---");
    spawn_join::demonstrate();

    println!("\n--- 2. scoped：只读共享缓冲区上的短期并行（解析/扫描） ---");
    scoped::demonstrate();

    println!("\n--- 3. 有界 channel：背压与慢消费者（队列/网关） ---");
    channels::demonstrate_bounded_backpressure();

    println!("\n--- 4. Arc<Mutex<>>：共享可变计数（指标/小状态） ---");
    shared_state::demonstrate_mutex_counter();

    println!("\n--- 5. 命名线程：可观测性与排障 ---");
    naming::demonstrate();

    summary::print_decision_cheat_sheet();
}
