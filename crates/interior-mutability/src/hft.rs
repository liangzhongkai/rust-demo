//! # HFT 生产场景下的 Interior Mutability
//!
//! 高频交易硬约束：
//! - **单线程 reactor**：行情 → 簿 → 策略 → 网关，常是一条线程；`RefCell` 让 trait 回调用 `&self`
//! - **热路径无锁**：计数、敞口、序号用 `Atomic*` / `Cell`，避免 Mutex 在 tick 上打架
//! - **配置热读**：`RwLock` 或 generation + 原子指针；写少读多
//! - **不变式**：复杂簿/队列要么单线程 RefCell，要么分片 Mutex，不能混用
//!
//! 下面 7 个场景对应订单簿、FIX 会话、风控、网关、策略分发里的真实写法。

#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

pub type SymbolId = u32;
pub type Px = i64;
pub type Qty = i64;

// ============================================================================
// 场景 1：L2 订单簿 —— RefCell 实现 &self 更新
// ============================================================================
/// **生产问题**：行情库回调 `fn on_depth(&self, delta)`，不能要 `&mut OrderBook`。
/// 多处 `Arc<OrderBook>` 共享同一簿，单线程 reactor 内更新。
///
/// **套路**：`RefCell<SideBook>`；热路径 `borrow_mut()` 极短，不跨 await / 回调。
pub mod orderbook_refcell {
    use super::*;

    #[derive(Default)]
    struct Side {
        levels: Vec<(Px, Qty)>,
    }

    impl Side {
        fn upsert(&mut self, px: Px, qty: Qty) {
            if qty == 0 {
                self.levels.retain(|(p, _)| *p != px);
                return;
            }
            if let Some(l) = self.levels.iter_mut().find(|(p, _)| *p == px) {
                l.1 = qty;
            } else {
                self.levels.push((px, qty));
            }
        }

        fn best(&self) -> Option<Px> {
            self.levels.iter().map(|(p, _)| *p).max()
        }
    }

    pub struct OrderBook {
        bids: RefCell<Side>,
        asks: RefCell<Side>,
        pub updates: Cell<u64>,
    }

    impl OrderBook {
        pub fn new() -> Self {
            Self {
                bids: RefCell::new(Side::default()),
                asks: RefCell::new(Side::default()),
                updates: Cell::new(0),
            }
        }

        pub fn on_l2(&self, is_bid: bool, px: Px, qty: Qty) {
            let side = if is_bid { &self.bids } else { &self.asks };
            side.borrow_mut().upsert(px, qty);
            self.updates.set(self.updates.get() + 1);
        }

        pub fn best_bid(&self) -> Option<Px> {
            self.bids.borrow().best()
        }
    }

    pub fn demonstrate() {
        println!("## HFT-1：L2 订单簿 RefCell（&self 回调）");

        let book = OrderBook::new();
        for _ in 0..100 {
            book.on_l2(true, 50_000_00, 3);
        }
        book.on_l2(true, 50_001_00, 1);
        println!(
            "best_bid={:?} updates={}",
            book.best_bid(),
            book.updates.get()
        );
        println!("单线程 reactor 标准模式；切忌跨线程共享 RefCell\n");
    }
}

// ============================================================================
// 场景 2：FIX / ITCH 会话序号 —— Cell
// ============================================================================
/// **生产问题**：会话对象被编码器、心跳、重传多处 `&Session` 引用，都要 bump seq。
/// 序号是 Copy 标量，不需要 RefCell 的运行时 borrow。
///
/// **套路**：`Cell<u64>` 存 `out_seq` / `in_seq`；`next_out()` 在 &self 上递增。
pub mod fix_session_cell {
    use super::*;

    pub struct FixSession {
        out_seq: Cell<u64>,
        in_seq: Cell<u64>,
        pub resends: Cell<u64>,
    }

    impl FixSession {
        pub fn new() -> Self {
            Self {
                out_seq: Cell::new(1),
                in_seq: Cell::new(1),
                resends: Cell::new(0),
            }
        }

        pub fn next_out(&self) -> u64 {
            let n = self.out_seq.get();
            self.out_seq.set(n + 1);
            n
        }

        pub fn on_inbound(&self, seq: u64) -> bool {
            let expected = self.in_seq.get();
            if seq > expected {
                self.resends.set(self.resends.get() + 1);
                return false;
            }
            self.in_seq.set(expected + 1);
            true
        }
    }

    pub fn demonstrate() {
        println!("## HFT-2：FIX 会话序号 Cell");

        let sess = FixSession::new();
        let s1 = sess.next_out();
        let s2 = sess.next_out();
        let _ = sess.on_inbound(1);
        println!(
            "out_seq {}→{} in_seq={} gap_resends={}",
            s1,
            s2,
            sess.in_seq.get(),
            sess.resends.get()
        );
        println!("Copy 标量优先 Cell，比 Mutex<u64> 零争用\n");
    }
}

// ============================================================================
// 场景 3：策略注册表 —— RefCell<Vec> 单线程分发
// ============================================================================
/// **生产问题**：事件循环里既要 `dispatch(&self, tick)` 又要动态 `register` /
/// `unregister` 策略；不能要求全局 `&mut Engine`。
///
/// **套路**：`RefCell<Vec<Box<dyn Strategy>>>`；dispatch 时短暂 borrow_mut。
pub mod strategy_registry {
    use super::*;

    pub trait Strategy {
        fn on_tick(&mut self, px: Px);
        fn id(&self) -> &'static str;
    }

    struct Momentum {
        last: Px,
    }

    impl Strategy for Momentum {
        fn on_tick(&mut self, px: Px) {
            self.last = px;
        }
        fn id(&self) -> &'static str {
            "mom"
        }
    }

    pub struct Engine {
        strategies: RefCell<Vec<Box<dyn Strategy>>>,
        pub ticks: Cell<u64>,
    }

    impl Engine {
        pub fn new() -> Self {
            Self {
                strategies: RefCell::new(Vec::new()),
                ticks: Cell::new(0),
            }
        }

        pub fn register(&self, s: Box<dyn Strategy>) {
            self.strategies.borrow_mut().push(s);
        }

        pub fn dispatch(&self, px: Px) {
            self.ticks.set(self.ticks.get() + 1);
            for s in self.strategies.borrow_mut().iter_mut() {
                s.on_tick(px);
            }
        }

        pub fn count(&self) -> usize {
            self.strategies.borrow().len()
        }
    }

    pub fn demonstrate() {
        println!("## HFT-3：策略表 RefCell（注册 + 分发同线程）");

        let eng = Engine::new();
        eng.register(Box::new(Momentum { last: 0 }));
        eng.dispatch(100);
        eng.dispatch(101);
        println!(
            "strategies={} ticks={}",
            eng.count(),
            eng.ticks.get()
        );
        println!("borrow 必须短；不要在策略回调里再 register（会 panic）\n");
    }
}

// ============================================================================
// 场景 4：敞口 / 仓位 —— AtomicI64 热路径
// ============================================================================
/// **生产问题**：成交回报线程与风控线程都要读 `net_pos`；每 tick Mutex 锁仓位太贵。
/// 单字段标量、可接受最终一致时用原子。
///
/// **套路**：`AtomicI64` 存 net position；`fetch_add` 更新；风控 `load` O(1)。
pub mod exposure_atomic {
    use super::*;

    pub struct Position {
        net: AtomicI64,
        pub version: AtomicU64,
    }

    impl Position {
        pub fn new() -> Self {
            Self {
                net: AtomicI64::new(0),
                version: AtomicU64::new(0),
            }
        }

        pub fn on_fill(&self, qty: Qty) {
            self.net.fetch_add(qty, Ordering::Release);
            self.version.fetch_add(1, Ordering::Release);
        }

        pub fn net_pos(&self) -> Qty {
            self.net.load(Ordering::Acquire)
        }

        pub fn within_limit(&self, max: Qty) -> bool {
            self.net_pos().abs() <= max
        }
    }

    pub fn demonstrate() {
        println!("## HFT-4：仓位 AtomicI64（无 Mutex 热读）");

        let pos = Arc::new(Position::new());
        pos.on_fill(10);
        pos.on_fill(-3);
        println!(
            "net={} within_20={}",
            pos.net_pos(),
            pos.within_limit(20)
        );
        println!("复杂簿/多字段仓位仍要 Mutex 或单线程 RefCell\n");
    }
}

// ============================================================================
// 场景 5：预交易风控计数 —— Atomic + 限流窗口
// ============================================================================
/// **生产问题**：每秒下单次数限制；网关线程在 send 前必须 O(1) 检查。
///
/// **套路**：`AtomicU64` 存窗口内计数 + `Cell` 存窗口起点（单线程 reset）。
pub mod rate_limit_atomic {
    use super::*;

    pub struct OrderRateGate {
        window_start_ms: Cell<u64>,
        count: AtomicU64,
        limit: u64,
    }

    impl OrderRateGate {
        pub fn new(limit: u64) -> Self {
            Self {
                window_start_ms: Cell::new(0),
                count: AtomicU64::new(0),
                limit,
            }
        }

        pub fn maybe_advance_window(&self, now_ms: u64, window_ms: u64) {
            let start = self.window_start_ms.get();
            if now_ms >= start + window_ms {
                self.window_start_ms.set(now_ms);
                self.count.store(0, Ordering::Relaxed);
            }
        }

        pub fn try_acquire(&self) -> bool {
            let c = self.count.fetch_add(1, Ordering::Relaxed) + 1;
            c <= self.limit
        }
    }

    pub fn demonstrate() {
        println!("## HFT-5：下单速率 Atomic 闸门");

        let gate = OrderRateGate::new(3);
        gate.maybe_advance_window(1000, 1000);
        let mut ok = 0;
        for _ in 0..5 {
            if gate.try_acquire() {
                ok += 1;
            }
        }
        println!("limit=3 放行={}（后 2 笔应拒）\n", ok);
    }
}

// ============================================================================
// 场景 6：Instrument 配置 —— RwLock 热读
// ============================================================================
/// **生产问题**：每 tick 读 tick_size / lot_size；运维偶尔热更新。Mutex 让读也互斥。
///
/// **套路**：`RwLock<HashMap<SymbolId, Meta>>`；热路径 `read()`，配置推送 `write()`。
pub mod instrument_config_rwlock {
    use super::*;

    #[derive(Clone, Copy, Debug)]
    pub struct Meta {
        pub tick: Px,
        pub lot: Qty,
    }

    pub struct InstrumentTable {
        inner: RwLock<HashMap<SymbolId, Meta>>,
    }

    impl InstrumentTable {
        pub fn new(entries: &[(SymbolId, Meta)]) -> Self {
            let mut m = HashMap::new();
            for &(s, meta) in entries {
                m.insert(s, meta);
            }
            Self {
                inner: RwLock::new(m),
            }
        }

        pub fn meta(&self, sym: SymbolId) -> Option<Meta> {
            self.inner.read().ok()?.get(&sym).copied()
        }

        pub fn upsert(&self, sym: SymbolId, meta: Meta) {
            if let Ok(mut g) = self.inner.write() {
                g.insert(sym, meta);
            }
        }
    }

    pub fn demonstrate() {
        println!("## HFT-6：合约元数据 RwLock 热读");

        let table = InstrumentTable::new(&[(1, Meta { tick: 100, lot: 1 })]);
        let m1 = table.meta(1).unwrap();
        table.upsert(1, Meta { tick: 50, lot: 1 });
        let m2 = table.meta(1).unwrap();
        println!("tick {} → {}（写少读多典型 RwLock）\n", m1.tick, m2.tick);
    }
}

// ============================================================================
// 场景 7：网关指标 —— 多 Atomic 无锁统计
// ============================================================================
/// **生产问题**：sent / ack / reject 分散在 IO 线程、回调线程；聚合不能全局 Mutex。
///
/// **套路**：字段级 `AtomicU64`；监控 scrape 时 Relaxed load 求和。
pub mod gateway_metrics {
    use super::*;

    pub struct GatewayStats {
        pub sent: AtomicU64,
        pub acked: AtomicU64,
        pub rejected: AtomicU64,
    }

    impl GatewayStats {
        pub fn new() -> Self {
            Self {
                sent: AtomicU64::new(0),
                acked: AtomicU64::new(0),
                rejected: AtomicU64::new(0),
            }
        }

        pub fn record_sent(&self) {
            self.sent.fetch_add(1, Ordering::Relaxed);
        }
        pub fn record_ack(&self) {
            self.acked.fetch_add(1, Ordering::Relaxed);
        }
        pub fn record_reject(&self) {
            self.rejected.fetch_add(1, Ordering::Relaxed);
        }

        pub fn snapshot(&self) -> (u64, u64, u64) {
            (
                self.sent.load(Ordering::Relaxed),
                self.acked.load(Ordering::Relaxed),
                self.rejected.load(Ordering::Relaxed),
            )
        }
    }

    pub fn demonstrate() {
        println!("## HFT-7：网关指标全 Atomic");

        let stats = Arc::new(GatewayStats::new());
        stats.record_sent();
        stats.record_sent();
        stats.record_ack();
        stats.record_reject();
        let (s, a, r) = stats.snapshot();
        println!("sent={} ack={} reject={}\n", s, a, r);
    }
}

pub fn demonstrate() {
    orderbook_refcell::demonstrate();
    fix_session_cell::demonstrate();
    strategy_registry::demonstrate();
    exposure_atomic::demonstrate();
    rate_limit_atomic::demonstrate();
    instrument_config_rwlock::demonstrate();
    gateway_metrics::demonstrate();
}
