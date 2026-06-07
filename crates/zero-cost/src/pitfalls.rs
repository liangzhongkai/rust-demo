//! # 零成本抽象常见陷阱与诊断
//!
//! 生产事故里反复出现的 8 个「以为零成本、实际有税」的坑：
//! - 现象（perf / 延迟里看到什么）
//! - 根因（编译器无法消除什么）
//! - 修法（一行改法 + 风格预防）

#![allow(dead_code)]

// ============================================================================
// 陷阱 1：热路径 Box<dyn Trait>
// ============================================================================
pub mod dyn_on_hot_path {
    pub trait Signal {
        fn score(&self, px: i64) -> i64;
    }

    pub struct AlwaysBuy;
    impl Signal for AlwaysBuy {
        fn score(&self, px: i64) -> i64 {
            px
        }
    }

    pub fn bad_run(signals: &[Box<dyn Signal>], px: i64) -> i64 {
        signals.iter().map(|s| s.score(px)).sum()
    }

    pub fn good_run<S: Signal>(signals: &[S], px: i64) -> i64 {
        signals.iter().map(|s| s.score(px)).sum()
    }

    pub fn demonstrate() {
        println!("## 陷阱 1：热路径 Box<dyn Trait>");
        let boxed: Vec<Box<dyn Signal>> = vec![Box::new(AlwaysBuy)];
        let statics = [AlwaysBuy];
        println!("dyn sum = {}", bad_run(&boxed, 100));
        println!("static sum = {}", good_run(&statics, 100));
        println!("规则：dyn 只放插件/脚本边界；tick 循环用泛型\n");
    }
}

// ============================================================================
// 陷阱 2：闭包捕获导致堆分配（Box<dyn Fn>）
// ============================================================================
pub mod boxed_closure {
    pub fn bad_filter(data: &[i64], threshold: i64) -> Vec<i64> {
        let pred = Box::new(move |x: &i64| *x > threshold);
        data.iter().filter(|x| pred(x)).copied().collect()
    }

    pub fn good_filter(data: &[i64], threshold: i64) -> Vec<i64> {
        data.iter().filter(|x| **x > threshold).copied().collect()
    }

    pub fn demonstrate() {
        println!("## 陷阱 2：Box<dyn Fn> 闭包");
        let data = [1, 5, 10];
        assert_eq!(bad_filter(&data, 3), good_filter(&data, 3));
        println!("结果相同；bad 版多一次 heap + 间接调用");
        println!("规则：直接用 `move |x|`，让编译器单态化闭包\n");
    }
}

// ============================================================================
// 陷阱 3：过度泛型导致二进制膨胀
// ============================================================================
pub mod monomorphization_bloat {
    pub fn serialize<T: Copy + std::fmt::Debug>(v: T) -> String {
        format!("{:?}", v)
    }

    pub fn demonstrate() {
        println!("## 陷阱 3：单态化代码膨胀");
        let _ = serialize(1u8);
        let _ = serialize(1u16);
        let _ = serialize(1u32);
        let _ = serialize(1u64);
        println!("4 个类型 → 4 份 serialize 机器码");
        println!("规则：热路径泛型 OK；冷路径用 dyn 或 type erasure 控体积\n");
    }
}

// ============================================================================
// 陷阱 4：跨 crate 热函数缺 #[inline]
// ============================================================================
pub mod missing_inline {
    pub fn helper(x: i64) -> i64 {
        x.wrapping_mul(3).wrapping_add(1)
    }

    pub fn hot_loop(data: &[i64]) -> i64 {
        data.iter().map(|&x| helper(x)).sum()
    }

    pub fn demonstrate() {
        println!("## 陷阱 4：跨 crate 缺 inline");
        println!("sum = {}", hot_loop(&[1, 2, 3]));
        println!("LTO 可部分补救；库 crate 热 helper 应 `#[inline]`");
        println!("验证：`cargo bloat -p zero-cost --release`\n");
    }
}

// ============================================================================
// 陷阱 5：热路径 String 分配
// ============================================================================
pub mod string_on_hot_path {
    pub fn bad_key(symbol: &str) -> String {
        format!("{}:USDT", symbol) // ❌ 每 tick 堆分配
    }

    pub fn good_key(symbol: &str) -> (&str, &str) {
        (symbol, "USDT") // ✅ 栈上引用对
    }

    pub fn demonstrate() {
        println!("## 陷阱 5：热路径 String");
        println!("bad  = {}", bad_key("BTC"));
        let (base, quote) = good_key("BTC");
        println!("good = {}:{}", base, quote);
        println!("规则：热路径用 `&str` / fixed buffer / SmallString\n");
    }
}

// ============================================================================
// 陷阱 6：Iterator 未消费 —— 白写 pipeline
// ============================================================================
pub mod iterator_not_consumed {
    pub fn demonstrate() {
        println!("## 陷阱 6：Iterator 未终端消费");
        let _pipeline = (0..1_000_000).map(|x| x * 2).filter(|x| x % 3 == 0);
        println!("上面一行 **零工作量** —— 没有 collect/sum/for_each");
        let n: u64 = (0..100).filter(|x| x % 3 == 0).sum();
        println!("加终端操作后 sum = {}\n", n);
    }
}

// ============================================================================
// 陷阱 7：Arc/Rc 在 tight loop —— 原子引用计数
// ============================================================================
pub mod arc_in_loop {
    use std::sync::Arc;

    pub fn bad_sum(data: &[Arc<i64>]) -> i64 {
        data.iter().map(|x| **x as i64).sum()
    }

    pub fn good_sum(data: &[i64]) -> i64 {
        data.iter().sum()
    }

    pub fn demonstrate() {
        println!("## 陷阱 7：Arc 在 tight loop");
        let owned = vec![Arc::new(42i64); 3];
        let plain = [42i64; 3];
        assert_eq!(bad_sum(&owned), good_sum(&plain));
        println!("Arc 每次 clone 原子操作；共享只读数据用 `&T` 或 `'static`\n");
    }
}

// ============================================================================
// 陷阱 8：release 热路径留 Debug 格式化
// ============================================================================
pub mod debug_in_release {
    pub fn bad_log(px: i64) -> i64 {
        #[cfg(debug_assertions)]
        eprintln!("tick px={}", px);
        px + 1
    }

    pub fn good_log(px: i64) -> i64 {
        px + 1
    }

    pub fn demonstrate() {
        println!("## 陷阱 8：热路径 Debug 格式化");
        println!("bad(100) = {}", bad_log(100));
        println!("good(100) = {}", good_log(100));
        println!("规则：`tracing` 用 level + target；避免 Debug 格式化在 loop 里\n");
    }
}

pub fn demonstrate() {
    dyn_on_hot_path::demonstrate();
    boxed_closure::demonstrate();
    monomorphization_bloat::demonstrate();
    missing_inline::demonstrate();
    string_on_hot_path::demonstrate();
    iterator_not_consumed::demonstrate();
    arc_in_loop::demonstrate();
    debug_in_release::demonstrate();
}
