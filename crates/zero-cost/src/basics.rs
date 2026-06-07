//! # 零成本抽象底层机制
//!
//! Rust 的「零成本抽象」不是口号，而是编译器契约：
//!
//! 1. **单态化（Monomorphization）**：泛型在编译期展开成具体类型，热路径无 vtable
//! 2. **静态分派**：`impl Trait` / 泛型 bound 在编译期选定实现
//! 3. **动态分派**：`dyn Trait` 有 vtable 间接调用 —— 有成本，但边界清晰
//! 4. **newtype**：包装域类型，运行时与底层整数/数组同布局，零开销
//! 5. **迭代器消除**：适配器链在 release 下常融合成单个循环（loop fusion）

#![allow(dead_code)]

// ============================================================================
// 1. 单态化：一份泛型 → N 份特化机器码
// ============================================================================
pub mod monomorphization {
    /// 泛型函数：编译后为每个 `T` 生成独立版本，调用点直接跳转，无运行时类型信息。
    #[inline]
    pub fn max<T: Copy + PartialOrd>(a: T, b: T) -> T {
        if a >= b {
            a
        } else {
            b
        }
    }

    /// 对比：如果写成 `fn max_f64(a: f64, b: f64)` 和 `fn max_i64(a: i64, b: i64)`
    /// 语义重复；泛型让源码 DRY，机器码仍是一份一份特化。
    pub fn demonstrate() {
        println!("## 1. 单态化 Monomorphization");
        println!("max(i64) = {}", max(100_i64, 99));
        println!("max(u64) = {}", max(100_u64, 99));
        println!("编译后：`max::<i64>` 与 `max::<u64>` 是两个独立符号，各自 inline\n");
    }
}

// ============================================================================
// 2. 静态分派 vs 动态分派
// ============================================================================
pub mod dispatch {
    pub trait Pricer {
        fn mid(&self, bid: i64, ask: i64) -> i64;
    }

    pub struct MidPricer;
    impl Pricer for MidPricer {
        #[inline]
        fn mid(&self, bid: i64, ask: i64) -> i64 {
            (bid + ask) / 2
        }
    }

    /// 静态分派：`P: Pricer` 在编译期确定，调用可 inline 进调用方。
    #[inline]
    pub fn quote_static<P: Pricer>(p: &P, bid: i64, ask: i64) -> i64 {
        p.mid(bid, ask)
    }

    /// 动态分派：vtable 间接调用，适合插件/脚本边界，不适合 tick 热路径。
    pub fn quote_dynamic(p: &dyn Pricer, bid: i64, ask: i64) -> i64 {
        p.mid(bid, ask)
    }

    pub fn demonstrate() {
        println!("## 2. 静态分派 vs 动态分派");
        let pricer = MidPricer;
        println!("static mid(100, 102) = {}", quote_static(&pricer, 100, 102));
        println!("dynamic mid(100, 102) = {}", quote_dynamic(&pricer, 100, 102));
        println!("语义相同；static 可跨 crate inline，dyn 每次经 vtable\n");
    }
}

// ============================================================================
// 3. newtype：域语义 + 同布局零开销
// ============================================================================
pub mod newtype {
    /// 价格 tick：1 USDT = 100_000_000 ticks，禁止与 Qty 混用。
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    pub struct Px(i64);

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Qty(i64);

    impl Px {
        pub fn raw(self) -> i64 {
            self.0
        }
    }

    impl Qty {
        pub fn raw(self) -> i64 {
            self.0
        }
    }

    /// 名义价值 = 价 × 量；类型系统阻止 Px + Qty 误写。
    pub fn notional(px: Px, qty: Qty) -> i128 {
        (px.raw() as i128) * (qty.raw() as i128)
    }

    pub fn demonstrate() {
        println!("## 3. newtype 零开销域类型");
        let px = Px(50_000_000);
        let qty = Qty(10);
        println!("notional = {}", notional(px, qty));
        println!("内存布局：Px 与 i64 相同（可用 transmute 验证，生产勿用）\n");
    }
}

// ============================================================================
// 4. 迭代器融合：抽象层在 release 下消失
// ============================================================================
pub mod iterator_fusion {
    /// 手写循环版本
    pub fn sum_evens_manual(data: &[i32]) -> i64 {
        let mut acc = 0i64;
        for &x in data {
            if x % 2 == 0 {
                acc += x as i64;
            }
        }
        acc
    }

    /// 迭代器链版本 —— release 下 LLVM 通常生成与上面等价的单循环
    pub fn sum_evens_iter(data: &[i32]) -> i64 {
        data.iter()
            .copied()
            .filter(|x| x % 2 == 0)
            .map(|x| x as i64)
            .sum()
    }

    pub fn demonstrate() {
        println!("## 4. 迭代器融合 Iterator Fusion");
        let data = [1, 2, 3, 4, 5, 6];
        let a = sum_evens_manual(&data);
        let b = sum_evens_iter(&data);
        assert_eq!(a, b);
        println!("manual = {}, iter = {}（release 下汇编应一致）", a, b);
        println!("验证：`cargo rustc -p zero-cost --release -- -C llvm-args=--print-after-all`\n");
    }
}

pub fn demonstrate() {
    monomorphization::demonstrate();
    dispatch::demonstrate();
    newtype::demonstrate();
    iterator_fusion::demonstrate();
}
