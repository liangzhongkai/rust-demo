//! # 泛型底层机制
//!
//! 这一节回答四个根本问题，后面 HFT/Web3 章节里所有套路都建立在这之上：
//!
//! 1. 泛型参数在编译期如何被「单态化」？
//! 2. Trait bound 与 where 子句各解决什么问题？
//! 3. 关联类型 vs 泛型参数：什么时候选哪个？
//! 4. const 泛型与 PhantomData 各自承担什么角色？

#![allow(dead_code)]

// ============================================================================
// 1. 单态化：零成本抽象的根基
// ============================================================================
/// Rust 泛型在编译期为每个具体类型生成独立函数/结构体实例。
/// 热路径上与手写 `u64` / `i64` 版本等价，没有 Java 式装箱开销。
pub mod monomorphization {
    /// 对任意可比较类型取最大值 —— 编译后为每个 `T` 各生成一份机器码。
    pub fn max<T: Ord>(a: T, b: T) -> T {
        if a >= b {
            a
        } else {
            b
        }
    }

    pub fn demonstrate() {
        println!("## 1. 单态化：一份源码，多份机器码");

        let p = max(100_000i64, 100_050i64);
        let q = max("bid".to_string(), "ask".to_string());
        println!("max(i64) = {}", p);
        println!("max(String) = {}", q);
        println!("`max::<i64>` 与 `max::<String>` 在汇编层是两套独立实现\n");
    }
}

// ============================================================================
// 2. Trait bound：表达「能力」而非「类型族」
// ============================================================================
/// bound 把「这个 T 必须能做什么」写进签名，调用方传入的类型自动被约束。
pub mod trait_bounds {
    use std::fmt::Display;

    /// 打印并返回原值 —— 要求 T 既能 Debug 又能 Display（演示多 bound）。
    pub fn log_and_return<T: std::fmt::Debug + Display>(v: T) -> T {
        println!("  [log] {} / {:?}", v, v);
        v
    }

    /// where 子句：签名复杂时把约束挪到函数体前，提升可读性。
    pub fn merge_sorted<A, B, C>(a: A, b: B) -> Vec<C>
    where
        A: IntoIterator<Item = C>,
        B: IntoIterator<Item = C>,
        C: Ord,
    {
        let mut out: Vec<C> = a.into_iter().chain(b).collect();
        out.sort();
        out
    }

    pub fn demonstrate() {
        println!("## 2. Trait bound 与 where 子句");

        let _ = log_and_return(42u64);
        let merged = merge_sorted(vec![3, 1], vec![2, 4]);
        println!("merge_sorted = {:?}", merged);
        println!("bound = 编译期契约；违反则编译失败，运行期零检查\n");
    }
}

// ============================================================================
// 3. 关联类型 vs 泛型参数
// ============================================================================
/// - **泛型参数**：一个类型可实现同一 trait 的多种实例（如 `Add<u32>` / `Add<i32>`）
/// - **关联类型**：trait 与实现之间 1:1 绑定（如 `Iterator::Item`）
pub mod associated_types {
    pub trait Encoder {
        type Output: AsRef<[u8]>;
        fn encode(&self) -> Self::Output;
    }

    pub struct U64Le(u64);

    impl Encoder for U64Le {
        type Output = [u8; 8];

        fn encode(&self) -> Self::Output {
            self.0.to_le_bytes()
        }
    }

    pub struct AsciiTag(&'static str);

    impl Encoder for AsciiTag {
        type Output = &'static [u8];

        fn encode(&self) -> Self::Output {
            self.0.as_bytes()
        }
    }

    /// 消费任意 Encoder，只关心「有 Output」而不关心 Output 具体是什么。
    pub fn wire_len<E: Encoder>(e: &E) -> usize {
        e.encode().as_ref().len()
    }

    pub fn demonstrate() {
        println!("## 3. 关联类型：1:1 绑定输出类型");

        let le = U64Le(0xDEAD_BEEF);
        let tag = AsciiTag("FIX.4.2");
        println!("U64Le wire len = {}", wire_len(&le));
        println!("AsciiTag wire len = {}", wire_len(&tag));
        println!("若把 Output 改成泛型参数，同一类型可实现 Encoder<T1> + Encoder<T2>，但 Iterator 式 1:1 语义用关联类型更清晰\n");
    }
}

// ============================================================================
// 4. const 泛型：编译期已知的数值参数
// ============================================================================
/// const 泛型把「容量、深度、对齐」等常量编进类型，避免运行期分支与额外字段。
pub mod const_generics {
    /// 定长环形缓冲 —— `N` 进入类型签名，不同 N 是不同类型。
    pub struct RingBuf<T, const N: usize> {
        data: [Option<T>; N],
        head: usize,
        len: usize,
    }

    impl<T, const N: usize> RingBuf<T, N> {
        pub fn new() -> Self {
            Self {
                data: std::array::from_fn(|_| None),
                head: 0,
                len: 0,
            }
        }

        pub fn push(&mut self, v: T) -> Option<T> {
            let idx = (self.head + self.len) % N;
            let evicted = if self.len == N {
                let old = self.data[self.head].take();
                self.head = (self.head + 1) % N;
                old
            } else {
                self.len += 1;
                None
            };
            self.data[idx] = Some(v);
            evicted
        }

        pub fn len(&self) -> usize {
            self.len
        }
    }

    pub fn demonstrate() {
        println!("## 4. const 泛型：容量编进类型");

        let mut rb: RingBuf<u64, 4> = RingBuf::new();
        for i in 1..=6 {
            let evicted = rb.push(i);
            println!("  push {} → evicted {:?}", i, evicted);
        }
        println!("RingBuf<u64, 4> 与 RingBuf<u64, 1024> 是不同类型；栈上数组无堆分配\n");
    }
}

// ============================================================================
// 5. PhantomData：零大小字段承载类型/生命周期信息
// ============================================================================
pub mod phantom_data {
    use std::marker::PhantomData;

    /// 价格单位品牌：同样 i64，不同 Phantom 标记不可混用。
    pub struct Price<Unit> {
        ticks: i64,
        _unit: PhantomData<Unit>,
    }

    pub struct UsdCents;
    pub struct Bps; // basis points

    impl<Unit> Price<Unit> {
        pub fn new(ticks: i64) -> Self {
            Self {
                ticks,
                _unit: PhantomData,
            }
        }

        pub fn ticks(&self) -> i64 {
            self.ticks
        }
    }

    /// 只有同 Unit 的价格才能相加 —— 编译期防止 USD 与 bps 混算。
    pub fn add_same_unit<Unit>(a: Price<Unit>, b: Price<Unit>) -> Price<Unit> {
        Price::new(a.ticks() + b.ticks())
    }

    pub fn demonstrate() {
        println!("## 5. PhantomData：零成本单位安全");

        let bid = Price::<UsdCents>::new(100_05);
        let ask = Price::<UsdCents>::new(100_10);
        let mid = add_same_unit(bid, ask);
        println!("mid ticks = {}（两价相加后 /2 可在外层做）", mid.ticks());
        // let bad = add_same_unit(Price::<UsdCents>::new(1), Price::<Bps>::new(1));
        // ^^ 编译错误：Unit 不一致
        println!("PhantomData 不占内存，只参与类型检查\n");
    }
}

pub fn demonstrate() {
    monomorphization::demonstrate();
    trait_bounds::demonstrate();
    associated_types::demonstrate();
    const_generics::demonstrate();
    phantom_data::demonstrate();
}
