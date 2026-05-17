//! # Arena 在 Rust 里「到底是什么」
//!
//! **核心语义**：一整块内存向上「线性 bump」；对象之间不单独 `free`，
//! 要么 **整块丢弃**（`Drop` arena），要么 **`unsafe` reset** 使所有借用以失效。
//!
//! **和系统分配器对比**：
//!
//! | 维度 | `malloc` / `Vec` | Bump / Arena |
//! |------|------------------|--------------|
//! | 单次分配成本 | 较高（元数据、锁、碎片） | 极低（仅移动指针） |
//! | 释放粒度 | 任意 | 只能整块回收 |
//! | 缓存局部性 | 取决于分配模式 | 天然连续，利于 prefetch |
//! | `Drop` | 自动 | bump 上分配默认 **不跑析构** —— 要显式用 `bumpalo::boxed::Box` |
//!
//! 下面用 `bumpalo` 演示最常用的 4 个 API 组合。生产里≈90%的代码路径只用得到这些。

#![allow(dead_code)]

use bumpalo::Bump;
use bumpalo::collections::Vec as BumpVec;

pub fn demonstrate() {
    println!("## 1. `alloc` 单对象");

    let bump = Bump::new();
    let x = bump.alloc(42u64);
    let y = bump.alloc([1_u8, 2, 3, 4]);
    println!("arena 上: *x = {}, y = {:?}\n", *x, y);

    println!("## 2. `alloc_slice_copy` / `alloc_slice_fill` 批量定长");

    let prices: &[i64] = &[100_00, 101_25, 99_50];
    let dst = bump.alloc_slice_copy(prices);
    println!("拷贝到 arena: {:?}\n", dst);

    println!("## 3. `BumpVec` —— arena 里「会长大的数组」");

    let mut v = BumpVec::new_in(&bump);
    v.extend([Side::Buy, Side::Sell, Side::Buy]);
    println!("BumpVec 长度 = {}, 底址与 bump 同一产能区\n", v.len());

    println!("## 4. 生命周期：`&Bump` 作为『内存池句柄』");

    let outer = Bump::new();
    let borrowed = scratch_sum(&outer, &[10, -3, 7]);
    println!("scratch_sum（arena 内临时表）= {}\n", borrowed);

    println!(
        "要点：Arena 解决的是 **『一批同生共死对象』**；\n\
         若对象寿命参差不齐，仍应用传统容器或分代/池化别的策略。"
    );
}

#[derive(Debug, Clone, Copy)]
enum Side {
    Buy,
    Sell,
}

/// 函数接收 `&Bump`，在调用栈内分配临时缓冲，避免堆上 `Vec` 抖动。
fn scratch_sum(bump: &Bump, xs: &[i64]) -> i64 {
    let tmp: &mut [i64] = bump.alloc_slice_fill_copy(xs.len(), 0);
    tmp.copy_from_slice(xs);
    tmp.iter().copied().sum()
}
