//! 将上述场景压缩成决策备忘（非可执行逻辑，仅供 main 打印）

pub fn print_decision_cheat_sheet() {
    println!(
        "\n  ┌─ 线程相关决策备忘 ─────────────────────────────────────────"
    );
    println!("  │ CPU 密集     → 线程数 ~ 核数；数据分块 + 归约；考虑 rayon / 线程池");
    println!("  │ 短期并行+借栈 → thread::scope");
    println!("  │ 解耦+背压    → 有界 channel；避免无界在突发流量下撑爆内存");
    println!("  │ 共享可变少   → Mutex/RwLock；热点计数 → 原子或分片");
    println!("  │ 大量阻塞 I/O → 往往用 async 或少量阻塞 worker + 队列，而非无限 OS 线程");
    println!("  │ 取消/超时    → 协作式（drop sender、AtomicBool）+ 文档化生命周期");
    println!("  └──────────────────────────────────────────────────────────────");
}
