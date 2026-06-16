//! # 泛化：从 HFT/Web3 场景到通用 SIMD 策略
//!
//! 把前两章具体业务里的 SIMD 套路抽象成决策矩阵：
//!
//! | 问题类型           | 标志特征                  | 首选套路                          |
//! |--------------------|---------------------------|-----------------------------------|
//! | 1. 规约 (reduce)   | sum/min/max/dot           | lane 累加 + 水平归约 + tail       |
//! | 2. 映射 (map)      | 逐元素变换                | 自动向量化优先，必要时 intrinsics   |
//! | 3. 过滤 (filter)   | 批量 eq/range test        | cmpeq + movemask + popcnt         |
//! | 4. 搜索 (search)   | 子串/锚点/边界            | memchr 或 SSE 滑动窗口            |
//! | 5. 哈希/位运算     | XOR/AND 固定宽度块        | AVX2 32B/64B load/xor/store       |
//! | 6. 布局 (layout)   | AoS 阻碍 vectorize        | SoA / struct-of-arrays            |
//! | 7. 分桶 (histogram)| 索引写共享数组            | per-thread bucket + merge         |
//! | 8. 可移植性        | 多架构部署                | feature detect + scalar fallback  |
//!
//! 下面 8 个策略各有一个通用模板，签名不带业务名词。

#![allow(dead_code)]

// ============================================================================
// 策略 1：规约 —— sum / min / max
// ============================================================================
/// 问题：把数组归约成单个标量。
/// 模式：`chunks_exact(LANES)` SIMD 累加 → 水平归约 → remainder 标量。
///
/// HFT: hft::best_price_scan, batch_notional_risk
/// Web3: merkle 层节点计数
pub mod reduce {
    pub fn sum_i64(data: &[i64]) -> i64 {
        crate::util::sum_i64(data)
    }

    pub fn demonstrate() {
        println!("## 策略 1：规约 sum");
        let v: Vec<i64> = (1..=100).collect();
        println!("sum = {}\n", sum_i64(&v));
    }
}

// ============================================================================
// 策略 2：映射 —— 逐元素变换
// ============================================================================
/// 问题：对数组每个元素做相同运算。
/// 模式：先写标量 iterator/map，让 LLVM 自动向量化；profiling 不够再 intrinsics。
///
/// HFT: hft::rolling_sum 冷启动
/// Web3: rlp 字节分类
pub mod map {
    pub fn scale(input: &[i64], factor: i64) -> Vec<i64> {
        input.iter().map(|&x| x * factor).collect()
    }

    pub fn demonstrate() {
        println!("## 策略 2：map 变换");
        let v = [1, 2, 3, 4];
        println!("scale×10 = {:?}\n", scale(&v, 10));
    }
}

// ============================================================================
// 策略 3：过滤 —— 批量相等/范围测试
// ============================================================================
/// 问题：从数组中筛出满足条件的元素或计数。
/// 模式：固定宽度块 cmpeq → movemask → 非零即匹配。
///
/// HFT: fix 消息类型过滤
/// Web3: web3::topic_filter
pub mod filter {
    pub fn count_eq(data: &[i64], target: i64) -> usize {
        data.iter().filter(|&&x| x == target).count()
    }

    pub fn demonstrate() {
        println!("## 策略 3：filter 计数");
        let v: Vec<i64> = (0..1000).map(|i| i % 7).collect();
        println!("eq 0 的个数 = {}\n", count_eq(&v, 0));
    }
}

// ============================================================================
// 策略 4：搜索 —— 子串/锚点
// ============================================================================
/// 问题：在 byte buffer 里找模式出现位置。
/// 模式：生产直接用 `memchr` / `memmem`；自研时用 SSE 批量 eq。
///
/// HFT: hft::fix_anchor_search
/// Web3: calldata magic bytes
pub mod search {
    pub fn find_anchor(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        crate::hft::fix_anchor_search::find_tag_simd(haystack, needle)
    }

    pub fn demonstrate() {
        println!("## 策略 4：锚点搜索");
        let buf = b"tag=price\x01value=100\x01";
        println!("`price` 位置 = {:?}\n", find_anchor(buf, b"price"));
    }
}

// ============================================================================
// 策略 5：位运算块 —— XOR/AND 固定宽度
// ============================================================================
/// 问题：对固定大小块（32B/64B）做逐字节位运算。
/// 模式：AVX2 load → xor/and → store；Merkle 预处理、Bloom bit 操作。
///
/// HFT: 二进制协议 checksum 块
/// Web3: web3::merkle_layer
pub mod bitwise_block {
    pub fn xor32(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
        let mut out = [0u8; 32];
        for i in 0..32 {
            out[i] = a[i] ^ b[i];
        }
        out
    }

    pub fn demonstrate() {
        println!("## 策略 5：32B XOR 块");
        let a = [1u8; 32];
        let b = [2u8; 32];
        let c = xor32(&a, &b);
        println!("xor 首字节 = 0x{:02x}\n", c[0]);
    }
}

// ============================================================================
// 策略 6：SoA 布局 —— 结构体数组转列存
// ============================================================================
/// 问题：AoS 阻碍 SIMD load。
/// 模式：热路径字段拆成 `Vec<T>` 平行数组。
///
/// HFT: hft::batch_vwap
/// Web3: 批量 tx gas/limit 列
pub mod struct_of_arrays {
    pub struct TradesSoa {
        pub px: Vec<i64>,
        pub qty: Vec<i64>,
    }

    pub fn from_pairs(pairs: &[(i64, i64)]) -> TradesSoa {
        let mut px = Vec::with_capacity(pairs.len());
        let mut qty = Vec::with_capacity(pairs.len());
        for &(p, q) in pairs {
            px.push(p);
            qty.push(q);
        }
        TradesSoa { px, qty }
    }

    pub fn demonstrate() {
        println!("## 策略 6：SoA 布局");
        let pairs = [(100, 10), (101, 5)];
        let soa = from_pairs(&pairs);
        println!("px = {:?}, qty = {:?}\n", soa.px, soa.qty);
    }
}

// ============================================================================
// 策略 7：分桶 —— per-thread histogram
// ============================================================================
/// 问题：样本分 bucket 计数，共享数组有写冲突。
/// 模式：thread-local `[u64; N]` → 最后 reduce 合并。
///
/// HFT: hft::latency_histogram
/// Web3: gas price 分布统计
pub mod histogram {
    pub fn bucket_index(value: u64, width: u64, num_buckets: usize) -> usize {
        (value / width).min(num_buckets as u64 - 1) as usize
    }

    pub fn demonstrate() {
        println!("## 策略 7：分桶索引");
        println!(
            "sample 47 → bucket {}\n",
            bucket_index(47, 10, 64)
        );
    }
}

// ============================================================================
// 策略 8：可移植 dispatch —— 特征检测 + 回退
// ============================================================================
/// 问题：同一份二进制要在 heterogeneous CPU 集群运行。
/// 模式：运行时 detect → 函数指针/enum dispatch → 标量兜底。
///
/// HFT/Web3: 所有 intrinsics 入口的统一模式
pub mod portable_dispatch {
    pub enum SumImpl {
        Simd,
        Scalar,
    }

    pub fn pick_sum_impl() -> SumImpl {
        #[cfg(target_arch = "x86_64")]
        {
            if std::arch::is_x86_feature_detected!("avx2") {
                return SumImpl::Simd;
            }
        }
        SumImpl::Scalar
    }

    pub fn sum(data: &[i64]) -> i64 {
        crate::util::sum_i64(data)
    }

    pub fn demonstrate() {
        println!("## 策略 8：可移植 dispatch");
        let impl_name = match pick_sum_impl() {
            SumImpl::Simd => "avx2",
            SumImpl::Scalar => "scalar",
        };
        println!("选用实现 = {impl_name}");
        println!("sum(1..5) = {}\n", sum(&[1, 2, 3, 4]));
    }
}

pub fn demonstrate() {
    reduce::demonstrate();
    map::demonstrate();
    filter::demonstrate();
    search::demonstrate();
    bitwise_block::demonstrate();
    struct_of_arrays::demonstrate();
    histogram::demonstrate();
    portable_dispatch::demonstrate();
}
