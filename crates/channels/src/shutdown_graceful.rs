//! 场景：线程池 / worker 退出：如何让消费者知道「不会再有新任务」
//!
//! **权衡**
//! - 关闭时 **drop 全部 `Sender`**（含 clone 出来的）：`Receiver::recv` 返回 `Err(Disconnected)`，可干净退出循环。
//! - 若只 drop 部分 `Sender`，只要还有一个存活，`recv` 会一直等 → 容易「假死」；生产上常配合 `JoinHandle`、或显式发「毒丸」消息。
//! - **泛化**：生命周期结束信号 = 「通道关闭」或「哨兵消息」二选一；多生产者场景必须统计/集中管理所有 `Sender`。

use std::sync::mpsc;
use std::thread;

pub fn demonstrate() {
    let (tx, rx) = mpsc::channel::<i32>();
    let tx_a = tx.clone();
    let tx_b = tx;

    let h1 = thread::spawn(move || {
        let _ = tx_a.send(1);
    });
    let h2 = thread::spawn(move || {
        let _ = tx_b.send(2);
    });
    h1.join().expect("h1");
    h2.join().expect("h2");
    // 两个分支的 Sender 都已 drop

    let worker = thread::spawn(move || loop {
        match rx.recv() {
            Ok(task) => println!("  worker: 执行任务 {task}"),
            Err(_) => {
                println!("  worker: 所有 Sender 已关闭，退出");
                break;
            }
        }
    });
    worker.join().expect("worker panicked");
}
