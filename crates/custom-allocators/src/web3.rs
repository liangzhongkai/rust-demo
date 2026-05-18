//! # Web3：RPC 体形、哈希缓冲、仿真批处理中的 **自定义生命周期内存**
//!
//! 典型负载：JSON-RPC / WS 大批量 `eth_getLogs`、默克尔证明解析、 mempool 回放。
//!
//! **`GlobalAlloc` 视角**：瞬时分配峰值 → OOM killer / GC STW（若为托管运行时侧车）；
//! **`池化`**：复用响应 buffer，可把 **峰值字节数「钉」在配置常量**。

#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::Mutex;

// =============================================================================
// 场景 A：节点 / 索引器重 JSON-RPC——`Vec<u8>` buffer 归还池（控制 RSS 峰值）
// =============================================================================
/// **生产问题**：批量 `logs`/`trace` 返回 MB 级 body，每台连接 `Vec::with_capacity`
/// 从零增长，`peak RSS` ≈ 「并发 RPC × 单次最大响应」，且分配器在多线程竞争。
///
/// **对策**：worker 处理后把 `capacity` 仍够大的 buffer **归还池**（可按大小分桶，此处单桶示意）。
pub struct ByteBufferPool {
    pool: Mutex<VecDeque<Vec<u8>>>,
    reuse_min_cap: usize,
}

impl ByteBufferPool {
    pub fn new(reuse_min_cap: usize) -> Self {
        Self {
            pool: Mutex::new(VecDeque::new()),
            reuse_min_cap,
        }
    }

    pub fn get(&self, hint: usize) -> Vec<u8> {
        if let Ok(mut q) = self.pool.lock() {
            while let Some(mut v) = q.pop_front() {
                if v.capacity() >= hint.max(self.reuse_min_cap) {
                    v.clear();
                    return v;
                }
            }
        }
        Vec::with_capacity(hint.max(self.reuse_min_cap))
    }

    pub fn put(&self, mut buf: Vec<u8>) {
        if buf.capacity() >= self.reuse_min_cap {
            buf.clear();
            if let Ok(mut q) = self.pool.lock() {
                const MAX_BUFFERS_PER_POOL: usize = 512;
                if q.len() < MAX_BUFFERS_PER_POOL {
                    q.push_back(buf);
                    return;
                }
            }
        }
        drop(buf);
    }
}

// =============================================================================
// 场景 B：ABI / RLP —— 「解析单次」用大栈数组不可行时用 **栈外预分配 slab**
// =============================================================================

/// 合约调用 `calldata` 常见上界 ~24KB；使用池化 `[u8; N]` 的 `Vec` 承载可避免热路径细碎 alloc。
///
/// （真实系统常结合 ` BytesMut`/`bump`，此处只强调 **容量重用**。）
pub fn parse_calldata_scratch(len: usize, pool: &ByteBufferPool) -> Vec<u8> {
    let mut buf = pool.get(len);
    buf.resize(len, 0);
    buf
}

// =============================================================================
// 场景 C：默克尔证明 / Trie 解压 —— 深度为 D 的证明节点句柄缓冲区
// =============================================================================

#[derive(Clone, Copy, Debug)]
pub struct TrieStep {
    pub index: u8,
    pub sibling_hash: [u8; 32],
}

/// **生产问题**：遍历证明链时每层 `Vec::push(Hash)`，`realloc` 在深度较大时虽不频繁但可测。
///
/// **对策**：`reserve_exact(depth)` **一次**，或直接使用 `tinyvec`/数组栈若深度编译期常数。
pub fn collect_proof_steps(capacity_hint: usize) -> Vec<TrieStep> {
    let mut v = Vec::new();
    v.reserve_exact(capacity_hint);
    v
}

// =============================================================================
// 场景 D：MEV/mempool dry-run —— 批内对象数上界已知：先 `reserve_exact`，再逐个 place
// =============================================================================

#[derive(Clone, Copy, Debug, Default)]
pub struct SimulatedTx {
    pub nonce: u64,
    pub gas_used: u64,
}

pub struct BatchDryRunArena {
    items: Vec<SimulatedTx>,
}

impl BatchDryRunArena {
    pub fn for_batch(batch_size: usize) -> Self {
        let mut items = Vec::new();
        items.reserve_exact(batch_size);
        Self { items }
    }

    pub fn push(&mut self, tx: SimulatedTx) {
        self.items.push(tx);
    }

    pub fn as_slice(&self) -> &[SimulatedTx] {
        &self.items
    }

    pub fn reset(&mut self) {
        self.items.clear();
        // Vec capacity is retained → 下一批无 growth.
    }

    pub fn backing_capacity(&self) -> usize {
        self.items.capacity()
    }
}

pub fn demonstrate() {
    println!("## Web3 场景 A：RPC body buffer 回收池");
    let pool = ByteBufferPool::new(4096);
    let mut buf = pool.get(20_000);
    buf.extend_from_slice(br#"{"jsonrpc":"2.0","result":[],"id":1}"#);
    let n = buf.len();
    println!("  填入 {} bytes，capacity={}", n, buf.capacity());
    pool.put(buf);

    println!("## Web3 场景 B：`parse_calldata_scratch`（容量来自池）");
    let p2 = ByteBufferPool::new(4096);
    let scratch = parse_calldata_scratch(256, &p2);
    println!("  scratch_cap={}", scratch.capacity());
    p2.put(scratch);

    println!("## Web3 场景 C：`collect_proof_steps`（一次 reserve_exact）");
    let mut steps = collect_proof_steps(128);
    steps.push(TrieStep {
        index: 0,
        sibling_hash: [0xAB; 32],
    });
    println!("  len={} cap={}", steps.len(), steps.capacity());

    println!("## Web3 场景 D：批回放 `BatchDryRunArena`");
    let mut batch = BatchDryRunArena::for_batch(4);
    batch.push(SimulatedTx {
        nonce: 1,
        gas_used: 21_000,
    });
    batch.reset();
    batch.push(SimulatedTx {
        nonce: 2,
        gas_used: 65_000,
    });
    println!(
        "  重置后capacity保留 => cap={}",
        batch.backing_capacity()
    );
}
