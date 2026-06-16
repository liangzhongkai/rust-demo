//! # SIMD 常见陷阱与诊断
//!
//! 生产事故里反复出现的 8 个「以为 vectorize、实际更慢或错误」的坑：
//! - 现象（perf / 正确性里看到什么）
//! - 根因（硬件/编译器/内存模型）
//! - 修法（一行改法 + 风格预防）

#![allow(dead_code)]

use crate::util;

// ============================================================================
// 陷阱 1：未做 runtime feature detect → SIGILL
// ============================================================================
pub mod missing_feature_detect {
    pub fn demonstrate() {
        println!("## 陷阱 1：缺少 Feature Detection");
        println!("avx2_available = {}", util::avx2_available());
        println!("现象：新 CPU 开发机 OK，老云主机 SIGILL crash");
        println!("规则：`is_x86_feature_detected!` + scalar fallback 或分发函数指针\n");
    }
}

// ============================================================================
// 陷阱 2：AoS 布局阻碍 vectorize
// ============================================================================
pub mod aos_vs_soa {
    #[derive(Clone, Copy)]
    pub struct Tick {
        pub ts: u64,
        pub px: i32,
        pub qty: i32,
    }

    pub fn sum_qty_aos(ticks: &[Tick]) -> i64 {
        ticks.iter().map(|t| t.qty as i64).sum()
    }

    pub fn sum_qty_soa(px: &[i32], qty: &[i32]) -> i64 {
        util::sum_i32(qty) as i64
    }

    pub fn demonstrate() {
        println!("## 陷阱 2：AoS vs SoA");
        let ticks: Vec<Tick> = (0..128)
            .map(|i| Tick {
                ts: i,
                px: 100,
                qty: i as i32,
            })
            .collect();
        let qty: Vec<i32> = ticks.iter().map(|t| t.qty).collect();
        println!("AoS sum = {}", sum_qty_aos(&ticks));
        println!("SoA sum = {}", sum_qty_soa(&[], &qty));
        println!("规则：热路径字段抽成连续数组；AoS 仅冷路径/debug\n");
    }
}

// ============================================================================
// 陷阱 3：内层分支杀死 auto-vectorization
// ============================================================================
pub mod branch_in_inner_loop {
    pub fn count_with_branch(data: &[i32], th: i32) -> usize {
        let mut n = 0;
        for &x in data {
            if x > th {
                n += 1;
            }
        }
        n
    }

    pub fn count_simd(data: &[i32], th: i32) -> usize {
        util::count_above_i32(data, th)
    }

    pub fn demonstrate() {
        let data: Vec<i32> = (0..10_000).map(|i| (i % 100) as i32).collect();
        let s = count_with_branch(&data, 50);
        let v = count_simd(&data, 50);
        println!("## 陷阱 3：内层分支");
        println!("branch count={s} simd count={v}");
        println!("规则：用 cmp+mask 替代 branch；或 `#[cold]` 分离 rare path\n");
    }
}

// ============================================================================
// 陷阱 4：忽略 tail epilogue → 越界或漏算
// ============================================================================
pub mod tail_epilogue {
    pub fn sum_wrong(data: &[f64]) -> f64 {
        // 错误示范：假设 len 总是 4 的倍数
        let mut total = 0.0;
        for chunk in data.chunks(4) {
            if chunk.len() == 4 {
                total += chunk.iter().sum::<f64>();
            }
        }
        total
    }

    pub fn sum_correct(data: &[f64]) -> f64 {
        util::sum_f64(data)
    }

    pub fn demonstrate() {
        let data: Vec<f64> = (0..17).map(|i| i as f64).collect();
        let wrong = sum_wrong(&data);
        let right = sum_correct(&data);
        println!("## 陷阱 4：尾处理 epilogue");
        println!("len=17 wrong={wrong} correct={right}");
        println!("规则：`chunks_exact` + remainder 标量；或 padding sentinel\n");
    }
}

// ============================================================================
// 陷阱 5：f64 累加顺序 → 对账 1 ULP 差
// ============================================================================
pub mod f64_associativity {
    pub fn demonstrate() {
        let v = vec![0.1_f64; 10_000_000];
        let s1: f64 = v.iter().take(5_000_000).sum::<f64>()
            + v.iter().skip(5_000_000).sum::<f64>();
        let s2 = util::sum_f64(&v);
        println!("## 陷阱 5：f64 结合律");
        println!("split sum vs simd sum delta = {:.2e}", (s1 - s2).abs());
        println!("规则：财务 PnL 用 i64 定点；f64 对账允许 ε 或 Kahan sum\n");
    }
}

// ============================================================================
// 陷阱 6：非连续 / 跨步访问
// ============================================================================
pub mod strided_access {
    pub fn sum_every_k(data: &[i32], k: usize) -> i64 {
        data.iter().step_by(k).map(|&x| x as i64).sum()
    }

    pub fn demonstrate() {
        let data: Vec<i32> = (0..1024).collect();
        println!("## 陷阱 6：跨步访问");
        println!("step_by(3) sum = {}", sum_every_k(&data, 3));
        println!("规则：gather 指令慢；重构为 compact index 或 SoA 列\n");
    }
}

// ============================================================================
// 陷阱 7：冷路径也 SIMD → 代码膨胀
// ============================================================================
pub mod code_bloat {
    pub fn cold_path_simd_overkill(data: &[f64]) -> f64 {
        // 日终批处理 OK；若只在 startup 调一次则浪费 icache
        util::sum_f64(data)
    }

    pub fn demonstrate() {
        println!("## 陷阱 7：冷路径过度 SIMD");
        let _ = cold_path_simd_overkill(&[1.0, 2.0]);
        println!("规则：profile 证明热才手写 intrinsics；冷路径靠 auto-vec\n");
    }
}

// ============================================================================
// 陷阱 8：False sharing 与错误对齐假设
// ============================================================================
pub mod false_sharing {
    use std::sync::atomic::{AtomicU64, Ordering};

    pub fn demonstrate() {
        println!("## 陷阱 8：False Sharing");
        let counters = [
            AtomicU64::new(0),
            AtomicU64::new(0),
            AtomicU64::new(0),
            AtomicU64::new(0),
        ];
        for c in &counters {
            c.fetch_add(1, Ordering::Relaxed);
        }
        println!("4×AtomicU64 同 cache line → 多线程 ping-pong");
        println!("规则：per-thread 累加 + 最后 merge；`#[repr(align(64))]` 隔离\n");
    }
}

pub fn demonstrate() {
    missing_feature_detect::demonstrate();
    aos_vs_soa::demonstrate();
    branch_in_inner_loop::demonstrate();
    tail_epilogue::demonstrate();
    f64_associativity::demonstrate();
    strided_access::demonstrate();
    code_bloat::demonstrate();
    false_sharing::demonstrate();
}

#[cfg(test)]
mod tests {
    use super::tail_epilogue::*;

    #[test]
    fn correct_sum_handles_tail() {
        let data: Vec<f64> = (0..17).map(|i| i as f64).collect();
        assert!((sum_correct(&data) - 136.0).abs() < 1e-9);
        assert!((sum_wrong(&data) - 136.0).abs() > 1.0);
    }
}
