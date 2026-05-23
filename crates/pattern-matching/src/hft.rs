//! # HFT 生产场景下的模式匹配
//!
//! 高频交易的硬约束：
//! - **延迟**：热路径分支必须可预测，避免 trait object 动态分发
//! - **正确**：状态机必须穷尽，漏分支 = 资金事故
//! - **可读**：协议字段多，解构比手动 `.field` 更不易写错
//!
//! 下面 7 个场景是真实交易系统里的高频写法。每个场景都标注：
//! - 用了什么模式匹配套路
//! - 解决什么生产问题
//! - 不用 match 会踩什么坑

#![allow(dead_code)]

pub type Px = i64;
pub type Qty = i64;
pub type TsNs = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Bid,
    Ask,
}

// ============================================================================
// 场景 1：行情消息分发（enum 穷尽 dispatch）
// ============================================================================
/// **生产问题**：交易所二进制 feed 每秒百万条，每条消息类型不同
/// （Heartbeat / Snapshot / Add / Modify / Delete / Trade），
/// 必须用零分配、可 inline 的分发逻辑。
///
/// **模式匹配套路**：顶层 `match msg`，每个 variant 解构出字段后直接处理。
/// 编译器生成跳转表（jump table）或 if-chain，无 vtable。
pub mod md_dispatch {
    use super::*;

    #[derive(Debug, Clone, Copy)]
    pub enum MdMsg {
        Heartbeat { seq: u64 },
        Add { side: Side, px: Px, qty: Qty },
        Modify { side: Side, px: Px, old_qty: Qty, new_qty: Qty },
        Delete { side: Side, px: Px },
        Trade { px: Px, qty: Qty, aggressor: Side },
    }

    #[derive(Debug, Default)]
    pub struct BookStats {
        pub adds: u64,
        pub modifies: u64,
        pub deletes: u64,
        pub trades: u64,
        pub heartbeats: u64,
    }

    #[inline]
    pub fn apply(stats: &mut BookStats, msg: MdMsg) {
        match msg {
            MdMsg::Heartbeat { .. } => stats.heartbeats += 1,
            MdMsg::Add { .. } => stats.adds += 1,
            MdMsg::Modify { .. } => stats.modifies += 1,
            MdMsg::Delete { .. } => stats.deletes += 1,
            MdMsg::Trade { .. } => stats.trades += 1,
        }
    }

    pub fn demonstrate() {
        println!("## 场景 1：行情消息分发（enum 穷尽 dispatch）");

        let feed = [
            MdMsg::Heartbeat { seq: 1 },
            MdMsg::Add {
                side: Side::Bid,
                px: 100_00,
                qty: 50,
            },
            MdMsg::Trade {
                px: 100_00,
                qty: 10,
                aggressor: Side::Ask,
            },
            MdMsg::Delete {
                side: Side::Bid,
                px: 99_50,
            },
        ];

        let mut stats = BookStats::default();
        for m in feed {
            apply(&mut stats, m);
        }
        println!("统计: {:?}", stats);
        println!("关键：enum match 无堆分配，编译器可生成 jump table\n");
    }
}

// ============================================================================
// 场景 2：订单生命周期状态机
// ============================================================================
/// **生产问题**：OMS 里每个订单经历 New → Ack → PartialFill → Filled /
/// Cancelled / Rejected。非法转移必须被编译期或运行时拦住。
///
/// **模式匹配套路**：`(current_state, event)` 二元 match，穷尽所有合法转移。
pub mod order_fsm {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum OrderState {
        New,
        Acked,
        Partial,
        Filled,
        Cancelled,
        Rejected,
    }

    #[derive(Debug, Clone, Copy)]
    pub enum OrderEvent {
        Ack,
        Fill { qty: Qty },
        CancelAck,
        Reject { code: u16 },
    }

    #[derive(Debug)]
    pub enum FsmError {
        InvalidTransition {
            from: OrderState,
            event: &'static str,
        },
    }

    pub fn transition(
        state: OrderState,
        ev: OrderEvent,
        filled: &mut Qty,
        total: Qty,
    ) -> Result<OrderState, FsmError> {
        use OrderEvent::*;
        use OrderState::*;

        match (state, ev) {
            (New, Ack) => Ok(Acked),
            (Acked | Partial, Fill { qty }) if qty > 0 => {
                *filled += qty;
                if *filled >= total {
                    Ok(Filled)
                } else {
                    Ok(Partial)
                }
            }
            (Acked | Partial, CancelAck) => Ok(Cancelled),
            (New, Reject { .. }) => Ok(Rejected),
            (s, _) if matches!(s, Filled | Cancelled | Rejected) => {
                Err(FsmError::InvalidTransition {
                    from: s,
                    event: "terminal state",
                })
            }
            (from, _) => Err(FsmError::InvalidTransition {
                from,
                event: "unknown",
            }),
        }
    }

    pub fn demonstrate() {
        println!("## 场景 2：订单生命周期 (state, event) 二元 match");

        let total = 100i64;
        let mut filled = 0i64;
        let mut state = OrderState::New;

        for ev in [
            OrderEvent::Ack,
            OrderEvent::Fill { qty: 30 },
            OrderEvent::Fill { qty: 70 },
        ] {
            state = transition(state, ev, &mut filled, total).unwrap();
            println!("  event {:?} → state {:?}, filled={}", ev, state, filled);
        }
        println!("关键：二元 match + 守卫表达 FSM，漏 arm 编译器会警告\n");
    }
}

// ============================================================================
// 场景 3：风控闸门（守卫 + 范围模式）
// ============================================================================
/// **生产问题**：下单前要做多层风控（价格带、仓位、notional）。
/// 不同违规级别触发不同动作：Warn / Throttle / Reject / KillSwitch。
///
/// **模式匹配套路**：先解构 Order，再用 `if guard` 分层；范围模式 `px @ 0..=0`
/// 处理特殊值。
pub mod risk_gate {
    use super::*;

    #[derive(Debug, Clone, Copy)]
    pub struct Order {
        pub side: Side,
        pub px: Px,
        pub qty: Qty,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum RiskAction {
        Pass,
        Warn,
        Reject,
        KillSwitch,
    }

    pub fn check(order: Order, position: Qty, max_pos: Qty, ref_px: Px) -> RiskAction {
        let notional = (order.px as i128) * (order.qty as i128);
        let band = ref_px / 20; // ±5% 价格带

        match order {
            Order { px: 0, .. } => RiskAction::Reject,
            Order { qty: q, .. } if q <= 0 => RiskAction::Reject,
            Order { px, side: _, qty } if px < ref_px - band || px > ref_px + band => {
                if notional > 1_000_000_000 {
                    RiskAction::KillSwitch
                } else {
                    RiskAction::Warn
                }
            }
            Order { side: Side::Bid, qty, .. }
                if position + qty > max_pos =>
            {
                RiskAction::Reject
            }
            Order { side: Side::Ask, qty, .. }
                if position - qty < -max_pos =>
            {
                RiskAction::Reject
            }
            _ => RiskAction::Pass,
        }
    }

    pub fn demonstrate() {
        println!("## 场景 3：风控闸门（解构 + 守卫 + 分层动作）");

        let ref_px = 100_00i64;
        let cases = [
            Order {
                side: Side::Bid,
                px: 100_00,
                qty: 10,
            },
            Order {
                side: Side::Bid,
                px: 120_00,
                qty: 500,
            },
            Order {
                side: Side::Bid,
                px: 0,
                qty: 1,
            },
        ];
        for o in cases {
            println!(
                "  px={} qty={} → {:?}",
                o.px,
                o.qty,
                check(o, 0, 1000, ref_px)
            );
        }
        println!("关键：守卫把「模式形状」和「业务条件」拆开，可读性高\n");
    }
}

// ============================================================================
// 场景 4：多交易所 feed 复用（嵌套 match）
// ============================================================================
/// **生产问题**：同时接 CME / Binance / 内部暗池，消息格式不同但
/// 最终都要归一化成统一的 `NormalizedTick`。
///
/// **模式匹配套路**：嵌套 match `(venue, raw_msg)`，外层选解析器，内层解构字段。
pub mod venue_multiplex {
    use super::*;

    #[derive(Debug, Clone, Copy)]
    pub enum Venue {
        Cme,
        Binance,
        DarkPool,
    }

    #[derive(Debug, Clone, Copy)]
    pub enum RawMsg {
        CmeTrade { px: Px, qty: Qty },
        BnAggTrade { p: f64, q: f64 }, // 教学：生产里仍应定点化
        DarkQuote { bid: Px, ask: Px },
    }

    #[derive(Debug, Clone, Copy)]
    pub struct NormalizedTick {
        pub px: Px,
        pub qty: Qty,
    }

    pub fn normalize(venue: Venue, raw: RawMsg) -> Option<NormalizedTick> {
        match (venue, raw) {
            (Venue::Cme, RawMsg::CmeTrade { px, qty }) => Some(NormalizedTick { px, qty }),
            (Venue::Binance, RawMsg::BnAggTrade { p, q }) => {
                let px = (p * 100.0) as Px;
                let qty = q as Qty;
                Some(NormalizedTick { px, qty })
            }
            (Venue::DarkPool, RawMsg::DarkQuote { bid, ask }) => {
                Some(NormalizedTick {
                    px: (bid + ask) / 2,
                    qty: 0,
                })
            }
            _ => None, // venue 与 msg 类型不匹配
        }
    }

    pub fn demonstrate() {
        println!("## 场景 4：多交易所复用（嵌套 (venue, msg) match）");

        let tick = normalize(
            Venue::Cme,
            RawMsg::CmeTrade {
                px: 100_00,
                qty: 5,
            },
        );
        println!("  CME trade → {:?}", tick);

        let mismatch = normalize(
            Venue::Cme,
            RawMsg::BnAggTrade { p: 100.0, q: 1.0 },
        );
        println!("  类型错配 → {:?}", mismatch);
        println!("关键：嵌套 match 替代 if 链 + cast，错配返回 None\n");
    }
}

// ============================================================================
// 场景 5：Tick 分桶（slice 模式 + 范围）
// ============================================================================
/// **生产问题**：把 tick 流按价格区间分桶做 volume profile /
/// 冰山单检测。边界 tick 必须归到正确桶。
///
/// **模式匹配套路**：`[head, tail @ ..]` slice 模式拆首元素；
/// `(px / tick_size)` 配合 range 做档位归类。
pub mod tick_bucket {
    use super::*;

    #[derive(Debug, Clone, Copy)]
    pub struct Tick {
        pub px: Px,
        pub qty: Qty,
    }

    pub fn bucket_index(px: Px, tick_size: Px, base: Px) -> usize {
        let slot = (px - base) / tick_size;
        match slot {
            s if s < 0 => 0,
            0..=9 => slot as usize,
            _ => 9, // 超出范围归到最后一桶
        }
    }

    pub fn split_batch(batch: &[Tick]) -> Option<(&Tick, &[Tick])> {
        match batch {
            [] => None,
            [first, rest @ ..] => Some((first, rest)),
        }
    }

    pub fn demonstrate() {
        println!("## 场景 5：Tick 分桶（slice 模式 + 范围）");

        let ticks = [
            Tick {
                px: 100_00,
                qty: 1,
            },
            Tick {
                px: 100_05,
                qty: 2,
            },
            Tick {
                px: 100_50,
                qty: 3,
            },
        ];
        let tick_size = 5;
        let base = 100_00;

        if let Some((head, tail)) = split_batch(&ticks) {
            println!("  head px={}, tail len={}", head.px, tail.len());
        }

        for t in &ticks {
            println!("  px={} → bucket {}", t.px, bucket_index(t.px, tick_size, base));
        }
        println!("关键：`[first, rest @ ..]` 零拷贝拆 batch\n");
    }
}

// ============================================================================
// 场景 6：熔断器 / Kill Switch（分层守卫）
// ============================================================================
/// **生产问题**：当日 PnL 跌破阈值、或 tick-to-trade 延迟超标时，
/// 必须立刻停止发单。级别不同：SoftStop（只 cancel）vs HardKill（全撤 + 告警）。
///
/// **模式匹配套路**：对 `(pnl, latency_us)` 二元组做 range + 守卫组合。
pub mod circuit_breaker {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum BreakerAction {
        Normal,
        SoftStop,
        HardKill,
    }

    pub fn evaluate(pnl: i64, latency_us: u64) -> BreakerAction {
        use BreakerAction::*;
        match (pnl, latency_us) {
            (p, _) if p <= -1_000_000 => HardKill,
            (_, l) if l > 500 => HardKill,
            (p, _) if p <= -100_000 => SoftStop,
            (_, l) if l > 100 => SoftStop,
            _ => Normal,
        }
    }

    pub fn demonstrate() {
        println!("## 场景 6：熔断器（(pnl, latency) 二元 range match）");

        for (pnl, lat) in [(-50_000, 50), (-200_000, 30), (-50_000, 200)] {
            println!("  pnl={}, latency={}μs → {:?}", pnl, lat, evaluate(pnl, lat));
        }
        println!("关键：多维度守卫比嵌套 if 更清楚表达优先级\n");
    }
}

// ============================================================================
// 场景 7：执行算法路由（struct 解构 + 字段守卫）
// ============================================================================
/// **生产问题**：同一 OMS 要路由 TWAP / IOC / POV / Iceberg 等算法，
/// 每种对 order 字段约束不同（time_in_force、display_qty、urgency）。
///
/// **模式匹配套路**：解构 `OrderReq { algo, tif, .. }`，守卫过滤非法组合。
pub mod exec_algo_router {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum TimeInForce {
        Gtc,
        Ioc,
        Fok,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Algo {
        Twap,
        Ioc,
        Iceberg,
        Pov,
    }

    #[derive(Debug, Clone, Copy)]
    pub struct OrderReq {
        pub side: Side,
        pub px: Px,
        pub qty: Qty,
        pub tif: TimeInForce,
        pub display_qty: Option<Qty>,
        pub algo: Algo,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Route {
        Scheduler,
        ImmediateMatch,
        HiddenLiquidity,
        VolumeParticipation,
    }

    pub fn route(req: OrderReq) -> Result<Route, &'static str> {
        match req {
            OrderReq {
                algo: Algo::Twap,
                tif: TimeInForce::Gtc,
                ..
            } => Ok(Route::Scheduler),
            OrderReq {
                algo: Algo::Ioc,
                tif: TimeInForce::Ioc | TimeInForce::Fok,
                ..
            } => Ok(Route::ImmediateMatch),
            OrderReq {
                algo: Algo::Iceberg,
                display_qty: Some(d),
                qty,
                ..
            } if d > 0 && d < qty => Ok(Route::HiddenLiquidity),
            OrderReq {
                algo: Algo::Pov,
                tif: TimeInForce::Gtc,
                ..
            } => Ok(Route::VolumeParticipation),
            OrderReq {
                algo: Algo::Iceberg,
                display_qty: None,
                ..
            } => Err("iceberg requires display_qty"),
            _ => Err("algo/tif mismatch"),
        }
    }

    pub fn demonstrate() {
        println!("## 场景 7：执行算法路由（struct 解构 + or-pattern）");

        let req = OrderReq {
            side: Side::Bid,
            px: 100_00,
            qty: 1000,
            tif: TimeInForce::Gtc,
            display_qty: Some(50),
            algo: Algo::Iceberg,
        };
        println!("  iceberg → {:?}", route(req));
        println!("关键：`Ioc | Fok` or-pattern 减少重复 arm\n");
    }
}

pub fn demonstrate() {
    md_dispatch::demonstrate();
    order_fsm::demonstrate();
    risk_gate::demonstrate();
    venue_multiplex::demonstrate();
    tick_bucket::demonstrate();
    circuit_breaker::demonstrate();
    exec_algo_router::demonstrate();
}
