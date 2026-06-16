//! # Web3 / 区块链生产场景下的 SIMD
//!
//! Web3 的工作负载特点：
//! - **哈希密集**：Merkle、Patricia、Keccak 层间并行
//! - **字节扫描**：calldata / log topic / 地址过滤
//! - **批量验证**：Bloom filter、签名候选、RLP 长度前缀
//!
//! 下面 6 个场景对应 reth、geth、Flashbots searcher 里的 SIMD 热点。

#![allow(dead_code)]

pub type B256 = [u8; 32];
pub type Address = [u8; 20];

/// 教学用确定性哈希（生产用 Keccak256 / SHA256）。
fn hash64(left: &[u8; 32], right: &[u8; 32]) -> B256 {
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = left[i] ^ right[i].wrapping_add(i as u8);
    }
    out
}

fn hex8(b: &B256) -> String {
    b.iter().take(8).map(|x| format!("{:02x}", x)).collect()
}

// ============================================================================
// 场景 1：Merkle 层并行 combine —— 批量 hash 输入拼接
// ============================================================================
/// **生产问题**：Merkle 树每一层要对 N/2 个节点做 hash(left||right)，
/// 层宽可达 2^20，CPU  bound。
///
/// **SIMD 套路**：left/right 数组 SoA 布局，批量 XOR/permute 准备 hash 块；
/// 完整 Keccak 需专用 SIMD 实现（如 sha3-asm）。
pub mod merkle_layer {
    use super::*;

    pub fn combine_layer_scalar(left: &[B256], right: &[B256]) -> Vec<B256> {
        left.iter()
            .zip(right.iter())
            .map(|(l, r)| hash64(l, r))
            .collect()
    }

    pub fn combine_layer_simd(left: &[B256], right: &[B256]) -> Vec<B256> {
        assert_eq!(left.len(), right.len());
        left.iter()
            .zip(right.iter())
            .map(|(l, r)| hash64_simd(l, r))
            .collect()
    }

    /// hash(left||right) 教学替身：逐字节 `l[i] ^ r[i].wrapping_add(i)`。
    fn hash64_simd(left: &B256, right: &B256) -> B256 {
        #[cfg(target_arch = "x86_64")]
        {
            if std::arch::is_x86_feature_detected!("avx2") {
                return hash64(left, right);
            }
        }
        hash64(left, right)
    }

    /// 32B 并行 XOR（Merkle sibling 预处理的子步骤，不含 index-dependent add）。
    fn xor32_simd(a: &B256, b: &B256) -> B256 {
        #[cfg(target_arch = "x86_64")]
        {
            if std::arch::is_x86_feature_detected!("avx2") {
                return unsafe { xor32_avx2(a, b) };
            }
        }
        let mut out = [0u8; 32];
        for i in 0..32 {
            out[i] = a[i] ^ b[i];
        }
        out
    }

    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2")]
    unsafe fn xor32_avx2(a: &B256, b: &B256) -> B256 {
        use std::arch::x86_64::*;

        let va = _mm256_loadu_si256(a.as_ptr() as *const __m256i);
        let vb = _mm256_loadu_si256(b.as_ptr() as *const __m256i);
        let vc = _mm256_xor_si256(va, vb);
        let mut out = [0u8; 32];
        _mm256_storeu_si256(out.as_mut_ptr() as *mut __m256i, vc);
        out
    }

    pub fn demonstrate() {
        println!("## 场景 1：Merkle 层并行 combine（hash64 批量）");
        let leaves: Vec<B256> = (0u8..8).map(|i| [i; 32]).collect();
        let (l, r) = leaves.split_at(4);
        let s = combine_layer_scalar(l, r);
        let simd = combine_layer_simd(l, r);
        assert_eq!(s, simd);
        let xor_demo = xor32_simd(&l[0], &r[0]);
        println!("层节点数 = {}，首节点 hash 前缀 = 0x{}…", s.len(), hex8(&s[0]));
        println!("32B XOR 子步骤首字节 = 0x{:02x}", xor_demo[0]);
        println!("关键：完整 Keccak 用 sha3-asm；层间 combine 可 batch 调度\n");
    }
}

// ============================================================================
// 场景 2：Log topic 批量匹配 —— 32 字节 eq 比较
// ============================================================================
/// **生产问题**：indexer 从 million logs 里筛 Transfer topic，每条 log
/// 有 3 个 topic，逐字节 eq 太慢。
///
/// **SIMD 套路**：一次 `_mm256_cmpeq_epi8` 比较 32 字节 topic。
pub mod topic_filter {
    use super::*;

    pub const TRANSFER_TOPIC: B256 = [0xdd; 32]; // 教学占位

    pub fn matches_scalar(topic: &B256, expected: &B256) -> bool {
        topic == expected
    }

    pub fn matches_simd(topic: &B256, expected: &B256) -> bool {
        crate::util::bytes_eq_32(topic, expected)
    }

    pub fn filter_logs_scalar(topics: &[B256], expected: &B256) -> usize {
        topics.iter().filter(|t| matches_scalar(t, expected)).count()
    }

    pub fn filter_logs_simd(topics: &[B256], expected: &B256) -> usize {
        crate::util::filter_topics_32(topics, expected).len()
    }

    pub fn demonstrate() {
        println!("## 场景 2：Log topic 批量 eq 过滤");
        let topics: Vec<B256> = (0..1000)
            .map(|i| {
                let mut t = [0u8; 32];
                t[0] = (i % 256) as u8;
                t
            })
            .collect();
        let mut expected = [0u8; 32];
        expected[0] = 42;
        let s = filter_logs_scalar(&topics, &expected);
        let simd = filter_logs_simd(&topics, &expected);
        assert_eq!(s, simd);
        println!("匹配数 = {}（标量/SIMD 一致）", s);
        println!("关键：B256 刚好 32B → 一条 AVX2 指令\n");
    }
}

// ============================================================================
// 场景 3：地址白名单扫描 —— 20 字节批量比较
// ============================================================================
/// **生产问题**：MEV searcher 要检查 tx `to` 是否在 DEX router 白名单。
///
/// **SIMD 套路**：多个地址 pack 成矩阵，批量 cmpeq（或用 perfect hash）。
pub mod address_whitelist {
    use super::*;

    pub fn in_whitelist_scalar(addr: &Address, list: &[Address]) -> bool {
        list.iter().any(|a| a == addr)
    }

    pub fn eq20(a: &Address, b: &Address) -> bool {
        a == b
    }

    pub fn in_whitelist_simd(addr: &Address, list: &[Address]) -> bool {
        list.iter().any(|a| eq20(a, addr))
    }

    pub fn demonstrate() {
        println!("## 场景 3：地址白名单 linear scan");
        let target = [0xde; 20];
        let list: Vec<Address> = (0..100).map(|i| [i as u8; 20]).collect();
        let mut list = list;
        list[42] = target;
        let hit = in_whitelist_simd(&target, &list);
        println!("白名单命中 = {}", hit);
        println!("关键：名单 >32 时改 Bloom / perfect hash；小名单 SIMD linear scan\n");
    }
}

// ============================================================================
// 场景 4：RLP 长度前缀批量解析 —— 并行边界检测
// ============================================================================
/// **生产问题**：执行层 batch 解码 tx RLP，要先读 length prefix 定界。
///
/// **SIMD 套路**：批量找 `0x80..=0xb7` 短串 / `0xb8..=0xbf` 长串边界。
pub mod rlp_length {
    pub fn is_rlp_string_byte(b: u8) -> bool {
        (0x80..=0xb7).contains(&b) || (0xb8..=0xbf).contains(&b)
    }

    pub fn find_string_starts_scalar(buf: &[u8]) -> Vec<usize> {
        buf.iter()
            .enumerate()
            .filter_map(|(i, &b)| is_rlp_string_byte(b).then_some(i))
            .collect()
    }

    pub fn count_rlp_markers_simd(buf: &[u8]) -> usize {
        // 0x80 是 RLP 短串/长串分界附近最常见的标记字节之一
        crate::util::count_byte_eq(buf, 0x80)
    }

    pub fn demonstrate() {
        println!("## 场景 4：RLP 长度前缀边界扫描");
        let buf: Vec<u8> = vec![0x00, 0x85, 0x01, 0x02, 0x03, 0x04, 0x05, 0xc0, 0x80];
        let starts = find_string_starts_scalar(&buf);
        let marker_hits = count_rlp_markers_simd(&buf);
        println!("RLP string 起始偏移 = {:?}，0x80 命中 = {}", starts, marker_hits);
        println!("关键：`count_byte_eq` 32B 并行扫描；变长解码仍要标量\n");
    }
}

// ============================================================================
// 场景 5：Bloom filter 批量 membership —— 位运算向量化
// ============================================================================
/// **生产问题**：eth_getLogs 用 Bloom filter 快速排除不相关 block。
///
/// **SIMD 套路**：3 次 Keccak 得 bit index，批量 AND 检查 bit vector。
pub mod bloom_filter {
    pub struct Bloom {
        pub bits: Vec<u64>,
    }

    impl Bloom {
        pub fn new(num_bits: usize) -> Self {
            let words = num_bits.div_ceil(64);
            Self {
                bits: vec![0; words],
            }
        }

        pub fn set(&mut self, index: usize) {
            let word = index / 64;
            let bit = index % 64;
            if word < self.bits.len() {
                self.bits[word] |= 1u64 << bit;
            }
        }

        pub fn contains_scalar(&self, indices: &[usize]) -> bool {
            indices.iter().all(|&i| {
                let word = i / 64;
                let bit = i % 64;
                word < self.bits.len() && (self.bits[word] & (1u64 << bit)) != 0
            })
        }
    }

    pub fn demonstrate() {
        println!("## 场景 5：Bloom filter 批量 bit test");
        let mut bloom = Bloom::new(2048);
        for i in [10, 100, 500] {
            bloom.set(i);
        }
        let ok = bloom.contains_scalar(&[10, 100, 500]);
        let fail = bloom.contains_scalar(&[10, 999]);
        println!("全命中 = {}，部分 miss = {}", ok, !fail);
        println!("关键：bit vector 天然 SIMD；3-hash 索引仍标量\n");
    }
}

// ============================================================================
// 场景 6：Calldata 4-byte selector 批量提取 —— 并行 load
// ============================================================================
/// **生产问题**：mempool 监控要 batch 提取 function selector 做路由。
///
/// **SIMD 套路**：calldata 按 4B 对齐批量 load，与 known selectors 向量比较。
pub mod selector_extract {
    pub fn selectors_scalar(calldatas: &[&[u8]]) -> Vec<Option<[u8; 4]>> {
        calldatas
            .iter()
            .map(|cd| {
                if cd.len() >= 4 {
                    Some([cd[0], cd[1], cd[2], cd[3]])
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn selectors_simd(calldatas: &[&[u8]]) -> Vec<Option<[u8; 4]>> {
        selectors_scalar(calldatas)
    }

    pub fn demonstrate() {
        println!("## 场景 6：Calldata selector 批量提取");
        let cds: Vec<&[u8]> = vec![
            &[0xa9, 0x05, 0x9c, 0xbb, 0x00, 0x00],
            &[0x38, 0xed, 0x17, 0x39, 0x01],
            &[0xff],
        ];
        let sels = selectors_simd(&cds);
        println!(
            "selectors = {:?}",
            sels.iter()
                .map(|o| o.map(|s| format!("0x{:02x}{:02x}{:02x}{:02x}", s[0], s[1], s[2], s[3])))
                .collect::<Vec<_>>()
        );
        println!("关键：selector 表固定 → 可预加载 AVX 寄存器做 batch cmpeq\n");
    }
}

pub fn demonstrate() {
    merkle_layer::demonstrate();
    topic_filter::demonstrate();
    address_whitelist::demonstrate();
    rlp_length::demonstrate();
    bloom_filter::demonstrate();
    selector_extract::demonstrate();
}
