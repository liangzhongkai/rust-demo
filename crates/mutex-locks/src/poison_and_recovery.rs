//! ## 场景：工作线程 panic 后，监控线程仍要「读最后一致快照」
//!
//! Rust 的 `Mutex` 在持有 guard 时若线程 panic，锁会被标记为 **poisoned**：
//! 提示「临界区可能没有跑完」，数据可能处于中间态。
//!
//! ### 权衡
//! - **严格失败**：`lock()` 返回 `Err` → 强迫调用方意识到风险。
//! - **恢复运行**：`PoisonError::into_inner()` 仍可拿到 guard，用于告警后重置、或导出现场。
//!
//! ### 泛化策略
//! - 生产上：减少在持锁区间做 **可能 panic** 的逻辑；大计算放在锁外。
//! - 若必须恢复：记录指标、打日志、用 `into_inner()` 修复或替换整段状态（等价于「人工熔断后重建」）。
//!
//! > 运行本段时 **stderr 会出现一条 panic 日志**，用于刻意展示毒化来源；生产应配合告警与采样。

use std::sync::{Arc, Mutex};
use std::thread;

pub fn demonstrate() {
    let state = Arc::new(Mutex::new(vec![1_i32, 2, 3]));

    let bad = state.clone();
    let _ = thread::spawn(move || {
        let mut g = bad.lock().unwrap();
        g.push(4);
        panic!("模拟：持锁路径上第三方库 panic");
    })
    .join();

    let lock_result = state.lock();
    match lock_result {
        Ok(g) => println!("    未毒化（意外）: {:?}", *g),
        Err(e) => {
            println!("    检测到毒化锁: {}", e);
            let mut g = e.into_inner();
            println!("    仍可取出 guard 做恢复; 当前: {:?}", *g);
            g.clear();
            g.extend([0, 0, 0]);
            println!("    恢复后占位状态: {:?}", *g);
        }
    }
}
