//! ## 场景：「谁拥有状态」—— 共享内存 + 锁 vs 消息传递
//!
//! 同一业务既可写成 **多线程 + `Arc<Mutex<State>>`**，也可写成 **单线程状态机 + channel 投递命令**。
//! Go 谚语「通过通信共享内存」在 Rust 里同样成立：锁适合 **低争用、极小临界区**；channel 适合 **明确所有权边界**。
//!
//! ### 权衡
//! - **Mutex**：延迟低（无排队调度），争用高时 **尾延迟差**。
//! - **Channel**：强制串行化写路径，天然 **无锁内逻辑膨胀**；但可能 **队列积压**（需有界 + 背压策略，见 `channels` crate）。
//!
//! ### 泛化策略
//! - 问自己：**不变式由谁保证**？若只能单线程维护 → actor + mailbox。
//! - 若读远多于写且可容忍快照 → `RwLock` / `ArcSwap`。
//! - 若只是计数 → **原子变量**，不要用 `Mutex<u64>`。

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

enum Cmd {
    Inc,
    Done(mpsc::Sender<u64>),
}

pub fn demonstrate() {
    let (tx, rx) = mpsc::channel::<Cmd>();
    let actor = thread::spawn(move || {
        let mut n = 0_u64;
        while let Ok(cmd) = rx.recv() {
            match cmd {
                Cmd::Inc => n += 1,
                Cmd::Done(reply) => {
                    let _ = reply.send(n);
                    break;
                }
            }
        }
    });

    for _ in 0..100 {
        tx.send(Cmd::Inc).unwrap();
    }
    let (done_tx, done_rx) = mpsc::channel();
    tx.send(Cmd::Done(done_tx)).unwrap();
    let channel_total = done_rx.recv().unwrap();
    actor.join().unwrap();

    let shared = Arc::new(Mutex::new(0_u64));
    let m = shared.clone();
    let h = thread::spawn(move || {
        for _ in 0..100 {
            *m.lock().unwrap() += 1;
        }
    });
    h.join().unwrap();
    let mutex_total = *shared.lock().unwrap();

    println!(
        "    同一逻辑（累加 100 次）: channel 结果 = {channel_total}, mutex 结果 = {mutex_total}"
    );
    println!("    对比维度：争用下的尾延迟、不变式复杂度、是否需背压 —— 而非谁绝对更快。");
}
