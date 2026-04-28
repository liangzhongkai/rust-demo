//! # Web3 / 区块链生产场景下的迭代器
//!
//! Web3 的工作负载是 *大量顺序处理 + 树状聚合*：
//! - 扫一段区块范围、过滤事件、解码 calldata
//! - 把交易归到 Merkle/Patricia 树
//! - 模拟 MEV bundle / mempool 排序
//!
//! 这恰好是迭代器最擅长的形态：**惰性、可组合、可短路**。
//! 下面 6 个场景都对应真实工具（如 reth、ethers-rs、Flashbots searcher）里的写法。

#![allow(dead_code)]

pub type Address = [u8; 20];
pub type B256 = [u8; 32];
pub type U256 = u128; // 教学用，生产里是 ruint::aliases::U256

/// 教学用确定性哈希：把任意 byte 串映射到 32 字节。
/// **生产里请用 `sha3::Keccak256`**；这里为了零依赖自包含。
fn keccak_like(bytes: &[u8]) -> B256 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut out = [0u8; 32];
    for (i, chunk) in bytes.chunks(32).enumerate() {
        let mut h = DefaultHasher::new();
        i.hash(&mut h);
        chunk.hash(&mut h);
        let v = h.finish().to_le_bytes();
        for (j, b) in v.iter().enumerate() {
            out[(i * 8 + j) % 32] ^= *b;
        }
    }
    // 至少做一轮全局混淆，避免空输入返回全 0
    let mut h = DefaultHasher::new();
    out.hash(&mut h);
    out[..8].copy_from_slice(&h.finish().to_le_bytes());
    out
}

fn hex8(b: &B256) -> String {
    b.iter().take(8).map(|x| format!("{:02x}", x)).collect()
}

// ============================================================================
// 场景 1：Merkle 根计算（chunks(2) + fold）
// ============================================================================
/// **生产问题**：以太坊 receipts/transactions root、空投白名单、L2 状态承诺
/// 都需要从一堆叶子算 Merkle 根。叶子数量任意，奇数时要 *复制最后一个* 配对。
///
/// **迭代器套路**：`chunks(2).map(combine).collect()` 一层层向上塌缩，
/// 用 `loop` 套 `chunks` 直到只剩一个节点。
pub mod merkle_root {
    use super::*;

    /// 把任意叶子折叠成 Merkle 根。
    pub fn merkle_root(leaves: &[B256]) -> B256 {
        if leaves.is_empty() {
            return [0u8; 32];
        }
        let mut layer: Vec<B256> = leaves.to_vec();
        while layer.len() > 1 {
            layer = layer
                .chunks(2)
                .map(|pair| {
                    let (l, r) = match pair {
                        [a, b] => (a, b),
                        [a] => (a, a), // 奇数：最后一个自配对（OpenZeppelin 风格）
                        _ => unreachable!(),
                    };
                    let mut buf = [0u8; 64];
                    buf[..32].copy_from_slice(l);
                    buf[32..].copy_from_slice(r);
                    keccak_like(&buf)
                })
                .collect();
        }
        layer[0]
    }

    /// 同时生成 Merkle proof：给定 index，返回从叶子到根的兄弟节点列表。
    pub fn merkle_proof(leaves: &[B256], mut idx: usize) -> Vec<B256> {
        let mut proof = Vec::new();
        let mut layer: Vec<B256> = leaves.to_vec();
        while layer.len() > 1 {
            let sibling_idx = idx ^ 1;
            let sib = if sibling_idx < layer.len() { layer[sibling_idx] } else { layer[idx] };
            proof.push(sib);
            layer = layer
                .chunks(2)
                .map(|pair| {
                    let (l, r) = match pair {
                        [a, b] => (a, b),
                        [a] => (a, a),
                        _ => unreachable!(),
                    };
                    let mut buf = [0u8; 64];
                    buf[..32].copy_from_slice(l);
                    buf[32..].copy_from_slice(r);
                    keccak_like(&buf)
                })
                .collect();
            idx /= 2;
        }
        proof
    }

    pub fn demonstrate() {
        println!("## 场景 1：Merkle 根 + 证明（chunks(2).fold 塌缩）");
        let leaves: Vec<B256> = (0u8..7).map(|i| keccak_like(&[i])).collect();
        let root = merkle_root(&leaves);
        let proof = merkle_proof(&leaves, 3);
        println!("叶子数 = {}，root = 0x{}…", leaves.len(), hex8(&root));
        println!("proof[3] 长度 = {}（log2 ⌈n⌉）", proof.len());
        println!("关键：每一层都是 `chunks(2).map(hash).collect`，结构性递归\n");
    }
}

// ============================================================================
// 场景 2：ERC20 Transfer 事件扫描管道
// ============================================================================
/// **生产问题**：indexer / 监控服务要从一段区块里筛出某地址的 ERC20 转账，
/// 要求：低延迟、零分配优先、可组合（topic 过滤、合约白名单、面值阈值）。
///
/// **迭代器套路**：`filter_map` 把「过滤 + 解码」合二为一；多层 filter
/// 在编译期被 fuse 成一个 closure，等价手写 if 链。
pub mod erc20_scanner {
    use super::*;

    /// ERC20 Transfer(address indexed from, address indexed to, uint256 value)
    /// topic0 = keccak256("Transfer(address,address,uint256)")
    pub const TRANSFER_TOPIC0: B256 = {
        let mut t = [0u8; 32];
        // 真实值: 0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef
        t[0] = 0xdd;
        t[1] = 0xf2;
        t[2] = 0x52;
        t[3] = 0xad;
        t
    };

    #[derive(Debug, Clone)]
    pub struct Log {
        pub address: Address,
        pub topics: Vec<B256>,
        pub data: Vec<u8>,
        pub block_number: u64,
    }

    #[derive(Debug)]
    pub struct Transfer {
        pub token: Address,
        pub from: Address,
        pub to: Address,
        pub value: U256,
        pub block: u64,
    }

    fn topic_to_address(t: &B256) -> Address {
        let mut a = [0u8; 20];
        a.copy_from_slice(&t[12..32]);
        a
    }

    /// 把任意 log 流变成解码后的 Transfer 流。生命周期自动追踪。
    pub fn transfers<'a, I>(logs: I, watched: &'a Address) -> impl Iterator<Item = Transfer> + 'a
    where
        I: Iterator<Item = Log> + 'a,
    {
        logs.filter(|l| l.topics.len() == 3)
            .filter(|l| l.topics[0] == TRANSFER_TOPIC0)
            .filter_map(move |l| {
                let from = topic_to_address(&l.topics[1]);
                let to = topic_to_address(&l.topics[2]);
                if &from != watched && &to != watched {
                    return None; // 提前丢弃，避免后续解码开销
                }
                if l.data.len() < 32 {
                    return None;
                }
                let mut v = 0u128;
                for &b in &l.data[16..32] {
                    v = (v << 8) | b as u128;
                }
                Some(Transfer { token: l.address, from, to, value: v, block: l.block_number })
            })
    }

    pub fn demonstrate() {
        println!("## 场景 2：ERC20 Transfer 扫描（filter_map 管道）");

        let me: Address = [0xab; 20];
        let other: Address = [0xcd; 20];
        let token: Address = [0xee; 20];

        let mut topic_from = [0u8; 32];
        topic_from[12..].copy_from_slice(&me);
        let mut topic_to = [0u8; 32];
        topic_to[12..].copy_from_slice(&other);

        let mut data = vec![0u8; 32];
        data[28] = 0x03;
        data[29] = 0xe8; // 1000

        let logs = vec![
            Log {
                address: token,
                topics: vec![TRANSFER_TOPIC0, topic_from, topic_to],
                data: data.clone(),
                block_number: 100,
            },
            // 不相关的事件，会被 topic0 过滤掉
            Log {
                address: token,
                topics: vec![[0xaa; 32], topic_from, topic_to],
                data: data.clone(),
                block_number: 101,
            },
            // 与 me 无关的转账，会被 watched 过滤
            Log {
                address: token,
                topics: vec![TRANSFER_TOPIC0, [0x11; 32], [0x22; 32]],
                data,
                block_number: 102,
            },
        ];

        let txs: Vec<_> = transfers(logs.into_iter(), &me).collect();
        println!("匹配到 {} 笔与 me 相关的转账", txs.len());
        for t in &txs {
            println!("  block={} value={} from=0x{:02x}.. to=0x{:02x}..",
                t.block, t.value, t.from[0], t.to[0]);
        }
        println!("关键：filter 提前终止昂贵的 data 解码 = 廉价 short-circuit\n");
    }
}

// ============================================================================
// 场景 3：MEV 三明治模拟（scan 状态演进）
// ============================================================================
/// **生产问题**：searcher 在 mempool 看到一笔大 swap，要 *先于* victim 买入、
/// 同区块 *后于* victim 卖出，估算利润是否覆盖 gas。需要按顺序模拟 3 笔 swap
/// 对 AMM 池的影响（恒积公式）。
///
/// **迭代器套路**：`scan` 是「带状态的 map」—— 把 (买/受害者/卖) 三笔操作
/// 顺次施加到池子状态，每步都产出该步执行后的快照。
pub mod sandwich_sim {
    /// Uniswap V2 风格 x*y=k，0.3% 手续费
    #[derive(Debug, Clone, Copy)]
    pub struct Pool {
        pub reserve_in: u128,
        pub reserve_out: u128,
    }

    impl Pool {
        /// 输入 amount_in token0，吐出 token1，更新 reserves
        pub fn swap_in_for_out(&mut self, amount_in: u128) -> u128 {
            let fee_num = 997u128;
            let fee_den = 1000u128;
            let amt_with_fee = amount_in * fee_num;
            let num = amt_with_fee * self.reserve_out;
            let den = self.reserve_in * fee_den + amt_with_fee;
            let out = num / den;
            self.reserve_in += amount_in;
            self.reserve_out -= out;
            out
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub enum Action {
        Frontrun(u128),
        Victim(u128),
        Backrun, // 把 frontrun 买到的 token 全卖回
    }

    pub fn simulate(start: Pool, frontrun: u128, victim: u128) -> i128 {
        let actions = [Action::Frontrun(frontrun), Action::Victim(victim), Action::Backrun];

        // scan：累加器是 (pool, my_holding)
        // map 出每步的「我手上的 out token 数量」，最终 last 即为 backrun 后剩余
        let last = actions
            .iter()
            .scan((start, 0u128), |state, action| {
                let (pool, holding) = state;
                match action {
                    Action::Frontrun(amt) => {
                        let got = pool.swap_in_for_out(*amt);
                        *holding += got;
                    }
                    Action::Victim(amt) => {
                        let _ = pool.swap_in_for_out(*amt);
                    }
                    Action::Backrun => {
                        // 反向 swap：用 holding 的 out 换回 in
                        let mut reverse = Pool {
                            reserve_in: pool.reserve_out,
                            reserve_out: pool.reserve_in,
                        };
                        let got_in = reverse.swap_in_for_out(*holding);
                        // 回写 reserves
                        pool.reserve_in = reverse.reserve_out;
                        pool.reserve_out = reverse.reserve_in;
                        // 我的最终收益用 i128 存（可能为负）
                        return Some(got_in as i128 - frontrun as i128);
                    }
                }
                Some(0)
            })
            .last()
            .unwrap_or(0);
        last
    }

    pub fn demonstrate() {
        println!("## 场景 3：MEV 三明治模拟（scan 状态机）");

        let pool = Pool { reserve_in: 1_000_000_000, reserve_out: 1_000_000_000 };
        let pnl = simulate(pool, 50_000_000, 200_000_000);
        println!("Frontrun 5e7, Victim 2e8 → 净利润 = {} (in-token unit)", pnl);
        println!("关键：scan 把 *顺序依赖* 的多步操作串成纯函数式管道\n");
    }
}

// ============================================================================
// 场景 4：Mempool gas 拍卖 + 出块约束
// ============================================================================
/// **生产问题**：builder 要从 mempool 选出一组 tx 装进区块，约束 gas <= 30M，
/// 总优先费 priority_fee 最大化。简化版：贪心按 gas_price 降序装 → 用迭代器表达。
///
/// **迭代器套路**：`BinaryHeap` 不实现 sorted Iterator，需要 `into_sorted_vec`
/// 或 pop loop。这里展示用 `take_while` 控制 cumulative gas 的标准技巧 ——
/// 通过 `scan` 把累计量物化进流，再 `take_while` 截断。
pub mod mempool_pack {
    #[derive(Debug, Clone, Copy)]
    pub struct PendingTx {
        pub hash: u64,
        pub gas: u64,
        pub gas_price: u64,
    }

    pub const BLOCK_GAS_LIMIT: u64 = 30_000_000;

    pub fn pack_block(mut pending: Vec<PendingTx>) -> Vec<PendingTx> {
        // 按 gas_price 降序（高优先费先入块）
        pending.sort_unstable_by(|a, b| b.gas_price.cmp(&a.gas_price));

        // scan 把「截至当前的累计 gas」物化进流
        // take_while 在累计超限的*那一笔*上立刻终止
        // map 还原回交易本身
        pending
            .into_iter()
            .scan(0u64, |cum, tx| {
                if *cum + tx.gas > BLOCK_GAS_LIMIT {
                    None // scan 返回 None → 流终止
                } else {
                    *cum += tx.gas;
                    Some(tx)
                }
            })
            .collect()
    }

    pub fn demonstrate() {
        println!("## 场景 4：Mempool 装块（scan + 累计约束）");

        let pool = vec![
            PendingTx { hash: 1, gas: 21_000, gas_price: 100 },
            PendingTx { hash: 2, gas: 12_000_000, gas_price: 200 },
            PendingTx { hash: 3, gas: 12_000_000, gas_price: 150 },
            PendingTx { hash: 4, gas: 8_000_000, gas_price: 50 },
            PendingTx { hash: 5, gas: 5_000_000, gas_price: 90 },
        ];

        let block = pack_block(pool);
        let total_gas: u64 = block.iter().map(|t| t.gas).sum();
        let total_fee: u64 = block.iter().map(|t| t.gas * t.gas_price).sum();
        println!("装入 {} 笔 tx，gas={}, fee={}", block.len(), total_gas, total_fee);
        for t in &block {
            println!("  tx#{} gas={} px={}", t.hash, t.gas, t.gas_price);
        }
        println!("关键：scan 返回 None 即可终止流；用纯函数表达「贪心 + 容量约束」\n");
    }
}

// ============================================================================
// 场景 5：多链区块头合并（k-way merge 风格）
// ============================================================================
/// **生产问题**：多链 indexer 同时拉 Ethereum / Arbitrum / Optimism 的最新
/// 区块头，下游想看一条「全局按 timestamp 升序」的统一流。
///
/// **迭代器套路**：k 路归并。std 没有内建 `kmerge`，但本质上就是：
/// 维护每条流的 peeked head，每次输出 timestamp 最小的那条。
/// 这里用 `Vec<Peekable<I>>` 手写，强调「Iterator 也是数据」的思路。
pub mod multichain_merge {
    use std::iter::Peekable;

    #[derive(Debug, Clone, Copy)]
    pub struct Head {
        pub chain_id: u64,
        pub block_number: u64,
        pub timestamp: u64,
    }

    pub struct KMerge<I: Iterator<Item = Head>> {
        streams: Vec<Peekable<I>>,
    }

    impl<I: Iterator<Item = Head>> KMerge<I> {
        pub fn new<S: IntoIterator<Item = I>>(streams: S) -> Self {
            Self { streams: streams.into_iter().map(|s| s.peekable()).collect() }
        }
    }

    impl<I: Iterator<Item = Head>> Iterator for KMerge<I> {
        type Item = Head;

        fn next(&mut self) -> Option<Head> {
            // 找 peek().timestamp 最小的那一路
            let (idx, _) = self
                .streams
                .iter_mut()
                .enumerate()
                .filter_map(|(i, s)| s.peek().map(|h| (i, h.timestamp)))
                .min_by_key(|&(_, ts)| ts)?;
            self.streams[idx].next()
        }
    }

    pub fn demonstrate() {
        println!("## 场景 5：多链 head k-way merge");
        let eth = vec![
            Head { chain_id: 1, block_number: 100, timestamp: 1000 },
            Head { chain_id: 1, block_number: 101, timestamp: 1012 },
        ];
        let arb = vec![
            Head { chain_id: 42161, block_number: 500, timestamp: 1003 },
            Head { chain_id: 42161, block_number: 501, timestamp: 1004 },
            Head { chain_id: 42161, block_number: 502, timestamp: 1015 },
        ];
        let op = vec![Head { chain_id: 10, block_number: 700, timestamp: 1010 }];

        let merged = KMerge::new(vec![eth.into_iter(), arb.into_iter(), op.into_iter()]);
        for h in merged {
            println!("  chain={} block={} ts={}", h.chain_id, h.block_number, h.timestamp);
        }
        println!("关键：把 Iterator 当一等公民放进 Vec，构造组合迭代器\n");
    }
}

// ============================================================================
// 场景 6：Bloom filter 预筛 + 去重
// ============================================================================
/// **生产问题**：节点收到 N 个新 tx 哈希，要丢弃曾经见过的（防 replay /
/// 减少广播）。用 Bloom filter 做廉价预筛，命中再查精确集合。
///
/// **迭代器套路**：`filter` + 副作用（更新 filter）的「检测并记录」模式。
/// 注意：副作用 closure 必须能被 `FnMut` 调用，且要警惕惰性求值踩坑
/// （pitfalls.rs 详解为什么不用 `map` 做副作用）。
pub mod bloom_dedupe {
    pub struct Bloom {
        bits: Vec<u64>,
        k: u32, // 哈希函数个数
    }

    impl Bloom {
        pub fn new(words: usize, k: u32) -> Self {
            Self { bits: vec![0u64; words], k }
        }

        fn idx(&self, hash: u64, i: u32) -> (usize, u64) {
            // 双重哈希构造 k 个位置（Kirsch-Mitzenmacher）
            let h = hash.wrapping_add((i as u64).wrapping_mul(0x9e3779b97f4a7c15));
            let total_bits = (self.bits.len() * 64) as u64;
            let bit = h % total_bits;
            ((bit / 64) as usize, 1u64 << (bit % 64))
        }

        pub fn contains(&self, hash: u64) -> bool {
            (0..self.k).all(|i| {
                let (w, m) = self.idx(hash, i);
                self.bits[w] & m != 0
            })
        }

        pub fn insert(&mut self, hash: u64) {
            for i in 0..self.k {
                let (w, m) = self.idx(hash, i);
                self.bits[w] |= m;
            }
        }
    }

    /// 返回「之前没见过」的 tx 哈希流。
    /// 副作用（写 bloom）放在 filter 里 —— 看起来不纯，但语义清晰：
    /// 元素「通过 filter ⇔ 我们决定记住它」。
    pub fn novel_txs<'a, I>(it: I, bloom: &'a mut Bloom) -> impl Iterator<Item = u64> + 'a
    where
        I: Iterator<Item = u64> + 'a,
    {
        it.filter(move |&h| {
            if bloom.contains(h) {
                false
            } else {
                bloom.insert(h);
                true
            }
        })
    }

    pub fn demonstrate() {
        println!("## 场景 6：Bloom 去重（filter + 内部状态）");
        let mut bloom = Bloom::new(1024, 4);
        // 第一批
        let first: Vec<u64> = novel_txs((0..10u64).chain(0..3), &mut bloom).collect();
        // 第二批，前 10 个全是重复
        let second: Vec<u64> = novel_txs(0..15u64, &mut bloom).collect();

        println!("第一批保留 {}（重复的 0..3 被丢）", first.len());
        println!("第二批保留 {}（10..15 是新的）", second.len());
        println!("关键：可变借用 `&mut bloom` 安全地穿过 closure 是 Rust 的招牌\n");
    }
}

pub fn demonstrate() {
    merkle_root::demonstrate();
    erc20_scanner::demonstrate();
    sandwich_sim::demonstrate();
    mempool_pack::demonstrate();
    multichain_merge::demonstrate();
    bloom_dedupe::demonstrate();
}
