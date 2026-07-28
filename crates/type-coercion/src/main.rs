//! # Type Coercion 深度实践
//!
//! 考察点：
//! 1. Rust **自动 coercion**（Deref、Unsize、子类型）与 **显式转换**（`as`、`From`、`TryFrom`）的边界
//! 2. HFT/Web3 中「静默截断」「精度丢失」「生命周期缩短」等生产事故
//! 3. 在 API 边界用强类型 + TryFrom，在内核用热路径友好类型（切片、Copy、固定宽度整数）
//! 4. 从具体场景泛化到一般性应对策略
//!
//! 运行：`cargo run -p type-coercion`

#![allow(dead_code)]

// =============================================================================
// 第一部分：HFT 生产场景
// =============================================================================

mod hft {
    // -------------------------------------------------------------------------
    // 场景 1：价格表示 —— 禁止 f64 静默 coercion 进撮合内核
    // -------------------------------------------------------------------------
    /// **生产问题**：行情网关用 f64 表示价格，撮合引擎用 i64 tick；
    /// `as i64` 截断或 `(px * 1e4) as i64` 浮点误差导致错价发单、Reg NMS 违规。
    pub mod fixed_point_price {
        /// 固定小数位价格：1 unit = 10^-scale 元
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct TickPrice {
            pub ticks: i64,
            pub scale: u8,
        }

        impl TickPrice {
            pub fn from_decimal_str(s: &str, scale: u8) -> Result<Self, &'static str> {
                let parts: Vec<&str> = s.split('.').collect();
                match parts.as_slice() {
                    [whole, frac] => {
                        let frac_padded = format!("{:0<width$}", frac, width = scale as usize);
                        if frac_padded.len() > scale as usize {
                            return Err("fraction too precise");
                        }
                        let whole: i64 = whole
                            .parse()
                            .map_err(|_| "invalid whole part")?;
                        let frac_part: i64 = frac_padded
                            .parse()
                            .map_err(|_| "invalid fraction")?;
                        let multiplier = 10_i64.pow(scale as u32);
                        Ok(TickPrice {
                            ticks: whole
                                .checked_mul(multiplier)
                                .and_then(|w| w.checked_add(frac_part))
                                .ok_or("overflow")?,
                            scale,
                        })
                    }
                    [whole] if scale == 0 => {
                        let whole: i64 = whole.parse().map_err(|_| "invalid")?;
                        Ok(TickPrice { ticks: whole, scale: 0 })
                    }
                    _ => Err("invalid decimal format"),
                }
            }

            /// ❌ 反模式：浮点桥接 —— 演示为何生产禁用
            pub fn from_f64_lossy(px: f64, scale: u8) -> Self {
                let multiplier = 10_f64.powi(scale as i32);
                TickPrice {
                    ticks: (px * multiplier) as i64,
                    scale,
                }
            }
        }

        pub fn match_prices(a: TickPrice, b: TickPrice) -> bool {
            assert_eq!(a.scale, b.scale, "scale mismatch — normalize first");
            a.ticks == b.ticks
        }

        pub fn demonstrate() {
            println!("## HFT-1：固定点价格（拒绝 f64 静默进内核）");
            // f64 只能精确表示到 2^53；更大整数会先丢精度再 as i64
            let good = TickPrice::from_decimal_str("9007199254740993", 0).unwrap();
            let bad = TickPrice::from_f64_lossy(9_007_199_254_740_993.0, 0);
            println!("  字符串解析 ticks = {}", good.ticks);
            println!("  f64  截断 ticks = {} （2^53+1 变成 2^53）", bad.ticks);
            assert_ne!(good.ticks, bad.ticks);
            println!("  策略：边界 parse → TickPrice；内核只比较 i64 ticks\n");
        }
    }

    // -------------------------------------------------------------------------
    // 场景 2：协议枚举 —— TryFrom 替代 as u8 强转
    // -------------------------------------------------------------------------
    /// **生产问题**：FIX Tag 54 Side 非法值被 `as` 强转成枚举，静默变成 Buy，
    /// 风控认为卖单实际是买单 → 敞口失控。
    pub mod wire_enum_boundary {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Side {
            Buy,
            Sell,
        }

        impl TryFrom<u8> for Side {
            type Error = &'static str;

            fn try_from(v: u8) -> Result<Self, Self::Error> {
                match v {
                    b'1' => Ok(Side::Buy),
                    b'2' => Ok(Side::Sell),
                    _ => Err("invalid FIX side"),
                }
            }
        }

        /// ❌ 反模式：非法值静默落到 Buy
        pub fn parse_side_lossy(raw: u8) -> Side {
            match raw {
                b'1' => Side::Buy,
                b'2' => Side::Sell,
                _ => Side::Buy, // 生产事故：未知 side 当买单
            }
        }

        pub fn parse_side_safe(raw: u8) -> Result<Side, &'static str> {
            Side::try_from(raw)
        }

        pub fn route_order(side: Side) -> &'static str {
            match side {
                Side::Buy => "BUY_GATE",
                Side::Sell => "SELL_GATE",
            }
        }

        pub fn demonstrate() {
            println!("## HFT-2：协议枚举（TryFrom 替代 as 强转）");
            assert_eq!(parse_side_safe(b'1').unwrap(), Side::Buy);
            assert!(parse_side_safe(b'9').is_err());
            let bogus = parse_side_lossy(b'9');
            println!("  合法 b'1' → {:?}", Side::try_from(b'1').unwrap());
            println!("  非法 b'9' TryFrom → Err");
            println!(
                "  非法 b'9' 默认分支 → {:?} （静默当 Buy，路由到 {}）",
                bogus,
                route_order(bogus)
            );
            println!("  策略：wire u8 → TryFrom → 领域枚举；match 穷尽\n");
        }
    }

    // -------------------------------------------------------------------------
    // 场景 3：Deref coercion —— API 宽进，热路径窄用
    // -------------------------------------------------------------------------
    /// **生产问题**：回调签名要 `&str`，调用方传 `String`/`&String`/`Cow<str>`；
    /// 依赖 Deref coercion 可以编译，但若在循环内反复 `to_string()` 则 GC/alloc 打爆延迟。
    pub mod deref_api_boundary {
        use std::borrow::Cow;

        pub struct SymbolRegistry {
            ids: std::collections::HashMap<String, u32>,
        }

        impl SymbolRegistry {
            pub fn new() -> Self {
                let mut ids = std::collections::HashMap::new();
                ids.insert("AAPL".into(), 1);
                ids.insert("MSFT".into(), 2);
                Self { ids }
            }

            /// 宽接口：接受任何 AsRef<str>，Deref coercion 自动 &String → &str
            pub fn lookup<S: AsRef<str>>(&self, sym: S) -> Option<u32> {
                self.ids.get(sym.as_ref()).copied()
            }

            /// 热路径：只接受 &str，避免隐藏分配
            pub fn lookup_hot(&self, sym: &str) -> Option<u32> {
                self.ids.get(sym).copied()
            }
        }

        pub fn batch_lookup(registry: &SymbolRegistry, symbols: &[&str]) -> Vec<u32> {
            symbols
                .iter()
                .filter_map(|s| registry.lookup_hot(s))
                .collect()
        }

        pub fn demonstrate() {
            println!("## HFT-3：Deref coercion（API 宽进，热路径 &str）");
            let reg = SymbolRegistry::new();
            let owned = String::from("AAPL");
            let cow = Cow::Borrowed("MSFT");

            // Deref coercion: &String → &str, Cow → &str
            assert_eq!(reg.lookup(&owned), Some(1));
            assert_eq!(reg.lookup(cow), Some(2));

            let ids = batch_lookup(&reg, &["AAPL", "MSFT"]);
            println!("  lookup(&String) / Cow 均可（Deref coercion）");
            println!("  热路径 batch → {:?}", ids);
            println!("  策略：冷路径 AsRef<str>；热路径 &str / symbol_id\n");
        }
    }

    // -------------------------------------------------------------------------
    // 场景 4：Unsize coercion —— 栈数组 → 切片，零拷贝批提交
    // -------------------------------------------------------------------------
    /// **生产问题**：策略一次产生固定上限 N 笔 intent，若 collect 成 Vec 再提交多一次 alloc；
    /// `[T; N]` 自动 unsizing 成 `&[T]` 传给下游。
    pub mod unsizing_batch {
        #[derive(Debug, Clone, Copy)]
        pub struct OrderIntent {
            pub symbol_id: u32,
            pub qty: u32,
            pub limit_px: i64,
        }

        pub struct OrderGateway;

        impl OrderGateway {
            /// 接受 &[OrderIntent] —— 调用方 [T;N] 自动 coerce
            pub fn submit_batch(&self, intents: &[OrderIntent]) -> usize {
                intents.len()
            }
        }

        pub fn strategy_emit<const N: usize>() -> ([OrderIntent; N], usize) {
            let intents = [OrderIntent {
                symbol_id: 1,
                qty: 100,
                limit_px: 150_00,
            }; N];
            let gw = OrderGateway;
            // Unsize: &[OrderIntent; N] → &[OrderIntent]
            let sent = gw.submit_batch(&intents);
            (intents, sent)
        }

        pub fn demonstrate() {
            println!("## HFT-4：Unsize coercion（[T;N] → &[T] 零拷贝批提交）");
            let (_buf, n) = strategy_emit::<8>();
            println!("  栈上 [OrderIntent; 8] 直接 submit_batch → {} 笔", n);
            println!("  策略：固定上限用数组；网关收 &[T]；避免 Vec alloc\n");
        }
    }

    // -------------------------------------------------------------------------
    // 场景 5：函数指针 coercion —— C 回调注册
    // -------------------------------------------------------------------------
    /// **生产问题**：交易所 C API 要求 `extern "C" fn(*const u8, usize)`，
    /// Rust 闭包不能 coerce；须用 fn item → fn pointer coercion。
    pub mod fn_ptr_ffi {
        use std::sync::atomic::{AtomicUsize, Ordering};

        pub type CTickCallback = extern "C" fn(*const u8, usize);

        static LAST_LEN: AtomicUsize = AtomicUsize::new(0);

        extern "C" fn on_tick(ptr: *const u8, len: usize) {
            LAST_LEN.store(len, Ordering::Relaxed);
            let _slice = unsafe { std::slice::from_raw_parts(ptr, len) };
        }

        pub struct NativeFeed {
            callback: CTickCallback,
        }

        impl NativeFeed {
            pub fn register(cb: CTickCallback) -> Self {
                Self { callback: cb }
            }

            pub fn dispatch(&self, data: &[u8]) {
                (self.callback)(data.as_ptr(), data.len());
            }
        }

        pub fn demonstrate() {
            println!("## HFT-5：函数指针 coercion（fn item → extern fn ptr）");
            // fn item coerces to fn pointer
            let feed = NativeFeed::register(on_tick);
            feed.dispatch(b"quote");
            assert_eq!(LAST_LEN.load(Ordering::Relaxed), 5);
            println!("  on_tick fn item → extern \"C\" fn 指针注册成功");
            println!("  策略：FFI 用 fn 指针；业务逻辑用泛型/trait 内部封装\n");
        }
    }

    // -------------------------------------------------------------------------
    // 场景 6：整数拓宽 —— symbol_id / qty 层级
    // -------------------------------------------------------------------------
    /// **生产问题**：wire 上 qty 是 u32，聚合敞口用 u64；静默 `as u64` 可行，
    /// 但反向 `as u32` 截断导致超大单被静默缩小。
    pub mod integer_widen_narrow {
        pub fn aggregate_qty(wire_qty: u32, existing: u64) -> Result<u64, &'static str> {
            let added = u64::from(wire_qty); // 显式拓宽，安全
            added
                .checked_add(existing)
                .ok_or("position overflow")
        }

        pub fn split_qty(total: u64, max_child: u32) -> Result<Vec<u32>, &'static str> {
            let max = u64::from(max_child);
            if total > max * (u32::MAX as u64) {
                return Err("too many splits");
            }
            let mut rem = total;
            let mut out = Vec::new();
            while rem > 0 {
                let chunk = rem.min(max);
                out.push(u32::try_from(chunk).map_err(|_| "chunk overflow")?);
                rem -= chunk;
            }
            Ok(out)
        }

        pub fn demonstrate() {
            println!("## HFT-6：整数拓宽/收窄（u32→u64 安全，反向 TryFrom）");
            assert_eq!(aggregate_qty(1_000, 1_000_000).unwrap(), 1_001_000);
            let chunks = split_qty(250, 100).unwrap();
            println!("  u32→u64 用 From；u64→u32 用 TryFrom");
            println!("  split_qty(250, max=100) → {:?}", chunks);
            assert!(u32::try_from(1_u64 << 40).is_err());
            println!("  大数收窄失败 → Err，而非静默截断\n");
        }
    }

    pub fn run_all() {
        fixed_point_price::demonstrate();
        wire_enum_boundary::demonstrate();
        deref_api_boundary::demonstrate();
        unsizing_batch::demonstrate();
        fn_ptr_ffi::demonstrate();
        integer_widen_narrow::demonstrate();
    }
}

// =============================================================================
// 第二部分：Web3 生产场景
// =============================================================================

mod web3 {
    // -------------------------------------------------------------------------
    // 场景 1：Address 边界 —— hex 字符串 vs [u8; 20]
    // -------------------------------------------------------------------------
    /// **生产问题**：用户粘贴 `0xAbCd...` 混合大小写，若用字符串比较或截断前导零，
    /// 可能转错地址导致资金发送到黑洞。
    pub mod address_boundary {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct Address([u8; 20]);

        impl Address {
            pub fn as_bytes(&self) -> &[u8; 20] {
                &self.0
            }
        }

        impl TryFrom<&str> for Address {
            type Error = &'static str;

            fn try_from(s: &str) -> Result<Self, Self::Error> {
                let hex = s.strip_prefix("0x").unwrap_or(s);
                if hex.len() != 40 {
                    return Err("address must be 20 bytes (40 hex chars)");
                }
                let mut out = [0u8; 20];
                for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
                    if chunk.len() != 2 {
                        return Err("odd hex length");
                    }
                    let hi = hex_nibble(chunk[0])?;
                    let lo = hex_nibble(chunk[1])?;
                    out[i] = (hi << 4) | lo;
                }
                Ok(Address(out))
            }
        }

        fn hex_nibble(b: u8) -> Result<u8, &'static str> {
            match b {
                b'0'..=b'9' => Ok(b - b'0'),
                b'a'..=b'f' => Ok(b - b'a' + 10),
                b'A'..=b'F' => Ok(b - b'A' + 10),
                _ => Err("invalid hex"),
            }
        }

        /// 内核只认 Address，不认 &str
        pub fn transfer(from: Address, to: Address, amount: u128) -> bool {
            let _ = (from, to, amount);
            true
        }

        pub fn demonstrate() {
            println!("## Web3-1：Address 边界（TryFrom &str → [u8;20]）");
            let addr =
                Address::try_from("0x0000000000000000000000000000000000000001").unwrap();
            assert!(Address::try_from("0x001").is_err());
            assert!(transfer(
                addr,
                Address::try_from("0x0000000000000000000000000000000000000002").unwrap(),
                1
            ));
            println!("  RPC/前端 &str → TryFrom → Address；合约交互只用 Address");
            println!("  非法长度/hex → Err，禁止 slice[0..20] 截断\n");
        }
    }

    // -------------------------------------------------------------------------
    // 场景 2：Wei 整数 —— 禁止 f64 ETH 进账本
    // -------------------------------------------------------------------------
    /// **生产问题**：`eth * 1e18 as u128` 浮点误差导致余额差 1 wei，MEV 会计对不上链上。
    pub mod wei_integer {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct Wei(u128);

        impl Wei {
            pub const fn zero() -> Self {
                Wei(0)
            }

            pub fn from_eth_decimal(s: &str) -> Result<Self, &'static str> {
                let parts: Vec<&str> = s.split('.').collect();
                let (whole, frac) = match parts.as_slice() {
                    [w] => (*w, ""),
                    [w, f] => (*w, *f),
                    _ => return Err("invalid eth format"),
                };
                let whole: u128 = whole.parse().map_err(|_| "bad whole")?;
                let frac_padded = format!("{:0<18}", frac);
                if frac_padded.len() > 18 {
                    return Err("sub-wei precision");
                }
                let frac_part: u128 = frac_padded.parse().map_err(|_| "bad frac")?;
                let base = 10_u128.pow(18);
                Ok(Wei(
                    whole
                        .checked_mul(base)
                        .and_then(|w| w.checked_add(frac_part))
                        .ok_or("overflow")?,
                ))
            }

            pub fn from_eth_f64_lossy(eth: f64) -> Self {
                Wei((eth * 1e18) as u128)
            }

            pub fn checked_add(self, rhs: Self) -> Option<Self> {
                self.0.checked_add(rhs.0).map(Wei)
            }
        }

        pub fn demonstrate() {
            println!("## Web3-2：Wei 整数（拒绝 f64 × 1e18）");
            let precise = Wei::from_eth_decimal("1.000000000000000001").unwrap();
            let lossy = Wei::from_eth_f64_lossy(1.000000000000000001);
            println!("  精确 wei = {}", precise.0);
            println!("  f64  wei = {} （可能差 1 wei）", lossy.0);
            assert_ne!(precise, lossy);
            println!("  策略：链上单位全 u128/U256；UI 层 decimal 字符串 parse\n");
        }
    }

    // -------------------------------------------------------------------------
    // 场景 3：Calldata 切片 —— Unsize + 生命周期
    // -------------------------------------------------------------------------
    /// **生产问题**：构造交易时 calldata 有时是栈上数组有时是 Vec；
    /// 统一 `&[u8]` 接口，靠 unsizing coercion 零拷贝传递。
    pub mod calldata_unsize {
        pub struct TxBuilder;

        impl TxBuilder {
            pub fn encode_call(&self, selector: [u8; 4], args: &[u8]) -> Vec<u8> {
                let mut out = Vec::with_capacity(4 + args.len());
                out.extend_from_slice(&selector);
                out.extend_from_slice(args);
                out
            }
        }

        pub fn build_swap_calldata(amount: u128) -> Vec<u8> {
            let mut args = [0u8; 32];
            args[16..].copy_from_slice(&amount.to_be_bytes());
            let selector = [0xa9, 0x05, 0x9c, 0xbb]; // swap selector 示意
            TxBuilder.encode_call(selector, &args) // [u8;32] → &[u8]
        }

        pub fn demonstrate() {
            println!("## Web3-3：Calldata Unsize（[u8;32] → &[u8]）");
            let calldata = build_swap_calldata(1_000_000_000_000_000_000);
            assert_eq!(calldata.len(), 36);
            println!("  selector + args 栈数组 unsizing 进 encode_call");
            println!("  策略：编码器收 &[u8]；调用方数组/Vec 均可 coerce\n");
        }
    }

    // -------------------------------------------------------------------------
    // 场景 4：Provider unsizing —— 具体 RPC → dyn Trait
    // -------------------------------------------------------------------------
    /// **生产问题**：测试用 MockProvider，生产用 HttpProvider；
    /// `Box<HttpProvider>` 需 unsize 成 `Box<dyn Provider>` 注入引擎。
    pub mod provider_unsize {
        pub trait Provider {
            fn block_number(&self) -> u64;
        }

        pub struct MockProvider {
            pub n: u64,
        }

        impl Provider for MockProvider {
            fn block_number(&self) -> u64 {
                self.n
            }
        }

        pub struct Engine {
            provider: Box<dyn Provider>,
        }

        impl Engine {
            pub fn new<P: Provider + 'static>(p: P) -> Self {
                Self {
                    provider: Box::new(p), // sized P → dyn Provider unsize
                }
            }

            pub fn sync(&self) -> u64 {
                self.provider.block_number()
            }
        }

        pub fn demonstrate() {
            println!("## Web3-4：Provider Unsize（Box<P> → Box<dyn Provider>）");
            let engine = Engine::new(MockProvider { n: 21_000_000 });
            assert_eq!(engine.sync(), 21_000_000);
            println!("  Box::new(concrete) 自动 unsize 为 Box<dyn Provider>");
            println!("  策略：边界 dyn；模拟/生产可替换\n");
        }
    }

    // -------------------------------------------------------------------------
    // 场景 5：Token amount 拓宽 —— u64 余额 vs u256 链上
    // -------------------------------------------------------------------------
    /// **生产问题**：ERC20 balanceOf 返回 U256，本地缓存用 u64；
    /// 必须在边界显式 try_into，不能 `as u64`。
    pub mod token_amount_widen {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct U256([u64; 4]); // 简化演示

        impl U256 {
            pub fn from_u64(v: u64) -> Self {
                U256([v, 0, 0, 0])
            }

            pub fn from_limbs(limbs: [u64; 4]) -> Self {
                U256(limbs)
            }

            pub fn try_to_u64(self) -> Result<u64, &'static str> {
                if self.0[1] | self.0[2] | self.0[3] != 0 {
                    return Err("value exceeds u64");
                }
                Ok(self.0[0])
            }
        }

        pub struct CacheBalance {
            pub amount: u64,
        }

        pub fn sync_from_chain(on_chain: U256) -> Result<CacheBalance, &'static str> {
            Ok(CacheBalance {
                amount: on_chain.try_to_u64()?,
            })
        }

        pub fn demonstrate() {
            println!("## Web3-5：Token amount（U256 → u64 用 TryInto）");
            assert_eq!(
                sync_from_chain(U256::from_u64(1_000)).unwrap().amount,
                1_000
            );
            assert!(sync_from_chain(U256::from_limbs([0, 1, 0, 0])).is_err());
            println!("  链上 U256 → try_to_u64 → 缓存");
            println!("  超大余额 → Err 触发告警，而非 as u64 截断\n");
        }
    }

    // -------------------------------------------------------------------------
    // 场景 6：Reborrow coercion —— &mut State → &State 只读快照
    // -------------------------------------------------------------------------
    /// **生产问题**：模拟执行前要读余额写日志，函数签名要 `&State`，
    /// 调用方只有 `&mut State`；Reborrow 自动 `&mut T` → `&T`。
    pub mod reborrow_snapshot {
        #[derive(Debug, Default)]
        pub struct State {
            pub balances: std::collections::HashMap<[u8; 20], u128>,
        }

        impl State {
            pub fn get_balance(&self, addr: &[u8; 20]) -> u128 {
                *self.balances.get(addr).unwrap_or(&0)
            }

            pub fn credit(&mut self, addr: [u8; 20], amt: u128) {
                *self.balances.entry(addr).or_insert(0) += amt;
            }
        }

        pub fn audit_snapshot(state: &State, addr: &[u8; 20]) -> u128 {
            state.get_balance(addr)
        }

        pub fn simulate(state: &mut State, addr: [u8; 20], delta: u128) -> u128 {
            // Reborrow: &mut State coerces to &State for audit_snapshot
            let before = audit_snapshot(state, &addr);
            state.credit(addr, delta);
            before
        }

        pub fn demonstrate() {
            println!("## Web3-6：Reborrow coercion（&mut T → &T 只读借用）");
            let mut st = State::default();
            let alice = [1u8; 20];
            st.credit(alice, 100);
            let before = simulate(&mut st, alice, 50);
            assert_eq!(before, 100);
            assert_eq!(st.get_balance(&alice), 150);
            println!("  simulate 内 audit_snapshot(&mut st) 自动 reborrow");
            println!("  策略：只读 API 收 &T；可变上下文可安全传入\n");
        }
    }

    pub fn run_all() {
        address_boundary::demonstrate();
        wei_integer::demonstrate();
        calldata_unsize::demonstrate();
        provider_unsize::demonstrate();
        token_amount_widen::demonstrate();
        reborrow_snapshot::demonstrate();
    }
}

// =============================================================================
// 第三部分：泛化策略
// =============================================================================

mod generalized {
    /// **Coercion vs Cast vs From**：决策对照
    pub mod coercion_vs_cast {
        pub fn deref_coercion_example(s: &String) -> usize {
            s.len() // &String → &str via Deref
        }

        pub fn explicit_from(s: String) -> Vec<u8> {
            s.into_bytes() // consumes String, not coercion
        }

        pub fn lossy_cast(x: f64) -> i64 {
            x as i64 // explicit, lossy — never in money path
        }

        pub fn demonstrate() {
            println!("## 泛化-1：Coercion vs Cast vs From");
            let s = String::from("ETH");
            assert_eq!(deref_coercion_example(&s), 3);
            assert_eq!(explicit_from(s), b"ETH".to_vec());
            println!("  Deref coercion：自动、无消耗、&SmartPointer → &Target");
            println!("  From/Into：显式、常消耗、类型变体");
            println!("  as cast：显式、可丢失精度、禁止用于钱/量/地址\n");
        }
    }

    /// **API 设计：宽进窄出**
    pub mod api_wide_narrow {
        pub fn parse_config(raw: &str) -> Result<u64, std::num::ParseIntError> {
            raw.parse::<u64>()
        }

        pub fn accept_any_string<S: AsRef<str>>(s: S) -> usize {
            s.as_ref().len()
        }

        pub fn demonstrate() {
            println!("## 泛化-2：API 宽进窄出");
            assert_eq!(accept_any_string(String::from("rpc")), 3);
            assert_eq!(parse_config("8080").unwrap(), 8080);
            println!("  边界：AsRef / TryFrom / parse — 一次转换");
            println!("  内核：Copy 整数、&str、&[u8] — 无隐藏 alloc\n");
        }
    }

    /// **Unsize 使用时机**
    pub mod when_unsize {
        pub trait Handler {
            fn handle(&self, msg: &[u8]);
        }

        pub struct LoggingHandler;

        impl Handler for LoggingHandler {
            fn handle(&self, msg: &[u8]) {
                let _ = msg.len();
            }
        }

        pub fn dispatch(h: &dyn Handler, msg: &[u8]) {
            h.handle(msg);
        }

        pub fn demonstrate() {
            println!("## 泛化-3：何时用 Unsize");
            let fixed = [1u8, 2, 3];
            let h = LoggingHandler;
            dispatch(&h, &fixed);
            println!("  [T;N] → &[T]：批数据、calldata、栈缓冲");
            println!("  T → dyn Trait：插件、Provider、Signer 运行时替换");
            println!("  不要：为了 dyn 把 Copy 热路径对象装箱\n");
        }
    }

    /// **边界转换模式（Parse, don't validate 的对偶）**
    pub mod boundary_pattern {
        #[derive(Clone, Copy, PartialEq, Eq)]
        pub struct OrderId(u64);

        impl TryFrom<&str> for OrderId {
            type Error = &'static str;

            fn try_from(s: &str) -> Result<Self, Self::Error> {
                if s.is_empty() {
                    return Err("empty");
                }
                let n: u64 = s.parse().map_err(|_| "not u64")?;
                Ok(OrderId(n))
            }
        }

        pub fn process(id: OrderId) -> u64 {
            id.0
        }

        pub fn demonstrate() {
            println!("## 泛化-4：边界强类型（Newtype + TryFrom）");
            let id = OrderId::try_from("42").unwrap();
            assert_eq!(process(id), 42);
            println!("  入口：wire/RPC/JSON → TryFrom → Newtype");
            println!("  内核：只接受 OrderId，不可能混入原始 &str\n");
        }
    }

    /// **Deref 陷阱清单**
    pub mod deref_traps {
        pub fn demonstrate() {
            println!("## 泛化-5：Deref 陷阱");
            println!("  1. 过度 Deref impl Newtype → 失去类型安全");
            println!("  2. 热路径隐式 String clone（先 as_ref 再 to_owned）");
            println!("  3. 多层 Deref 降低可读性 — 显式 .as_ref() 更清晰");
            println!("  4. 自定义 Deref 带副作用 — 禁止\n");
        }
    }

    /// **总决策手册**
    pub fn print_playbook() {
        println!("## 泛化-6：Type Coercion 决策手册");
        println!("  ┌────────────────────┬──────────────────────────────────────────┐");
        println!("  │ 场景               │ 推荐做法                                  │");
        println!("  ├────────────────────┼──────────────────────────────────────────┤");
        println!("  │ 价格/金额/wei      │ 固定点整数 / U256；禁 f64 + as            │");
        println!("  │ 协议枚举/状态码    │ TryFrom + match；禁 transmute/as enum     │");
        println!("  │ 字符串参数         │ 冷路径 AsRef<str>；热路径 &str              │");
        println!("  │ 批量消息/订单      │ [T;N] 栈数组 unsizing → &[T]              │");
        println!("  │ 插件/RPC/Signer    │ Box<dyn Trait> unsize；热路径避免 dyn      │");
        println!("  │ 整数收窄           │ TryFrom；禁 as 到更小类型                   │");
        println!("  │ 整数拓宽           │ From / u64::from(u32)；安全可隐式           │");
        println!("  │ FFI 回调           │ fn item → fn pointer；闭包需 trampolines   │");
        println!("  │ 只读 peek          │ &mut T 可 reborrow 为 &T                   │");
        println!("  └────────────────────┴──────────────────────────────────────────┘");
        println!();
        println!("  HFT 典型事故链：f64 报价 → as i64 → 错价 → 风控滞后 → 敞口");
        println!("  Web3 典型事故链：f64 ETH → as u128 → 差 1 wei → 模拟≠链上 → 亏 gas");
        println!("  共同原则：coercion 只用于「无损、编译器证明安全」；其余一律显式 TryFrom");
        println!("  口诀：边界 parse，内核 strong type，热路径 slice + Copy，钱禁 float");
    }

    pub fn run_all() {
        coercion_vs_cast::demonstrate();
        api_wide_narrow::demonstrate();
        when_unsize::demonstrate();
        boundary_pattern::demonstrate();
        deref_traps::demonstrate();
        print_playbook();
    }
}

fn main() {
    println!("=== Rust Type Coercion：HFT / Web3 生产场景 ===\n");

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
    fn hft_fixed_point_rejects_f64_precision() {
        let good =
            hft::fixed_point_price::TickPrice::from_decimal_str("9007199254740993", 0).unwrap();
        let bad = hft::fixed_point_price::TickPrice::from_f64_lossy(9_007_199_254_740_993.0, 0);
        assert_ne!(good.ticks, bad.ticks);
    }

    #[test]
    fn hft_side_try_from_rejects_invalid() {
        assert!(hft::wire_enum_boundary::parse_side_safe(b'9').is_err());
    }

    #[test]
    fn web3_address_length_check() {
        assert!(web3::address_boundary::Address::try_from("0x01").is_err());
    }

    #[test]
    fn web3_wei_no_f64() {
        let precise = web3::wei_integer::Wei::from_eth_decimal("1.000000000000000001").unwrap();
        let lossy = web3::wei_integer::Wei::from_eth_f64_lossy(1.000000000000000001);
        assert_ne!(precise, lossy);
    }

    #[test]
    fn web3_u256_to_u64_fails_on_overflow() {
        use web3::token_amount_widen::U256;
        assert!(
            web3::token_amount_widen::sync_from_chain(U256::from_limbs([0, 1, 0, 0])).is_err()
        );
    }
}
