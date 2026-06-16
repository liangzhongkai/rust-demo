//! 跨模块复用的 SIMD 工具：x86_64 显式 intrinsics + 标量回退。
//!
//! 生产环境应配合 `is_x86_feature_detected!` 做 runtime dispatch；
//! 本 crate 为教学可读性，在支持 AVX2 的 CPU 上走 intrinsics，否则标量。

#![allow(dead_code)]

/// 运行时是否可用 AVX2（FMA 通常同代 CPU 一并具备）。
#[inline]
pub fn avx2_available() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        std::arch::is_x86_feature_detected!("avx2")
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

/// f64 数组求和：4-lane AVX2 累加 + 标量尾。
pub fn sum_f64(data: &[f64]) -> f64 {
    #[cfg(target_arch = "x86_64")]
    {
        if avx2_available() {
            return unsafe { sum_f64_avx2(data) };
        }
    }
    data.iter().sum()
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2", enable = "fma")]
unsafe fn sum_f64_avx2(data: &[f64]) -> f64 {
    use std::arch::x86_64::*;

    let mut acc = _mm256_setzero_pd();
    let mut chunks = data.chunks_exact(4);
    for chunk in chunks.by_ref() {
        let v = _mm256_loadu_pd(chunk.as_ptr());
        acc = _mm256_add_pd(acc, v);
    }
    let hi = _mm256_extractf128_pd(acc, 1);
    let lo = _mm256_castpd256_pd128(acc);
    let sum128 = _mm_add_pd(lo, hi);
    let shuf = _mm_shuffle_pd(sum128, sum128, 0b01);
    let sum64 = _mm_add_sd(sum128, shuf);
    let mut total = _mm_cvtsd_f64(sum64);
    for &x in chunks.remainder() {
        total += x;
    }
    total
}

/// i32 数组求和：8-lane AVX2。
pub fn sum_i32(data: &[i32]) -> i64 {
    #[cfg(target_arch = "x86_64")]
    {
        if avx2_available() {
            return unsafe { sum_i32_avx2(data) } as i64;
        }
    }
    data.iter().map(|&x| x as i64).sum()
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn sum_i32_avx2(data: &[i32]) -> i32 {
    use std::arch::x86_64::*;

    let mut acc = _mm256_setzero_si256();
    let mut chunks = data.chunks_exact(8);
    for chunk in chunks.by_ref() {
        let v = _mm256_loadu_si256(chunk.as_ptr() as *const __m256i);
        acc = _mm256_add_epi32(acc, v);
    }
    let sum128 = _mm_add_epi32(
        _mm256_castsi256_si128(acc),
        _mm256_extracti128_si256(acc, 1),
    );
    let shuf = _mm_shuffle_epi32(sum128, 0b01_00_11_10);
    let sum64 = _mm_add_epi32(sum128, shuf);
    let shuf2 = _mm_shuffle_epi32(sum64, 0b00_01_00_01);
    let final32 = _mm_add_epi32(sum64, shuf2);
    let mut total = _mm_cvtsi128_si32(final32);
    for &x in chunks.remainder() {
        total += x;
    }
    total
}

/// 两数组逐元素乘加累加：`sum(a[i] * b[i])`，VWAP / 名义价值常用。
pub fn dot_f64(a: &[f64], b: &[f64]) -> f64 {
    assert_eq!(a.len(), b.len());
    #[cfg(target_arch = "x86_64")]
    {
        if avx2_available() {
            return unsafe { dot_f64_avx2(a, b) };
        }
    }
    a.iter().zip(b).map(|(&x, &y)| x * y).sum()
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2", enable = "fma")]
unsafe fn dot_f64_avx2(a: &[f64], b: &[f64]) -> f64 {
    use std::arch::x86_64::*;

    let mut acc = _mm256_setzero_pd();
    let len = a.len();
    let mut i = 0;
    while i + 4 <= len {
        let va = _mm256_loadu_pd(a.as_ptr().add(i));
        let vb = _mm256_loadu_pd(b.as_ptr().add(i));
        acc = _mm256_fmadd_pd(va, vb, acc);
        i += 4;
    }
    let hi = _mm256_extractf128_pd(acc, 1);
    let lo = _mm256_castpd256_pd128(acc);
    let sum128 = _mm_add_pd(lo, hi);
    let shuf = _mm_shuffle_pd(sum128, sum128, 0b01);
    let sum64 = _mm_add_sd(sum128, shuf);
    let mut total = _mm_cvtsd_f64(sum64);
    while i < len {
        total += a[i] * b[i];
        i += 1;
    }
    total
}

/// 32 字节常量时间相等比较（topic / hash / address）。
#[inline]
pub fn bytes_eq_32(a: &[u8; 32], b: &[u8; 32]) -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        if avx2_available() {
            return unsafe { bytes_eq_32_avx2(a, b) };
        }
    }
    a == b
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn bytes_eq_32_avx2(a: &[u8; 32], b: &[u8; 32]) -> bool {
    use std::arch::x86_64::*;

    let va = _mm256_loadu_si256(a.as_ptr() as *const __m256i);
    let vb = _mm256_loadu_si256(b.as_ptr() as *const __m256i);
    let cmp = _mm256_cmpeq_epi8(va, vb);
    _mm256_movemask_epi8(cmp) as u32 == 0xFFFF_FFFF
}

/// 批量 topic 匹配：返回命中索引。
pub fn filter_topics_32(logs: &[[u8; 32]], target: &[u8; 32]) -> Vec<usize> {
    logs.iter()
        .enumerate()
        .filter_map(|(i, t)| bytes_eq_32(t, target).then_some(i))
        .collect()
}

/// i32 数组中大于 threshold 的元素计数（信号触发次数）。
pub fn count_above_i32(data: &[i32], threshold: i32) -> usize {
    #[cfg(target_arch = "x86_64")]
    {
        if avx2_available() {
            return unsafe { count_above_i32_avx2(data, threshold) };
        }
    }
    data.iter().filter(|&&x| x > threshold).count()
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn count_above_i32_avx2(data: &[i32], threshold: i32) -> usize {
    use std::arch::x86_64::*;

    let thresh = _mm256_set1_epi32(threshold);
    let mut count = 0usize;
    let mut chunks = data.chunks_exact(8);
    for chunk in chunks.by_ref() {
        let v = _mm256_loadu_si256(chunk.as_ptr() as *const __m256i);
        let gt = _mm256_cmpgt_epi32(v, thresh);
        count += _mm256_movemask_ps(_mm256_castsi256_ps(gt)).count_ones() as usize;
    }
    for &x in chunks.remainder() {
        if x > threshold {
            count += 1;
        }
    }
    count
}

/// u8 数组中等于 needle 的字节计数（RLP / 分隔符扫描）。
pub fn count_byte_eq(data: &[u8], needle: u8) -> usize {
    #[cfg(target_arch = "x86_64")]
    {
        if avx2_available() {
            return unsafe { count_byte_eq_avx2(data, needle) };
        }
    }
    data.iter().filter(|&&b| b == needle).count()
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn count_byte_eq_avx2(data: &[u8], needle: u8) -> usize {
    use std::arch::x86_64::*;

    let n = _mm256_set1_epi8(needle as i8);
    let mut count = 0usize;
    let mut chunks = data.chunks_exact(32);
    for chunk in chunks.by_ref() {
        let v = _mm256_loadu_si256(chunk.as_ptr() as *const __m256i);
        let eq = _mm256_cmpeq_epi8(v, n);
        count += _mm256_movemask_epi8(eq).count_ones() as usize;
    }
    for &b in chunks.remainder() {
        if b == needle {
            count += 1;
        }
    }
    count
}

/// 两 i32 数组逐元素差绝对值之和（跨所价差监控）。
pub fn abs_diff_sum_i32(a: &[i32], b: &[i32]) -> i64 {
    assert_eq!(a.len(), b.len());
    #[cfg(target_arch = "x86_64")]
    {
        if avx2_available() {
            return unsafe { abs_diff_sum_i32_avx2(a, b) };
        }
    }
    a.iter()
        .zip(b)
        .map(|(&x, &y)| (x as i64 - y as i64).unsigned_abs() as i64)
        .sum()
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn abs_diff_sum_i32_avx2(a: &[i32], b: &[i32]) -> i64 {
    use std::arch::x86_64::*;

    let mut total = 0i64;
    let len = a.len();
    let mut i = 0;
    while i + 8 <= len {
        let va = _mm256_loadu_si256(a.as_ptr().add(i) as *const __m256i);
        let vb = _mm256_loadu_si256(b.as_ptr().add(i) as *const __m256i);
        let diff = _mm256_sub_epi32(va, vb);
        let abs_diff = _mm256_abs_epi32(diff);
        total += horizontal_sum_i32x8(abs_diff);
        i += 8;
    }
    while i < len {
        total += (a[i] as i64 - b[i] as i64).unsigned_abs() as i64;
        i += 1;
    }
    total
}

#[cfg(target_arch = "x86_64")]
unsafe fn horizontal_sum_i32x8(v: std::arch::x86_64::__m256i) -> i64 {
    use std::arch::x86_64::*;

    let sum128 = _mm_add_epi32(
        _mm256_castsi256_si128(v),
        _mm256_extracti128_si256(v, 1),
    );
    let shuf = _mm_shuffle_epi32(sum128, 0b01_00_11_10);
    let sum64 = _mm_add_epi32(sum128, shuf);
    let shuf2 = _mm_shuffle_epi32(sum64, 0b00_01_00_01);
    let final32 = _mm_add_epi32(sum64, shuf2);
    _mm_cvtsi128_si32(final32) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sum_f64_matches_scalar() {
        let data: Vec<f64> = (0..100).map(|i| i as f64 * 0.5).collect();
        let scalar: f64 = data.iter().sum();
        assert!((sum_f64(&data) - scalar).abs() < 1e-9);
    }

    #[test]
    fn dot_f64_matches_scalar() {
        let a: Vec<f64> = (0..64).map(|i| i as f64).collect();
        let b: Vec<f64> = (0..64).map(|i| (i * 2) as f64).collect();
        let scalar: f64 = a.iter().zip(&b).map(|(&x, &y)| x * y).sum();
        assert!((dot_f64(&a, &b) - scalar).abs() < 1e-6);
    }

    #[test]
    fn bytes_eq_32_works() {
        let a = [1u8; 32];
        let b = [1u8; 32];
        let c = [2u8; 32];
        assert!(bytes_eq_32(&a, &b));
        assert!(!bytes_eq_32(&a, &c));
    }
}
