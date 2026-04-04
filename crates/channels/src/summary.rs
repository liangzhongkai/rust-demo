//! 将上述场景压缩成决策备忘（供 main 打印）

pub fn print_decision_cheat_sheet() {
    println!(
        "\n  ┌─ Channel 与消息传递备忘 ───────────────────────────────────"
    );
    println!("  │ 多源上报、单点聚合       → mpsc + clone Sender；确认只要一个消费者");
    println!("  │ 防内存爆、限流下游       → sync_channel(有界)；send 阻塞=背压");
    println!("  │ 零队列握手、严格流水线   → sync_channel(0) rendezvous");
    println!("  │ 优雅退出                 → drop 全部 Sender；或毒丸/取消 token");
    println!("  │ 单线程多职责轮询         → recv_timeout / try_recv（避免忙等）");
    println!("  │ 大消息 / 多读者只读负载   → Arc<T> 或传 id+共享存储");
    println!("  │ 多 channel 同时等        → std 无 select；crossbeam / async select");
    println!("  └──────────────────────────────────────────────────────────────");
}
