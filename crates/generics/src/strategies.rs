//! # 泛化：从 HFT/Web3 场景到通用应对策略
//!
//! 把前两章具体业务里的泛型套路抽象出来，得到一张
//! **「问题类型 → 推荐套路」决策矩阵**：
//!
//! | 问题类型           | 标志特征                  | 首选套路                          |
//! |--------------------|---------------------------|-----------------------------------|
//! | 1. 单位/品牌安全   | 同表示不同语义            | PhantomData 品牌参数              |
//! | 2. 编译期常量配置  | 容量、深度、对齐已知      | const 泛型                        |
//! | 3. 热路径多态      | 策略/ISA 固定于部署       | 泛型参数 + trait bound 静态分发   |
//! | 4. 可组合校验链    | 规则顺序 AND/OR           | trait + 元组/链式 impl            |
//! | 5. 协议/编解码族   | 布局不同、流程相同        | 关联类型 Spec::Output             |
//! | 6. 多租户/多链参数 | 行为差在常量与解析        | 空枚举类型参数 ChainSpec          |
//! | 7. 运行时插件      | 类型部署后才确定          | dyn Trait / type erasure          |
//! | 8. 抑制代码膨胀    | N×M 单态化爆炸            | 冷路径合并 + 热路径保留泛型       |
//!
//! 下面 8 个策略各有一个 *通用模板*，签名上不带业务名词。

#![allow(dead_code)]

// ============================================================================
// 策略 1：PhantomData 品牌 —— 同底层不同语义
// ============================================================================
/// HFT: 见 hft::unit_branding（Price<Venue>）
/// Web3: Address vs Hash32 虽都是 [u8; N] 但语义不同
pub mod phantom_branding {
    use std::marker::PhantomData;

    pub struct Tagged<T, Tag> {
        value: T,
        _tag: PhantomData<Tag>,
    }

    impl<T, Tag> Tagged<T, Tag> {
        pub fn new(value: T) -> Self {
            Self {
                value,
                _tag: PhantomData,
            }
        }

        pub fn inner(&self) -> &T {
            &self.value
        }
    }

    pub struct Meters;
    pub struct Feet;

    pub fn add_same_unit<T: Copy + std::ops::Add<Output = T>, Tag>(
        a: Tagged<T, Tag>,
        b: Tagged<T, Tag>,
    ) -> Tagged<T, Tag> {
        Tagged::new(a.value + b.value)
    }

    pub fn demonstrate() {
        println!("## 策略 1：PhantomData 品牌");
        let a = Tagged::<f64, Meters>::new(10.0);
        let b = Tagged::<f64, Meters>::new(5.0);
        let c = add_same_unit(a, b);
        println!("10m + 5m = {}m", c.inner());
        println!();
    }
}

// ============================================================================
// 策略 2：const 泛型 —— 编译期已知的数值
// ============================================================================
/// HFT: 见 hft::spsc_ring、latency_histogram
/// Web3: Merkle depth、Bloom 位数
pub mod const_generic_config {
    pub struct StackBuffer<T, const N: usize> {
        data: [Option<T>; N],
        len: usize,
    }

    impl<T, const N: usize> StackBuffer<T, N> {
        pub fn new() -> Self {
            Self {
                data: std::array::from_fn(|_| None),
                len: 0,
            }
        }

        pub fn push(&mut self, v: T) -> bool {
            if self.len >= N {
                return false;
            }
            self.data[self.len] = Some(v);
            self.len += 1;
            true
        }

        pub fn len(&self) -> usize {
            self.len
        }
    }

    pub fn demonstrate() {
        println!("## 策略 2：const 泛型定长容器");
        let mut b: StackBuffer<u8, 2> = StackBuffer::new();
        assert!(b.push(1));
        assert!(b.push(2));
        assert!(!b.push(3));
        println!("len = {}, 第三次 push 失败\n", b.len());
    }
}

// ============================================================================
// 策略 3：静态分发框架 —— Engine<S: Trait>
// ============================================================================
/// HFT: 见 hft::strategy_plugin
/// Web3: 见 web3::vm_dispatch
pub mod static_dispatch_framework {
    pub trait Worker {
        fn work(&mut self, n: u32) -> u32;
    }

    pub struct Double;
    impl Worker for Double {
        fn work(&mut self, n: u32) -> u32 {
            n * 2
        }
    }

    pub struct Runner<W: Worker> {
        worker: W,
    }

    impl<W: Worker> Runner<W> {
        pub fn new(worker: W) -> Self {
            Self { worker }
        }

        pub fn run(&mut self, inputs: &[u32]) -> u32 {
            inputs.iter().map(|&n| self.worker.work(n)).sum()
        }
    }

    pub fn demonstrate() {
        println!("## 策略 3：泛型 Runner 静态分发");
        let mut r = Runner::new(Double);
        println!("sum = {}", r.run(&[1, 2, 3]));
        println!();
    }
}

// ============================================================================
// 策略 4：可组合校验链 —— trait + 元组
// ============================================================================
/// HFT: 见 hft::risk_pipeline
/// Web3: 见 web3::mempool_filter
pub mod composable_checks {
    pub trait Validate<C> {
        fn check(&self, ctx: &C) -> Result<(), &'static str>;
    }

    impl<C, A, B> Validate<C> for (A, B)
    where
        A: Validate<C>,
        B: Validate<C>,
    {
        fn check(&self, ctx: &C) -> Result<(), &'static str> {
            self.0.check(ctx)?;
            self.1.check(ctx)
        }
    }

    pub struct Min(pub i32);
    pub struct Max(pub i32);

    impl Validate<i32> for Min {
        fn check(&self, ctx: &i32) -> Result<(), &'static str> {
            if *ctx >= self.0 {
                Ok(())
            } else {
                Err("below min")
            }
        }
    }

    impl Validate<i32> for Max {
        fn check(&self, ctx: &i32) -> Result<(), &'static str> {
            if *ctx <= self.0 {
                Ok(())
            } else {
                Err("above max")
            }
        }
    }

    pub fn demonstrate() {
        println!("## 策略 4：元组组合校验链");
        let chain = (Min(0), Max(100));
        println!("50 → {:?}", chain.check(&50));
        println!("200 → {:?}", chain.check(&200));
        println!();
    }
}

// ============================================================================
// 策略 5：关联类型 Spec —— 编解码/解析族
// ============================================================================
/// HFT: 见 hft::zero_copy_decoder
/// Web3: 见 web3::abi_codec
pub mod spec_associated_type {
    pub trait Codec {
        type Value;
        fn decode(bytes: &[u8]) -> Option<Self::Value>;
    }

    pub struct Utf8Codec;
    impl Codec for Utf8Codec {
        type Value = String;

        fn decode(bytes: &[u8]) -> Option<Self::Value> {
            std::str::from_utf8(bytes).ok().map(|s| s.to_string())
        }
    }

    pub struct LeU64Codec;
    impl Codec for LeU64Codec {
        type Value = u64;

        fn decode(bytes: &[u8]) -> Option<Self::Value> {
            let arr: [u8; 8] = bytes.try_into().ok()?;
            Some(u64::from_le_bytes(arr))
        }
    }

    pub fn decode_with<C: Codec>(bytes: &[u8]) -> Option<C::Value> {
        C::decode(bytes)
    }

    pub fn demonstrate() {
        println!("## 策略 5：关联类型 Codec");
        let s = decode_with::<Utf8Codec>(b"hi").unwrap();
        let n = decode_with::<LeU64Codec>(&1u64.to_le_bytes()).unwrap();
        println!("utf8 = {}, le u64 = {}", s, n);
        println!();
    }
}

// ============================================================================
// 策略 6：类型级常量参数 —— ChainSpec / Config marker
// ============================================================================
/// Web3: 见 web3::rpc_adapter
/// 通用：Feature flag、Region、Tenant tier 用空类型承载
pub mod type_level_constants {
    pub trait Env {
        const REGION: &'static str;
        const TIMEOUT_MS: u64;
    }

    pub struct Prod;
    pub struct Staging;

    impl Env for Prod {
        const REGION: &'static str = "ap-sg-1";
        const TIMEOUT_MS: u64 = 500;
    }

    impl Env for Staging {
        const REGION: &'static str = "ap-sg-stg";
        const TIMEOUT_MS: u64 = 5_000;
    }

    pub struct Client<E> {
        _e: std::marker::PhantomData<E>,
    }

    impl<E: Env> Client<E> {
        pub fn config_summary(&self) -> (&'static str, u64) {
            (E::REGION, E::TIMEOUT_MS)
        }
    }

    pub fn demonstrate() {
        println!("## 策略 6：类型级常量 Env");
        let prod = Client::<Prod> { _e: std::marker::PhantomData };
        let stg = Client::<Staging> { _e: std::marker::PhantomData };
        println!("prod {:?}, staging {:?}", prod.config_summary(), stg.config_summary());
        println!();
    }
}

// ============================================================================
// 策略 7：运行时插件 —— dyn Trait 边界
// ============================================================================
/// 当类型部署后才确定、或需要动态加载 .so 时，泛型让位于 trait object。
pub mod runtime_plugin {
    pub trait Plugin {
        fn name(&self) -> &'static str;
        fn run(&self, input: &str) -> String;
    }

    pub struct Uppercase;
    impl Plugin for Uppercase {
        fn name(&self) -> &'static str {
            "uppercase"
        }

        fn run(&self, input: &str) -> String {
            input.to_uppercase()
        }
    }

    pub struct Registry {
        plugins: Vec<Box<dyn Plugin>>,
    }

    impl Registry {
        pub fn new() -> Self {
            Self {
                plugins: Vec::new(),
            }
        }

        pub fn register(&mut self, p: Box<dyn Plugin>) {
            self.plugins.push(p);
        }

        pub fn run_all(&self, input: &str) -> Vec<String> {
            self.plugins.iter().map(|p| p.run(input)).collect()
        }
    }

    pub fn demonstrate() {
        println!("## 策略 7：dyn Plugin 运行时扩展");
        let mut reg = Registry::new();
        reg.register(Box::new(Uppercase));
        println!("{:?}", reg.run_all("hello"));
        println!("配置加载、FFI 插件、测试 mock → dyn；撮合核心 → 泛型\n");
    }
}

// ============================================================================
// 策略 8：抑制单态化膨胀 —— 热/冷路径分离
// ============================================================================
pub mod control_monomorph_bloat {
    pub trait Serialize {
        fn write(&self, buf: &mut Vec<u8>);
    }

    pub struct I32(pub i32);
    impl Serialize for I32 {
        fn write(&self, buf: &mut Vec<u8>) {
            buf.extend_from_slice(&self.0.to_le_bytes());
        }
    }

    /// 热路径：泛型，可内联
    pub fn write_one<S: Serialize>(s: &S, buf: &mut Vec<u8>) {
        s.write(buf);
    }

    /// 冷路径：type erasure，一份机器码
    pub enum DynScalar {
        I32(i32),
        Str(String),
    }

    impl DynScalar {
        pub fn write(&self, buf: &mut Vec<u8>) {
            match self {
                DynScalar::I32(v) => buf.extend_from_slice(&v.to_le_bytes()),
                DynScalar::Str(s) => buf.extend_from_slice(s.as_bytes()),
            }
        }
    }

    pub fn demonstrate() {
        println!("## 策略 8：热路径泛型 + 冷路径 erasure");
        let mut hot = Vec::new();
        write_one(&I32(42), &mut hot);
        let mut cold = Vec::new();
        DynScalar::I32(42).write(&mut cold);
        println!("hot {:?}, cold {:?}", hot, cold);
        println!();
    }
}

// ============================================================================
// 反向：什么时候 *不要* 用泛型
// ============================================================================
pub mod when_not_to_use {
    pub fn demonstrate() {
        println!("## 反例：什么时候不要用泛型");
        println!("  - 只有 1 种具体类型 → 直接写死类型");
        println!("  - 需要运行时换实现 → dyn Trait / enum");
        println!("  - 用户可调容量/深度 → 普通字段，非 const 泛型");
        println!("  - 公开 API 稳定优先 → 少暴露类型参数，用 trait object 或 enum");
        println!("  - binary 体积敏感 → 审计单态化份数，冷路径合并\n");
    }
}

pub fn demonstrate() {
    phantom_branding::demonstrate();
    const_generic_config::demonstrate();
    static_dispatch_framework::demonstrate();
    composable_checks::demonstrate();
    spec_associated_type::demonstrate();
    type_level_constants::demonstrate();
    runtime_plugin::demonstrate();
    control_monomorph_bloat::demonstrate();
    when_not_to_use::demonstrate();
}
