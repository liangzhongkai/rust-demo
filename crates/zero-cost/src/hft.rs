//! # HFT 生产场景下的零成本抽象
//!
//! 高频交易的硬约束：
//! - **延迟**：决策热路径禁止 vtable、堆分配、虚函数
//! - **吞吐**：编译期已知分支 → LLVM 可 vectorize / 展开
//! - **正确**：newtype + 泛型让「价/量/序列号」混用成为编译错误
//!
//! 下面 7 个场景是真实系统里「抽象但不付 runtime 税」的写法。

#![allow(dead_code)]

pub type TsNs = u64;

#[derive(Debug, Clone, Copy)]
pub struct Tick {
    pub ts_ns: TsNs,
    pub bid: i64,
    pub ask: i64,
}

// ============================================================================
// 场景 1：定点价格 newtype —— 替代 f64，零运行时包装
// ============================================================================
/// **生产问题**：策略用 `f64` 表示价格，出现 0.1+0.2≠0.3、NaN 传染、
/// 不同 venue 精度不一致，导致回测与实盘 PnL 对不上。
///
/// **零成本套路**：`Px(i64)` newtype，1 tick = 1e-8 USDT；运算仍是整数指令。
pub mod fixed_point_px {
    #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    pub struct Px(i64);

    impl Px {
        pub const SCALE: i64 = 100_000_000;

        #[inline]
        pub fn from_usdt(usdt: i64, frac: i64) -> Self {
            Self(usdt * Self::SCALE + frac)
        }

        #[inline]
        pub fn spread(bid: Px, ask: Px) -> i64 {
            ask.0 - bid.0
        }
    }

    pub fn demonstrate() {
        println!("## 场景 1：定点 Px newtype");
        let bid = Px::from_usdt(100, 0);
        let ask = Px::from_usdt(100, 5_000_000);
        println!("spread = {} ticks", Px::spread(bid, ask));
        println!("关键：与裸 i64 同布局；禁止 f64 进入订单簿\n");
    }
}

// ============================================================================
// 场景 2：静态分派策略 —— 泛型替代 Box<dyn Strategy>
// ============================================================================
/// **生产问题**：每 tick 调用 `Box<dyn Strategy>`，vtable + 间接分支
/// 在 500k tick/s 下占 measurable CPU，且阻碍 inline。
///
/// **零成本套路**：`run_strategy<S: Strategy>` 单态化；换策略 = 换类型，重启进程即可。
pub mod static_strategy {
    use super::*;

    pub trait Strategy {
        fn on_tick(&mut self, t: Tick) -> i64;
    }

    pub struct MidSpread {
        threshold: i64,
    }

    impl Strategy for MidSpread {
        #[inline]
        fn on_tick(&mut self, t: Tick) -> i64 {
            let mid = (t.bid + t.ask) / 2;
            let spread = t.ask - t.bid;
            if spread <= self.threshold {
                mid
            } else {
                0
            }
        }
    }

    pub struct PassThrough;
    impl Strategy for PassThrough {
        #[inline]
        fn on_tick(&mut self, t: Tick) -> i64 {
            (t.bid + t.ask) / 2
        }
    }

    #[inline]
    pub fn run<S: Strategy>(s: &mut S, ticks: &[Tick]) -> i64 {
        ticks.iter().map(|t| s.on_tick(*t)).sum()
    }

    pub fn demonstrate() {
        println!("## 场景 2：静态分派 Strategy");
        let ticks = [
            Tick { ts_ns: 1, bid: 100, ask: 102 },
            Tick { ts_ns: 2, bid: 101, ask: 103 },
        ];
        let mut mid = MidSpread { threshold: 5 };
        let mut passthrough = PassThrough;
        println!("MidSpread sum = {}", run(&mut mid, &ticks));
        println!("PassThrough sum = {}", run(&mut passthrough, &ticks));
        println!("关键：`run::<MidSpread>` 与 `run::<PassThrough>` 各一份机器码\n");
    }
}

// ============================================================================
// 场景 3：内联二进制解码 —— 热路径零分配 parse
// ============================================================================
/// **生产问题**：行情帧每微秒到达，若 decode 走 `Vec` + `format!` + trait object，
/// P99 延迟抖动明显。
///
/// **零成本套路**：`#[inline]` + 固定布局 `from_le_bytes`，返回栈上 struct。
pub mod inline_decode {
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct QuoteFrame {
        pub seq: u64,
        pub bid: i64,
        pub ask: i64,
    }

    #[inline]
    pub fn decode(buf: &[u8; 24]) -> Option<QuoteFrame> {
        let seq = u64::from_le_bytes(buf[0..8].try_into().ok()?);
        let bid = i64::from_le_bytes(buf[8..16].try_into().ok()?);
        let ask = i64::from_le_bytes(buf[16..24].try_into().ok()?);
        Some(QuoteFrame { seq, bid, ask })
    }

    pub fn demonstrate() {
        println!("## 场景 3：内联固定布局 decode");
        let mut buf = [0u8; 24];
        buf[0..8].copy_from_slice(&42u64.to_le_bytes());
        buf[8..16].copy_from_slice(&100_i64.to_le_bytes());
        buf[16..24].copy_from_slice(&102_i64.to_le_bytes());
        let q = decode(&buf).unwrap();
        println!("seq={} bid={} ask={}", q.seq, q.bid, q.ask);
        println!("关键：无 heap；LLVM 常把 decode inline 进 read 循环\n");
    }
}

// ============================================================================
// 场景 4：const 泛型环缓冲 —— 容量编译期固定，索引取模变位掩码
// ============================================================================
/// **生产问题**：SPSC 队列容量若运行时传入，编译器难以证明 bounds；
/// 取模 `% cap` 比 `& (cap-1)` 慢（cap 非 2 幂时无法优化成 mask）。
///
/// **零成本套路**：`const N: usize` 且 N 为 2 幂，索引 `& (N-1)` 在编译期已知。
pub mod const_ring {
    pub struct RingBuf<T, const N: usize> {
        data: [T; N],
        head: usize,
        tail: usize,
    }

    impl<T: Copy + Default, const N: usize> RingBuf<T, N> {
        pub const CAP: usize = N;

        pub fn new() -> Self {
            Self {
                data: [T::default(); N],
                head: 0,
                tail: 0,
            }
        }

        #[inline]
        pub fn push(&mut self, v: T) -> bool {
            let next = (self.head + 1) & (N - 1);
            if next == self.tail {
                return false;
            }
            self.data[self.head] = v;
            self.head = next;
            true
        }

        #[inline]
        pub fn pop(&mut self) -> Option<T> {
            if self.tail == self.head {
                return None;
            }
            let v = self.data[self.tail];
            self.tail = (self.tail + 1) & (N - 1);
            Some(v)
        }
    }

    pub fn demonstrate() {
        println!("## 场景 4：const 泛型 RingBuf<N=1024>");
        let mut q = RingBuf::<u64, 1024>::new();
        assert!(q.push(1));
        assert!(q.push(2));
        assert_eq!(q.pop(), Some(1));
        println!("push/pop OK；N=1024 时索引 `& 1023` 单条 AND 指令\n");
    }
}

// ============================================================================
// 场景 5：泛型订单簿侧 —— 比较器单态化，无函数指针
// ============================================================================
/// **生产问题**：买卖两侧排序规则不同（bid 降序 / ask 升序），
/// 若用 `fn cmp(a, b) -> Ordering` 回调，每次比较间接跳转。
///
/// **零成本套路**：`Side` 作为类型参数，`Ord` 实现在编译期选定。
pub mod orderbook_side {
    pub trait SideOrder {
        fn is_better_px(a: i64, b: i64) -> bool;
    }

    pub struct Bid;
    impl SideOrder for Bid {
        #[inline]
        fn is_better_px(a: i64, b: i64) -> bool {
            a > b // 买价越高越好
        }
    }

    pub struct Ask;
    impl SideOrder for Ask {
        #[inline]
        fn is_better_px(a: i64, b: i64) -> bool {
            a < b // 卖价越低越好
        }
    }

    #[inline]
    pub fn best<S: SideOrder>(levels: &[(i64, i64)]) -> Option<(i64, i64)> {
        levels.iter().copied().reduce(|a, b| {
            if S::is_better_px(a.0, b.0) {
                a
            } else if S::is_better_px(b.0, a.0) {
                b
            } else if a.1 >= b.1 {
                a
            } else {
                b
            }
        })
    }

    pub fn demonstrate() {
        println!("## 场景 5：泛型 SideOrder 比较器");
        let bids = [(100, 10), (101, 5), (99, 20)];
        let asks = [(103, 8), (102, 15), (104, 2)];
        println!("best bid = {:?}", best::<Bid>(&bids));
        println!("best ask = {:?}", best::<Ask>(&asks));
        println!("关键：`best::<Bid>` 内联 Bid::is_better_px，无 fn pointer\n");
    }
}

// ============================================================================
// 场景 6：热路径禁止 dyn —— 仅在 IO 边界用 trait object
// ============================================================================
/// **生产问题**：把整个 pipeline 写成 `Vec<Box<dyn Handler>>`，
/// 每个 tick 遍历 vtable，cache miss 严重。
///
/// **零成本套路**：热路径 `HandlerPipeline<H1, H2>` 或宏生成固定链；
/// `dyn` 只留在配置加载 / 插件边界。
pub mod handler_chain {
    use super::Tick;

    pub trait Handler {
        fn handle(&mut self, t: Tick) -> Tick;
    }

    pub struct NormalizeSpread {
        max_spread: i64,
    }
    impl Handler for NormalizeSpread {
        #[inline]
        fn handle(&mut self, t: Tick) -> Tick {
            let spread = t.ask - t.bid;
            if spread > self.max_spread {
                Tick { ask: t.bid + self.max_spread, ..t }
            } else {
                t
            }
        }
    }

    pub struct StampTs;
    impl Handler for StampTs {
        #[inline]
        fn handle(&mut self, mut t: Tick) -> Tick {
            t.ts_ns = t.ts_ns.wrapping_add(1);
            t
        }
    }

    /// 编译期固定两阶段 pipeline —— 等价于手写两次变换，无虚表。
    pub struct Pipeline<H1, H2> {
        pub h1: H1,
        pub h2: H2,
    }

    impl<H1: Handler, H2: Handler> Pipeline<H1, H2> {
        #[inline]
        pub fn process(&mut self, t: Tick) -> Tick {
            let t = self.h1.handle(t);
            self.h2.handle(t)
        }
    }

    pub fn demonstrate() {
        println!("## 场景 6：编译期 Handler 链");
        let mut pipe = Pipeline {
            h1: NormalizeSpread { max_spread: 3 },
            h2: StampTs,
        };
        let out = pipe.process(Tick { ts_ns: 0, bid: 100, ask: 110 });
        println!("normalized+stamped: bid={} ask={} ts={}", out.bid, out.ask, out.ts_ns);
        println!("关键：process 可被 inline 成单一函数体\n");
    }
}

// ============================================================================
// 场景 7：编译期 Feed 配置 —— type-level 替代 runtime flag
// ============================================================================
/// **生产问题**：`if feed == ITCH { ... } else if feed == OUCH { ... }`
/// 在热循环里造成不可预测分支；且 dead branch 仍占 icache。
///
/// **零成本套路**：`FeedParser<F: Feed>` 单态化；部署时选一种 feed 类型编译/链接。
pub mod type_level_feed {
    pub trait Feed {
        const NAME: &'static str;
        fn parse_line(line: &[u8]) -> Option<(i64, i64)>;
    }

    pub struct Itch;
    impl Feed for Itch {
        const NAME: &'static str = "ITCH";
        #[inline]
        fn parse_line(line: &[u8]) -> Option<(i64, i64)> {
            if line.len() < 16 {
                return None;
            }
            let bid = i64::from_le_bytes(line[0..8].try_into().ok()?);
            let ask = i64::from_le_bytes(line[8..16].try_into().ok()?);
            Some((bid, ask))
        }
    }

    pub struct Ouch;
    impl Feed for Ouch {
        const NAME: &'static str = "OUCH";
        #[inline]
        fn parse_line(line: &[u8]) -> Option<(i64, i64)> {
            if line.len() < 16 {
                return None;
            }
            let bid = i64::from_be_bytes(line[0..8].try_into().ok()?);
            let ask = i64::from_be_bytes(line[8..16].try_into().ok()?);
            Some((bid, ask))
        }
    }

    #[inline]
    pub fn parse_batch<F: Feed>(lines: &[&[u8]]) -> Vec<(i64, i64)> {
        lines.iter().filter_map(|l| F::parse_line(l)).collect()
    }

    pub fn demonstrate() {
        println!("## 场景 7：type-level Feed 单态化");
        let line = {
            let mut b = [0u8; 16];
            b[0..8].copy_from_slice(&100_i64.to_le_bytes());
            b[8..16].copy_from_slice(&102_i64.to_le_bytes());
            b
        };
        let itch = parse_batch::<Itch>(&[&line[..]]);
        let ouch = parse_batch::<Ouch>(&[&line[..]]);
        println!("ITCH {:?} vs OUCH {:?}", itch, ouch);
        println!("关键：运行时无 feed 分支；换 feed = 换 binary feature flag\n");
    }
}

pub fn demonstrate() {
    fixed_point_px::demonstrate();
    static_strategy::demonstrate();
    inline_decode::demonstrate();
    const_ring::demonstrate();
    orderbook_side::demonstrate();
    handler_chain::demonstrate();
    type_level_feed::demonstrate();
}
