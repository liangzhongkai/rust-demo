//! # HFT 生产场景下的生命周期
//!
//! 高频交易硬约束：
//! - **零拷贝**：FIX/ITCH/SBE 热路径禁止分配；字段必须是 `'buf` 切片视图
//! - **tick 边界**：一次行情事件内的临时对象同生共死 → arena / bump
//! - **单线程 reactor**：策略链可传 `MarketCtx<'a>`；borrow 不能跨线程
//! - **网关边界**：发单线程必须拿到 owned `ClOrdId`，不能持上游 buffer 引用
//!
//! 下面 7 个场景对应行情解析、簿视图、策略上下文、符号表、HRTB 回调、跨线程边界。

#![allow(dead_code)]

use std::sync::mpsc;
use std::thread;

pub type Px = i64;
pub type Qty = i64;

const SOH: u8 = 0x01;

// ============================================================================
// 场景 1：FIX 零拷贝 —— `NewOrder<'buf>` 绑定 wire buffer
// ============================================================================
/// **生产问题**：每秒数万条 ExecutionReport / NewOrderSingle，不能 `String::from` 每个 tag。
///
/// **套路**：`parse_new_order(raw: &'buf [u8]) -> NewOrder<'buf>`；ClOrdId 仍是 `&[u8]`。
pub mod fix_zero_copy {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Side {
        Buy,
        Sell,
    }

    #[derive(Debug)]
    pub struct NewOrder<'buf> {
        pub cl_ord_id: &'buf [u8],
        pub side: Side,
        pub px: Px,
        pub qty: Qty,
    }

    struct Field<'buf> {
        tag: u16,
        value: &'buf [u8],
    }

    fn scan_fields<'buf>(raw: &'buf [u8]) -> impl Iterator<Item = Field<'buf>> + 'buf {
        raw.split(|&b| b == SOH).filter_map(|chunk| {
            let eq = chunk.iter().position(|&b| b == b'=')?;
            let tag = std::str::from_utf8(&chunk[..eq]).ok()?.parse().ok()?;
            Some(Field {
                tag,
                value: &chunk[eq + 1..],
            })
        })
    }

    fn parse_decimal(bytes: &[u8]) -> Result<i64, &'static str> {
        let s = std::str::from_utf8(bytes).map_err(|_| "bad utf8")?;
        s.parse().map_err(|_| "bad number")
    }

    pub fn parse_new_order(raw: &[u8]) -> Result<NewOrder<'_>, &'static str> {
        let mut cl_ord_id = None;
        let mut side = None;
        let mut px = None;
        let mut qty = None;

        for f in scan_fields(raw) {
            match f.tag {
                11 => cl_ord_id = Some(f.value),
                54 => {
                    side = Some(match f.value {
                        b"1" => Side::Buy,
                        b"2" => Side::Sell,
                        _ => return Err("invalid side"),
                    });
                }
                44 => px = Some(parse_decimal(f.value)?),
                38 => qty = Some(parse_decimal(f.value)?),
                _ => {}
            }
        }

        Ok(NewOrder {
            cl_ord_id: cl_ord_id.ok_or("missing 11")?,
            side: side.ok_or("missing 54")?,
            px: px.ok_or("missing 44")?,
            qty: qty.ok_or("missing 38")?,
        })
    }

    pub fn demonstrate() {
        println!("## HFT-1：FIX 零拷贝 NewOrder<'buf>");

        let wire = b"11=ORD-20250723-001\x0154=1\x0144=10050\x0138=500\x01";
        let order = parse_new_order(wire).unwrap();
        println!(
            "cl_ord_id={:?} side={:?} px={} qty={}",
            std::str::from_utf8(order.cl_ord_id).unwrap(),
            order.side,
            order.px,
            order.qty
        );
        println!("关键：order 不能比 wire buffer 活得更久\n");
    }
}

// ============================================================================
// 场景 2：Tick-scoped Arena —— 一批临时对象同绑一次事件
// ============================================================================
/// **生产问题**：每个 tick 要建临时 level 表、字符串拼接、小 Vec；堆分配抖动明显。
///
/// **套路**：`Bump::new()`  per tick；`drop(bump)` 后所有 `'arena` 引用失效。
pub mod tick_arena {
    use super::*;
    use bumpalo::collections::Vec as BumpVec;
    use bumpalo::Bump;

    #[derive(Debug)]
    pub struct Level<'arena> {
        pub px: Px,
        pub qty: Qty,
        pub tag: &'arena str,
    }

    pub fn process_tick<'arena>(
        bump: &'arena Bump,
        deltas: &[(Px, Qty, &str)],
    ) -> BumpVec<'arena, Level<'arena>> {
        let mut levels = BumpVec::new_in(bump);
        for &(px, qty, tag) in deltas {
            let tag_in_arena: &str = bump.alloc_str(tag);
            levels.push(Level {
                px,
                qty,
                tag: tag_in_arena,
            });
        }
        levels
    }

    pub fn demonstrate() {
        println!("## HFT-2：Tick-scoped Arena");

        let bump = Bump::new();
        let deltas = [(100_50, 200, "BID"), (100_55, 150, "ASK")];
        let levels = process_tick(&bump, &deltas);
        for lv in levels.iter() {
            println!("  level px={} qty={} tag={}", lv.px, lv.qty, lv.tag);
        }
        drop(levels);
        drop(bump);
        println!("bump drop 后 arena 内引用全部失效 —— 不可泄漏到下一 tick\n");
    }
}

// ============================================================================
// 场景 3：策略链 MarketCtx —— 同一次 borrow 贯穿 reactor
// ============================================================================
/// **生产问题**：行情 → 簿 → 策略 → 风控，希望传 `&MarketCtx` 而非 clone 整簿。
///
/// **套路**：`MarketCtx<'snap>` 持有 `&'snap [Level]`；整条链在同一线程、同一 scope 内完成。
pub mod market_ctx_pipeline {
    use super::*;

    pub struct Level {
        pub px: Px,
        pub qty: Qty,
    }

    pub struct MarketCtx<'snap> {
        pub symbol: &'snap str,
        pub bids: &'snap [Level],
        pub ts_ns: u64,
    }

    pub trait Strategy {
        fn on_ctx(&mut self, ctx: &MarketCtx<'_>) -> Option<Px>;
    }

    pub struct MidPxStrategy;

    impl Strategy for MidPxStrategy {
        fn on_ctx(&mut self, ctx: &MarketCtx<'_>) -> Option<Px> {
            let best_bid = ctx.bids.first()?.px;
            let best_ask = best_bid + 5; // 简化
            Some((best_bid + best_ask) / 2)
        }
    }

    pub fn run_pipeline<'snap>(
        ctx: &MarketCtx<'snap>,
        strategy: &mut impl Strategy,
    ) -> Option<Px> {
        strategy.on_ctx(ctx)
    }

    pub fn demonstrate() {
        println!("## HFT-3：MarketCtx<'snap> 策略链");

        let symbol = "BTC-USDT";
        let bids = [Level { px: 100_000, qty: 10 }, Level { px: 99_990, qty: 5 }];
        let ctx = MarketCtx {
            symbol,
            bids: &bids,
            ts_ns: 1_725_000_000_000_000,
        };
        let mut strat = MidPxStrategy;
        let mid = run_pipeline(&ctx, &mut strat);
        println!("symbol={} mid={:?}", ctx.symbol, mid);
        println!("ctx 引用不能存进 `static` 或跨 tick 缓存\n");
    }
}

// ============================================================================
// 场景 4：符号表 —— `&'static str` 启动时 intern
// ============================================================================
/// **生产问题**：每条消息带 symbol 字符串，反复比较/哈希 `String` 浪费 CPU。
///
/// **套路**：启动加载 universe → `&'static str` 或 `SymbolId`；热路径只比整数。
pub mod static_symbol_table {
    use std::collections::HashMap;

    pub struct SymbolRegistry {
        by_id: Vec<&'static str>,
        by_name: HashMap<&'static str, u32>,
    }

    impl SymbolRegistry {
        pub fn from_static_list(names: &[&'static str]) -> Self {
            let mut by_id = Vec::new();
            let mut by_name = HashMap::new();
            for (i, &name) in names.iter().enumerate() {
                let id = i as u32;
                by_id.push(name);
                by_name.insert(name, id);
            }
            Self { by_id, by_name }
        }

        pub fn resolve(&self, name: &str) -> Option<u32> {
            self.by_name.get(name).copied()
        }

        pub fn name(&self, id: u32) -> Option<&'static str> {
            self.by_id.get(id as usize).copied()
        }
    }

    pub fn demonstrate() {
        println!("## HFT-4：符号表 `&'static str`");

        let reg = SymbolRegistry::from_static_list(&["BTC-USDT", "ETH-USDT", "SOL-USDT"]);
        let id = reg.resolve("ETH-USDT").unwrap();
        println!("ETH-USDT -> id={} name={}", id, reg.name(id).unwrap());
        println!("热路径用 SymbolId；`'static` 仅用于启动配置 intern\n");
    }
}

// ============================================================================
// 场景 5：簿快照视图 —— `BookSnapshot<'snap>` 不拷贝 levels
// ============================================================================
/// **生产问题**：风控 / 策略要读 top-N，不能每次 clone 整个 `Vec<Level>`。
///
/// **套路**：快照结构只持 `&'snap [Level]`；快照寿命 ≤ 簿更新 scope。
pub mod book_snapshot_view {
    use super::*;

    pub struct Level {
        pub px: Px,
        pub qty: Qty,
    }

    pub struct BookSnapshot<'snap> {
        pub bids: &'snap [Level],
        pub asks: &'snap [Level],
        pub seq: u64,
    }

    impl<'snap> BookSnapshot<'snap> {
        pub fn spread(&self) -> Option<Px> {
            let bid = self.bids.first()?.px;
            let ask = self.asks.first()?.px;
            Some(ask - bid)
        }
    }

    pub fn demonstrate() {
        println!("## HFT-5：BookSnapshot<'snap> 零拷贝读 top-N");

        let bids = [Level { px: 100, qty: 50 }, Level { px: 99, qty: 30 }];
        let asks = [Level { px: 101, qty: 40 }];
        let snap = BookSnapshot {
            bids: &bids,
            asks: &asks,
            seq: 42,
        };
        println!("spread={:?} seq={}", snap.spread(), snap.seq);
        println!("下一帧更新前 snap 有效；不能 async 持有\n");
    }
}

// ============================================================================
// 场景 6：HRTB 回调 —— `for<'a> Fn(&'a Tick)` 适配任意短借
// ============================================================================
/// **生产问题**：feed handler 注册多种回调，有的读栈上 tick，有的读 ring buffer 槽位。
///
/// **套路**：HRTB 让 handler 对「任何 `'a` 的 `&Tick`」都合法。
pub mod hrtb_feed_handler {
    use super::*;

    #[derive(Clone, Copy, Debug)]
    pub struct Tick {
        pub bid: Px,
        pub ask: Px,
        pub ts_ns: u64,
    }

    pub struct FeedRouter<F> {
        handler: F,
        pub count: u64,
    }

    impl<F> FeedRouter<F>
    where
        F: for<'a> Fn(&'a Tick),
    {
        pub fn new(handler: F) -> Self {
            Self { handler, count: 0 }
        }

        pub fn dispatch<'a>(&mut self, tick: &'a Tick) {
            (self.handler)(tick);
            self.count += 1;
        }
    }

    pub fn demonstrate() {
        println!("## HFT-6：HRTB feed handler");

        let mut router = FeedRouter::new(|t: &Tick| {
            let _spread = t.ask - t.bid;
        });

        let stack_tick = Tick {
            bid: 100,
            ask: 101,
            ts_ns: 1,
        };
        router.dispatch(&stack_tick);

        let ring = [Tick { bid: 200, ask: 201, ts_ns: 2 }];
        router.dispatch(&ring[0]);
        println!("dispatched {} ticks\n", router.count);
    }
}

// ============================================================================
// 场景 7：网关边界 —— borrowed → owned（Cow / Vec）再跨线程
// ============================================================================
/// **生产问题**：解析线程拿到 `NewOrder<'buf>`，发单线程生命周期独立。
///
/// **套路**：边界处 `Cow::Owned` 或 `to_vec()`；channel 传 owned。
pub mod gateway_own_boundary {
    use super::*;
    use fix_zero_copy::{parse_new_order, Side};

    pub struct GatewayOrder {
        pub cl_ord_id: Vec<u8>,
        pub side: Side,
        pub px: Px,
        pub qty: Qty,
    }

    impl GatewayOrder {
        pub fn from_borrowed(order: fix_zero_copy::NewOrder<'_>) -> Self {
            Self {
                cl_ord_id: order.cl_ord_id.to_vec(),
                side: order.side,
                px: order.px,
                qty: order.qty,
            }
        }
    }

    pub fn demonstrate() {
        println!("## HFT-7：网关边界 borrowed → owned");

        let wire = b"11=GW-001\x0154=2\x0144=9990\x0138=100\x01".to_vec();
        let parsed = parse_new_order(&wire).unwrap();
        let owned = GatewayOrder::from_borrowed(parsed);
        // wire 可以回收；owned 可跨线程
        drop(wire);

        let (tx, rx) = mpsc::channel();
        let order_for_thread = owned;
        thread::spawn(move || {
            tx.send(order_for_thread.cl_ord_id.len()).unwrap();
        })
        .join()
        .unwrap();
        let len = rx.recv().unwrap();
        println!("gateway thread received order cl_ord_id len={}", len);
        println!("规则：跨线程 / channel 必须 `'static` 或 owned\n");
    }
}

pub fn demonstrate() {
    fix_zero_copy::demonstrate();
    tick_arena::demonstrate();
    market_ctx_pipeline::demonstrate();
    static_symbol_table::demonstrate();
    book_snapshot_view::demonstrate();
    hrtb_feed_handler::demonstrate();
    gateway_own_boundary::demonstrate();
}
