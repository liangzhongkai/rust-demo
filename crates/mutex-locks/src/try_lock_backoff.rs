//! ## 场景：后台刷盘线程 + 用户请求线程，避免「长时间嵌套持锁」
//!
//! 线程 A 已持有锁 L1，若再去 **阻塞** 等待 L2，而 L2 的持有者在等 L1，易死锁。
//! 生产里常见折中：对 **次要资源** 用 `try_lock`，失败则 **释放已持锁、退避重试** 或 **跳过本轮**。
//!
//! ### 权衡
//! - **`lock()`**：语义简单，可能长时间阻塞。
//! - **`try_lock()`**：不阻塞；需 **重试策略**（上限、指数退避），否则 CPU 空转。
//! - **标准库无 `lock_timeout`**：硬超时常依赖 `parking_lot::Mutex` 或异步 `tokio::time::timeout` 包一层任务。
//!
//! ### 泛化策略
//! - 把锁持有时间降到 **纳秒级业务含义**：只更新指针/计数，I/O 在锁外。
//! - 「必须带超时」的临界区：考虑 **把状态机化** 为单 actor + channel，而不是多把裸 `Mutex`。

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub fn demonstrate() {
    let resource = Arc::new(Mutex::new(0_u64));

    let (ready_tx, ready_rx) = mpsc::channel::<()>();
    let hog = resource.clone();
    let _bg = thread::spawn(move || {
        let _g = hog.lock().unwrap();
        let _ = ready_tx.send(());
        thread::sleep(Duration::from_millis(25));
    });
    ready_rx.recv().unwrap();

    let mut successes = 0_u32;
    let mut would_block = 0_u32;
    for _ in 0..40 {
        match resource.try_lock() {
            Ok(mut g) => {
                *g += 1;
                successes += 1;
            }
            Err(_) => {
                would_block += 1;
                // 模拟：锁被占用时让出，而非阻塞整个调用栈
                thread::sleep(Duration::from_millis(1));
            }
        }
    }

    let _ = _bg.join();

    println!(
        "    40 次 try_lock：成功进入临界区 {} 次，TryLockError（等价 WouldBlock）{} 次",
        successes, would_block
    );
    println!("    当前值: {}", *resource.lock().unwrap());
}
