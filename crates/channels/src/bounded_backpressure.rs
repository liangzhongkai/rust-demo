//! 场景：上游突发流量（网关、抓取、消息队列消费者）快于下游处理能力
//!
//! **权衡**
//! - **有界** `sync_channel(n)`：`send` 在缓冲区满时阻塞，把压力推回生产者 → 天然背压，保护内存；但可能拖慢整条链，需配合超时/丢弃策略。
//! - **无界** `channel()`：发送几乎不阻塞，慢消费者会导致队列无限增长 → OOM 风险；适合确信下游跟得上或另有全局限流。
//! - **泛化**：先选「背压放在哪一层」（网络、进程内队列、DB）；有界 channel 是进程内最简单的一种背压实现。

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

pub fn demonstrate() {
    let (tx, rx) = mpsc::sync_channel::<u32>(2);
    println!("  缓冲区容量=2：第 3、4 个 send 会阻塞直到 recv 腾出空位");

    let producer = thread::spawn(move || {
        for i in 1..=5 {
            println!("  producer: 尝试 send({i}) …");
            tx.send(i).expect("consumer alive");
            println!("  producer: send({i}) 完成");
        }
    });

    thread::sleep(Duration::from_millis(30));
    for _ in 0..5 {
        match rx.recv_timeout(Duration::from_millis(120)) {
            Ok(v) => println!("  consumer: recv -> {v}"),
            Err(e) => println!("  consumer: {e:?}"),
        }
    }

    producer.join().expect("producer panicked");
}
