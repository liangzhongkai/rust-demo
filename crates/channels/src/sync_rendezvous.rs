//! 场景：流水线阶段之间「零队列」交接（希望上一阶段在下一阶段真正取走前不提前堆积）
//!
//! **权衡**
//! - `sync_channel(0)`：发送与接收 **汇合**（rendezvous），等价于无缓冲握手；延迟最低队列开销，但吞吐受最慢环节严格限制。
//! - 容量 `k>0`：允许 `k` 个待处理项，平滑瞬时抖动，但峰值下仍可能堆满并阻塞 `send`。
//! - **泛化**：缓冲深度 = 允许在途中的「未完成工作」量；0 表示不允许在途积压，调参即调系统弹性与内存占用。

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

pub fn demonstrate() {
    let (tx, rx) = mpsc::sync_channel::<&'static str>(0);

    let stage_b = thread::spawn(move || {
        for _ in 0..3 {
            let work = rx.recv().expect("stage_a connected");
            println!("  stage_b: 处理 {work}（recv 后 stage_a 的 send 才返回）");
            thread::sleep(Duration::from_millis(20));
        }
    });

    for item in ["frame-1", "frame-2", "frame-3"] {
        println!("  stage_a: send({item}) …（阻塞直到 stage_b recv）");
        tx.send(item).expect("stage_b alive");
    }
    drop(tx);

    stage_b.join().expect("stage_b panicked");
}
