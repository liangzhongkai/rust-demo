//! # Interior Mutability 底层机制
//!
//! Rust 默认「&T 不可变 ⇒ 不能改」。**内部可变性** 在类型仍暴露 `&self` 时，
//! 通过运行时检查（RefCell）或同步原语（Mutex / Atomic）改变内部状态。
//!
//! ```text
//! 单线程、逻辑共享 &self  → Cell / RefCell
//! 多线程、简单标量         → Atomic*
//! 多线程、复杂不变式       → Mutex / RwLock
//! 一次性 / 延迟初始化      → OnceLock
//! ```
//!
//! 与「可变借用 &mut T」的区别：外部接口可以是 `&self`，适合回调、trait object、
//! 多处持有同一引用却仍需更新的场景（订单簿、解析器、订阅分发）。

#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};

// ============================================================================
// Cell：Copy 类型、无借用冲突检查
// ============================================================================
/// `Cell<T>` 适合 **Copy** 小值：序号、标志位。`get/set` 按值拷贝，无运行时 borrow 检查。
pub mod cell_seq {
    use super::*;
    pub struct Session {
        seq: Cell<u64>,
    }

    impl Session {
        pub fn new() -> Self {
            Self { seq: Cell::new(1) }
        }

        /// 对外仍是 &self，内部递增序号。
        pub fn next_seq(&self) -> u64 {
            let n = self.seq.get();
            self.seq.set(n + 1);
            n
        }
    }

    pub fn demonstrate() {
        println!("## 基础-1：Cell 序号（&self 内递增）");
        let s = Session::new();
        let a = s.next_seq();
        let b = s.next_seq();
        println!("seq: {} → {}（接口是 &self，无 &mut）\n", a, b);
    }
}

// ============================================================================
// RefCell：单线程、非 Copy、需运行时 borrow 检查
// ============================================================================
/// `RefCell<T>` 在 **单线程** 提供 `borrow_mut()`；冲突时 **panic**（非数据竞争）。
pub mod refcell_book {
    use super::*;

    #[derive(Debug, Default)]
    pub struct Book {
        bids: Vec<(i64, i64)>,
    }

    impl Book {
        pub fn apply_delta(&mut self, px: i64, qty: i64) {
            if qty == 0 {
                self.bids.retain(|(p, _)| *p != px);
            } else if let Some(l) = self.bids.iter_mut().find(|(p, _)| *p == px) {
                l.1 = qty;
            } else {
                self.bids.push((px, qty));
            }
        }

        pub fn best_bid(&self) -> Option<i64> {
            self.bids.iter().map(|(p, _)| *p).max()
        }
    }

    pub struct OrderBook {
        book: RefCell<Book>,
    }

    impl OrderBook {
        pub fn new() -> Self {
            Self {
                book: RefCell::new(Book::default()),
            }
        }

        /// 行情回调通常只给 &self —— RefCell 让簿可以在 &self 上更新。
        pub fn on_l2(&self, px: i64, qty: i64) {
            self.book.borrow_mut().apply_delta(px, qty);
        }

        pub fn snapshot_best(&self) -> Option<i64> {
            self.book.borrow().best_bid()
        }
    }

    pub fn demonstrate() {
        println!("## 基础-2：RefCell 订单簿（回调 &self 内 mutate）");
        let ob = OrderBook::new();
        ob.on_l2(100_00, 10);
        ob.on_l2(100_05, 5);
        println!("best_bid = {:?}\n", ob.snapshot_best());
    }
}

// ============================================================================
// Atomic：多线程无锁标量
// ============================================================================
pub mod atomic_counter {
    use super::*;

    pub struct Metrics {
        pub fills: AtomicU64,
        pub rejects: AtomicU64,
    }

    impl Metrics {
        pub fn new() -> Self {
            Self {
                fills: AtomicU64::new(0),
                rejects: AtomicU64::new(0),
            }
        }

        pub fn record_fill(&self) {
            self.fills.fetch_add(1, Ordering::Relaxed);
        }

        pub fn snapshot(&self) -> (u64, u64) {
            (
                self.fills.load(Ordering::Relaxed),
                self.rejects.load(Ordering::Relaxed),
            )
        }
    }

    pub fn demonstrate() {
        println!("## 基础-3：Atomic 热路径计数（&self + Relaxed）");
        let m = Metrics::new();
        m.record_fill();
        m.record_fill();
        let (f, r) = m.snapshot();
        println!("fills={} rejects={}\n", f, r);
    }
}

// ============================================================================
// Mutex / RwLock：多线程复杂状态
// ============================================================================
pub mod sync_locks {
    use super::*;

    pub struct SharedConfig {
        inner: RwLock<HashMapLite>,
    }

    #[derive(Clone, Copy)]
    struct HashMapLite {
        tick: i64,
    }

    impl SharedConfig {
        pub fn new(tick: i64) -> Self {
            Self {
                inner: RwLock::new(HashMapLite { tick }),
            }
        }

        pub fn tick_size(&self) -> i64 {
            self.inner.read().unwrap().tick
        }

        pub fn set_tick(&self, tick: i64) {
            self.inner.write().unwrap().tick = tick;
        }
    }

    pub struct PendingQueue {
        inner: Mutex<Vec<u64>>,
    }

    impl PendingQueue {
        pub fn new() -> Self {
            Self {
                inner: Mutex::new(Vec::new()),
            }
        }

        pub fn push(&self, id: u64) {
            self.inner.lock().unwrap().push(id);
        }

        pub fn len(&self) -> usize {
            self.inner.lock().unwrap().len()
        }
    }

    pub fn demonstrate() {
        println!("## 基础-4：RwLock 热读 / Mutex 队列");
        let cfg = SharedConfig::new(100);
        println!("tick={}", cfg.tick_size());
        cfg.set_tick(50);
        println!("热更新 tick={}", cfg.tick_size());

        let q = PendingQueue::new();
        q.push(1);
        q.push(2);
        println!("pending len={}\n", q.len());
    }
}

// ============================================================================
// OnceLock：延迟 / 一次性初始化
// ============================================================================
pub mod once_init {
    use super::*;

    static VENUE_DECODER: OnceLock<fn(&[u8]) -> usize> = OnceLock::new();

    pub fn decode_once(buf: &[u8]) -> usize {
        let f = *VENUE_DECODER.get_or_init(|| |b| b.len());
        f(buf)
    }

    pub fn demonstrate() {
        println!("## 基础-5：OnceLock 延迟初始化");
        let n = decode_once(&[1, 2, 3]);
        let m = decode_once(&[1, 2, 3, 4]);
        println!("first={} second={}（第二次走已初始化路径）\n", n, m);
    }
}

pub fn demonstrate() {
    cell_seq::demonstrate();
    refcell_book::demonstrate();
    atomic_counter::demonstrate();
    sync_locks::demonstrate();
    once_init::demonstrate();
}
