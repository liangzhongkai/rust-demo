//! ## 场景：同一服务里多路独立计数器 vs 一把大锁包所有「小指标」
//!
//! 监控里常有几十个 `Counter`；若用 **一把 `Mutex<MetricsBundle>`**，任意字段更新都会互斥。
//! **按字段分锁** 可降低无关线程间的假争用，但代码复杂度与死锁面（多锁顺序）上升。
//!
//! ### 权衡
//! - **粗锁**：实现快、顺序简单；高并发下 **无关更新也排队**。
//! - **细锁**：吞吐更好；要 **统一加锁顺序**、避免锁粒度过细导致 CPU cache 抖动（视平台而定）。
//!
//! ### 泛化策略
//! - 高频路径用 **`Atomic*`** 或 **每线程累加再合并**（减少共享写）。
//! - 中频、多字段：按 **业务域** 分锁，而不是每个字节一把锁。

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

pub fn demonstrate() {
    let coarse = Arc::new(Mutex::new((0_u64, 0_u64)));
    let t0 = Instant::now();
    let mut hs = vec![];
    for _ in 0..4 {
        let c = coarse.clone();
        hs.push(thread::spawn(move || {
            for _ in 0..50_000 {
                let mut g = c.lock().unwrap();
                g.0 += 1;
                g.1 += 1;
            }
        }));
    }
    for h in hs {
        h.join().unwrap();
    }
    let coarse_ms = t0.elapsed().as_millis();

    let f1 = Arc::new(Mutex::new(0_u64));
    let f2 = Arc::new(Mutex::new(0_u64));
    let t1 = Instant::now();
    let mut hs = vec![];
    for i in 0..4 {
        let a = f1.clone();
        let b = f2.clone();
        hs.push(thread::spawn(move || {
            for _ in 0..50_000 {
                if i % 2 == 0 {
                    *a.lock().unwrap() += 1;
                } else {
                    *b.lock().unwrap() += 1;
                }
            }
        }));
    }
    for h in hs {
        h.join().unwrap();
    }
    let fine_ms = t1.elapsed().as_millis();

    println!(
        "    粗锁 (同一把 Mutex 更新两个字段): ~{} ms（本机仅供参考）",
        coarse_ms
    );
    println!("    细锁 (两字段分锁、线程交替写不同字段): ~{} ms", fine_ms);
    println!("    说明：数字随 CPU/调度变化；重点是「无关写能否并行」的结构性差异。");
}
