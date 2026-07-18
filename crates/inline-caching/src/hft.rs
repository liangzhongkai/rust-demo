//! # HFT 生产场景下的 Inline Cache
//!
//! 高频交易硬约束：
//! - **延迟**：热路径每次 HashMap 查找都可能是几十 ns；同 symbol 连打时浪费
//! - **可预测**：分支/缓存命中率决定 P99，不是 mean
//! - **正确**：缓存失效（改 tick size、风控限额、协议版本）必须立刻可见
//!
//! 下面 7 个场景对应行情网关、撮合适配、风控、策略分发里的真实写法。

#![allow(dead_code)]

use std::collections::HashMap;

pub type SymbolId = u32;
pub type Px = i64;
pub type Qty = i64;

// ============================================================================
// 场景 1：Symbol 元数据 IC —— 同标的连打
// ============================================================================
/// **生产问题**：每 tick 查 `tick_size` / `lot_size` 走全局 `HashMap<Symbol, Meta>`。
/// 一个 feed 上 95%+ 消息是同一活跃合约，Map 查找 + 哈希成为可见开销。
///
/// **IC 套路**：调用点旁 `MonoIc { last_sym, meta }`；symbol 未变则跳过 HashMap。
pub mod symbol_meta_ic {
    use super::*;

    #[derive(Clone, Copy, Debug)]
    pub struct Meta {
        pub tick_size: Px,
        pub lot_size: Qty,
    }

    pub struct SymbolMetaCache {
        table: HashMap<SymbolId, Meta>,
        last_sym: Option<SymbolId>,
        last_meta: Meta,
        pub hits: u64,
        pub misses: u64,
    }

    impl SymbolMetaCache {
        pub fn new(entries: &[(SymbolId, Meta)]) -> Self {
            let mut table = HashMap::new();
            for &(s, m) in entries {
                table.insert(s, m);
            }
            Self {
                table,
                last_sym: None,
                last_meta: Meta {
                    tick_size: 0,
                    lot_size: 0,
                },
                hits: 0,
                misses: 0,
            }
        }

        pub fn meta(&mut self, sym: SymbolId) -> Option<Meta> {
            if self.last_sym == Some(sym) {
                self.hits += 1;
                return Some(self.last_meta);
            }
            self.misses += 1;
            let m = *self.table.get(&sym)?;
            self.last_sym = Some(sym);
            self.last_meta = m;
            Some(m)
        }

        /// 配置热更新：必须清 IC，否则用旧 tick_size 下单。
        pub fn update_meta(&mut self, sym: SymbolId, meta: Meta) {
            self.table.insert(sym, meta);
            if self.last_sym == Some(sym) {
                self.last_sym = None;
            }
        }
    }

    pub fn demonstrate() {
        println!("## HFT-1：Symbol 元数据单态 IC");

        let mut cache = SymbolMetaCache::new(&[
            (
                1,
                Meta {
                    tick_size: 100,
                    lot_size: 1,
                },
            ),
            (
                2,
                Meta {
                    tick_size: 25,
                    lot_size: 5,
                },
            ),
        ]);

        for _ in 0..50 {
            let _ = cache.meta(1);
        }
        let _ = cache.meta(2);
        let _ = cache.meta(1);
        println!(
            "hits={} misses={}（同一 BTC 连打应接近 50 hit）",
            cache.hits, cache.misses
        );

        cache.update_meta(
            1,
            Meta {
                tick_size: 50,
                lot_size: 1,
            },
        );
        let m = cache.meta(1).unwrap();
        println!("热更新后 tick_size={}（IC 已失效）\n", m.tick_size);
    }
}

// ============================================================================
// 场景 2：Venue 消息 Handler IC —— 协议 tag → 解码器
// ============================================================================
/// **生产问题**：多 venue 统一 ingress：`match msg_type` 每包都走完整分支树；
/// 或 `HashMap<u8, fn(...)>` 间接调用。实际上单个 socket 上 tag 高度局部。
///
/// **IC 套路**：缓存 `(last_tag, handler_idx)`；tag 命中则直接调函数指针。
pub mod venue_handler_ic {
    #[derive(Clone, Copy)]
    pub enum MsgKind {
        Trade,
        Book,
        Heartbeat,
    }

    type Handler = fn(&[u8]) -> usize;

    fn decode_trade(buf: &[u8]) -> usize {
        buf.len().saturating_add(1)
    }
    fn decode_book(buf: &[u8]) -> usize {
        buf.len().saturating_add(2)
    }
    fn decode_hb(buf: &[u8]) -> usize {
        buf.len()
    }

    pub struct HandlerIc {
        last_tag: Option<u8>,
        last_handler: Handler,
        pub hits: u64,
        pub misses: u64,
    }

    impl HandlerIc {
        pub fn new() -> Self {
            Self {
                last_tag: None,
                last_handler: decode_hb,
                hits: 0,
                misses: 0,
            }
        }

        fn resolve(tag: u8) -> Option<Handler> {
            match tag {
                1 => Some(decode_trade),
                2 => Some(decode_book),
                3 => Some(decode_hb),
                _ => None,
            }
        }

        pub fn dispatch(&mut self, tag: u8, payload: &[u8]) -> Option<usize> {
            if self.last_tag == Some(tag) {
                self.hits += 1;
                return Some((self.last_handler)(payload));
            }
            self.misses += 1;
            let h = Self::resolve(tag)?;
            self.last_tag = Some(tag);
            self.last_handler = h;
            Some(h(payload))
        }
    }

    pub fn demonstrate() {
        println!("## HFT-2：Venue handler 单态 IC");

        let mut ic = HandlerIc::new();
        let payload = &[0u8; 32];
        // 突发全是 trade
        for _ in 0..20 {
            let _ = ic.dispatch(1, payload);
        }
        let _ = ic.dispatch(2, payload);
        println!(
            "trade 突发：hits={} misses={}",
            ic.hits, ic.misses
        );
        println!("比每包 `match tag` 少一次大 jump table 压力；tag 抖动时回退正常\n");
    }
}

// ============================================================================
// 场景 3：订单簿侧别 / 更新类型 PIC
// ============================================================================
/// **生产问题**：L2 更新有 New/Change/Delete × Bid/Ask；完全 megamorphic 时
/// 分支预测失效。多数时段只有 1~2 种组合占主导。
///
/// **IC 套路**：`PolyIc<2>` 缓存最近两种 `(side, action) → 处理路径索引`。
pub mod book_update_pic {
    #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
    pub struct UpdateShape {
        pub side: u8,   // 0=bid 1=ask
        pub action: u8, // 0=new 1=chg 2=del
    }

    pub struct BookPic {
        slots: [Option<(UpdateShape, u8)>; 2],
        len: usize,
        pub hits: u64,
        pub misses: u64,
    }

    impl BookPic {
        pub fn new() -> Self {
            Self {
                slots: [None, None],
                len: 0,
                hits: 0,
                misses: 0,
            }
        }

        fn resolve(shape: UpdateShape) -> u8 {
            shape.side * 3 + shape.action
        }

        pub fn path(&mut self, shape: UpdateShape) -> u8 {
            for i in 0..self.len {
                if let Some((s, idx)) = self.slots[i] {
                    if s == shape {
                        self.hits += 1;
                        if i > 0 {
                            self.slots.swap(0, i);
                        }
                        return idx;
                    }
                }
            }
            self.misses += 1;
            let idx = Self::resolve(shape);
            if self.len < 2 {
                self.slots[self.len] = Some((shape, idx));
                self.len += 1;
            } else {
                // 替换 MRU 次槽，保持「最近两种」特化
                self.slots[1] = Some((shape, idx));
            }
            idx
        }
    }

    pub fn demonstrate() {
        println!("## HFT-3：Book update 多态 IC（2-slot）");

        let mut pic = BookPic::new();
        let bid_new = UpdateShape {
            side: 0,
            action: 0,
        };
        let ask_chg = UpdateShape {
            side: 1,
            action: 1,
        };
        for _ in 0..10 {
            let _ = pic.path(bid_new);
            let _ = pic.path(ask_chg);
        }
        let _ = pic.path(UpdateShape {
            side: 0,
            action: 2,
        });
        println!(
            "hits={} misses={} paths 稳定时 PIC 近似单态速度\n",
            pic.hits, pic.misses
        );
    }
}

// ============================================================================
// 场景 4：风控限额 IC —— 按 instrument 缓存
// ============================================================================
/// **生产问题**：下单前查 `max_pos / max_order / rate_limit`；全表查 + 锁争用贵。
/// 策略线程通常连续打同一标的。
///
/// **IC 套路**：线程局部 `last_sym + Limits`；风控配置推送时 bump generation。
pub mod risk_limits_ic {
    use super::*;

    #[derive(Clone, Copy, Debug)]
    pub struct Limits {
        pub max_order: Qty,
        pub max_pos: Qty,
        pub gen: u64,
    }

    pub struct RiskIc {
        table: HashMap<SymbolId, Limits>,
        cached_sym: Option<SymbolId>,
        cached: Limits,
        pub hits: u64,
        pub misses: u64,
    }

    impl RiskIc {
        pub fn new(entries: &[(SymbolId, Limits)]) -> Self {
            let mut table = HashMap::new();
            for &(s, l) in entries {
                table.insert(s, l);
            }
            Self {
                table,
                cached_sym: None,
                cached: Limits {
                    max_order: 0,
                    max_pos: 0,
                    gen: 0,
                },
                hits: 0,
                misses: 0,
            }
        }

        pub fn limits(&mut self, sym: SymbolId) -> Option<Limits> {
            if let Some(s) = self.cached_sym {
                if s == sym {
                    // 还要核对 generation，防止后台改表未清 IC
                    if let Some(fresh) = self.table.get(&sym) {
                        if fresh.gen == self.cached.gen {
                            self.hits += 1;
                            return Some(self.cached);
                        }
                    }
                }
            }
            self.misses += 1;
            let l = *self.table.get(&sym)?;
            self.cached_sym = Some(sym);
            self.cached = l;
            Some(l)
        }

        pub fn bump(&mut self, sym: SymbolId, mut lim: Limits) {
            lim.gen = self.table.get(&sym).map(|l| l.gen + 1).unwrap_or(1);
            self.table.insert(sym, lim);
        }
    }

    pub fn demonstrate() {
        println!("## HFT-4：风控限额 + generation guard");

        let mut ic = RiskIc::new(&[(
            42,
            Limits {
                max_order: 100,
                max_pos: 1000,
                gen: 1,
            },
        )]);
        for _ in 0..10 {
            let _ = ic.limits(42);
        }
        ic.bump(
            42,
            Limits {
                max_order: 50,
                max_pos: 500,
                gen: 0, // bump 内部会改写 gen
            },
        );
        let l = ic.limits(42).unwrap();
        println!(
            "降额后 max_order={} gen={} hits={} misses={}\n",
            l.max_order, l.gen, ic.hits, ic.misses
        );
    }
}

// ============================================================================
// 场景 5：Feed 适配器 IC —— 多源归一
// ============================================================================
/// **生产问题**：同一策略消费 Binance / CME / 内部 mirror；`dyn Feed` 虚调用
/// 在 1M msg/s 下可测。实际上单个线程 pin 到一个源。
///
/// **IC 套路**：缓存 `source_id → 归一化函数`；源切换才 miss。
pub mod feed_adapter_ic {
    pub type NormalizeFn = fn(i64) -> i64;

    fn binance_px(raw: i64) -> i64 {
        raw
    }
    fn cme_px(raw: i64) -> i64 {
        raw * 4
    }

    pub struct FeedIc {
        last_src: Option<u8>,
        norm: NormalizeFn,
        pub hits: u64,
        pub misses: u64,
    }

    impl FeedIc {
        pub fn new() -> Self {
            Self {
                last_src: None,
                norm: binance_px,
                hits: 0,
                misses: 0,
            }
        }

        pub fn normalize(&mut self, src: u8, raw: i64) -> Option<i64> {
            if self.last_src == Some(src) {
                self.hits += 1;
                return Some((self.norm)(raw));
            }
            self.misses += 1;
            let f: NormalizeFn = match src {
                1 => binance_px,
                2 => cme_px,
                _ => return None,
            };
            self.last_src = Some(src);
            self.norm = f;
            Some(f(raw))
        }
    }

    pub fn demonstrate() {
        println!("## HFT-5：多源 Feed 归一化 IC");

        let mut ic = FeedIc::new();
        for _ in 0..30 {
            let _ = ic.normalize(2, 2500);
        }
        let px = ic.normalize(2, 2500).unwrap();
        println!("CME pin：hits={} 末次 px={}", ic.hits, px);
        println!("线程 pin 单源时，IC ≈ 直接函数调用；勿在热路径用 dyn\n");
    }
}

// ============================================================================
// 场景 6：策略信号分发 IC —— 预测路径
// ============================================================================
/// **生产问题**：信号总线 `enum Signal { Entry, Exit, Hedge, Cancel }`；
/// 某策略时段几乎只有 Entry。完整 match 四路伤害 BTB。
///
/// **IC 套路**：缓存「上次 variant 判别式 + 处理函数」；命中走预测路径。
pub mod signal_dispatch_ic {
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum Signal {
        Entry,
        Exit,
        Hedge,
        Cancel,
    }

    type Handle = fn(Signal) -> &'static str;

    fn on_entry(_: Signal) -> &'static str {
        "entry"
    }
    fn on_exit(_: Signal) -> &'static str {
        "exit"
    }
    fn on_hedge(_: Signal) -> &'static str {
        "hedge"
    }
    fn on_cancel(_: Signal) -> &'static str {
        "cancel"
    }

    pub struct SignalIc {
        last: Option<Signal>,
        handler: Handle,
        pub hits: u64,
        pub misses: u64,
    }

    impl SignalIc {
        pub fn new() -> Self {
            Self {
                last: None,
                handler: on_entry,
                hits: 0,
                misses: 0,
            }
        }

        fn resolve(s: Signal) -> Handle {
            match s {
                Signal::Entry => on_entry,
                Signal::Exit => on_exit,
                Signal::Hedge => on_hedge,
                Signal::Cancel => on_cancel,
            }
        }

        pub fn handle(&mut self, s: Signal) -> &'static str {
            if self.last == Some(s) {
                self.hits += 1;
                return (self.handler)(s);
            }
            self.misses += 1;
            let h = Self::resolve(s);
            self.last = Some(s);
            self.handler = h;
            h(s)
        }
    }

    pub fn demonstrate() {
        println!("## HFT-6：策略信号预测路径 IC");

        let mut ic = SignalIc::new();
        for _ in 0..40 {
            let _ = ic.handle(Signal::Entry);
        }
        let last = ic.handle(Signal::Exit);
        println!("Entry 主导：hits={} last={}", ic.hits, last);
        println!("编译期已知唯一策略时，更优解是泛型静态分发；IC 服务运行时混合\n");
    }
}

// ============================================================================
// 场景 7：价格带 / 熔断参数 IC
// ============================================================================
/// **生产问题**：每个 quote 校验是否跌破 `band_lo/hi`；参数按 symbol 存表。
/// 做市报价循环内同标的重复读。
///
/// **IC 套路**：与 HFT-1 相同模式，强调 **写路径失效** 与 **只读热路径**。
pub mod price_band_ic {
    use super::*;

    #[derive(Clone, Copy)]
    pub struct Band {
        pub lo: Px,
        pub hi: Px,
    }

    pub struct BandIc {
        table: HashMap<SymbolId, Band>,
        last: Option<(SymbolId, Band)>,
        pub hits: u64,
        pub rejects: u64,
    }

    impl BandIc {
        pub fn new(table: HashMap<SymbolId, Band>) -> Self {
            Self {
                table,
                last: None,
                hits: 0,
                rejects: 0,
            }
        }

        pub fn accept(&mut self, sym: SymbolId, px: Px) -> bool {
            let band = if let Some((s, b)) = self.last {
                if s == sym {
                    self.hits += 1;
                    b
                } else {
                    self.fetch(sym)
                }
            } else {
                self.fetch(sym)
            };

            let ok = px >= band.lo && px <= band.hi;
            if !ok {
                self.rejects += 1;
            }
            ok
        }

        fn fetch(&mut self, sym: SymbolId) -> Band {
            let b = self.table.get(&sym).copied().unwrap_or(Band { lo: 0, hi: 0 });
            self.last = Some((sym, b));
            b
        }

        pub fn widen(&mut self, sym: SymbolId, band: Band) {
            self.table.insert(sym, band);
            if matches!(self.last, Some((s, _)) if s == sym) {
                self.last = None;
            }
        }
    }

    pub fn demonstrate() {
        println!("## HFT-7：价格带校验 IC");

        let mut table = HashMap::new();
        table.insert(7, Band { lo: 100, hi: 200 });
        let mut ic = BandIc::new(table);

        let mut ok = 0;
        for px in [150, 160, 170, 180, 250] {
            if ic.accept(7, px) {
                ok += 1;
            }
        }
        println!(
            "accepted={} hits={} rejects={}",
            ok, ic.hits, ic.rejects
        );
        ic.widen(7, Band { lo: 100, hi: 300 });
        println!(
            "widen 后 250 可过？{}\n",
            ic.accept(7, 250)
        );
    }
}

pub fn demonstrate() {
    symbol_meta_ic::demonstrate();
    venue_handler_ic::demonstrate();
    book_update_pic::demonstrate();
    risk_limits_ic::demonstrate();
    feed_adapter_ic::demonstrate();
    signal_dispatch_ic::demonstrate();
    price_band_ic::demonstrate();
}
