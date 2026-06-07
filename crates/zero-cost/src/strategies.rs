//! # 泛化：从 HFT/Web3 场景到通用零成本策略
//!
//! 把前两章具体业务里的套路抽象成决策矩阵：
//!
//! | 问题类型           | 标志特征                  | 首选套路                          |
//! |--------------------|---------------------------|-----------------------------------|
//! | 1. 热路径多态      | 每 tick/tx 调用           | 泛型 + 静态分派                   |
//! | 2. 域类型安全      | 价/量/地址混用风险        | newtype + trait impl              |
//! | 3. 固定布局 I/O    | 二进制协议 / ABI          | `#[inline]` + `from_*_bytes`      |
//! | 4. 编译期常量      | 缓冲容量 / 精度已知       | const 泛型 + const fn             |
//! | 5. 算法特化        | 同一逻辑多种规则          | type parameter 选实现             |
//! | 6. 流水线组合      | 多阶段变换                | 泛型 struct 链 / 宏展开           |
//! | 7. 冷路径插件      | 运行时加载扩展            | `dyn Trait` 边界隔离              |
//! | 8. 验证零成本      | 怀疑抽象有税              | perf / asm / criterion 三件套     |
//!
//! 下面 8 个策略各有一个通用模板，签名不带业务名词。

#![allow(dead_code)]

// ============================================================================
// 策略 1：热路径静态分派
// ============================================================================
/// 问题：高频调用点需要多实现，但不能 vtable。
/// 模式：`fn run<T: Trait>(t: &mut T)` 单态化。
///
/// HFT: hft::static_strategy
/// Web3: web3::evm_static
pub mod static_dispatch {
    pub trait Worker {
        fn work(&mut self, n: i64) -> i64;
    }

    pub struct Double;
    impl Worker for Double {
        #[inline]
        fn work(&mut self, n: i64) -> i64 {
            n * 2
        }
    }

    pub fn batch<W: Worker>(w: &mut W, input: &[i64]) -> i64 {
        input.iter().map(|&x| w.work(x)).sum()
    }

    pub fn demonstrate() {
        println!("## 策略 1：热路径静态分派");
        let mut w = Double;
        println!("batch = {}\n", batch(&mut w, &[1, 2, 3]));
    }
}

// ============================================================================
// 策略 2：newtype 域建模
// ============================================================================
/// 问题：底层都是整数/字节，但业务语义不同。
/// 模式：`struct UserId(u64)` —— 编译期防混用，运行时零包装。
///
/// HFT: hft::fixed_point_px
/// Web3: web3::u256_newtype, web3::address_newtype
pub mod newtype_domain {
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct UserId(u64);

    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct OrderId(u64);

    pub fn route(user: UserId, _order: OrderId) -> UserId {
        user
    }

    pub fn demonstrate() {
        println!("## 策略 2：newtype 域建模");
        let u = UserId(1);
        let o = OrderId(99);
        let _ = route(u, o);
        // route(o, u)  // 编译错误：类型不匹配
        println!("UserId / OrderId 不可互换\n");
    }
}

// ============================================================================
// 策略 3：内联固定布局 decode
// ============================================================================
/// 问题：二进制记录解析在热路径。
/// 模式：固定长度 + `#[inline]` + 栈 struct，错误用 `Option`。
///
/// HFT: hft::inline_decode
/// Web3: web3::abi_generic（encode 侧同理）
pub mod inline_decode {
    #[derive(Clone, Copy, Debug)]
    pub struct Record {
        pub id: u32,
        pub flags: u16,
    }

    #[inline]
    pub fn decode(buf: &[u8; 6]) -> Option<Record> {
        Some(Record {
            id: u32::from_le_bytes(buf[0..4].try_into().ok()?),
            flags: u16::from_le_bytes(buf[4..6].try_into().ok()?),
        })
    }

    pub fn demonstrate() {
        println!("## 策略 3：内联固定布局 decode");
        let buf = [1, 0, 0, 0, 0xFF, 0x00];
        println!("{:?}\n", decode(&buf));
    }
}

// ============================================================================
// 策略 4：const 泛型固定容量
// ============================================================================
/// 问题：缓冲/窗口大小部署时已知，希望无 runtime bounds check 优化空间。
/// 模式：`struct Buf<T, const N: usize>([T; N])`。
///
/// HFT: hft::const_ring
/// Web3: 固定大小 topic bloom filter
pub mod const_generic {
    pub struct FixedStack<T, const N: usize> {
        data: [T; N],
        len: usize,
    }

    impl<T: Copy + Default, const N: usize> FixedStack<T, N> {
        pub fn new() -> Self {
            Self {
                data: [T::default(); N],
                len: 0,
            }
        }

        pub fn push(&mut self, v: T) -> bool {
            if self.len >= N {
                return false;
            }
            self.data[self.len] = v;
            self.len += 1;
            true
        }

        pub fn as_slice(&self) -> &[T] {
            &self.data[..self.len]
        }
    }

    pub fn demonstrate() {
        println!("## 策略 4：const 泛型 FixedStack<N=8>");
        let mut s = FixedStack::<i32, 8>::new();
        s.push(42);
        println!("slice = {:?}\n", s.as_slice());
    }
}

// ============================================================================
// 策略 5：type parameter 选算法变体
// ============================================================================
/// 问题：同一接口，多种 endian / hash / fork 规则。
/// 模式：`fn process<R: Rule>()` 替代 runtime flag。
///
/// HFT: hft::type_level_feed, hft::orderbook_side
/// Web3: web3::merkle_generic
pub mod type_param_rule {
    pub trait Endian {
        fn read_u32(buf: &[u8; 4]) -> u32;
    }

    pub struct Le;
    impl Endian for Le {
        fn read_u32(buf: &[u8; 4]) -> u32 {
            u32::from_le_bytes(*buf)
        }
    }

    pub struct Be;
    impl Endian for Be {
        fn read_u32(buf: &[u8; 4]) -> u32 {
            u32::from_be_bytes(*buf)
        }
    }

    pub fn read<E: Endian>(buf: &[u8; 4]) -> u32 {
        E::read_u32(buf)
    }

    pub fn demonstrate() {
        println!("## 策略 5：type parameter 选规则");
        let b = [0, 0, 0, 42];
        println!("LE = {}, BE = {}\n", read::<Le>(&b), read::<Be>(&b));
    }
}

// ============================================================================
// 策略 6：编译期 pipeline 组合
// ============================================================================
/// 问题：多阶段 map/filter，runtime 插件链太慢。
/// 模式：嵌套泛型 struct 或 tuple chain。
///
/// HFT: hft::handler_chain
/// Web3: indexer middleware 链
pub mod pipeline_compose {
    pub trait Stage {
        fn run(&mut self, x: i64) -> i64;
    }

    pub struct AddOne;
    impl Stage for AddOne {
        fn run(&mut self, x: i64) -> i64 {
            x + 1
        }
    }

    pub struct MulTwo;
    impl Stage for MulTwo {
        fn run(&mut self, x: i64) -> i64 {
            x * 2
        }
    }

    pub struct Chain<A, B> {
        pub a: A,
        pub b: B,
    }

    impl<A: Stage, B: Stage> Chain<A, B> {
        pub fn apply(&mut self, x: i64) -> i64 {
            self.b.run(self.a.run(x))
        }
    }

    pub fn demonstrate() {
        println!("## 策略 6：编译期 pipeline");
        let mut c = Chain { a: AddOne, b: MulTwo };
        println!("(3+1)*2 = {}\n", c.apply(3));
    }
}

// ============================================================================
// 策略 7：dyn 隔离在边界
// ============================================================================
/// 问题：确实需要运行时插件，但不能让 dyn 泄漏进内核。
/// 模式：启动时 resolve 成 enum 或具体类型；或 `OnceLock` 初始化后不再变。
///
/// HFT: 券商 adapter 加载后固定
/// Web3: precompile 注册表
pub mod dyn_at_boundary {
    pub trait Plugin {
        fn name(&self) -> &'static str;
    }

    pub struct PluginA;
    impl Plugin for PluginA {
        fn name(&self) -> &'static str {
            "A"
        }
    }

    pub enum Resolved {
        A(PluginA),
    }

    pub fn load(raw: &str) -> Option<Resolved> {
        match raw {
            "A" => Some(Resolved::A(PluginA)),
            _ => None,
        }
    }

    pub fn run(p: &Resolved) -> &'static str {
        match p {
            Resolved::A(a) => a.name(),
        }
    }

    pub fn demonstrate() {
        println!("## 策略 7：dyn 隔离在边界");
        let p = load("A").unwrap();
        println!("plugin = {}\n", run(&p));
    }
}

// ============================================================================
// 策略 8：测量验证零成本假设
// ============================================================================
/// 问题：「应该零成本」不等于「已经零成本」。
/// 模式：手写 loop vs 抽象版 benchmark + `cargo asm` 对比。
///
/// HFT/Web3: 任何 P99 回归都应走此流程
pub mod verify_zero_cost {
    pub fn manual_sum(data: &[i32]) -> i64 {
        let mut s = 0i64;
        for &x in data {
            if x > 0 {
                s += x as i64;
            }
        }
        s
    }

    pub fn iter_sum(data: &[i32]) -> i64 {
        data.iter().filter(|&&x| x > 0).map(|&x| x as i64).sum()
    }

    pub fn demonstrate() {
        println!("## 策略 8：验证零成本假设");
        let d = [1, -2, 3, 4];
        assert_eq!(manual_sum(&d), iter_sum(&d));
        println!("manual == iter == {}", manual_sum(&d));
        println!("工具链：");
        println!("  cargo bench -p zero-cost");
        println!("  cargo asm -p zero-cost --release");
        println!("  perf record -g ./target/release/zero-cost\n");
    }
}

// ============================================================================
// 反例：什么时候主动接受 runtime 成本
// ============================================================================
pub mod when_to_pay_cost {
    pub fn demonstrate() {
        println!("## 反例：何时主动付 runtime 税");
        println!("  - 用户上传的 WASM/JS 插件 → 必须 dyn");
        println!("  - 配置组合爆炸（>50 变体）→ dyn 或 interpreter 控体积");
        println!("  - 一次性冷启动（读 config）→ String/HashMap OK");
        println!("  - 快速原型 / 非热路径 admin API → 可读性优先");
        println!("  - 手写前先问：这条路径 QPS 多少？P99 预算多少？\n");
    }
}

pub fn demonstrate() {
    static_dispatch::demonstrate();
    newtype_domain::demonstrate();
    inline_decode::demonstrate();
    const_generic::demonstrate();
    type_param_rule::demonstrate();
    pipeline_compose::demonstrate();
    dyn_at_boundary::demonstrate();
    verify_zero_cost::demonstrate();
    when_to_pay_cost::demonstrate();
}
