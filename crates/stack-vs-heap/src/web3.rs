//! # Web3 生产场景下的栈 vs 堆
//!
//! 区块链节点的硬约束：
//! - **吞吐**：mempool 过滤 / block replay 每秒数万 tx，alloc 是头号敌人
//! - **内存**：全节点 state trie 必须 GB 级堆；但 *热路径* 仍应栈/预分配
//! - **确定性**：相同输入相同 gas/memory —— 堆分配次数影响 benchmark 可复现性
//!
//! 下面 6 个场景是以太坊类系统的典型内存布局决策。

#![allow(dead_code)]

use crate::util::{AllocCounter, BumpArena, InlineBuffer};

pub type Hash32 = [u8; 32];
pub type Address = [u8; 20];

// ============================================================================
// 场景 1：Hash / Address —— 栈上 [u8; N]，拒绝 Vec<u8>
// ============================================================================
/// **生产问题**：用 `Vec<u8>` 存 32-byte hash，每次比较多一次指针解引用，
/// HashMap key 多一次堆 indirection，trie cache 命中率下降。
///
/// **栈/堆套路**：`type Hash32 = [u8; 32]` —— Copy、Eq、可直接做 map key。
pub mod fixed_hash {
    use super::*;

    pub fn hash_eq_stack(a: Hash32, b: Hash32) -> bool {
        a == b
    }

    pub fn hash_eq_heap(a: &Vec<u8>, b: &Vec<u8>) -> bool {
        a == b
    }

    pub fn demonstrate() {
        println!("## 场景 1：Hash32 栈数组 vs Vec<u8>");

        let h1 = [0xabu8; 32];
        let h2 = h1;
        println!("  stack Copy compare: {}", hash_eq_stack(h1, h2));
        println!("  size Hash32 = {} bytes (栈)", std::mem::size_of::<Hash32>());
        println!("  size Vec<u8> = {} bytes (指针+len+cap)", std::mem::size_of::<Vec<u8>>());
        println!("  关键：定长 digest 永远用 [u8; N]；只在 RPC JSON 边界转 hex String\n");
    }
}

// ============================================================================
// 场景 2：Calldata 解码 —— 借用输入 buffer
// ============================================================================
/// **生产问题**：mempool 过滤对每笔 tx `decode()` 成 struct + 多个 `String` 字段，
/// 10k tx/s 节点 CPU 30% 耗在 alloc。
///
/// **栈/堆套路**：decoder 返回 `&[u8]` slice 指向原始 calldata；selector 用 `[u8;4]`。
pub mod calldata_decode {
    use super::*;

    pub const TRANSFER_SELECTOR: [u8; 4] = [0xa9, 0x05, 0x9c, 0xbb]; // transfer(address,uint256)

    #[derive(Debug, Clone, Copy)]
    pub struct TransferCall<'a> {
        pub selector: [u8; 4],
        pub to: Address,
        pub amount: u128,
        pub raw: &'a [u8],
    }

    pub fn decode_transfer(calldata: &[u8]) -> Option<TransferCall<'_>> {
        if calldata.len() < 4 + 32 + 32 {
            return None;
        }
        let selector: [u8; 4] = calldata[0..4].try_into().ok()?;
        if selector != TRANSFER_SELECTOR {
            return None;
        }
        let mut to = [0u8; 20];
        to.copy_from_slice(&calldata[4 + 12..4 + 32]);
        let amount = u128::from_be_bytes(calldata[36..52].try_into().ok()?);
        Some(TransferCall {
            selector,
            to,
            amount,
            raw: calldata,
        })
    }

    pub fn decode_transfer_slow(calldata: &[u8]) -> Option<(String, String)> {
        let t = decode_transfer(calldata)?;
        Some((
            format!("0x{}", hex_encode(&t.to)),
            format!("{}", t.amount),
        ))
    }

    fn hex_encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn demonstrate() {
        println!("## 场景 2：ERC20 transfer calldata 解码");

        let mut cd = vec![0u8; 68];
        cd[0..4].copy_from_slice(&TRANSFER_SELECTOR);
        cd[4 + 12..4 + 32].copy_from_slice(&[0x11u8; 20]);
        cd[36..52].copy_from_slice(&1000u128.to_be_bytes());

        let decoded = decode_transfer(&cd).unwrap();
        println!("  零堆 decode: amount={}", decoded.amount);

        let mut counter = AllocCounter::default();
        for _ in 0..500 {
            let _ = decode_transfer_slow(&cd);
            counter.allocs += 1; // 每次至少 2 个 String
        }
        println!("  hex String 路径 500 次: ~{} alloc 事件", counter.allocs);
        println!("  关键：filter 阶段只 Copy+借用；hex 只在 JSON-RPC 响应时做\n");
    }
}

// ============================================================================
// 场景 3：Mempool 过滤 —— 栈上条件 struct
// ============================================================================
/// **生产问题**：过滤器配置从 JSON 加载后每笔 tx 重新 `parse` 成 `HashMap`。
///
/// **栈/堆套路**：启动时解析成 `FilterCriteria` Copy struct，热路径只读栈。
pub mod mempool_filter {
    use super::*;

    #[derive(Debug, Clone, Copy)]
    pub struct FilterCriteria {
        pub min_gas_price: u64,
        pub max_calldata_len: u16,
        pub allowed_to: Option<Address>,
    }

    pub fn passes(c: FilterCriteria, gas_price: u64, calldata_len: usize, to: Address) -> bool {
        if gas_price < c.min_gas_price {
            return false;
        }
        if calldata_len > c.max_calldata_len as usize {
            return false;
        }
        if let Some(allowed) = c.allowed_to {
            if to != allowed {
                return false;
            }
        }
        true
    }

    pub fn passes_slow(
        rules: &std::collections::HashMap<String, String>,
        gas_price: u64,
    ) -> bool {
        let min = rules.get("min_gas_price").and_then(|s| s.parse().ok()).unwrap_or(0);
        gas_price >= min
    }

    pub fn demonstrate() {
        println!("## 场景 3：Mempool 过滤条件（Copy struct vs HashMap）");

        let criteria = FilterCriteria {
            min_gas_price: 1_000_000_000,
            max_calldata_len: 10_000,
            allowed_to: None,
        };
        let to = [0u8; 20];
        println!(
            "  passes = {}",
            passes(criteria, 2_000_000_000, 100, to)
        );
        println!("  FilterCriteria size = {} bytes (栈)", std::mem::size_of::<FilterCriteria>());
        println!("  关键：配置解析一次；热路径零堆 lookup\n");
    }
}

// ============================================================================
// 场景 4：Merkle proof 验证 —— 栈上 proof 数组
// ============================================================================
/// **生产问题**：Merkle proof 深度通常 ≤ 32，用 `Vec<Hash32>` 每 proof 一次 alloc。
///
/// **栈/堆套路**：`[(Hash32, bool); 32]` + depth 计数，或 `[Hash32; 32]` + len。
pub mod merkle_proof {
    use super::*;

    pub const MAX_DEPTH: usize = 32;

    #[derive(Debug, Clone, Copy)]
    pub struct MerkleProof {
        pub siblings: [Hash32; MAX_DEPTH],
        pub path_bits: u32, // bit i = 0 left, 1 right
        pub depth: u8,
    }

    pub fn verify(mut hash: Hash32, proof: MerkleProof) -> Hash32 {
        for i in 0..proof.depth as usize {
            let sibling = proof.siblings[i];
            let right = (proof.path_bits >> i) & 1 == 1;
            hash = if right {
                keccak_pair(&sibling, &hash)
            } else {
                keccak_pair(&hash, &sibling)
            };
        }
        hash
    }

    fn keccak_pair(a: &Hash32, b: &Hash32) -> Hash32 {
        // 教学替身：真实环境用 keccak256(a||b)
        let mut out = [0u8; 32];
        for i in 0..32 {
            out[i] = a[i] ^ b[i];
        }
        out
    }

    pub fn verify_heap(mut hash: Hash32, siblings: &[Hash32], path_bits: u32) -> Hash32 {
        for (i, sibling) in siblings.iter().enumerate() {
            let right = (path_bits >> i) & 1 == 1;
            hash = if right {
                keccak_pair(sibling, &hash)
            } else {
                keccak_pair(&hash, sibling)
            };
        }
        hash
    }

    pub fn demonstrate() {
        println!("## 场景 4：Merkle proof（栈数组 vs Vec siblings）");

        let leaf = [1u8; 32];
        let proof = MerkleProof {
            siblings: [[2u8; 32]; MAX_DEPTH],
            path_bits: 0b101,
            depth: 3,
        };
        let root_stack = verify(leaf, proof);
        let siblings_vec: Vec<Hash32> = proof.siblings[..3].to_vec();
        let root_heap = verify_heap(leaf, &siblings_vec, proof.path_bits);
        println!("  stack root[0..4] = {:02x?}", &root_stack[..4]);
        println!("  heap  root[0..4] = {:02x?}", &root_heap[..4]);
        println!(
            "  MerkleProof size = {} bytes (栈上)",
            std::mem::size_of::<MerkleProof>()
        );
        println!("  关键：depth 有界 → 栈数组；Vec 留给动态深度 RPC 输入\n");
    }
}

// ============================================================================
// 场景 5：Block header —— 全 Copy 栈 struct
// ============================================================================
/// **生产问题**：Block header 字段用 `String` 存 hex hash，replay 时反复 parse/format。
///
/// **栈/堆套路**：`BlockHeader` 全部 Copy 字段；只在 logging 边界 format。
pub mod block_header {
    use super::*;

    #[derive(Debug, Clone, Copy)]
    pub struct BlockHeader {
        pub number: u64,
        pub timestamp: u64,
        pub gas_used: u64,
        pub gas_limit: u64,
        pub parent_hash: Hash32,
        pub state_root: Hash32,
    }

    impl BlockHeader {
        pub fn base_fee(&self) -> u64 {
            // EIP-1559 简化
            self.gas_limit / 2
        }
    }

    pub fn process_headers(headers: &[BlockHeader]) -> u64 {
        headers.iter().map(|h| h.gas_used).sum()
    }

    pub fn demonstrate() {
        println!("## 场景 5：Block header（全 Copy 栈 struct）");

        let headers = [
            BlockHeader {
                number: 1,
                timestamp: 1_700_000_000,
                gas_used: 15_000_000,
                gas_limit: 30_000_000,
                parent_hash: [0u8; 32],
                state_root: [1u8; 32],
            },
            BlockHeader {
                number: 2,
                timestamp: 1_700_000_012,
                gas_used: 20_000_000,
                gas_limit: 30_000_000,
                parent_hash: [0u8; 32],
                state_root: [2u8; 32],
            },
        ];
        println!("  total gas_used = {}", process_headers(&headers));
        println!(
            "  BlockHeader size = {} bytes",
            std::mem::size_of::<BlockHeader>()
        );
        println!("  关键：replay 热路径传 &[BlockHeader]；hex 只在 RPC 层\n");
    }
}

// ============================================================================
// 场景 6：Block replay scratch —— Bump arena 请求级堆
// ============================================================================
/// **生产问题**：replay 一个 block 要临时存数百个 decode 中间态，
/// 每笔 tx `Vec::new()` 导致 allocator 碎片；block 结束应一次性释放。
///
/// **栈/堆套路**：block 级 `BumpArena`：热路径小对象从 arena 切，block 结束 `reset()`。
pub mod block_arena {
    use super::*;

    pub struct TxScratch {
        pub calldata: &'static [u8], // 教学：真实代码用 arena 生命周期
        pub logs: InlineBuffer<Hash32, 4>,
    }

    pub fn replay_block(tx_count: usize) -> (u64, usize) {
        let mut arena = BumpArena::new(4096);
        let mut total_gas = 0u64;

        for i in 0..tx_count {
            let calldata_len = 68 + (i % 10) * 32;
            let slice = arena.alloc_bytes(calldata_len);
            slice[0] = i as u8;
            total_gas += calldata_len as u64 * 16;

            let mut logs = InlineBuffer::<Hash32, 4>::default();
            logs.push([i as u8; 32]);
        }

        let chunks = arena.chunk_count();
        arena.reset();
        (total_gas, chunks)
    }

    pub fn replay_naive(tx_count: usize) -> (u64, u64) {
        let mut allocs = 0u64;
        let mut total_gas = 0u64;
        for i in 0..tx_count {
            let mut cd = Vec::new();
            let len = 68 + (i % 10) * 32;
            cd.resize(len, 0);
            allocs += 1;
            total_gas += len as u64 * 16;
            let _ = cd;
        }
        (total_gas, allocs)
    }

    pub fn demonstrate() {
        println!("## 场景 6：Block replay scratch（Bump arena vs 逐 tx Vec）");

        let n = 200;
        let (gas_a, chunks) = replay_block(n);
        let (gas_n, allocs) = replay_naive(n);
        assert_eq!(gas_a, gas_n);
        println!("  arena: {} txs, {} chunk(s), reset 后零残留", n, chunks);
        println!("  naive: {} txs, {} 独立 Vec alloc", n, allocs);
        println!("  关键：请求/block 级 arena 摊销 malloc；见 arena-allocators crate\n");
    }
}

pub fn demonstrate() {
    fixed_hash::demonstrate();
    calldata_decode::demonstrate();
    mempool_filter::demonstrate();
    merkle_proof::demonstrate();
    block_header::demonstrate();
    block_arena::demonstrate();
}
