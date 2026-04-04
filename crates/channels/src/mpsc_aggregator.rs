//! 场景：多源事件汇总（多服务实例、多线程 worker 向单一聚合器上报指标或日志片段）
//!
//! **权衡**
//! - `Sender::clone` 很便宜；每个生产者线程持有一份 `Sender`，无需全局锁排队写消息。
//! - 标准库 `mpsc` 是 **多生产者、单消费者**；若业务要「多消费者抢任务」，需换模型（分区多个 channel、`crossbeam_channel`、或 `Arc<Mutex<VecDeque>>` 等工作队列）。
//! - **泛化**：一对多「上报/汇聚」→ 克隆 `Sender` + 单 `Receiver` 消费；先确认是否只要一个消费者。

use std::sync::mpsc;
use std::thread;

pub fn demonstrate() {
    let (tx, rx) = mpsc::channel();
    let mut handles = Vec::new();

    for shard in 0..3 {
        let tx = tx.clone();
        handles.push(thread::spawn(move || {
            for seq in 0..2 {
                tx.send(format!("shard{shard}-evt{seq}"))
                    .expect("aggregator still listening");
            }
        }));
    }
    drop(tx);

    for msg in rx {
        println!("  aggregated: {msg}");
    }

    for h in handles {
        h.join().expect("worker panicked");
    }
    println!("  （main 已消费完；所有 Sender 已 drop，channel 关闭）");
}
