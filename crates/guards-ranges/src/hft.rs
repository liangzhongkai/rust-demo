//! # HFT 生产场景：守卫与范围
//!
//! 高频交易里 guard/range 出现在三类硬约束上：
//! - **价格格点**：tick size、band、fat finger —— 整数 range 分档
//! - **延迟 SLA**：tick-to-trade 分桶决定路由 / 降级
//! - **风控层级**：notional / position / spread 多维 guard，arm 顺序 = 优先级
//!
//! 下面 7 个场景对应 OMS、风控网关、执行算法里的常见写法。

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
// 场景 1：价格格点校验（range + 取模 guard）
// ============================================================================
/// **生产问题**：交易所规定 tick_size=0.01，非格点价格会被拒单；
/// 离参考价过远可能是 fat finger。
///
/// **守卫/范围套路**：先 range 筛特殊值，再用 guard `(px - base) % tick == 0`。
pub mod tick_lattice {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum PxVerdict {
        Valid,
        OffTick,
        FatFinger,
        Zero,
    }

    pub fn validate(px: Px, ref_px: Px, tick: Px) -> PxVerdict {
        match px {
            0 => PxVerdict::Zero,
            p if p < ref_px / 2 || p > ref_px * 2 => PxVerdict::FatFinger,
            p if (p - ref_px).rem_euclid(tick) != 0 => PxVerdict::OffTick,
            _ => PxVerdict::Valid,
        }
    }

    pub fn demonstrate() {
        println!("## 场景 1：价格格点（特殊 range + 取模 guard）");

        let ref_px = 100_00i64;
        let tick = 5i64; // 0.05 定点
        for px in [100_00, 100_02, 100_50, 250_00, 0] {
            println!("  px={} → {:?}", px, validate(px, ref_px, tick));
        }
        println!("关键：`0` 用字面量 arm；fat finger 用 guard 而非嵌套 if\n");
    }
}

// ============================================================================
// 场景 2：延迟 SLA 分桶（纯 range dispatch）
// ============================================================================
/// **生产问题**：tick-to-trade 延迟决定走直连 FPGA 还是软件 fallback；
/// P99 监控也按桶聚合。
///
/// **范围套路**：`0..=10 / 11..=50 / ...` 闭区间分桶，边界值写进测试。
pub mod latency_sla {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Route {
        Fpga,
        KernelBypass,
        Userspace,
        Degraded,
    }

    pub fn route(latency_us: u64) -> Route {
        match latency_us {
            0..=10 => Route::Fpga,
            11..=50 => Route::KernelBypass,
            51..=200 => Route::Userspace,
            _ => Route::Degraded,
        }
    }

    pub fn demonstrate() {
        println!("## 场景 2：延迟 SLA 分桶（range dispatch）");

        for us in [5, 10, 11, 50, 51, 500] {
            println!("  {}μs → {:?}", us, route(us));
        }
        println!("关键：10 和 11 必须落不同桶；`..=` 避免 off-by-one\n");
    }
}

// ============================================================================
// 场景 3：Notional 分层路由（range + side guard）
// ============================================================================
/// **生产问题**：小单走 internalizer，中单走 smart router，大单需 TWAP /
/// 人工审批；Bid/Ask 方向影响仓位 guard。
///
/// **守卫/范围套路**：notional range 分档，guard 叠加 position 上限。
pub mod notional_tier {
    use super::*;

    #[derive(Debug, Clone, Copy)]
    pub struct Order {
        pub side: Side,
        pub px: Px,
        pub qty: Qty,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Route {
        Internalize,
        SmartRouter,
        AlgoDesk,
        Reject,
    }

    pub fn route(order: Order, position: Qty, max_pos: Qty) -> Route {
        let notional = (order.px as i128).saturating_mul(order.qty as i128);

        match (notional, order.side) {
            (n, Side::Bid) if n > 0 && position + order.qty > max_pos => Route::Reject,
            (n, Side::Ask) if n > 0 && position - order.qty < -max_pos => Route::Reject,
            (0..=100_000, _) => Route::Internalize,
            (100_001..=5_000_000, _) => Route::SmartRouter,
            (5_000_001..=i128::MAX, _) => Route::AlgoDesk,
            _ => Route::Reject,
        }
    }

    pub fn demonstrate() {
        println!("## 场景 3：Notional 分层（range + 仓位 guard）");

        let cases = [
            (Order { side: Side::Bid, px: 100, qty: 500 }, 0),
            (Order { side: Side::Bid, px: 100, qty: 60_000 }, 0),
            (Order { side: Side::Bid, px: 100, qty: 500 }, 999),
        ];
        for (o, pos) in cases {
            println!(
                "  qty={} pos={} → {:?}",
                o.qty,
                pos,
                route(o, pos, 1000)
            );
        }
        println!("关键：Reject guard 必须在 range 分档 **之前**\n");
    }
}

// ============================================================================
// 场景 4：交易时段窗口（时间 range + 休市 guard）
// ============================================================================
/// **生产问题**：CME / 港股市场有开盘、午休、夜盘；非交易时段拒单或
/// 转 GTC 队列。
///
/// **范围套路**：`(hour, minute)` 二元 range 表达 session。
pub mod session_window {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum SessionAction {
        Trade,
        QueueGtc,
        Reject,
    }

    pub fn on_order(hour: u8, minute: u8, is_holiday: bool) -> SessionAction {
        if is_holiday {
            return SessionAction::Reject;
        }
        match (hour, minute) {
            (9, 30..=59) | (10..=11, _) => SessionAction::Trade,
            (13, _) | (14, 0..=57) => SessionAction::Trade,
            (12, 0..=59) => SessionAction::QueueGtc, // 午休
            (8, 0..=29) | (14, 58..=59) => SessionAction::QueueGtc, // 预开盘 / 收市竞价
            _ => SessionAction::Reject,
        }
    }

    pub fn demonstrate() {
        println!("## 场景 4：交易时段（(hour, minute) range）");

        for (h, m) in [(9, 45), (12, 30), (3, 0)] {
            println!("  {:02}:{:02} → {:?}", h, m, on_order(h, m, false));
        }
        println!("关键：`(10..=11, _)` 一次覆盖整小时；休市用前置 guard\n");
    }
}

// ============================================================================
// 场景 5：Spread 体制分类（range + depth guard）
// ============================================================================
/// **生产问题**：宽 spread + 浅 depth → 流动性差，策略要降频或撤单。
///
/// **守卫/范围套路**：spread_bps range 分档，guard 检查 depth。
pub mod spread_regime {
    use super::Qty;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Regime {
        Tight,
        Normal,
        Wide,
        Halt,
    }

    pub fn classify(spread_bps: u32, bid_depth: Qty, ask_depth: Qty) -> Regime {
        let min_depth = bid_depth.min(ask_depth);
        match spread_bps {
            0..=2 if min_depth >= 100 => Regime::Tight,
            0..=2 => Regime::Normal,
            3..=20 if min_depth >= 50 => Regime::Normal,
            3..=20 => Regime::Wide,
            21..=100 => Regime::Wide,
            _ => Regime::Halt,
        }
    }

    pub fn demonstrate() {
        println!("## 场景 5：Spread 体制（range + depth guard）");

        for (bps, d) in [(1, 200), (1, 10), (50, 100)] {
            println!("  spread={}bps depth={} → {:?}", bps, d, classify(bps, d, d));
        }
        println!("关键：同 range 内用 guard 区分 Tight/Normal\n");
    }
}

// ============================================================================
// 场景 6：自成交防护（identity guard）
// ============================================================================
/// **生产问题**：同一 parent order 的 Bid/Ask 不能互撮，否则违反交易所规则。
///
/// **守卫套路**：`(side, own_id, counterparty_id)` 三元 guard。
pub mod self_trade_prevention {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum StpAction {
        Allow,
        CancelResting,
        CancelAggressive,
    }

    pub fn check(
        agg_side: Side,
        agg_parent: u64,
        rest_side: Side,
        rest_parent: u64,
    ) -> StpAction {
        match (agg_side, rest_side) {
            (Side::Bid, Side::Ask) | (Side::Ask, Side::Bid)
                if agg_parent == rest_parent && agg_parent != 0 =>
            {
                StpAction::CancelAggressive
            }
            (Side::Bid, Side::Ask) | (Side::Ask, Side::Bid) if agg_parent == rest_parent => {
                StpAction::CancelResting
            }
            _ => StpAction::Allow,
        }
    }

    pub fn demonstrate() {
        println!("## 场景 6：自成交防护（side 形状 + identity guard）");

        let r = check(Side::Bid, 1001, Side::Ask, 1001);
        println!("  same parent → {:?}", r);
        println!("关键：先 match side 组合，guard 过滤 parent_id\n");
    }
}

// ============================================================================
// 场景 7：熔断器多维阈值（分层 guard，与 pattern-matching 互补）
// ============================================================================
/// **生产问题**：PnL + 延迟 + 拒单率三维触发 SoftStop / HardKill。
///
/// **守卫套路**：最高优先级 arm 在前；range 用于拒单率分档。
pub mod kill_switch {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Action {
        Normal,
        SoftStop,
        HardKill,
    }

    pub fn evaluate(pnl: i64, latency_us: u64, reject_rate_pct: u8) -> Action {
        match (pnl, latency_us, reject_rate_pct) {
            (p, _, _) if p <= -1_000_000 => Action::HardKill,
            (_, l, _) if l > 500 => Action::HardKill,
            (_, _, r) if r > 50 => Action::HardKill,
            (p, _, _) if p <= -100_000 => Action::SoftStop,
            (_, l, _) if l > 100 => Action::SoftStop,
            (_, _, 20..=50) => Action::SoftStop,
            _ => Action::Normal,
        }
    }

    pub fn demonstrate() {
        println!("## 场景 7：多维熔断（guard 优先级 + reject rate range）");

        for case in [(-50_000, 50, 5), (-200_000, 30, 10), (0, 30, 55)] {
            println!("  {:?} → {:?}", case, evaluate(case.0, case.1, case.2));
        }
        println!("关键：HardKill 三维 guard 全部排在 SoftStop 之前\n");
    }
}

pub fn demonstrate() {
    tick_lattice::demonstrate();
    latency_sla::demonstrate();
    notional_tier::demonstrate();
    session_window::demonstrate();
    spread_regime::demonstrate();
    self_trade_prevention::demonstrate();
    kill_switch::demonstrate();
}
