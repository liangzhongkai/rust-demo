//! # Trait 系统深度实践
//!
//! 考察点：
//! 1. Trait 作为「行为契约」解耦 HFT/Web3 中的多变实现
//! 2. 静态分发 (`impl Trait` / 泛型) vs 动态分发 (`dyn Trait`) 的取舍
//! 3. 关联类型、Supertrait、Object Safety 在生产中的约束
//! 4. 从具体场景泛化到一般性设计策略
//!
//! 运行：`cargo run -p trait-system`

#![allow(dead_code)]

// =============================================================================
// 第一部分：HFT 生产场景
// =============================================================================

mod hft {
    // -------------------------------------------------------------------------
    // 场景 1：策略插件 —— 热路径用静态分发，冷路径用 dyn
    // -------------------------------------------------------------------------
    /// **生产问题**：同一引擎要跑做市、套利、TWAP 等策略；回测与实盘需共享接口，
    /// 但热路径 `on_tick` 不能接受 vtable 开销。
    pub mod strategy_plugin {
        #[derive(Debug, Clone, Copy)]
        pub struct Tick {
            pub symbol_id: u32,
            pub bid: i64,
            pub ask: i64,
        }

        pub trait Strategy: Send {
            fn name(&self) -> &str;
            fn on_tick(&mut self, tick: Tick) -> Option<OrderIntent>;
        }

        #[derive(Debug, Clone, Copy)]
        pub struct OrderIntent {
            pub side: u8,
            pub qty: u32,
            pub limit_px: i64,
        }

        pub struct MarketMaker {
            pub spread_threshold: i64,
            pub orders_sent: u64,
        }

        impl Strategy for MarketMaker {
            fn name(&self) -> &str {
                "mm"
            }

            fn on_tick(&mut self, tick: Tick) -> Option<OrderIntent> {
                let spread = tick.ask - tick.bid;
                if spread >= self.spread_threshold {
                    self.orders_sent += 1;
                    Some(OrderIntent {
                        side: 0,
                        qty: 1,
                        limit_px: tick.bid + 1,
                    })
                } else {
                    None
                }
            }
        }

        /// 静态分发：编译期单态化，热路径零 vtable
        pub fn run_hot_path<S: Strategy>(strategy: &mut S, ticks: &[Tick]) -> u64 {
            let mut sent = 0u64;
            for tick in ticks {
                if strategy.on_tick(*tick).is_some() {
                    sent += 1;
                }
            }
            sent
        }

        /// 动态分发：运行时切换策略（配置热加载、A/B 实验）
        pub fn run_with_runtime_switch(strategies: &mut [Box<dyn Strategy>], tick: Tick) {
            for s in strategies {
                let _ = s.on_tick(tick);
            }
        }

        pub fn demonstrate() {
            println!("## HFT-1：策略插件（静态 vs 动态分发）");
            let ticks = [
                Tick {
                    symbol_id: 1,
                    bid: 100,
                    ask: 105,
                },
                Tick {
                    symbol_id: 1,
                    bid: 100,
                    ask: 101,
                },
            ];
            let mut mm = MarketMaker {
                spread_threshold: 3,
                orders_sent: 0,
            };
            let sent = run_hot_path(&mut mm, &ticks);
            assert_eq!(sent, 1);
            println!(
                "  热路径 {} 发单 {} 次（静态分发，无 vtable）",
                mm.name(),
                sent
            );

            let mut plugins: Vec<Box<dyn Strategy>> =
                vec![Box::new(MarketMaker {
                    spread_threshold: 2,
                    orders_sent: 0,
                })];
            run_with_runtime_switch(&mut plugins, ticks[1]);
            println!("  冷路径可 Box<dyn Strategy> 热切换策略");
            println!("  关键：热路径 impl Trait；配置/实验 dyn Trait\n");
        }
    }

    // -------------------------------------------------------------------------
    // 场景 2：行情解码 —— 关联类型绑定「输出类型族」
    // -------------------------------------------------------------------------
    /// **生产问题**：CME ITCH、Nasdaq OUCH、Binance JSON 解码后都应是统一的 `BookUpdate`，
    /// 但中间缓冲类型各不同；用关联类型避免泛型参数爆炸。
    pub mod market_data_decoder {
        #[derive(Debug, PartialEq)]
        pub struct BookUpdate {
            pub symbol_id: u32,
            pub bid_px: i64,
            pub bid_qty: u32,
        }

        pub trait MarketDataDecoder {
            type WireBuffer: AsRef<[u8]>;
            type ParseError: std::fmt::Display;

            fn decode(&self, wire: Self::WireBuffer) -> Result<BookUpdate, Self::ParseError>;
        }

        pub struct ItchDecoder;

        impl MarketDataDecoder for ItchDecoder {
            type WireBuffer = [u8; 16];
            type ParseError = &'static str;

            fn decode(&self, wire: Self::WireBuffer) -> Result<BookUpdate, Self::ParseError> {
                if wire[0] != b'Q' {
                    return Err("bad msg type");
                }
                Ok(BookUpdate {
                    symbol_id: u32::from_le_bytes(wire[4..8].try_into().unwrap()),
                    bid_px: i64::from_le_bytes(wire[8..16].try_into().unwrap()),
                    bid_qty: 1,
                })
            }
        }

        pub struct JsonDecoder;

        impl MarketDataDecoder for JsonDecoder {
            type WireBuffer = &'static str;
            type ParseError = &'static str;

            fn decode(&self, wire: Self::WireBuffer) -> Result<BookUpdate, Self::ParseError> {
                // 简化：生产用 simd-json / sonic-rs
                if !wire.contains("\"bid\"") {
                    return Err("missing bid");
                }
                Ok(BookUpdate {
                    symbol_id: 42,
                    bid_px: 100,
                    bid_qty: 10,
                })
            }
        }

        pub fn ingest<D: MarketDataDecoder>(decoder: &D, wire: D::WireBuffer) -> Option<BookUpdate> {
            decoder.decode(wire).ok()
        }

        pub fn demonstrate() {
            println!("## HFT-2：行情解码（关联类型）");
            let itch = ItchDecoder;
            let mut buf = [0u8; 16];
            buf[0] = b'Q';
            buf[4..8].copy_from_slice(&7u32.to_le_bytes());
            buf[8..16].copy_from_slice(&999i64.to_le_bytes());
            let u = ingest(&itch, buf).unwrap();
            assert_eq!(u.symbol_id, 7);
            println!("  ITCH 解码 symbol_id={}", u.symbol_id);

            let json = JsonDecoder;
            let u2 = ingest(&json, r#"{"bid":100}"#).unwrap();
            println!("  JSON 解码 bid_px={}", u2.bid_px);
            println!("  关键：关联类型把 WireBuffer/ParseError 与实现绑定\n");
        }
    }

    // -------------------------------------------------------------------------
    // 场景 3：订单路由 —— Supertrait 表达线程安全约束
    // -------------------------------------------------------------------------
    /// **生产问题**：Smart Order Router 要把单分到多个 venue；路由实现会被多个线程共享，
    /// 必须在类型层面要求 `Send + Sync`。
    pub mod order_router {
        #[derive(Debug, Clone, Copy)]
        pub struct Order {
            pub symbol_id: u32,
            pub qty: u32,
            pub side: u8,
        }

        #[derive(Debug, Clone, Copy, PartialEq)]
        pub enum RouteTarget {
            InternalCross,
            VenueA,
            VenueB,
        }

        pub trait OrderRouter: Send + Sync {
            fn route(&self, order: &Order) -> RouteTarget;
        }

        pub struct LatencyAwareRouter {
            pub venue_a_latency_us: u64,
            pub venue_b_latency_us: u64,
        }

        impl OrderRouter for LatencyAwareRouter {
            fn route(&self, order: &Order) -> RouteTarget {
                let _ = order;
                if self.venue_a_latency_us <= self.venue_b_latency_us {
                    RouteTarget::VenueA
                } else {
                    RouteTarget::VenueB
                }
            }
        }

        pub struct InternalCrossRouter;

        impl OrderRouter for InternalCrossRouter {
            fn route(&self, order: &Order) -> RouteTarget {
                if order.qty <= 100 {
                    RouteTarget::InternalCross
                } else {
                    RouteTarget::VenueA
                }
            }
        }

        pub fn dispatch<R: OrderRouter + ?Sized>(router: &R, order: Order) -> RouteTarget {
            router.route(&order)
        }

        pub fn demonstrate() {
            println!("## HFT-3：订单路由（Supertrait Send + Sync）");
            let routers: Vec<Box<dyn OrderRouter>> = vec![
                Box::new(LatencyAwareRouter {
                    venue_a_latency_us: 50,
                    venue_b_latency_us: 80,
                }),
                Box::new(InternalCrossRouter),
            ];
            let order = Order {
                symbol_id: 1,
                qty: 50,
                side: 0,
            };
            for (i, r) in routers.iter().enumerate() {
                println!("  router[{}] → {:?}", i, dispatch(r.as_ref(), order));
            }
            println!("  关键：Supertrait 把并发约束写进契约，而非注释\n");
        }
    }

    // -------------------------------------------------------------------------
    // 场景 4：风控链 —— 组合多个 RiskCheck（trait 对象 + 组合模式）
    // -------------------------------------------------------------------------
    /// **生产问题**：下单前要经过仓位、速率、价格偏离等多道检查；各 desk 组合不同，
    /// 需要可插拔的检查链。
    pub mod risk_pipeline {
        #[derive(Debug, Clone, Copy)]
        pub struct Order {
            pub notional: i64,
            pub price: i64,
            pub max_notional: i64,
        }

        pub enum RiskVerdict {
            Pass,
            Reject(&'static str),
        }

        pub trait RiskCheck: Send + Sync {
            fn check(&self, order: &Order) -> RiskVerdict;
        }

        pub struct NotionalLimit;

        impl RiskCheck for NotionalLimit {
            fn check(&self, order: &Order) -> RiskVerdict {
                if order.notional > order.max_notional {
                    RiskVerdict::Reject("notional exceeded")
                } else {
                    RiskVerdict::Pass
                }
            }
        }

        pub struct PriceBand {
            pub ref_px: i64,
            pub bps: i64,
        }

        impl RiskCheck for PriceBand {
            fn check(&self, order: &Order) -> RiskVerdict {
                let limit = self.ref_px * self.bps / 10_000;
                if (order.price - self.ref_px).abs() > limit {
                    RiskVerdict::Reject("price band")
                } else {
                    RiskVerdict::Pass
                }
            }
        }

        pub struct RiskPipeline {
            checks: Vec<Box<dyn RiskCheck>>,
        }

        impl RiskPipeline {
            pub fn new(checks: Vec<Box<dyn RiskCheck>>) -> Self {
                Self { checks }
            }

            pub fn evaluate(&self, order: &Order) -> RiskVerdict {
                for c in &self.checks {
                    match c.check(order) {
                        RiskVerdict::Pass => continue,
                        reject @ RiskVerdict::Reject(_) => return reject,
                    }
                }
                RiskVerdict::Pass
            }
        }

        pub fn demonstrate() {
            println!("## HFT-4：风控链（trait 对象组合）");
            let pipeline = RiskPipeline::new(vec![
                Box::new(NotionalLimit),
                Box::new(PriceBand {
                    ref_px: 100,
                    bps: 50,
                }),
            ]);
            let ok = Order {
                notional: 1_000,
                price: 100,
                max_notional: 5_000,
            };
            let bad = Order {
                notional: 9_000,
                price: 100,
                max_notional: 5_000,
            };
            assert!(matches!(pipeline.evaluate(&ok), RiskVerdict::Pass));
            assert!(matches!(
                pipeline.evaluate(&bad),
                RiskVerdict::Reject("notional exceeded")
            ));
            println!("  合规单 Pass，超限单 Reject(notional exceeded)");
            println!("  关键：Vec<Box<dyn RiskCheck>> 即策略模式 + 责任链\n");
        }
    }

    // -------------------------------------------------------------------------
    // 场景 5：成交回报 —— Fn trait 解耦策略与日志/指标
    // -------------------------------------------------------------------------
    /// **生产问题**：策略只关心 fill，但运维要 Prometheus、合规要 audit log；
    /// 用回调 trait 避免策略 crate 依赖 observability crate。
    pub mod execution_sink {
        #[derive(Debug, Clone, Copy)]
        pub struct Fill {
            pub order_id: u64,
            pub px: i64,
            pub qty: u32,
        }

        pub trait FillHandler {
            fn on_fill(&mut self, fill: Fill);
        }

        pub struct StrategyEngine<H: FillHandler> {
            pub handler: H,
            pub position: i64,
        }

        impl<H: FillHandler> StrategyEngine<H> {
            pub fn apply_fill(&mut self, fill: Fill) {
                self.position += fill.qty as i64;
                self.handler.on_fill(fill);
            }
        }

        pub struct MetricsHandler {
            pub fill_count: u64,
        }

        impl FillHandler for MetricsHandler {
            fn on_fill(&mut self, fill: Fill) {
                self.fill_count += 1;
                let _ = fill;
            }
        }

        pub struct AuditHandler {
            pub lines: Vec<String>,
        }

        impl FillHandler for AuditHandler {
            fn on_fill(&mut self, fill: Fill) {
                self.lines
                    .push(format!("FILL id={} px={} qty={}", fill.order_id, fill.px, fill.qty));
            }
        }

        pub fn demonstrate() {
            println!("## HFT-5：成交回报（泛型 handler，静态分发）");
            let mut engine = StrategyEngine {
                handler: MetricsHandler { fill_count: 0 },
                position: 0,
            };
            engine.apply_fill(Fill {
                order_id: 1,
                px: 100,
                qty: 10,
            });
            assert_eq!(engine.handler.fill_count, 1);
            assert_eq!(engine.position, 10);
            println!("  MetricsHandler fill_count={}", engine.handler.fill_count);

            let mut audit_engine = StrategyEngine {
                handler: AuditHandler { lines: vec![] },
                position: 0,
            };
            audit_engine.apply_fill(Fill {
                order_id: 2,
                px: 101,
                qty: 5,
            });
            println!("  AuditHandler: {}", audit_engine.handler.lines[0]);
            println!("  关键：泛型 H: FillHandler 零成本注入副作用\n");
        }
    }

    // -------------------------------------------------------------------------
    // 场景 6：序列化 —— AsRef / From 实现零拷贝边界转换
    // -------------------------------------------------------------------------
    /// **生产问题**：FIX/SBE 报文在不同层之间传递；频繁 `to_vec()` 会打爆 GC/allocator。
    /// 标准 trait `AsRef<[u8]>` 让 API 同时接受 `&[u8]`、`Vec<u8>`、`FixedBuffer`。
    pub mod wire_encoding {
        pub struct FixedBuffer<const N: usize> {
            pub len: usize,
            pub data: [u8; N],
        }

        impl<const N: usize> AsRef<[u8]> for FixedBuffer<N> {
            fn as_ref(&self) -> &[u8] {
                &self.data[..self.len]
            }
        }

        impl<const N: usize> From<&[u8]> for FixedBuffer<N> {
            fn from(slice: &[u8]) -> Self {
                let mut data = [0u8; N];
                let len = slice.len().min(N);
                data[..len].copy_from_slice(&slice[..len]);
                Self { len, data }
            }
        }

        pub trait WireMessage: AsRef<[u8]> {
            fn msg_type(&self) -> u8 {
                self.as_ref().first().copied().unwrap_or(0)
            }
        }

        impl<T: AsRef<[u8]>> WireMessage for T {}

        pub fn checksum(payload: impl AsRef<[u8]>) -> u32 {
            payload.as_ref().iter().map(|b| *b as u32).sum()
        }

        pub fn demonstrate() {
            println!("## HFT-6：报文边界（AsRef / From 零拷贝友好）");
            let buf: FixedBuffer<64> = From::from(&b"8=FIX.4.2"[..]);
            assert_eq!(buf.msg_type(), b'8');
            let vec_payload = vec![b'N', b'E', b'W'];
            let c1 = checksum(&buf);
            let c2 = checksum(vec_payload.as_slice());
            assert!(c1 > 0 && c2 > 0);
            println!("  FixedBuffer checksum={c1}, Vec checksum={c2}");
            println!("  FixedBuffer 与 Vec 统一走 AsRef<[u8]>");
            println!("  关键：边界 API 用 impl AsRef，避免调用方被迫 clone\n");
        }
    }

    pub fn run_all() {
        strategy_plugin::demonstrate();
        market_data_decoder::demonstrate();
        order_router::demonstrate();
        risk_pipeline::demonstrate();
        execution_sink::demonstrate();
        wire_encoding::demonstrate();
    }
}

// =============================================================================
// 第二部分：Web3 生产场景
// =============================================================================

mod web3 {
    // -------------------------------------------------------------------------
    // 场景 1：状态访问 —— 模拟器与 archival node 共享接口
    // -------------------------------------------------------------------------
    /// **生产问题**：MEV 搜索要 fork 主网状态并在上面模拟；真实节点读 LevelDB，
    /// 模拟器读内存 HashMap；两者应实现同一 `StateReader`。
    pub mod state_access {
        use std::collections::HashMap;

        pub type Address = [u8; 20];

        pub trait StateReader {
            fn get_balance(&self, addr: &Address) -> u128;
            fn get_storage(&self, addr: &Address, slot: u64) -> [u8; 32];
        }

        pub trait StateWriter: StateReader {
            fn set_balance(&mut self, addr: &Address, balance: u128);
            fn set_storage(&mut self, addr: &Address, slot: u64, value: [u8; 32]);
        }

        pub struct MemoryState {
            balances: HashMap<Address, u128>,
            storage: HashMap<(Address, u64), [u8; 32]>,
        }

        impl MemoryState {
            pub fn new() -> Self {
                Self {
                    balances: HashMap::new(),
                    storage: HashMap::new(),
                }
            }
        }

        impl StateReader for MemoryState {
            fn get_balance(&self, addr: &Address) -> u128 {
                self.balances.get(addr).copied().unwrap_or(0)
            }

            fn get_storage(&self, addr: &Address, slot: u64) -> [u8; 32] {
                self.storage
                    .get(&(*addr, slot))
                    .copied()
                    .unwrap_or([0u8; 32])
            }
        }

        impl StateWriter for MemoryState {
            fn set_balance(&mut self, addr: &Address, balance: u128) {
                self.balances.insert(*addr, balance);
            }

            fn set_storage(&mut self, addr: &Address, slot: u64, value: [u8; 32]) {
                self.storage.insert((*addr, slot), value);
            }
        }

        pub fn simulate_transfer<S: StateWriter>(state: &mut S, from: &Address, to: &Address, amount: u128) -> bool {
            let bal = state.get_balance(from);
            if bal < amount {
                return false;
            }
            state.set_balance(from, bal - amount);
            state.set_balance(to, state.get_balance(to) + amount);
            true
        }

        pub fn demonstrate() {
            println!("## Web3-1：状态访问（Supertrait StateWriter: StateReader）");
            let mut state = MemoryState::new();
            let alice = [1u8; 20];
            let bob = [2u8; 20];
            state.set_balance(&alice, 1_000);
            assert!(simulate_transfer(&mut state, &alice, &bob, 300));
            assert_eq!(state.get_balance(&bob), 300);
            println!("  模拟转账后 alice={} bob={}", state.get_balance(&alice), state.get_balance(&bob));
            println!("  关键：Supertrait 表达「可读才可写」的层级关系\n");
        }
    }

    // -------------------------------------------------------------------------
    // 场景 2：签名器 —— 本地/HSM/远程 KMS 可替换
    // -------------------------------------------------------------------------
    /// **生产问题**：测试用本地 key，生产用 AWS KMS 或 Ledger；业务层只依赖 `Signer`。
    pub mod signer {
        pub type Hash32 = [u8; 32];
        pub type Signature = [u8; 65];

        #[derive(Debug)]
        pub enum SignerError {
            HsmTimeout,
            InvalidKey,
        }

        pub trait Signer: Send + Sync {
            fn sign(&self, hash: &Hash32) -> Result<Signature, SignerError>;
            fn address(&self) -> [u8; 20];
        }

        pub struct LocalSigner {
            pub key_id: u32,
            pub addr: [u8; 20],
        }

        impl Signer for LocalSigner {
            fn sign(&self, hash: &Hash32) -> Result<Signature, SignerError> {
                let mut sig = [0u8; 65];
                sig[0..32].copy_from_slice(hash);
                sig[32] = self.key_id as u8;
                Ok(sig)
            }

            fn address(&self) -> [u8; 20] {
                self.addr
            }
        }

        pub struct KmsSigner {
            pub key_arn: String,
        }

        impl Signer for KmsSigner {
            fn sign(&self, _hash: &Hash32) -> Result<Signature, SignerError> {
                // 生产：HTTP 调 KMS；这里模拟超时
                if self.key_arn.is_empty() {
                    Err(SignerError::InvalidKey)
                } else {
                    Ok([1u8; 65])
                }
            }

            fn address(&self) -> [u8; 20] {
                [0xde; 20]
            }
        }

        pub fn build_and_sign<S: Signer>(signer: &S, tx_hash: Hash32) -> Result<Signature, SignerError> {
            signer.sign(&tx_hash)
        }

        pub fn demonstrate() {
            println!("## Web3-2：签名器（trait 抽象 KMS / 本地）");
            let local = LocalSigner {
                key_id: 7,
                addr: [0xab; 20],
            };
            let hash = [0x11u8; 32];
            let sig = build_and_sign(&local, hash).unwrap();
            assert_eq!(sig[32], 7);
            println!("  LocalSigner sig[32]={}", sig[32]);

            let kms = KmsSigner {
                key_arn: "arn:aws:kms:us-east-1:123:key/abc".into(),
            };
            assert!(build_and_sign(&kms, hash).is_ok());
            println!("  KmsSigner 同一接口，集成测试 mock、生产 KMS");
            println!("  关键：依赖倒置——上层只认识 Signer，不认识 KMS SDK\n");
        }
    }

    // -------------------------------------------------------------------------
    // 场景 3：DEX 报价 —— 泛型池状态 + trait 统一套利搜索
    // -------------------------------------------------------------------------
    /// **生产问题**：Uniswap V2 常数乘积、V3 集中流动性、Curve StableSwap 公式不同，
    /// 套利引擎用 `DexPool` 统一 `quote_out`。
    pub mod dex_pool {
        pub trait DexPool {
            fn pool_id(&self) -> &str;
            fn quote_out(&self, amount_in: u128) -> u128;
        }

        pub struct UniV2Pool {
            pub reserve_in: u128,
            pub reserve_out: u128,
        }

        impl DexPool for UniV2Pool {
            fn pool_id(&self) -> &str {
                "uni-v2-eth-usdc"
            }

            fn quote_out(&self, amount_in: u128) -> u128 {
                // x * y = k，含 0.3% 费
                let amount_in_with_fee = amount_in * 997;
                let numerator = amount_in_with_fee * self.reserve_out;
                let denominator = self.reserve_in * 1000 + amount_in_with_fee;
                numerator / denominator
            }
        }

        pub struct CurveStablePool {
            pub amp: u128,
            pub balance_a: u128,
            pub balance_b: u128,
        }

        impl DexPool for CurveStablePool {
            fn pool_id(&self) -> &str {
                "curve-3pool-usdc-usdt"
            }

            fn quote_out(&self, amount_in: u128) -> u128 {
                // 教学简化：stable pool 近似 1:1 减滑点
                let _ = self.amp;
                amount_in.saturating_sub(amount_in / 10_000)
            }
        }

        pub fn best_quote(pools: &[Box<dyn DexPool>], amount_in: u128) -> Option<(u128, &str)> {
            pools
                .iter()
                .map(|p| (p.quote_out(amount_in), p.pool_id()))
                .max_by_key(|(out, _)| *out)
        }

        pub fn demonstrate() {
            println!("## Web3-3：DEX 报价（dyn DexPool 多池比较）");
            let pools: Vec<Box<dyn DexPool>> = vec![
                Box::new(UniV2Pool {
                    reserve_in: 1_000_000,
                    reserve_out: 2_000_000,
                }),
                Box::new(CurveStablePool {
                    amp: 200,
                    balance_a: 5_000_000,
                    balance_b: 5_000_000,
                }),
            ];
            let (best, id) = best_quote(&pools, 10_000).unwrap();
            println!("  10_000 in → best out={} via {}", best, id);
            println!("  关键：trait 让套利搜索与具体 AMM 数学解耦\n");
        }
    }

    // -------------------------------------------------------------------------
    // 场景 4：交易校验 —— TryFrom 在边界解析地址/ calldata
    // -------------------------------------------------------------------------
    /// **生产问题**：RPC 入参是 hex string，内部应是 `[u8; 20]`；在边界一次性 `TryFrom`，
    /// 内核只处理强类型。
    pub mod typed_boundary {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
            pub struct Address([u8; 20]);

            impl Address {
                pub fn as_bytes(&self) -> &[u8; 20] {
                    &self.0
                }
            }

            #[derive(Debug)]
            pub enum ParseError {
                WrongLength,
                InvalidHex,
            }

            impl TryFrom<&str> for Address {
                type Error = ParseError;

                fn try_from(s: &str) -> Result<Self, Self::Error> {
                    let s = s.strip_prefix("0x").unwrap_or(s);
                    if s.len() != 40 {
                        return Err(ParseError::WrongLength);
                    }
                    let mut out = [0u8; 20];
                    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
                        if chunk.len() != 2 {
                            return Err(ParseError::InvalidHex);
                        }
                        let hi = hex_nibble(chunk[0]).ok_or(ParseError::InvalidHex)?;
                        let lo = hex_nibble(chunk[1]).ok_or(ParseError::InvalidHex)?;
                        out[i] = (hi << 4) | lo;
                    }
                    Ok(Address(out))
                }
            }

            fn hex_nibble(b: u8) -> Option<u8> {
                match b {
                    b'0'..=b'9' => Some(b - b'0'),
                    b'a'..=b'f' => Some(b - b'a' + 10),
                    b'A'..=b'F' => Some(b - b'A' + 10),
                    _ => None,
                }
            }

            pub trait TxValidator {
                fn validate(&self, from: &Address, calldata: &[u8]) -> Result<(), &'static str>;
            }

            pub struct NonEmptyCalldata;

            impl TxValidator for NonEmptyCalldata {
                fn validate(&self, _from: &Address, calldata: &[u8]) -> Result<(), &'static str> {
                    if calldata.is_empty() {
                        Err("empty calldata")
                    } else {
                        Ok(())
                    }
                }
            }

            pub fn demonstrate() {
                println!("## Web3-4：类型边界（TryFrom + TxValidator）");
                let addr = Address::try_from("0x0000000000000000000000000000000000000001").unwrap();
                let validator = NonEmptyCalldata;
                assert!(validator.validate(&addr, &[0x01]).is_ok());
                assert!(validator.validate(&addr, &[]).is_err());
                println!("  Address TryFrom 在 RPC 边界 parse-once");
                println!("  关键：内核 Address 强类型，边界 TryFrom 集中失败\n");
            }
        }

    // -------------------------------------------------------------------------
    // 场景 5：MEV Bundle —— Iterator + trait 筛选 profitable bundle
    // -------------------------------------------------------------------------
    /// **生产问题**：Builder 要从候选 tx 组合里找 profit 最大的 bundle；用 `Iterator`
    /// 与自定义 `Profitable` trait 组合，避免一次性 materialize 全排列。
    pub mod bundle_search {
        pub struct CandidateTx {
            pub gas: u64,
            pub priority_fee: u128,
            pub profit_wei: i128,
        }

        pub trait Profitable {
            fn net_profit(&self) -> i128;
        }

        impl Profitable for CandidateTx {
            fn net_profit(&self) -> i128 {
                self.profit_wei - self.priority_fee as i128
            }
        }

        pub fn top_profitable<I>(candidates: I, k: usize) -> Vec<i128>
        where
            I: IntoIterator<Item = CandidateTx>,
        {
            let mut profits: Vec<i128> = candidates
                .into_iter()
                .filter(|c| c.net_profit() > 0)
                .map(|c| c.net_profit())
                .collect();
            profits.sort_by(|a, b| b.cmp(a));
            profits.truncate(k);
            profits
        }

        pub fn demonstrate() {
            println!("## Web3-5：Bundle 搜索（Iterator + Profitable trait）");
            let candidates = vec![
                CandidateTx {
                    gas: 21000,
                    priority_fee: 1,
                    profit_wei: 100,
                },
                CandidateTx {
                    gas: 100_000,
                    priority_fee: 50,
                    profit_wei: 30,
                },
                CandidateTx {
                    gas: 80_000,
                    priority_fee: 10,
                    profit_wei: 200,
                },
            ];
            let top = top_profitable(candidates, 2);
            assert_eq!(top, vec![190, 99]);
            println!("  top-2 net profits: {:?}", top);
            println!("  关键：自定义 trait 表达领域语义，Iterator 组合零额外抽象成本\n");
        }
    }

    // -------------------------------------------------------------------------
    // 场景 6：事件索引 ——  blanket impl 为所有 T: Clone 提供 snapshot
    // -------------------------------------------------------------------------
    /// **生产问题**：Reorg 时需要回滚到某个 block 的状态快照； blanket impl 减少样板。
    pub mod reorg_journal {
        use std::collections::HashMap;

        pub trait Snapshot {
            type State: Clone;
            fn capture(&self) -> Self::State;
            fn restore(&mut self, snapshot: Self::State);
        }

        pub struct AccountDb {
            pub balances: HashMap<u64, u128>,
        }

        impl Snapshot for AccountDb {
            type State = HashMap<u64, u128>;

            fn capture(&self) -> Self::State {
                self.balances.clone()
            }

            fn restore(&mut self, snapshot: Self::State) {
                self.balances = snapshot;
            }
        }

        pub struct Journal<S: Snapshot> {
            pub db: S,
            pub checkpoints: Vec<S::State>,
        }

        impl<S: Snapshot> Journal<S> {
            pub fn checkpoint(&mut self) {
                self.checkpoints.push(self.db.capture());
            }

            pub fn rollback(&mut self, depth: usize) {
                if let Some(snap) = self.checkpoints.get(self.checkpoints.len().saturating_sub(depth)) {
                    self.db.restore(snap.clone());
                }
            }
        }

        pub fn demonstrate() {
            println!("## Web3-6：Reorg 回滚（Snapshot 关联类型 + Journal 泛型）");
            let mut journal = Journal {
                db: AccountDb {
                    balances: HashMap::from([(1, 100u128)]),
                },
                checkpoints: vec![],
            };
            journal.checkpoint();
            journal.db.balances.insert(1, 50);
            journal.rollback(1);
            assert_eq!(journal.db.balances[&1], 100);
            println!("  reorg 后 balance 恢复为 100");
            println!("  关键：关联类型 State 与组件状态类型精确绑定\n");
        }
    }

    pub fn run_all() {
        state_access::demonstrate();
        signer::demonstrate();
        dex_pool::demonstrate();
        typed_boundary::demonstrate();
        bundle_search::demonstrate();
        reorg_journal::demonstrate();
    }
}

// =============================================================================
// 第三部分：泛化 —— 一般性问题与应对策略
// =============================================================================

mod generalized {
    /// **Object Safety 速查**：下列写法不能 `dyn Trait`（编译器会报错）：
    /// - 方法返回 `Self`
    /// - 带未指定类型的泛型方法 `fn foo<T>(&self)`
    /// - 关联函数非 `Self: Sized` 约束时作为 trait object 调用
    ///
    /// **应对**：返回 `Box<dyn Trait>` 或关联类型；热路径改用泛型。
    pub mod object_safety {
        pub trait GoodForDyn {
            fn name(&self) -> &str;
        }

        pub struct Echo;

        impl GoodForDyn for Echo {
            fn name(&self) -> &str {
                "echo"
            }
        }

        pub fn as_dyn(obj: &dyn GoodForDyn) -> &str {
            obj.name()
        }

        pub fn demonstrate() {
            println!("## 泛化-1：Object Safety");
            let echo = Echo;
            let name = as_dyn(&echo);
            println!("  dyn GoodForDyn name={}", name);
            println!("  规则：要 dyn 则避免返回 Self / 泛型方法");
            println!("  策略：插件、配置驱动 → dyn；热路径 → 泛型\n");
        }
    }

    /// **静态 vs 动态分发决策树**
    pub mod dispatch_strategy {
        pub trait Encoder {
            fn encode(&self, out: &mut Vec<u8>);
        }

        pub struct RlpEncoder;

        impl Encoder for RlpEncoder {
            fn encode(&self, out: &mut Vec<u8>) {
                out.push(0xff);
            }
        }

        // 静态：编译期已知实现
        pub fn encode_static<E: Encoder>(enc: &E, out: &mut Vec<u8>) {
            enc.encode(out);
        }

        // 动态：运行时选择
        pub fn encode_dynamic(enc: &dyn Encoder, out: &mut Vec<u8>) {
            enc.encode(out);
        }

        pub fn demonstrate() {
            println!("## 泛化-2：静态 vs 动态分发");
            let mut buf = vec![];
            encode_static(&RlpEncoder, &mut buf);
            assert_eq!(buf, vec![0xff]);
            buf.clear();
            encode_dynamic(&RlpEncoder, &mut buf);
            assert_eq!(buf, vec![0xff]);
            println!("  结果相同；差异在编译期单态化 vs 运行时 vtable");
            println!("  | 场景           | 推荐          | 原因");
            println!("  |----------------|---------------|------------------");
            println!("  | 撮合/解码热路径 | impl Trait    | 零 vtable，可 inline");
            println!("  | 插件/脚本/配置  | dyn Trait     | 运行时加载");
            println!("  | 测试 mock       | impl Trait    | 单测编译期替换");
            println!("  | FFI 回调        | dyn Trait     | 外部函数指针\n");
        }
    }

    /// **关联类型 vs 泛型参数**
    pub mod associated_type_vs_generic {
        pub trait ParserGeneric<P> {
            fn parse(&self, input: P) -> u64;
        }

        pub trait ParserAssoc {
            type Input;
            fn parse(&self, input: Self::Input) -> u64;
        }

        pub struct U64Parser;

        impl ParserGeneric<&str> for U64Parser {
            fn parse(&self, input: &str) -> u64 {
                input.parse().unwrap_or(0)
            }
        }

        impl ParserAssoc for U64Parser {
            type Input = &'static str;

            fn parse(&self, input: Self::Input) -> u64 {
                input.parse().unwrap_or(0)
            }
        }

        pub fn demonstrate() {
            println!("## 泛化-3：关联类型 vs 泛型参数");
            let p = U64Parser;
            assert_eq!(ParserGeneric::parse(&p, "42"), 42);
            assert_eq!(ParserAssoc::parse(&p, "42"), 42);
            println!("  每个 impl 只一种 Input → 用关联类型（trait 更简洁）");
            println!("  同一 impl 多种 Input → 用泛型参数");
            println!("  HFT 解码/Web3 State 典型用关联类型\n");
        }
    }

    /// **Newtype + trait：孤儿规则 workaround**
    pub mod newtype_pattern {
        pub struct UserId(u64);

        pub trait Identifiable {
            fn id(&self) -> u64;
        }

        impl Identifiable for UserId {
            fn id(&self) -> u64 {
                self.0
            }
        }

        pub fn demonstrate() {
            println!("## 泛化-4：Newtype 绕过孤儿规则");
            let u = UserId(42);
            println!("  不能 impl Display for u64（孤儿），但可以 newtype 包装");
            println!("  UserId.id() = {}", u.id());
            println!("  策略：外部类型 + 本地 trait → newtype 包装\n");
        }
    }

    /// **Extension trait：为已有类型追加领域方法**
    pub mod extension_trait {
        pub trait SaturationAdd {
            fn sat_add(self, rhs: Self) -> Self;
        }

        impl SaturationAdd for u64 {
            fn sat_add(self, rhs: Self) -> Self {
                self.saturating_add(rhs)
            }
        }

        pub fn demonstrate() {
            println!("## 泛化-5：Extension trait");
            let a: u64 = u64::MAX;
            println!("  u64::MAX.sat_add(1) = {}", a.sat_add(1));
            println!("  策略：不修改 std 类型，用 trait 扩展领域语义\n");
        }
    }

    /// **总决策表**
    pub fn print_playbook() {
        println!("## 泛化-6：Trait 设计决策手册");
        println!("  1. 先问「有几种实现？」—— 1 种不必 trait，2+ 才抽象");
        println!("  2. 问「何时确定实现？」—— 编译期 → 泛型；运行时 → dyn");
        println!("  3. 问「是否跨线程？」—— 加 Send + Sync supertrait");
        println!("  4. 问「输入输出类型是否随 impl 变？」—— 关联类型");
        println!("  5. 问「是否要组合多个检查/ handler？」—— Vec<Box<dyn T>> 责任链");
        println!("  6. 问「是否只转换一次？」—— TryFrom/From 在边界，内核强类型");
        println!("  7. 问「能否 blanket impl？」—— 谨慎：易 impl 冲突，优先显式 impl");
        println!();
        println!("  HFT 典型组合：Strategy + RiskCheck + OrderRouter + AsRef 报文");
        println!("  Web3 典型组合：StateReader/Writer + Signer + DexPool + TryFrom 边界");
        println!("  共同原则：trait 定义「做什么」，impl 定义「怎么做」，main/引擎只做编排");
    }

    pub fn run_all() {
        object_safety::demonstrate();
        dispatch_strategy::demonstrate();
        associated_type_vs_generic::demonstrate();
        newtype_pattern::demonstrate();
        extension_trait::demonstrate();
        print_playbook();
    }
}

fn main() {
    println!("=== Rust Trait System：HFT / Web3 生产场景 ===\n");

    println!("--- 第一部分：HFT ---\n");
    hft::run_all();

    println!("--- 第二部分：Web3 ---\n");
    web3::run_all();

    println!("--- 第三部分：泛化策略 ---\n");
    generalized::run_all();

    println!("=== 全部示例运行完毕 ===");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hft_strategy_static_dispatch() {
        let ticks = [hft::strategy_plugin::Tick {
            symbol_id: 1,
            bid: 100,
            ask: 105,
        }];
        let mut mm = hft::strategy_plugin::MarketMaker {
            spread_threshold: 3,
            orders_sent: 0,
        };
        assert_eq!(hft::strategy_plugin::run_hot_path(&mut mm, &ticks), 1);
    }

    #[test]
    fn web3_state_transfer() {
        use web3::state_access::{MemoryState, StateReader, StateWriter};

        let mut state = MemoryState::new();
        let alice = [1u8; 20];
        let bob = [2u8; 20];
        state.set_balance(&alice, 500);
        assert!(web3::state_access::simulate_transfer(
            &mut state, &alice, &bob, 100
        ));
        assert_eq!(state.get_balance(&bob), 100);
    }

    #[test]
    fn web3_address_try_from() {
        let addr =
            web3::typed_boundary::Address::try_from("0x0000000000000000000000000000000000000001")
                .unwrap();
        assert_eq!(addr.as_bytes()[19], 1);
    }
}
