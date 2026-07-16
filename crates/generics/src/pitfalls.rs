//! # 泛型常见陷阱与诊断
//!
//! 这一章把生产事故里反复出现的 8 个泛型陷阱解剖清楚：
//! - 现象（用户在监控/编译器里看到什么）
//! - 根因（类型系统层面发生了什么）
//! - 解决方案（修法 + 预防）

#![allow(dead_code)]

// ============================================================================
// 陷阱 1：过度泛型（YAGNI）—— 可读性与编译时间双输
// ============================================================================
/// **现象**：新人读函数签名要 3 分钟；改一行触发全 workspace 重编 5 分钟。
/// **根因**：把尚只有 1 个实例的代码抽成 `fn foo<T, U, V, F, G>(...)`。
/// **修法**：Rule of Three —— 第三份重复再抽象；否则 concrete type 直写。
pub mod over_generic {
    // ❌ 过早抽象
    pub fn transform<T, U, F>(input: T, f: F) -> U
    where
        F: FnOnce(T) -> U,
    {
        f(input)
    }

    // ✅ 当前只有 u64 → String 一种用法
    pub fn price_tag(px: u64) -> String {
        format!("px={}", px)
    }

    pub fn demonstrate() {
        println!("## 陷阱 1：过度泛型");

        let tag = price_tag(100_050);
        println!("concrete = {}", tag);
        println!("`transform` 不是错，但在只有 1 种用法时是噪音");
        println!("Rule of Three：三处重复再泛型\n");
    }
}

// ============================================================================
// 陷阱 2：该用关联类型却用泛型参数 —— 约束爆炸
// ============================================================================
/// **现象**：`impl Encoder<Output=[u8;8]> for X` 和 `impl Encoder<Output=&[u8]> for X`
/// 想给同一类型两种 Output —— 编译器拒绝（冲突）。
/// **根因**：关联类型表达 1:1，泛型参数表达 1:N。
/// **修法**：Output 用关联类型；只有确实需要 1:N 时才用泛型参数。
pub mod assoc_vs_type_param {
    pub trait Encoder {
        type Output: AsRef<[u8]>;
        fn encode(&self) -> Self::Output;
    }

    pub struct Tag(&'static str);

    impl Encoder for Tag {
        type Output = &'static [u8];
        fn encode(&self) -> Self::Output {
            self.0.as_bytes()
        }
    }

    pub fn demonstrate() {
        println!("## 陷阱 2：关联类型 vs 泛型参数");

        let t = Tag("swap");
        println!("encoded = {:?}", t.encode().as_ref());
        println!("Iterator::Item 是关联类型教科书；不要写成 Iterator<Item=T>\n");
    }
}

// ============================================================================
// 陷阱 3：热路径滥用 dyn Trait —— 隐式动态分发
// ============================================================================
/// **现象**：P99 延迟突然多 20–50ns；perf 里看到 indirect call。
/// **根因**：`Box<dyn Strategy>` 每次 `on_tick` vtable 跳转 + 难内联。
/// **修法**：框架用泛型 `Engine<S: Strategy>`；只有插件边界才 `dyn`。
pub mod dyn_on_hot_path {
    pub trait Strategy {
        fn signal(&self) -> i32;
    }

    pub struct Scalper;
    impl Strategy for Scalper {
        fn signal(&self) -> i32 {
            1
        }
    }

    // ❌ 热路径
    pub fn run_dyn(s: &dyn Strategy) -> i32 {
        s.signal()
    }

    // ✅ 静态分发
    pub fn run_static<S: Strategy>(s: &S) -> i32 {
        s.signal()
    }

    pub fn demonstrate() {
        println!("## 陷阱 3：热路径 dyn vs 泛型");

        let sc = Scalper;
        println!("dyn = {}, static = {}", run_dyn(&sc), run_static(&sc));
        println!("HFT/VM 核心循环：泛型；配置加载/测试 mock：dyn\n");
    }
}

// ============================================================================
// 陷阱 4：生命周期 + 泛型交织 —— 签名看不懂
// ============================================================================
/// **现象**：`fn decode<'a, T: ...>(buf: &'a [u8]) -> T` 编译不过或 HRTB 报错。
/// **根因**：返回引用与 `T` 的生命周期关系未写清。
/// **修法**：返回引用则 `T: 'a` 或直接用关联类型 `type View<'a>`。
pub mod lifetime_generic {
    pub trait Parser<'a> {
        type Out;
        fn parse(buf: &'a [u8]) -> Option<Self::Out>;
    }

    pub struct LineParser;

    impl<'a> Parser<'a> for LineParser {
        type Out = &'a str;

        fn parse(buf: &'a [u8]) -> Option<Self::Out> {
            std::str::from_utf8(buf).ok()
        }
    }

    pub fn demonstrate() {
        println!("## 陷阱 4：生命周期 + 泛型");

        let raw = b"hello";
        let s = LineParser::parse(raw).unwrap();
        println!("parsed = {}", s);
        println!("GAT `trait Parser<'a>` 或 HRTB 可进一步简化复杂签名\n");
    }
}

// ============================================================================
// 陷阱 5：Turbofish 滥用 —— 类型推断本可完成
// ============================================================================
/// **现象**：满屏 `::<Vec<u64>, _>`，代码像 C++ 模板。
/// **根因**：中间变量切断推断链。
/// **修法**：让终端操作或参数类型锚定 `T`。
pub mod turbofish_noise {
    pub fn double<T: Copy + std::ops::Mul<Output = T>>(x: T) -> T {
        x * x
    }

    pub fn demonstrate() {
        println!("## 陷阱 5：Turbofish 噪音");

        // ❌ 不必要
        let a = double::<i32>(3);

        // ✅ 推断即可
        let b = double(3i32);

        println!("a={}, b={}", a, b);
        println!("只在 `collect()` / `parse()` 推断失败时写 turbofish\n");
    }
}

// ============================================================================
// 陷阱 6：const 泛型与运行期配置混淆
// ============================================================================
/// **现象**：想 runtime 改 buffer 大小，却写成 `RingBuf<T, N>` 后被迫改类型。
/// **根因**：const 泛型是编译期常量，不是运行期参数。
/// **修法**：部署期固定 → const 泛型；用户可调 → 普通字段 + `with_capacity`。
pub mod const_vs_runtime {
    pub struct RuntimeBuf<T> {
        data: Vec<T>,
        cap: usize,
    }

    impl<T> RuntimeBuf<T> {
        pub fn with_capacity(cap: usize) -> Self {
            Self {
                data: Vec::with_capacity(cap),
                cap,
            }
        }

        pub fn capacity(&self) -> usize {
            self.cap
        }
    }

    pub fn demonstrate() {
        println!("## 陷阱 6：const 泛型 ≠ 运行期配置");

        let rb = RuntimeBuf::<u8>::with_capacity(1024);
        println!("runtime cap = {}", rb.capacity());
        println!("HFT 部署文件写死 CAP=1024 → const；SaaS 租户自定义 → Vec\n");
    }
}

// ============================================================================
// 陷阱 7：孤儿规则阻塞 blanket impl
// ============================================================================
/// **现象**：想 `impl<T> Display for Wrapper<T>` 但编译器报 orphan rule。
/// **根因**：trait 和类型至少有一个要在当前 crate 定义。
/// **修法**：newtype 包装本地类型，或 trait 定义在本 crate。
pub mod orphan_rule {
    pub struct LocalId(u64);

    impl std::fmt::Display for LocalId {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "id:{}", self.0)
        }
    }

    pub fn demonstrate() {
        println!("## 陷阱 7：孤儿规则");

        println!("LocalId = {}", LocalId(42));
        println!("不能给外部 `Vec<T>` impl 外部 `Display`；用 newtype 或 extension trait\n");
    }
}

// ============================================================================
// 陷阱 8：单态化代码膨胀（binary bloat）
// ============================================================================
/// **现象**：release binary 从 2MB 涨到 20MB；iOS/嵌入式 OOM link。
/// **根因**：对 50 种 `T` 各生成一份完整机器码。
/// **修法**：热路径保留泛型；冷路径合并为 `dyn` 或 type erasure；`-Zprint-type-sizes` 审计。
pub mod monomorph_bloat {
    pub fn serialize_u64(v: u64) -> [u8; 8] {
        v.to_le_bytes()
    }

    pub fn serialize_u32(v: u32) -> [u8; 4] {
        v.to_le_bytes()
    }

    // 冷路径统一入口
    pub enum Scalar {
        U32(u32),
        U64(u64),
    }

    pub fn serialize_cold(s: Scalar) -> Vec<u8> {
        match s {
            Scalar::U32(v) => v.to_le_bytes().to_vec(),
            Scalar::U64(v) => v.to_le_bytes().to_vec(),
        }
    }

    pub fn demonstrate() {
        println!("## 陷阱 8：单态化代码膨胀");

        let hot = serialize_u64(1);
        let cold = serialize_cold(Scalar::U64(1));
        println!("hot {:?}, cold len {}", hot, cold.len());
        println!("50 种消息类型 × 10 种编码器 = 500 份机器码 → 冷路径 type erasure\n");
    }
}

pub fn demonstrate() {
    over_generic::demonstrate();
    assoc_vs_type_param::demonstrate();
    dyn_on_hot_path::demonstrate();
    lifetime_generic::demonstrate();
    turbofish_noise::demonstrate();
    const_vs_runtime::demonstrate();
    orphan_rule::demonstrate();
    monomorph_bloat::demonstrate();
}
