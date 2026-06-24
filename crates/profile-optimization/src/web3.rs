//! # Web3 / 区块链生产场景下的性能分析与优化
//!
//! Web3 profiling 特点：
//! - **流水线分解**：RPC / bundle sim 多阶段，要 stage timer
//! - **批量 vs 单条**：mempool 过滤顺序决定 CPU 预算
//! - **deadline 感知**：超 budget 就截断，profile 指导 cutoff 点
//!
//! 下面 6 个场景对应 reth、searcher、RPC 节点的 profiling 热点。

#![allow(dead_code)]

use crate::util::StageTimer;

pub type B256 = [u8; 32];
pub type Address = [u8; 20];

fn fake_hash(input: &[u8]) -> B256 {
    let mut out = [0u8; 32];
    for (i, &b) in input.iter().enumerate().take(32) {
        out[i] = b.wrapping_mul(31).wrapping_add(i as u8);
    }
    out
}

// ============================================================================
// 场景 1：Block replay 吞吐 —— 定位 per-tx 开销
// ============================================================================
/// **生产问题**：reth 回放 1M block 要 8h，profile 显示 state commit 占 70%。
///
/// **Profiling 套路**：按 block/tx 分段计时 → 对比 batch commit vs 逐 tx flush。
pub mod block_replay_throughput {
    use super::{fake_hash, B256};
    use crate::util::StageTimer;
    #[inline(never)]
    pub fn replay_per_tx(txs: &[B256]) -> usize {
        let mut committed = 0;
        for tx in txs {
            let _ = fake_hash(tx);
            committed += 1;
        }
        committed
    }

    #[inline(never)]
    pub fn replay_batch(txs: &[B256]) -> usize {
        let mut buf = Vec::with_capacity(txs.len());
        for tx in txs {
            buf.push(fake_hash(tx));
        }
        buf.len()
    }

    pub fn demonstrate() {
        println!("## 场景 1：Block replay 吞吐");
        let txs: Vec<B256> = (0..512).map(|i| [i as u8; 32]).collect();

        let mut timer = StageTimer::new();
        let n = timer.time("per_tx", || replay_per_tx(&txs));
        let per_ns = timer.stages[0].1.as_nanos() as u64 / txs.len() as u64;

        let mut timer2 = StageTimer::new();
        let n2 = timer2.time("batch", || replay_batch(&txs));
        let batch_ns = timer2.stages[0].1.as_nanos() as u64 / txs.len() as u64;

        println!("per_tx {n} txs ≈ {per_ns}ns/tx，batch {n2} txs ≈ {batch_ns}ns/tx");
        println!("关键：stage timer 找 commit 瓶颈；profile 后再 batch/parallelize\n");
    }
}

// ============================================================================
// 场景 2：Mempool 过滤流水线 —— 廉价 filter 前置
// ============================================================================
/// **生产问题**：searcher 每秒扫 50k pending tx，CPU 打满；profile 显示
/// 先跑 heavy sim 再查 gas price。
///
/// **Profiling 套路**：各 filter stage 单独 bench → 按 ns/tx 排序重排 pipeline。
pub mod mempool_filter {
    use crate::util::bench_per_op_ns;
    #[inline(never)]
    pub fn filter_heavy_first(gas: u64, calldata: &[u8]) -> bool {
        let _ = heavy_sim(calldata);
        gas >= 20_000_000
    }

    #[inline(never)]
    pub fn filter_cheap_first(gas: u64, calldata: &[u8]) -> bool {
        if gas < 20_000_000 {
            return false;
        }
        heavy_sim(calldata)
    }

    #[inline(never)]
    fn heavy_sim(calldata: &[u8]) -> bool {
        let mut acc = 0u64;
        for &b in calldata {
            acc = acc.wrapping_add(b as u64 * 17);
        }
        acc % 2 == 0
    }

    pub fn demonstrate() {
        println!("## 场景 2：Mempool 过滤顺序");
        let cd: Vec<u8> = (0..128).map(|i| i as u8).collect();
        let low_gas = 5_000_000u64;
        let high_gas = 50_000_000u64;

        let (bad_ns, _) = bench_per_op_ns(10, 200, 1, || {
            filter_heavy_first(low_gas, &cd);
        });
        let (good_ns, _) = bench_per_op_ns(10, 200, 1, || {
            filter_cheap_first(low_gas, &cd);
        });

        println!("低 gas heavy-first ≈ {bad_ns}ns（白跑 sim）");
        println!("低 gas cheap-first ≈ {good_ns}ns（早退）");
        println!(
            "高 gas 两者等价；关键：profile 各 stage，按 cost 排序 filter\n"
        );
        let _ = filter_cheap_first(high_gas, &cd);
    }
}

// ============================================================================
// 场景 3：Merkle 层重建 —— profile 发现 O(n) 全量 rebuild
// ============================================================================
/// **生产问题**：每次 state root 全量 rebuild Merkle，profile 显示 hash 占 85%。
///
/// **Profiling 套路**：对比 full rebuild vs incremental sibling update 的 ns/node。
pub mod merkle_rebuild {
    use super::{fake_hash, B256};
    use crate::util::bench_per_op_ns;
    #[inline(never)]
    pub fn rebuild_full(leaves: &[B256]) -> B256 {
        if leaves.is_empty() {
            return [0u8; 32];
        }
        let mut layer: Vec<B256> = leaves.to_vec();
        while layer.len() > 1 {
            let mut next = Vec::with_capacity(layer.len().div_ceil(2));
            for i in (0..layer.len()).step_by(2) {
                let left = layer[i];
                let right = if i + 1 < layer.len() {
                    layer[i + 1]
                } else {
                    left
                };
                let mut combined = [0u8; 32];
                for j in 0..32 {
                    combined[j] = left[j] ^ right[j];
                }
                next.push(fake_hash(&combined));
            }
            layer = next;
        }
        layer[0]
    }

    #[inline(never)]
    pub fn update_single(root_siblings: &[B256], new_leaf: B256) -> B256 {
        let mut acc = new_leaf;
        for sib in root_siblings {
            let mut combined = [0u8; 32];
            for j in 0..32 {
                combined[j] = acc[j] ^ sib[j];
            }
            acc = fake_hash(&combined);
        }
        acc
    }

    pub fn demonstrate() {
        println!("## 场景 3：Merkle 全量 rebuild vs 增量");
        let leaves: Vec<B256> = (0..256).map(|i| [i as u8; 32]).collect();
        let siblings: Vec<B256> = (0..8).map(|i| [i as u8; 32]).collect();

        let (full_ns, _) = bench_per_op_ns(5, 50, leaves.len() as u64, || {
            rebuild_full(&leaves);
        });
        let (incr_ns, _) = bench_per_op_ns(10, 100, 1, || {
            update_single(&siblings, [99u8; 32]);
        });

        println!("全量 rebuild ≈ {full_ns}ns/leaf，单叶增量 ≈ {incr_ns}ns/update");
        println!("关键：perf 见 hash 热点 → 增量 Merkle / 缓存 sibling 路径\n");
    }
}

// ============================================================================
// 场景 4：eth_call RPC 分解 —— stage breakdown
// ============================================================================
/// **生产问题**：eth_call P99 200ms，不知道卡在 decode / state / evm。
///
/// **Profiling 套路**：StageTimer + trace span；对齐 RPC timeout budget。
pub mod rpc_eth_call_breakdown {
    use super::*;

    #[inline(never)]
    fn decode_calldata(cd: &[u8]) -> u64 {
        cd.iter().map(|&b| b as u64).sum()
    }

    #[inline(never)]
    fn load_state(key: u64) -> B256 {
        fake_hash(&key.to_le_bytes())
    }

    #[inline(never)]
    fn evm_execute(_state: B256) -> u64 {
        let mut acc = 0u64;
        for i in 0..500 {
            acc = acc.wrapping_add(i * 3);
        }
        acc
    }

    pub fn eth_call(cd: &[u8]) -> u64 {
        let mut timer = StageTimer::new();
        let sum = timer.time("decode", || decode_calldata(cd));
        let state = timer.time("state", || load_state(sum));
        let gas = timer.time("evm", || evm_execute(state));
        println!("## 场景 4：eth_call stage breakdown");
        timer.print_breakdown();
        println!("  total: {:.2?}", timer.total());
        gas
    }

    pub fn demonstrate() {
        let cd: Vec<u8> = (0..64).map(|i| i as u8).collect();
        let _ = eth_call(&cd);
        println!("关键：最大 stage 优先优化；对齐 client timeout\n");
    }
}

// ============================================================================
// 场景 5：Trie lookup cache miss —— perf stat cache-misses
// ============================================================================
/// **生产问题**：state trie random key 查询，L3 miss 高，QPS 上不去。
///
/// **Profiling 套路**：`perf stat -e cache-misses` + 对比 LRU cache 命中率。
pub mod trie_lookup_cache {
    use super::B256;
    use crate::util::bench_per_op_ns;
    use std::collections::HashMap;

    #[inline(never)]
    pub fn trie_get_random(map: &HashMap<u64, B256>, keys: &[u64]) -> usize {
        keys.iter().filter(|k| map.contains_key(k)).count()
    }

    #[inline(never)]
    pub fn trie_get_cached(cache: &mut HashMap<u64, B256>, map: &HashMap<u64, B256>, key: u64) -> bool {
        if cache.contains_key(&key) {
            return true;
        }
        if let Some(v) = map.get(&key) {
            cache.insert(key, *v);
            return true;
        }
        false
    }

    pub fn demonstrate() {
        println!("## 场景 5：Trie lookup cache");
        let map: HashMap<u64, B256> = (0..10_000).map(|i| (i, [i as u8; 32])).collect();
        let random_keys: Vec<u64> = (0..256).map(|i| (i * 37 + 13) % 10_000).collect();

        let (rand_ns, _) = bench_per_op_ns(10, 100, random_keys.len() as u64, || {
            trie_get_random(&map, &random_keys);
        });

        let mut cache = HashMap::with_capacity(256);
        let (cached_ns, _) = bench_per_op_ns(10, 100, random_keys.len() as u64, || {
            for &k in &random_keys {
                trie_get_cached(&mut cache, &map, k);
            }
        });

        println!("随机 key 扫描 ≈ {rand_ns}ns/key，LRU 热 key ≈ {cached_ns}ns/key");
        println!("关键：perf cache-misses；reth 用 MDBX + 自有 trie cache\n");
    }
}

// ============================================================================
// 场景 6：Bundle simulation deadline —— budget 内截断
// ============================================================================
/// **生产问题**：Flashbots bundle sim 超 2s 被丢弃；要在 budget 内 profile 截断。
///
/// **Profiling 套路**：记录每条 tx sim 耗时分布 → 超 P95 的 tx 类型单独优化或跳过。
pub mod bundle_simulation_budget {
    use crate::util::LatencyHistogram;

    const BUDGET_MS: u64 = 50;

    #[inline(never)]
    pub fn sim_tx(gas: u64) -> u64 {
        let mut acc = 0u64;
        let loops = (gas / 1000).min(5000);
        for i in 0..loops {
            acc = acc.wrapping_add(i);
        }
        acc
    }

    pub fn simulate_bundle(gas_list: &[u64]) -> (u64, LatencyHistogram) {
        let mut hist = LatencyHistogram::new(64, 100_000);
        let mut total_ms = 0u64;
        let mut completed = 0u64;

        for &gas in gas_list {
            let start = std::time::Instant::now();
            sim_tx(gas);
            let elapsed_ms = start.elapsed().as_millis() as u64;
            hist.record(elapsed_ms * 1_000_000);
            total_ms += elapsed_ms;
            completed += 1;
            if total_ms >= BUDGET_MS {
                break;
            }
        }
        (completed, hist)
    }

    pub fn demonstrate() {
        println!("## 场景 6：Bundle sim deadline {BUDGET_MS}ms");
        let gas_list: Vec<u64> = vec![
            21_000,
            50_000,
            100_000,
            500_000,
            1_000_000,
            2_000_000,
        ];
        let (done, hist) = simulate_bundle(&gas_list);
        println!("budget 内完成 {done}/{} txs，per-tx p99={}ms",
            gas_list.len(),
            hist.p99_ns() / 1_000_000
        );
        println!("关键：profile 每条 tx → 识别高 gas 合约；deadline 内贪心排序\n");
    }
}

pub fn demonstrate() {
    block_replay_throughput::demonstrate();
    mempool_filter::demonstrate();
    merkle_rebuild::demonstrate();
    rpc_eth_call_breakdown::demonstrate();
    trie_lookup_cache::demonstrate();
    bundle_simulation_budget::demonstrate();
}
