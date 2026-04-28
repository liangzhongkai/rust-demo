//! # 迭代器底层机制
//!
//! 这一节回答四个根本问题，后面 HFT/Web3 章节里所有套路都建立在这之上：
//!
//! 1. `Iterator` trait 到底是什么？
//! 2. 惰性求值意味着什么？什么时候真正开始干活？
//! 3. 适配器为什么是「零成本抽象」？
//! 4. `size_hint` 为什么决定了 `collect` 的性能？

#![allow(dead_code)]

/// `Iterator` trait 的本质：一个可以被反复 poll 的状态机。
///
/// ```ignore
/// pub trait Iterator {
///     type Item;
///     fn next(&mut self) -> Option<Self::Item>;
///     // 其余 75+ 方法都基于 next() 默认实现
/// }
/// ```
///
/// 所以「迭代器」不是数据结构，而是 *协议*：只要实现 `next`，
/// `map / filter / fold / collect / sum` 全部免费获得。
pub mod what_is_an_iterator {
    /// 自己实现一个斐波那契迭代器：本质就是一个状态机。
    pub struct Fib {
        a: u64,
        b: u64,
    }

    impl Fib {
        pub fn new() -> Self {
            Self { a: 0, b: 1 }
        }
    }

    impl Iterator for Fib {
        type Item = u64;

        fn next(&mut self) -> Option<u64> {
            let cur = self.a;
            // 状态机推进；溢出时停止迭代
            let next = self.a.checked_add(self.b)?;
            self.a = self.b;
            self.b = next;
            Some(cur)
        }
    }

    pub fn demonstrate() {
        println!("## 1. Iterator 是状态机协议");

        // 实现了 next 后，整个生态自动可用
        let sum: u64 = Fib::new().take(10).sum();
        let evens: Vec<u64> = Fib::new().take(10).filter(|x| x % 2 == 0).collect();
        println!("前 10 个 Fibonacci 之和 = {}", sum);
        println!("前 10 个里的偶数 = {:?}", evens);
        println!();
    }
}

/// 三种迭代语义对应三种所有权立场。
/// 选错语义 = 选错性能模型。
pub mod three_flavors {
    pub fn demonstrate() {
        println!("## 2. iter / iter_mut / into_iter 三种立场");

        let prices = vec![100u64, 101, 102];

        // iter() -> &T，只读，不消费容器
        let sum_borrowed: u64 = prices.iter().sum();

        // iter_mut() -> &mut T，原地修改
        let mut prices_copy = prices.clone();
        prices_copy.iter_mut().for_each(|p| *p += 1);

        // into_iter() -> T，消费容器；闭包可以拿走 T
        let owned: Vec<String> = vec!["BTC".to_string(), "ETH".to_string()]
            .into_iter()
            .map(|s| s + "-USDT")
            .collect();

        println!("借用求和 = {} (原 vec 仍可用: {:?})", sum_borrowed, prices);
        println!("可变借用后: {:?}", prices_copy);
        println!("消费式 map 后: {:?}", owned);
        println!("规则：`for x in v` 默认是 into_iter；`for x in &v` 是 iter\n");
    }
}

/// 惰性求值：链式调用其实只是「拼装状态机」，
/// 直到调用 `consumer`（next/collect/fold/sum/for_each/...）才真正执行。
pub mod laziness {
    pub fn demonstrate() {
        println!("## 3. 惰性求值与终端操作");

        // 这一行什么都不做：没有终端操作
        let _pipeline = (0..1_000_000_000u64)
            .map(|x| {
                // 如果立即执行，下面 panic 会立刻发生
                if x == 999 {
                    panic!("不会触发，因为还没消费");
                }
                x * 2
            })
            .filter(|x| x % 3 == 0);

        println!("拼装管道（10 亿元素）：未执行，0 开销");

        // 加一个 take(5) 仍然不执行
        let lazy = (0..u64::MAX).map(|x| x * 2).take(5);
        println!("加 take(5)：仍然不执行");

        // collect/sum/for_each/next 才是触发器
        let realized: Vec<u64> = lazy.collect();
        println!("调用 collect 才真正运行: {:?}", realized);

        println!("惰性的代价：忘记终端操作 = 静默 bug（pitfalls.rs 详解）\n");
    }
}

/// 零成本抽象：链式适配器经过单态化后，等价于手写循环。
/// 这是 HFT 敢用迭代器写热路径的根本原因。
pub mod zero_cost {
    /// 写法 A：朴素循环
    pub fn handwritten_loop(prices: &[u64]) -> u64 {
        let mut sum = 0u64;
        for &p in prices {
            if p > 100 {
                sum += p * 2;
            }
        }
        sum
    }

    /// 写法 B：迭代器链
    /// 编译后（release）和写法 A 生成几乎完全相同的汇编。
    pub fn iterator_chain(prices: &[u64]) -> u64 {
        prices.iter().filter(|&&p| p > 100).map(|&p| p * 2).sum()
    }

    pub fn demonstrate() {
        println!("## 4. 零成本抽象");

        let prices: Vec<u64> = (0u64..1000).collect();
        let a = handwritten_loop(&prices);
        let b = iterator_chain(&prices);

        assert_eq!(a, b);
        println!("两种写法结果相同：{}", a);
        println!("release 模式下汇编几乎一致；用 `cargo asm` 可验证");
        println!("结论：高频路径上写迭代器链，没有性能负担\n");
    }
}

/// `size_hint` 是迭代器告诉下游「我大概还能产出多少个」的契约。
/// `collect::<Vec<_>>` 据此预分配，避免反复 realloc。
pub mod size_hint_matters {
    /// 一个故意撒谎的迭代器：声称只有 0 个，导致 collect 反复扩容。
    pub struct Lying<I: Iterator>(pub I);

    impl<I: Iterator> Iterator for Lying<I> {
        type Item = I::Item;
        fn next(&mut self) -> Option<I::Item> {
            self.0.next()
        }
        fn size_hint(&self) -> (usize, Option<usize>) {
            (0, None) // 谎报：未知上界
        }
    }

    pub fn demonstrate() {
        println!("## 5. size_hint 决定 collect 的性能");

        let n = 100_000;

        // 准确 hint：(100_000, Some(100_000)) -> 一次性分配
        let v1: Vec<u64> = (0..n).collect();
        // 谎报 hint：从默认容量 0/4 开始反复 *2 扩容（log2 次 realloc）
        let v2: Vec<u64> = Lying(0..n).collect();

        assert_eq!(v1.len(), v2.len());
        println!("两个 Vec 都是 {} 个元素", v1.len());
        println!("但 v2 经历了多次 realloc + memcpy");
        println!("自定义 Iterator 时永远要正确实现 size_hint！\n");
    }
}

/// 三个常被忽略但生产代码里很值钱的子 trait。
pub mod sub_traits {
    /// `ExactSizeIterator`：长度精确已知（`len()` 返回 usize）。
    /// `DoubleEndedIterator`：可以从尾部 `next_back`，撑起 `rev()`。
    /// `FusedIterator`：一旦返回 `None` 就永远 `None`，让链式优化更激进。
    pub fn demonstrate() {
        println!("## 6. ExactSize / DoubleEnded / Fused");

        let v = vec![1, 2, 3, 4, 5];

        // ExactSizeIterator：collect 可以 0 次 realloc
        let it = v.iter();
        println!("len() = {}（编译期可知，O(1)）", it.len());

        // DoubleEndedIterator：处理订单簿时常见（从最优价向外扩展）
        let last_two: Vec<_> = v.iter().rev().take(2).collect();
        println!("从尾部取 2 个: {:?}", last_two);

        // FusedIterator：保证语义稳定，避免「再 poll 一次返回 Some」的怪异行为
        // std 的大部分迭代器都自动实现 Fused
        println!("自定义 Iterator 时，加 #[derive] 不能解决，要手动 impl FusedIterator\n");
    }
}

pub fn demonstrate() {
    what_is_an_iterator::demonstrate();
    three_flavors::demonstrate();
    laziness::demonstrate();
    zero_cost::demonstrate();
    size_hint_matters::demonstrate();
    sub_traits::demonstrate();
}
