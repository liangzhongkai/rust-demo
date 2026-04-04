//! # 通道（channels）：生产向场景与权衡
//!
//! 用标准库 `std::sync::mpsc` 演示常见模式，并把具体问题抽象成可复用的应对策略。
//! 运行：`cargo run -p channels`

mod bounded_backpressure;
mod mpsc_aggregator;
mod payload_sharing;
mod recv_timeout_poll;
mod shutdown_graceful;
mod summary;
mod sync_rendezvous;

fn main() {
    println!("=== Channels：场景、权衡与泛化策略 ===\n");

    println!("--- 1. MPSC 聚合：多生产者、单消费者（指标/日志汇聚） ---");
    mpsc_aggregator::demonstrate();

    println!("\n--- 2. 有界 channel：背压与内存（突发流量 vs 慢消费者） ---");
    bounded_backpressure::demonstrate();

    println!("\n--- 3. sync_channel(0)：无缓冲汇合、流水线握手 ---");
    sync_rendezvous::demonstrate();

    println!("\n--- 4. 关闭语义：drop 全部 Sender，recv 感知 Disconnected ---");
    shutdown_graceful::demonstrate();

    println!("\n--- 5. recv_timeout：阻塞接收与周期任务折中 ---");
    recv_timeout_poll::demonstrate();

    println!("\n--- 6. 消息负载：大对象与 Arc 只读共享 ---");
    payload_sharing::demonstrate();

    summary::print_decision_cheat_sheet();
}
