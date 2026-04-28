//! # HFT 生产场景下的迭代器
//!
//! 高频交易的硬约束：
//! - **延迟**：单次决策 P99 < 10μs，禁止热路径堆分配
//! - **吞吐**：每秒百万级 tick，必须 cache-friendly / SIMD-friendly
//! - **正确**：永远用 *定点整数* 表示价格，绝不用 `f64`
//!
//! 下面 7 个场景是真实交易系统里的高频写法。每个场景都标注：
//! - 用了什么迭代器套路
//! - 解决什么生产问题
//! - 不用迭代器会踩什么坑

#![allow(dead_code)]

/// 全市场用定点整数表示价格：1 USDT = 100_000_000 ticks
pub type Px = i64;
pub type Qty = i64;
pub type TsNs = u64;

#[derive(Debug, Clone, Copy)]
pub struct Trade {
    pub ts_ns: TsNs,
    pub px: Px,
    pub qty: Qty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy)]
pub struct Order {
    pub id: u64,
    pub side: Side,
    pub px: Px,
    pub qty: Qty,
}

// ============================================================================
// 场景 1：VWAP 滚动窗口
// ============================================================================
/// **生产问题**：策略需要最近 N 笔成交的 VWAP（成交量加权均价）作为
/// 信号过滤器。每来一笔 tick 都要重算，不能 O(N)。
///
/// **迭代器套路**：自定义 `Iterator`，内部维护 *增量* 状态
/// `(sum_pq, sum_q)`，保证 O(1) 更新。配合 `chunks_exact` 还能批处理。
pub mod vwap_rolling {
    use super::*;
    use std::collections::VecDeque;

    pub struct RollingVwap {
        window: VecDeque<Trade>,
        cap: usize,
        sum_pq: i128, // 用 i128 防止溢出（i64*i64 累加）
        sum_q: i128,
    }

    impl RollingVwap {
        pub fn new(cap: usize) -> Self {
            Self {
                window: VecDeque::with_capacity(cap),
                cap,
                sum_pq: 0,
                sum_q: 0,
            }
        }

        /// 输入一笔 tick，返回当前 VWAP（None 表示窗口还没有数据）
        #[inline]
        pub fn push(&mut self, t: Trade) -> Option<Px> {
            self.sum_pq += (t.px as i128) * (t.qty as i128);
            self.sum_q += t.qty as i128;

            if self.window.len() == self.cap {
                let old = self.window.pop_front().unwrap();
                self.sum_pq -= (old.px as i128) * (old.qty as i128);
                self.sum_q -= old.qty as i128;
            }
            self.window.push_back(t);

            (self.sum_q != 0).then(|| (self.sum_pq / self.sum_q) as Px)
        }
    }

    /// 一种更声明式的写法：用 `scan` 把 VWAP 适配器化
    pub fn vwap_stream<I: Iterator<Item = Trade>>(it: I, cap: usize) -> impl Iterator<Item = Px> {
        let mut state = RollingVwap::new(cap);
        it.filter_map(move |t| state.push(t))
    }

    pub fn demonstrate() {
        println!("## 场景 1：VWAP 滚动窗口（O(1) 增量更新）");

        let trades = vec![
            Trade { ts_ns: 1, px: 100_00, qty: 10 },
            Trade { ts_ns: 2, px: 101_00, qty: 5 },
            Trade { ts_ns: 3, px: 99_00, qty: 20 },
            Trade { ts_ns: 4, px: 102_00, qty: 3 },
        ];

        let vwaps: Vec<Px> = vwap_stream(trades.into_iter(), 3).collect();
        println!("滚动 VWAP（窗口=3）: {:?}", vwaps);
        println!("关键：`scan/filter_map` + 内部增量状态 = 0 分配 + O(1) 更新\n");
    }
}

// ============================================================================
// 场景 2：L2 订单簿增量 diff
// ============================================================================
/// **生产问题**：交易所推送 L2 全量快照，本地需要算出和上一帧的差异
/// （增删改），只把 delta 推给下游策略，避免重复处理。
///
/// **迭代器套路**：`zip` + `filter_map`，对两个 *已排序* 的迭代器做归并比较。
/// 这是经典的「合并已排序流」模式，下面 Web3 mempool 也会复用。
pub mod l2_diff {
    use super::*;

    #[derive(Debug, Clone, Copy)]
    pub struct Level {
        pub px: Px,
        pub qty: Qty,
    }

    #[derive(Debug, PartialEq, Eq)]
    pub enum Delta {
        Inserted(Px, Qty),
        Updated(Px, Qty),
        Removed(Px),
    }

    /// `prev` 和 `curr` 都按价格升序排列（卖盘）。
    /// 返回所有 delta，零分配地懒求值。
    pub fn diff_levels<'a>(
        prev: &'a [Level],
        curr: &'a [Level],
    ) -> impl Iterator<Item = Delta> + 'a {
        // 用 itertools 的 merge_join_by 会更优雅；这里手写一个等价版本
        // 展示「双指针 + 状态机」如何包装成 Iterator。
        struct Diff<'a> {
            p: std::slice::Iter<'a, Level>,
            c: std::slice::Iter<'a, Level>,
            p_peek: Option<&'a Level>,
            c_peek: Option<&'a Level>,
        }

        impl<'a> Iterator for Diff<'a> {
            type Item = Delta;
            fn next(&mut self) -> Option<Delta> {
                loop {
                    if self.p_peek.is_none() {
                        self.p_peek = self.p.next();
                    }
                    if self.c_peek.is_none() {
                        self.c_peek = self.c.next();
                    }
                    match (self.p_peek, self.c_peek) {
                        (None, None) => return None,
                        (Some(p), None) => {
                            self.p_peek = None;
                            return Some(Delta::Removed(p.px));
                        }
                        (None, Some(c)) => {
                            self.c_peek = None;
                            return Some(Delta::Inserted(c.px, c.qty));
                        }
                        (Some(p), Some(c)) => match p.px.cmp(&c.px) {
                            std::cmp::Ordering::Less => {
                                self.p_peek = None;
                                return Some(Delta::Removed(p.px));
                            }
                            std::cmp::Ordering::Greater => {
                                self.c_peek = None;
                                return Some(Delta::Inserted(c.px, c.qty));
                            }
                            std::cmp::Ordering::Equal => {
                                let (p, c) = (*p, *c);
                                self.p_peek = None;
                                self.c_peek = None;
                                if p.qty != c.qty {
                                    return Some(Delta::Updated(c.px, c.qty));
                                }
                                // 数量相同 → 没有 delta，继续
                            }
                        },
                    }
                }
            }
        }

        Diff { p: prev.iter(), c: curr.iter(), p_peek: None, c_peek: None }
    }

    pub fn demonstrate() {
        println!("## 场景 2：L2 订单簿 diff（双指针 Iterator）");

        let prev = vec![
            Level { px: 100_00, qty: 10 },
            Level { px: 101_00, qty: 5 },
            Level { px: 102_00, qty: 8 },
        ];
        let curr = vec![
            Level { px: 100_00, qty: 10 }, // 不变
            Level { px: 101_00, qty: 7 },  // 改
            // 102_00 被吃完
            Level { px: 103_00, qty: 4 }, // 新增
        ];

        let deltas: Vec<_> = diff_levels(&prev, &curr).collect();
        println!("增量: {:?}", deltas);
        println!("关键：归并已排序流 = 自定义 Iterator + 双 peek 状态\n");
    }
}

// ============================================================================
// 场景 3：Pre-trade 风控链（短路求值）
// ============================================================================
/// **生产问题**：下单前要顺序跑一串风控（持仓限额、价格偏离、交易所限制、
/// 自成交保护…）。任何一项失败必须立即拒单，且要返回 *第一条* 失败原因。
///
/// **迭代器套路**：`try_fold` / `find_map`。一旦 closure 返回 `Err`/`Some`，
/// 整个迭代器立刻停止，不再调用后续检查 —— 短路语义对延迟至关重要。
pub mod pretrade_risk {
    use super::*;

    #[derive(Debug)]
    pub enum RiskError {
        OverPositionLimit,
        PriceDeviation,
        SelfMatch,
        ExchangeLimit,
    }

    pub struct PortfolioState {
        pub net_pos: Qty,
        pub last_mid: Px,
        pub own_resting_ids: Vec<u64>,
    }

    /// 每个 check 都是一个 `fn(&Order, &PortfolioState) -> Result<(), RiskError>`。
    /// 用 trait object 串起来，方便配置驱动。
    type Check = fn(&Order, &PortfolioState) -> Result<(), RiskError>;

    fn check_pos(o: &Order, p: &PortfolioState) -> Result<(), RiskError> {
        let after = match o.side {
            Side::Buy => p.net_pos + o.qty,
            Side::Sell => p.net_pos - o.qty,
        };
        if after.abs() > 1000 { Err(RiskError::OverPositionLimit) } else { Ok(()) }
    }

    fn check_deviation(o: &Order, p: &PortfolioState) -> Result<(), RiskError> {
        let dev = (o.px - p.last_mid).abs();
        if dev > p.last_mid / 100 { Err(RiskError::PriceDeviation) } else { Ok(()) }
    }

    fn check_self_match(o: &Order, p: &PortfolioState) -> Result<(), RiskError> {
        if p.own_resting_ids.contains(&o.id) { Err(RiskError::SelfMatch) } else { Ok(()) }
    }

    pub fn validate(order: &Order, port: &PortfolioState) -> Result<(), RiskError> {
        let checks: [Check; 3] = [check_pos, check_deviation, check_self_match];

        // try_fold：一旦 Err，整个迭代立即终止
        // 注意 `()` 作为累加器；我们只关心控制流。
        checks.iter().try_fold((), |(), check| check(order, port))
    }

    pub fn demonstrate() {
        println!("## 场景 3：Pre-trade 风控链（try_fold 短路）");

        let port = PortfolioState {
            net_pos: 950,
            last_mid: 100_00,
            own_resting_ids: vec![],
        };

        // 第一项就违反 → 后面 check 不会被调用（生产里 check 可能涉及锁/查询）
        let bad = Order { id: 1, side: Side::Buy, px: 100_00, qty: 100 };
        let good = Order { id: 2, side: Side::Buy, px: 100_00, qty: 10 };

        println!("拒单: {:?}", validate(&bad, &port));
        println!("放行: {:?}", validate(&good, &port));
        println!("关键：try_fold 在第一个 Err 处终止，省掉无意义检查的延迟\n");
    }
}

// ============================================================================
// 场景 4：Tick → OHLCV Bar 聚合
// ============================================================================
/// **生产问题**：把毫秒级 tick 流压缩成 1 秒 OHLCV bar 给策略和监控。
/// 时间桶不等间距（按 tick 时间戳分组），不能用 `chunks(N)`。
///
/// **迭代器套路**：自定义 `Iterator`，缓存「跨桶第一笔」作为 carry。
/// 这是 stateful streaming aggregation 的标准模板。
pub mod ohlcv_bars {
    use super::*;

    #[derive(Debug, Clone, Copy)]
    pub struct Bar {
        pub bucket_ns: TsNs,
        pub o: Px,
        pub h: Px,
        pub l: Px,
        pub c: Px,
        pub v: Qty,
    }

    pub struct BarAggregator<I: Iterator<Item = Trade>> {
        inner: std::iter::Peekable<I>,
        bucket_ns: TsNs,
    }

    impl<I: Iterator<Item = Trade>> BarAggregator<I> {
        pub fn new(it: I, bucket_ns: TsNs) -> Self {
            Self { inner: it.peekable(), bucket_ns }
        }
    }

    impl<I: Iterator<Item = Trade>> Iterator for BarAggregator<I> {
        type Item = Bar;

        fn next(&mut self) -> Option<Bar> {
            let first = self.inner.next()?;
            let bucket = first.ts_ns - first.ts_ns % self.bucket_ns;
            let mut bar = Bar {
                bucket_ns: bucket,
                o: first.px,
                h: first.px,
                l: first.px,
                c: first.px,
                v: first.qty,
            };

            while let Some(&t) = self.inner.peek() {
                if t.ts_ns - t.ts_ns % self.bucket_ns != bucket {
                    break;
                }
                let _ = self.inner.next();
                bar.h = bar.h.max(t.px);
                bar.l = bar.l.min(t.px);
                bar.c = t.px;
                bar.v += t.qty;
            }
            Some(bar)
        }
    }

    pub fn demonstrate() {
        println!("## 场景 4：Tick → 1 秒 OHLCV Bar");

        let trades = vec![
            Trade { ts_ns: 1_000, px: 100_00, qty: 5 },
            Trade { ts_ns: 999_999_999, px: 101_00, qty: 3 },
            Trade { ts_ns: 1_000_000_001, px: 99_00, qty: 8 },
            Trade { ts_ns: 1_500_000_000, px: 102_00, qty: 2 },
        ];

        let bars: Vec<_> = BarAggregator::new(trades.into_iter(), 1_000_000_000).collect();
        for b in &bars {
            println!("  bar@{}ns OHLC=({},{},{},{}) V={}", b.bucket_ns, b.o, b.h, b.l, b.c, b.v);
        }
        println!("关键：peekable + 桶 key 比较 = 流式分组，无需收集到 Vec\n");
    }
}

// ============================================================================
// 场景 5：延迟直方图（HDR 风格 buckets）
// ============================================================================
/// **生产问题**：要在不分配内存的前提下，从一串延迟样本里实时计算 P50/P99/P999。
///
/// **迭代器套路**：`fold` 把样本归并到固定 buckets；查询用 `scan`
/// 累积分布，再 `find` 找到分位点。整个过程 0 分配。
pub mod latency_histogram {
    pub const BUCKETS: usize = 32; // 2^0 .. 2^31 ns，覆盖 1ns..2s

    pub struct Histogram(pub [u64; BUCKETS]);

    impl Histogram {
        pub fn from_samples<I: IntoIterator<Item = u64>>(samples: I) -> Self {
            let h = samples.into_iter().fold([0u64; BUCKETS], |mut acc, ns| {
                // 找到 ns 落在哪个 2 的幂段
                let bucket = (64 - ns.leading_zeros() as usize).min(BUCKETS - 1);
                acc[bucket] += 1;
                acc
            });
            Histogram(h)
        }

        /// 返回某分位点（如 p99 = 0.99）所在 bucket 的上界（ns）。
        pub fn percentile(&self, p: f64) -> u64 {
            let total: u64 = self.0.iter().sum();
            let target = (total as f64 * p) as u64;

            // scan 累积分布，find 第一个 >= target 的 bucket
            self.0
                .iter()
                .scan(0u64, |acc, &c| {
                    *acc += c;
                    Some(*acc)
                })
                .enumerate()
                .find(|(_, cum)| *cum >= target)
                .map(|(i, _)| 1u64 << i)
                .unwrap_or(0)
        }
    }

    pub fn demonstrate() {
        println!("## 场景 5：延迟直方图（fold + scan + find）");
        // 模拟样本：典型延迟 1-10μs，偶尔有 1ms 长尾
        let samples = (0..1000).map(|i| if i % 100 == 0 { 1_000_000 } else { 5_000 + (i % 5) * 1000 });
        let h = Histogram::from_samples(samples);

        println!("P50  ≈ {} ns", h.percentile(0.50));
        println!("P99  ≈ {} ns", h.percentile(0.99));
        println!("P999 ≈ {} ns", h.percentile(0.999));
        println!("关键：fold 替代 mut 循环；scan + find 实现累积查找，无堆分配\n");
    }
}

// ============================================================================
// 场景 6：零分配 batch parser
// ============================================================================
/// **生产问题**：交易所行情用定长二进制帧推送（FIX/SBE/ITCH 都是这个套路）。
/// 收到一个大 buffer，要切成定长记录解析，热路径 *不能* 调 `Vec::new`。
///
/// **迭代器套路**：`chunks_exact` —— 只在 slice 上滑动，0 分配，且每块
/// 都是 `&[u8; N]` 般的固定切片，便于 SIMD 与编译器自动向量化。
pub mod zero_alloc_parser {
    use super::*;

    /// 24 字节定长 tick: u64 ts | i64 px | i64 qty
    pub const RECORD_LEN: usize = 24;

    #[inline(always)]
    fn parse_one(buf: &[u8]) -> Trade {
        // 这里 buf 长度被 chunks_exact 静态保证为 24
        Trade {
            ts_ns: u64::from_le_bytes(buf[0..8].try_into().unwrap()),
            px: i64::from_le_bytes(buf[8..16].try_into().unwrap()),
            qty: i64::from_le_bytes(buf[16..24].try_into().unwrap()),
        }
    }

    /// 返回 `impl Iterator`，调用者既可以 `for` 流处理，也可以 `collect`。
    pub fn parse<'a>(buf: &'a [u8]) -> impl Iterator<Item = Trade> + 'a {
        buf.chunks_exact(RECORD_LEN).map(parse_one)
        // 注：tail（不足 24B）被 chunks_exact 自动丢弃；用 .remainder() 可获取
    }

    pub fn demonstrate() {
        println!("## 场景 6：零分配 batch parser");

        // 模拟一段 wire buffer
        let mut buf = Vec::new();
        for i in 0..3 {
            buf.extend_from_slice(&(1000u64 + i).to_le_bytes());
            buf.extend_from_slice(&(100_00i64 + i as i64).to_le_bytes());
            buf.extend_from_slice(&((10 + i) as i64).to_le_bytes());
        }

        // 整条管道：解析 → 过滤 → 求和。一行 = 一个 SIMD 友好的循环
        let total_qty: Qty = parse(&buf).filter(|t| t.px >= 100_00).map(|t| t.qty).sum();
        println!("过滤 + 累加成交量 = {}（0 次堆分配）", total_qty);
        println!("关键：chunks_exact 让边界检查在循环外做一次，热体内零分支\n");
    }
}

// ============================================================================
// 场景 7：撮合引擎模拟（peekable + take_while）
// ============================================================================
/// **生产问题**：撮合时 taker 单要从对手簿最优价开始吃单，直到价格不利
/// 或 taker 数量耗尽。每一步都要看下一档的价格再决定是否消费。
///
/// **迭代器套路**：`peekable` 看一眼不消费 + `take_while` 配合状态。
/// 注意：`take_while` 闭包返回 false 后，那一项 **不会** 重新进入 inner，
/// 所以处理「能成交的 maker 流」时常常要换成手写 loop + peek。
pub mod matching_engine {
    use super::*;

    /// 限价单 taker 来吃 sorted maker 簿。
    /// `makers` 必须按 taker 视角的「最优」排序：
    /// - taker 是 Buy → makers 是 ask 簿，按 px 升序
    /// - taker 是 Sell → makers 是 bid 簿，按 px 降序
    pub fn match_taker<I: Iterator<Item = Order>>(
        taker: Order,
        makers: I,
    ) -> (Qty, Vec<(Order, Qty)>) {
        let mut remaining = taker.qty;
        let mut fills = Vec::new();
        let mut iter = makers.peekable();

        while remaining > 0 {
            let m = match iter.peek() {
                Some(m) => *m,
                None => break,
            };
            // 价格不再撮合 → 停止
            let crosses = match taker.side {
                Side::Buy => m.px <= taker.px,
                Side::Sell => m.px >= taker.px,
            };
            if !crosses {
                break;
            }
            let _ = iter.next(); // 真正消费这一档
            let traded = remaining.min(m.qty);
            fills.push((m, traded));
            remaining -= traded;
        }

        (remaining, fills)
    }

    pub fn demonstrate() {
        println!("## 场景 7：撮合引擎（peekable + 手写 while）");

        // ask 簿：按价格升序
        let asks = vec![
            Order { id: 10, side: Side::Sell, px: 100_00, qty: 5 },
            Order { id: 11, side: Side::Sell, px: 100_50, qty: 8 },
            Order { id: 12, side: Side::Sell, px: 101_00, qty: 20 }, // 价格越线
        ];
        let taker = Order { id: 99, side: Side::Buy, px: 100_50, qty: 10 };

        let (left, fills) = match_taker(taker, asks.into_iter());
        println!("Taker 剩余 = {}", left);
        for (m, q) in fills {
            println!("  吃 maker#{} @{} × {}", m.id, m.px, q);
        }
        println!("关键：peek 决定要不要 next，是 take_while 不能完全替代的模式\n");
    }
}

pub fn demonstrate() {
    vwap_rolling::demonstrate();
    l2_diff::demonstrate();
    pretrade_risk::demonstrate();
    ohlcv_bars::demonstrate();
    latency_histogram::demonstrate();
    zero_alloc_parser::demonstrate();
    matching_engine::demonstrate();
}
