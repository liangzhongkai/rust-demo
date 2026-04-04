//! 场景：线程池/任务源结束时要通知消费者「不会再有新任务」（优雅退出、刷队列后 join）
//!
//! **权衡**
//! - `mpsc` 的完成信号：**所有** `Sender` 被 drop 后，`recv` 返回 `Err`、迭代结束。漏 drop 一个 clone 的 `Sender` = 隐性死锁/挂起。
//! - 与「显式毒丸消息」对比：毒丸可携带元数据且顺序明确，但要约定协议；纯 drop 更轻但错误更难查。
//! - **泛化**：生命周期与「谁拥有最后一根 Sender」必须在架构上明确；复杂系统常结合 `cancel` token（见 async/取消专题）。

use std::sync::mpsc;
use std::thread;

pub fn demonstrate() {
    let (tx, rx) = mpsc::channel::<i32>();

    let worker = thread::spawn(move || {
        while let Ok(task) = rx.recv() {
            println!("  worker: 处理 task {task}");
        }
        println!("  worker: channel 关闭，退出循环");
    });

    let tx2 = tx.clone();
    thread::spawn(move || {
        let _ = tx2.send(1);
    });
    let _ = tx.send(2);

    drop(tx);

    let _ = worker.join();
}
