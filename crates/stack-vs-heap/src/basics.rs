//! # 栈与堆的底层机制
//!
//! 这一节回答四个根本问题，后面 HFT/Web3 章节里所有内存决策都建立在这之上：
//!
//! 1. 栈帧是什么？什么时候自动分配/释放？
//! 2. 堆分配的真实成本（syscall / allocator / cache miss）？
//! 3. `Copy` 类型 vs 堆拥有型（`String`/`Vec`/`Box`）的内存模型？
//! 4. 编译器如何把「小数组放栈」优化成寄存器操作？

#![allow(dead_code)]

use crate::util::{bench_ns, layout, AllocCounter};

/// 栈：LIFO、编译期已知大小、函数返回自动 pop。
/// 堆：运行时向 allocator 申请、Drop 时释放、大小可变。
pub mod stack_vs_heap_model {
    use super::*;

    #[derive(Clone, Copy, Debug)]
    struct Quote {
        bid: i64,
        ask: i64,
    }

    fn stack_frame() -> Quote {
        // Quote 在栈上：24 bytes，无 malloc
        let q = Quote { bid: 100, ask: 101 };
        q
    }

    fn heap_owned() -> Box<Quote> {
        // Box 本身在栈（8 bytes 指针），Quote 在堆
        Box::new(Quote { bid: 100, ask: 101 })
    }

    pub fn demonstrate() {
        println!("## 1. 栈帧 vs 堆指针");

        layout::<Quote>("Quote (Copy, 栈)");
        layout::<Box<Quote>>("Box<Quote> (指针栈 + 数据堆)");
        layout::<String>("String (24B 栈 + heap buffer)");
        layout::<Vec<u64>>("Vec<u64> (24B 栈 + heap buffer)");

        let _s = stack_frame();
        let _h = heap_owned();
        println!("  函数返回时栈帧 pop；Box 在 drop 时 free 堆块\n");
    }
}

/// `Copy` 语义 = 按位复制栈上的值，不涉及堆。
pub mod copy_semantics {
    pub fn demonstrate() {
        println!("## 2. Copy 类型按位复制，零堆开销");

        #[derive(Clone, Copy, Debug)]
        struct Level {
            px: i64,
            qty: i64,
        }

        let a = Level { px: 100, qty: 5 };
        let b = a; // 按位复制 16 bytes
        println!("  a = {:?}, b = {:?}（a 仍可用）", a, b);

        let s1 = String::from("BTC");
        let s2 = s1.clone(); // 堆 buffer 深拷贝
        println!("  String clone 会 malloc + memcpy: len={}", s2.len());
        println!("  规则：热路径优先 Copy 类型；String 只在边界出现\n");
    }
}

/// Vec 增长 = 多次 realloc；`with_capacity` 一次到位。
pub mod vec_growth {
    use super::*;

    pub fn slow_push(n: usize) -> (Vec<u64>, AllocCounter) {
        let mut v = Vec::new();
        let mut counter = AllocCounter::default();
        for i in 0..n {
            counter.track_vec_push(&mut v, i as u64);
        }
        (v, counter)
    }

    pub fn fast_push(n: usize) -> (Vec<u64>, AllocCounter) {
        let mut v = Vec::with_capacity(n);
        let mut counter = AllocCounter::default();
        for i in 0..n {
            counter.track_vec_push(&mut v, i as u64);
        }
        (v, counter)
    }

    pub fn demonstrate() {
        println!("## 3. Vec 增长 vs 预分配");

        let n = 10_000;
        let (v1, c1) = slow_push(n);
        let (v2, c2) = fast_push(n);
        assert_eq!(v1, v2);
        println!("  默认 push {} 次: {} 次 realloc, ~{} bytes", n, c1.allocs, c1.bytes);
        println!(
            "  with_capacity: {} 次 realloc, ~{} bytes",
            c2.allocs, c2.bytes
        );
        println!("  规则：已知上界 → reserve / with_capacity\n");
    }
}

/// 固定大小数组在栈上，LLVM 可完全展开。
pub mod stack_array {
    use super::*;

    pub fn sum_stack(prices: &[i64]) -> i64 {
        let mut buf = [0i64; 64];
        let n = prices.len().min(64);
        buf[..n].copy_from_slice(&prices[..n]);
        buf[..n].iter().sum()
    }

    pub fn sum_heap(prices: &[i64]) -> i64 {
        let v: Vec<i64> = prices.iter().take(64).copied().collect();
        v.iter().sum()
    }

    pub fn demonstrate() {
        println!("## 4. 栈数组 vs 临时 Vec");

        let prices: Vec<i64> = (0..64).collect();
        assert_eq!(sum_stack(&prices), sum_heap(&prices));

        let (min_s, mean_s) = bench_ns(100, 5000, || {
            let _ = sum_stack(&prices);
        });
        let (min_h, mean_h) = bench_ns(100, 5000, || {
            let _ = sum_heap(&prices);
        });
        println!("  sum_stack: min={}ns mean={}ns", min_s, mean_s);
        println!("  sum_heap:  min={}ns mean={}ns", min_h, mean_h);
        println!("  关键：[T; N] 无 alloc；临时 Vec 至少 1 次 malloc\n");
    }
}

pub fn demonstrate() {
    stack_vs_heap_model::demonstrate();
    copy_semantics::demonstrate();
    vec_growth::demonstrate();
    stack_array::demonstrate();
}
