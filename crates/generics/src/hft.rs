//! # HFT 生产场景下的泛型
//!
//! 高频交易的硬约束：
//! - **延迟**：热路径禁止动态分发与隐式堆分配
//! - **正确**：价格/数量单位不可混用；协议字段布局编译期确定
//! - **扩展**：多 venue / 多资产共享框架，但具体类型在编译期选定
//!
//! 下面 7 个场景是真实交易系统里的高频写法。每个场景都标注：
//! - 用了什么泛型套路
//! - 解决什么生产问题
//! - 不用泛型会踩什么坑

#![allow(dead_code)]

pub type Px = i64;
pub type Qty = i64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

// ============================================================================
// 场景 1：单位安全新类型（PhantomData 品牌）
// ============================================================================
/// **生产问题**：Binance 用 8 位小数 tick，CME 用 1/4 tick；把两个 venue 的
/// `i64` 价格直接相加会导致静默错单。风控和 PnL 报表里 USD 与 bps 混用更致命。
///
/// **泛型套路**：`Price<Venue>` + `PhantomData<Venue>`，同底层表示、不同类型。
/// 转换必须显式 `convert()`，编译器拦住隐式混算。
pub mod unit_branding {
    use std::marker::PhantomData;

    pub struct VenueBinance;
    pub struct VenueCme;

    pub struct Price<V> {
        raw: i64,
        _v: PhantomData<V>,
    }

    impl<V> Price<V> {
        pub fn new(raw: i64) -> Self {
            Self {
                raw,
                _v: PhantomData,
            }
        }

        pub fn raw(&self) -> i64 {
            self.raw
        }
    }

    /// 显式换算 —— 唯一合法跨 venue 路径。
    pub fn cme_to_binance(p: Price<VenueCme>) -> Price<VenueBinance> {
        Price::new(p.raw() * 4)
    }

    pub fn demonstrate() {
        println!("## HFT-1：PhantomData 品牌防混价");

        let cme = Price::<VenueCme>::new(2500);
        let bnb = cme_to_binance(cme);
        println!("CME 2500 → Binance raw = {}", bnb.raw());
        println!("`Price<Cme> + Price<Binance>` 无法编译；比运行期 assert 更早失败\n");
    }
}

// ============================================================================
// 场景 2：const 泛型定长 SPSC 队列
// ============================================================================
/// **生产问题**：行情网关 → 策略线程用无锁 SPSC，容量在部署时确定（512/1024），
/// 运行期不应再 malloc；不同策略实例容量不同但逻辑相同。
///
/// **泛型套路**：`SpscQueue<T, const CAP: usize>`，栈上数组 + 编译期容量检查。
pub mod spsc_ring {
    use std::mem::MaybeUninit;

    pub struct SpscQueue<T, const CAP: usize> {
        buf: [MaybeUninit<T>; CAP],
        head: usize,
        tail: usize,
        len: usize,
    }

    impl<T, const CAP: usize> SpscQueue<T, CAP> {
        pub fn new() -> Self {
            Self {
                buf: std::array::from_fn(|_| MaybeUninit::uninit()),
                head: 0,
                tail: 0,
                len: 0,
            }
        }

        pub fn try_push(&mut self, v: T) -> Result<(), T> {
            if self.len == CAP {
                return Err(v);
            }
            self.buf[self.tail].write(v);
            self.tail = (self.tail + 1) % CAP;
            self.len += 1;
            Ok(())
        }

        pub fn try_pop(&mut self) -> Option<T> {
            if self.len == 0 {
                return None;
            }
            let v = unsafe { self.buf[self.head].assume_init_read() };
            self.head = (self.head + 1) % CAP;
            self.len -= 1;
            Some(v)
        }

        pub fn len(&self) -> usize {
            self.len
        }
    }

    impl<T, const CAP: usize> Drop for SpscQueue<T, CAP> {
        fn drop(&mut self) {
            while self.try_pop().is_some() {}
        }
    }

    pub fn demonstrate() {
        println!("## HFT-2：const 泛型 SPSC 队列");

        let mut q: SpscQueue<u64, 3> = SpscQueue::new();
        assert!(q.try_push(1).is_ok());
        assert!(q.try_push(2).is_ok());
        assert!(q.try_push(3).is_ok());
        assert!(q.try_push(4).is_err());
        println!("满则 try_push 返回 Err(4)，不 panic、不 alloc");
        println!("pop 顺序 = {:?}", [q.try_pop(), q.try_pop(), q.try_pop()]);
        println!();
    }
}

// ============================================================================
// 场景 3：泛型策略插件（静态分发）
// ============================================================================
/// **生产问题**：同一 OMS 框架要接 TWAP / POV / Iceberg，热路径不能 `dyn Strategy`
/// 每次 vtable 间接调用（见 stack-vs-heap 专题）。
///
/// **泛型套路**：`Engine<S: Strategy>` 在编译期单态化具体策略，零开销多态。
pub mod strategy_plugin {
    use super::{Px, Qty, Side};

    pub trait Strategy {
        fn on_tick(&mut self, mid: Px) -> Option<(Side, Qty)>;
    }

    pub struct Twap {
        slices_left: u32,
        qty_per_slice: Qty,
    }

    impl Twap {
        pub fn new(slices: u32, qty_per_slice: Qty) -> Self {
            Self {
                slices_left: slices,
                qty_per_slice,
            }
        }
    }

    impl Strategy for Twap {
        fn on_tick(&mut self, _mid: Px) -> Option<(Side, Qty)> {
            if self.slices_left == 0 {
                return None;
            }
            self.slices_left -= 1;
            Some((Side::Buy, self.qty_per_slice))
        }
    }

    pub struct Engine<S: Strategy> {
        strategy: S,
        fills: u32,
    }

    impl<S: Strategy> Engine<S> {
        pub fn new(strategy: S) -> Self {
            Self {
                strategy,
                fills: 0,
            }
        }

        pub fn run_ticks(&mut self, mids: &[Px]) -> u32 {
            for &mid in mids {
                if let Some(_) = self.strategy.on_tick(mid) {
                    self.fills += 1;
                }
            }
            self.fills
        }
    }

    pub fn demonstrate() {
        println!("## HFT-3：泛型 Engine 静态分发策略");

        let mut eng = Engine::new(Twap::new(2, 100));
        let n = eng.run_ticks(&[100, 101, 102, 103]);
        println!("TWAP 2 slice → {} fills（第 3、4 tick 不再下单）", n);
        println!("`Engine<Twap>` 与 `Engine<Pov>` 各生成独立机器码，无 vtable\n");
    }
}

// ============================================================================
// 场景 4：泛型风控链（组合子 + trait bound）
// ============================================================================
/// **生产问题**：pre-trade 要顺序跑价格带、notional、持仓限额；新增规则不应
/// 改框架代码。热路径仍要内联。
///
/// **泛型套路**：`RiskPipeline<C, Checks>` 用 `where` 约束 `Check<C>`，编译期组装链。
pub mod risk_pipeline {
    use super::{Px, Qty};

    pub struct OrderCtx {
        pub px: Px,
        pub qty: Qty,
        pub position: Qty,
    }

    pub trait Check<C> {
        fn validate(&self, ctx: &C) -> Result<(), &'static str>;
    }

    pub struct PriceBand {
        pub max_px: Px,
    }

    impl Check<OrderCtx> for PriceBand {
        fn validate(&self, ctx: &OrderCtx) -> Result<(), &'static str> {
            if ctx.px > self.max_px {
                Err("price above band")
            } else {
                Ok(())
            }
        }
    }

    pub struct PositionLimit {
        pub max_abs: Qty,
    }

    impl Check<OrderCtx> for PositionLimit {
        fn validate(&self, ctx: &OrderCtx) -> Result<(), &'static str> {
            let after = ctx.position + ctx.qty;
            if after.abs() > self.max_abs {
                Err("position limit")
            } else {
                Ok(())
            }
        }
    }

    pub struct Pipeline<C, Checks> {
        checks: Checks,
        _ctx: std::marker::PhantomData<C>,
    }

    impl<C, Checks> Pipeline<C, Checks> {
        pub fn new(checks: Checks) -> Self {
            Self {
                checks,
                _ctx: std::marker::PhantomData,
            }
        }
    }

    impl<C, Checks> Pipeline<C, Checks>
    where
        Checks: Check<C>,
    {
        pub fn run(&self, ctx: &C) -> Result<(), &'static str> {
            self.checks.validate(ctx)
        }
    }

    /// 元组实现链式 AND —— (A, B) 先 A 后 B。
    impl<C, A, B> Check<C> for (A, B)
    where
        A: Check<C>,
        B: Check<C>,
    {
        fn validate(&self, ctx: &C) -> Result<(), &'static str> {
            self.0.validate(ctx)?;
            self.1.validate(ctx)
        }
    }

    pub fn demonstrate() {
        println!("## HFT-4：泛型风控链 (Check trait + 元组组合)");

        let pipe = Pipeline::new((
            PriceBand { max_px: 105 },
            PositionLimit { max_abs: 500 },
        ));

        let ok = OrderCtx {
            px: 100,
            qty: 50,
            position: 100,
        };
        let bad = OrderCtx {
            px: 110,
            qty: 50,
            position: 100,
        };
        println!("ok  → {:?}", pipe.run(&ok));
        println!("bad → {:?}", pipe.run(&bad));
        println!("新增规则 = 新 struct + impl Check；Pipeline 签名不变\n");
    }
}

// ============================================================================
// 场景 5：泛型零拷贝协议解码（生命周期 + 关联类型）
// ============================================================================
/// **生产问题**：FIX / ITCH / SBE 帧布局不同，但解析模式相同：按 spec 从 `&[u8]`
/// 切字段，零拷贝。不能把每个协议 copy 一份 parser 逻辑。
///
/// **泛型套路**：`WireView<'a, S: FieldSpec>`，`S::parse` 关联类型固定输出视图。
pub mod zero_copy_decoder {
    pub trait FieldSpec<'a> {
        type View;
        fn parse(raw: &'a [u8]) -> Option<Self::View>;
    }

    pub struct FixOrderView<'a> {
        pub order_id: &'a [u8],
        pub price: u64,
    }

    pub struct FixSpec;

    impl<'a> FieldSpec<'a> for FixSpec {
        type View = FixOrderView<'a>;

        fn parse(raw: &'a [u8]) -> Option<Self::View> {
            // 极简：假设格式 id|price
            let sep = raw.iter().position(|&b| b == b'|')?;
            let price_bytes = &raw[sep + 1..];
            let mut price = 0u64;
            for &b in price_bytes {
                if b.is_ascii_digit() {
                    price = price * 10 + (b - b'0') as u64;
                }
            }
            Some(FixOrderView {
                order_id: &raw[..sep],
                price,
            })
        }
    }

    pub struct ItchOrderView<'a> {
        pub stock: &'a [u8],
        pub shares: u32,
    }

    pub struct ItchSpec;

    impl<'a> FieldSpec<'a> for ItchSpec {
        type View = ItchOrderView<'a>;

        fn parse(raw: &'a [u8]) -> Option<Self::View> {
            if raw.len() < 6 {
                return None;
            }
            let shares = u32::from_le_bytes(raw[raw.len() - 4..].try_into().ok()?);
            Some(ItchOrderView {
                stock: &raw[..raw.len() - 4],
                shares,
            })
        }
    }

    pub fn decode<'a, S: FieldSpec<'a>>(raw: &'a [u8]) -> Option<S::View> {
        S::parse(raw)
    }

    pub fn demonstrate() {
        println!("## HFT-5：泛型零拷贝解码 FieldSpec");

        let fix_raw = b"ORD42|100050";
        let itch_raw = b"AAPL\x10\x00\x00\x00";

        let fix = decode::<FixSpec>(fix_raw).unwrap();
        let itch = decode::<ItchSpec>(itch_raw).unwrap();

        println!(
            "FIX  id={:?} price={}",
            std::str::from_utf8(fix.order_id).unwrap(),
            fix.price
        );
        println!(
            "ITCH stock={:?} shares={}",
            std::str::from_utf8(itch.stock).unwrap(),
            itch.shares
        );
        println!("同一 `decode` 函数，单态化为两套内联解析循环\n");
    }
}

// ============================================================================
// 场景 6：泛型事件总线（类型索引分发）
// ============================================================================
/// **生产问题**：OMS 内成交回报、行情 tick、风控告警要走不同 handler，但发布端
/// 不想知道订阅者列表；又不能把所有事件塞进 `enum Event` 导致巨型 match。
///
/// **泛型套路**：`EventBus` 对每种 `E: Event` 维护独立 handler 列表（此处简化为单 handler）。
pub mod typed_event_bus {
    pub trait Event: 'static {
        fn kind() -> &'static str;
    }

    #[derive(Debug, Clone)]
    pub struct FillEvent {
        pub order_id: u64,
        pub px: i64,
    }

    impl Event for FillEvent {
        fn kind() -> &'static str {
            "fill"
        }
    }

    #[derive(Debug, Clone)]
    pub struct TickEvent {
        pub px: i64,
    }

    impl Event for TickEvent {
        fn kind() -> &'static str {
            "tick"
        }
    }

    pub trait Handler<E: Event> {
        fn on_event(&mut self, e: &E);
    }

    pub struct LoggingHandler;

    impl<E: Event + std::fmt::Debug> Handler<E> for LoggingHandler {
        fn on_event(&mut self, e: &E) {
            println!("  [bus] {} → {:?}", E::kind(), e);
        }
    }

    pub struct Bus<H> {
        handler: H,
    }

    impl<H> Bus<H> {
        pub fn new(handler: H) -> Self {
            Self { handler }
        }

        pub fn publish<E>(&mut self, e: &E)
        where
            H: Handler<E>,
            E: Event,
        {
            self.handler.on_event(e);
        }
    }

    pub fn demonstrate() {
        println!("## HFT-6：泛型事件总线 Handler<E>");

        let mut bus = Bus::new(LoggingHandler);
        bus.publish(&FillEvent {
            order_id: 7,
            px: 100,
        });
        bus.publish(&TickEvent { px: 101 });
        println!("每种事件类型独立 Handler 实现；扩展事件 = 新 struct + impl Event\n");
    }
}

// ============================================================================
// 场景 7：泛型延迟直方图（const 桶边界）
// ============================================================================
/// **生产问题**：P99 监控要把微秒样本扔进固定桶；桶边界因业务不同（HFT 用 µs，
/// 批处理用 ms），但直方图逻辑相同。
///
/// **泛型套路**：`Histogram<const BOUNDS: usize>` + 编译期边界数组。
pub mod latency_histogram {
    pub struct Histogram<const N: usize> {
        bounds_ns: [u64; N],
        counts: [u64; N],
        overflow: u64,
    }

    impl<const N: usize> Histogram<N> {
        pub const fn new(bounds_ns: [u64; N]) -> Self {
            Self {
                bounds_ns,
                counts: [0; N],
                overflow: 0,
            }
        }

        pub fn record(&mut self, sample_ns: u64) {
            for (i, &b) in self.bounds_ns.iter().enumerate() {
                if sample_ns <= b {
                    self.counts[i] += 1;
                    return;
                }
            }
            self.overflow += 1;
        }

        pub fn bucket(&self, i: usize) -> u64 {
            self.counts[i]
        }

        pub fn overflow(&self) -> u64 {
            self.overflow
        }
    }

    pub fn demonstrate() {
        println!("## HFT-7：const 泛型延迟直方图");

        let mut h: Histogram<4> = Histogram::new([1_000, 10_000, 100_000, 1_000_000]);
        for s in [500, 5_000, 50_000, 2_000_000] {
            h.record(s);
        }
        println!(
            "buckets = [{}, {}, {}, {}], overflow = {}",
            h.bucket(0),
            h.bucket(1),
            h.bucket(2),
            h.bucket(3),
            h.overflow()
        );
        println!("边界数组是类型的一部分；换边界 = 换类型，非运行期配置\n");
    }
}

pub fn demonstrate() {
    unit_branding::demonstrate();
    spsc_ring::demonstrate();
    strategy_plugin::demonstrate();
    risk_pipeline::demonstrate();
    zero_copy_decoder::demonstrate();
    typed_event_bus::demonstrate();
    latency_histogram::demonstrate();
}
