//! # Interior Mutability 常见陷阱与诊断
//!
//! 现象 → 根因 → 修法。内部可变性把编译期 borrow 检查换成运行时规则；
//! 用错类型或跨线程共享会导致 **panic** 或 **静默错误**。

#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

// ============================================================================
// 陷阱 1：RefCell 重入 borrow —— 运行时 panic
// ============================================================================
/// **现象**：偶发 `already borrowed` panic，多在回调嵌套。
/// **根因**：`borrow_mut` 未 drop 又 `borrow` / 回调里再 mutate 同一 RefCell。
/// **修法**：缩短 borrow 作用域；先收集再处理；或 channel 串行。
pub mod refcell_reentrancy {
    use super::*;
    pub struct BadEngine {
        state: RefCell<Vec<u32>>,
    }

    impl BadEngine {
        pub fn bad_dispatch(&self) {
            let mut s = self.state.borrow_mut();
            s.push(1);
            // ❌ 嵌套 borrow 同一 RefCell
            // let _ = self.state.borrow(); // would panic
            let _ = s.len();
        }
    }

    pub struct GoodEngine {
        state: RefCell<Vec<u32>>,
    }

    impl GoodEngine {
        pub fn good_dispatch(&self) {
            let snapshot: Vec<u32> = {
                let mut s = self.state.borrow_mut();
                s.push(1);
                s.clone()
            }; // borrow 结束
            let _ = snapshot.len();
        }
    }

    pub fn demonstrate() {
        println!("## 陷阱 1：RefCell 重入 borrow");
        let good = GoodEngine {
            state: RefCell::new(vec![]),
        };
        good.good_dispatch();
        println!("好写法：borrow 块内完成 mutate，不跨回调");
        println!("坏写法：策略 on_tick 里再 register → panic\n");
    }
}

// ============================================================================
// 陷阱 2：Atomic 非原子复合更新 —— 丢更新
// ============================================================================
/// **现象**：计数偏小；nonce 重复；敞口与 version 不一致。
/// **根因**：`load` + `store` 非 CAS；两字段分别更新无顺序。
/// **修法**：`fetch_add` / `compare_exchange`；或用 Mutex 保护不变式。
pub mod atomic_lost_update {
    use super::*;
    pub struct BadCounter {
        n: AtomicU64,
    }

    impl BadCounter {
        pub fn bad_inc_twice_racy(&self) {
            let v = self.n.load(Ordering::Relaxed);
            // 另一线程可能在此处插入
            self.n.store(v + 1, Ordering::Relaxed);
        }
    }

    pub struct GoodCounter {
        n: AtomicU64,
    }

    impl GoodCounter {
        pub fn good_inc(&self) {
            self.n.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn demonstrate() {
        println!("## 陷阱 2：Atomic load+store 丢更新");
        let good = GoodCounter {
            n: AtomicU64::new(0),
        };
        good.good_inc();
        good.good_inc();
        println!(
            "fetch_add 结果={}（Racy load+store 在并发下会偏小）\n",
            good.n.load(Ordering::Relaxed)
        );
    }
}

// ============================================================================
// 陷阱 3：RefCell 跨线程 —— 编译能过但逻辑错
// ============================================================================
/// **现象**：`Arc<RefCell<T>>` 在线程间传递；数据竞争或逻辑错乱。
/// **根因**：RefCell 非 Sync；Arc 只保证 Send 类型可跨线程，不保证 RefCell 安全共享。
/// **修法**：多线程用 Mutex/RwLock；或每线程一份 + 原子汇总。
pub mod refcell_not_sync {
    pub fn demonstrate() {
        println!("## 陷阱 3：RefCell 不能替代 Mutex");
        println!("Arc<RefCell<T>>: T 需 Send，但多线程同时 borrow_mut 仍 UB 级错误");
        println!("→ 单线程 RefCell；多线程 Mutex / 分片 Atomic\n");
    }
}

// ============================================================================
// 陷阱 4：RwLock 写者饥饿 / 读锁堆积
// ============================================================================
/// **现象**：配置热更新卡住；P99 尖刺。
/// **根因**：持续 read 锁阻止 write；或 read 内做慢 IO。
/// **修法**：ArcSwap / 世代指针；读路径拷贝快照；写路径 swap。
pub mod rwlock_writer_starvation {
    use super::*;
    pub fn demonstrate() {
        println!("## 陷阱 4：RwLock 读多写少仍可能饿死写者");
        let lock = Arc::new(RwLock::new(0u64));
        let reader = Arc::clone(&lock);
        let _r = reader.read().unwrap();
        // 持 read 期间 write 阻塞
        println!("持 read 跨慢路径会阻塞 tick_size 热更新");
        println!("→ 快照指针 / generation bump + 无锁读\n");
    }
}

// ============================================================================
// 陷阱 5：Mutex 热路径争用 —— P99 爆炸
// ============================================================================
/// **现象**：每 tick 锁全局 `Mutex<OrderBook>`。
/// **根因**：把本可单线程 RefCell 或分片的状态塞进一把大锁。
/// **修法**：单线程 reactor；或按 symbol 分片 Mutex；Atomics 做汇总。
pub mod mutex_on_hot_path {
    use super::*;
    pub fn demonstrate() {
        println!("## 陷阱 5：Mutex 在热路径");
        let _book = Arc::new(Mutex::new(vec![0i64; 1000]));
        println!("每 delta 都 lock 全簿 → 多策略线程互斥");
        println!("→ HFT 常见：单线程 RefCell 簿 + Atomic 敞口\n");
    }
}

// ============================================================================
// 陷阱 6：Cell 存非 Copy —— 编译失败（好事）
// ============================================================================
/// **现象**：试图 `Cell<String>` 或 `Cell<Vec<_>>`。
/// **根因**：Cell 只能 Copy；非 Copy 要 RefCell 或 owned Mutex。
/// **修法**：小标量 Cell；结构体 RefCell；跨线程 Mutex。
pub mod cell_non_copy {
    use super::*;
    pub fn demonstrate() {
        println!("## 陷阱 6：Cell 仅适用于 Copy");
        let flags: Cell<u8> = Cell::new(0);
        flags.set(flags.get() | 1);
        println!("Cell<u8> OK；String/Vec 请用 RefCell 或 Mutex\n");
    }
}

// ============================================================================
// 陷阱 7：Ordering 滥用 —— 可见性 bug
// ============================================================================
/// **现象**：线程 A store 配置，线程 B 读到旧 gas / 旧 nonce。
/// **根因**：全用 Relaxed；release-acquire 配对缺失。
/// **修法**：发布用 Release，读取依赖数据用 Acquire；纯统计可 Relaxed。
pub mod ordering_mismatch {
    use super::*;
    pub struct Config {
        value: AtomicU64,
    }

    impl Config {
        pub fn publish(&self, v: u64) {
            self.value.store(v, Ordering::Release);
        }

        pub fn read(&self) -> u64 {
            self.value.load(Ordering::Acquire)
        }
    }

    pub fn demonstrate() {
        println!("## 陷阱 7：Ordering 与可见性");
        let c = Config {
            value: AtomicU64::new(0),
        };
        c.publish(99);
        println!("read={}（Release/Acquire 配对）", c.read());
        println!("纯计数可用 Relaxed；「先写 data 再 publish flag」要配对\n");
    }
}

// ============================================================================
// 陷阱 8：Mutex poison 后忽略
// ============================================================================
/// **现象**：持锁 panic 后锁 poisoned，后续全 Err。
/// **根因**：`.unwrap()` 直接崩或未 `into_inner` 恢复。
/// **修法**：日志 + poison 恢复 + 重置状态；或隔离 worker。
pub mod poison_recovery {
    use super::*;
    pub fn demonstrate() {
        println!("## 陷阱 8：Mutex poison");
        let m = Arc::new(Mutex::new(0u64));
        let m2 = Arc::clone(&m);
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("simulated");
        })
        .join();
        match m.lock() {
            Err(e) => {
                let inner = e.into_inner();
                println!("poison 后恢复 inner={}", *inner);
            }
            Ok(g) => println!("inner={}", *g),
        }
        println!();
    }
}

pub fn demonstrate() {
    refcell_reentrancy::demonstrate();
    atomic_lost_update::demonstrate();
    refcell_not_sync::demonstrate();
    rwlock_writer_starvation::demonstrate();
    mutex_on_hot_path::demonstrate();
    cell_non_copy::demonstrate();
    ordering_mismatch::demonstrate();
    poison_recovery::demonstrate();
}
