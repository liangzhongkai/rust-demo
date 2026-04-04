//! 场景：单线程里既要周期性干活（心跳、UI tick、看其他 fd），又要从 channel 取消息
//!
//! **权衡**
//! - `recv_timeout`：在等消息与「做别的事」之间折中；超时不是错误状态，需与 `Disconnected` 区分。
//! - `try_recv`：完全不阻塞；若空转循环需加 `sleep` 或退避，否则 CPU 空转（反模式）。
//! - **泛化**：「多事件源」在标准库无 `select!`；可每源一个线程再汇总，或引入 `crossbeam-channel` / 异步运行时 `select!`。

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

pub fn demonstrate() {
    let (tx, rx) = mpsc::channel::<&'static str>();

    let feeder = thread::spawn(move || {
        thread::sleep(Duration::from_millis(40));
        let _ = tx.send("ping");
    });

    let mut got = None;
    for attempt in 1..=6 {
        match rx.recv_timeout(Duration::from_millis(25)) {
            Ok(msg) => {
                got = Some(msg);
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                println!("  main: 第 {attempt} 次 tick（超时，继续轮询）");
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    feeder.join().ok();
    println!(
        "  结果: {}",
        got.unwrap_or("未等到消息（演示用短超时，正常业务可调长）")
    );
}
