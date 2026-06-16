//! # SIMD 底层机制
//!
//! 向量化不是「换一个库」那么简单，而是数据布局 + 指令集 + 编译器三者的协作：
//!
//! 1. **Lane 宽度**：AVX2 = 256-bit → 4×f64 / 8×i32 / 32×u8 并行
//! 2. **显式 vs 自动**：`std::arch` intrinsics 可控；plain loop 靠 LLVM auto-vectorize
//! 3. **对齐**：`_mm256_loadu_*` 不要求对齐，但 aligned load 在部分场景更快
//! 4. **水平归约**：垂直 lane 累加 cheap，lane 间求和是常见瓶颈
//! 5. **尾处理**：长度非 lane 整数倍时必须标量 epilogue

#![allow(dead_code)]

use crate::util;

// ============================================================================
// 1. Lane 宽度与数据类型映射
// ============================================================================
pub mod lane_width {
    pub fn demonstrate() {
        println!("## 1. Lane 宽度与类型映射");
        println!("AVX2 256-bit: 4×f64 | 8×i32 | 32×u8");
        println!("选类型：定点价用 i32 tick 可 8-wide；名义价值用 f64 dot");
        println!("i64 全宽需 AVX-512；HFT 常拆成 i32 或走标量 epilogue\n");
    }
}

// ============================================================================
// 2. 显式 intrinsics vs 编译器自动向量化
// ============================================================================
pub mod explicit_vs_auto {
    /// 编译器友好写法：简单计数，release 下常 auto-vectorize。
    #[inline(never)]
    pub fn count_positive_auto(data: &[i32]) -> usize {
        data.iter().filter(|&&x| x > 0).count()
    }

    /// 显式 SIMD：行为确定，不依赖 LLVM heuristics。
    pub fn count_positive_explicit(data: &[i32]) -> usize {
        util::count_above_i32(data, 0)
    }

    pub fn demonstrate() {
        let data: Vec<i32> = (-50..50).collect();
        let auto = count_positive_auto(&data);
        let explicit = count_positive_explicit(&data);
        println!("## 2. 显式 vs 自动向量化");
        println!("count_positive auto={auto} explicit={explicit}");
        println!("auto：依赖 opt-level + 无分支 + 连续内存");
        println!("explicit：intrinsics 可控，需 runtime feature detect\n");
    }
}

// ============================================================================
// 3. 水平归约（vertical + horizontal）
// ============================================================================
pub mod horizontal_reduce {
    pub fn demonstrate() {
        let prices: Vec<f64> = (0..10_000).map(|i| 100.0 + (i % 7) as f64 * 0.01).collect();
        let total = util::sum_f64(&prices);
        println!("## 3. 水平归约");
        println!("10k prices sum = {total:.2}");
        println!("垂直：lane 内并行 add；水平：lane 间 shuffle + add");
        println!("瓶颈：归约链延迟；大数组可 partial sum + 二次归约\n");
    }
}

// ============================================================================
// 4. FMA 融合乘加
// ============================================================================
pub mod fma_dot {
    pub fn demonstrate() {
        let qty: Vec<f64> = (1..=1024).map(|i| i as f64).collect();
        let px: Vec<f64> = qty.iter().map(|&q| 100.0 + q * 0.001).collect();
        let notional = util::dot_f64(&px, &qty);
        println!("## 4. FMA 融合乘加 dot(px, qty)");
        println!("notional = {notional:.2}");
        println!("_mm256_fmadd_pd：一次指令完成 mul+add，精度与两次 round 不同\n");
    }
}

// ============================================================================
// 5. 32 字节并行比较（memcmp 类操作）
// ============================================================================
pub mod memcmp_simd {
    pub fn demonstrate() {
        let a = [0xab_u8; 32];
        let mut b = a;
        b[31] = 0xac;
        println!("## 5. 32-byte SIMD 比较");
        println!("equal = {}", util::bytes_eq_32(&a, &a));
        println!("diff last byte = {}", util::bytes_eq_32(&a, &b));
        println!("Web3 topic/hash 过滤、HFT 固定长帧 magic 校验复用此模式\n");
    }
}

// ============================================================================
// 6. Runtime feature detection
// ============================================================================
pub mod feature_detect {
    pub fn demonstrate() {
        println!("## 6. Runtime Feature Detection");
        println!("avx2_available = {}", util::avx2_available());
        println!("生产：启动时检测 → 函数指针表 / ifun 缓存 / 多版本 binary");
        println!("云部署：baseline CPU feature flag 与 build target 对齐\n");
    }
}

pub fn demonstrate() {
    lane_width::demonstrate();
    explicit_vs_auto::demonstrate();
    horizontal_reduce::demonstrate();
    fma_dot::demonstrate();
    memcmp_simd::demonstrate();
    feature_detect::demonstrate();
}

#[cfg(test)]
mod tests {
    use super::explicit_vs_auto::*;

    #[test]
    fn auto_explicit_agree() {
        let data: Vec<i32> = (-100..100).collect();
        assert_eq!(count_positive_auto(&data), count_positive_explicit(&data));
    }
}
