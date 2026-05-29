//! # Web3 / 区块链生产场景下的数据结构
//!
//! Web3 的工作负载是 *大量键值状态 + 密码学承诺 + 优先级调度*：
//! - 账户/存储：Trie、Merkle Tree
//! - 交易池：HashMap 索引 + Heap 排序
//! - 依赖图：邻接表 DAG
//! - 去重：Bloom + HashSet 两级
//!
//! 下面 6 个场景对应 reth、geth、Flashbots builder 里的常见写法。

#![allow(dead_code)]

pub type Address = [u8; 20];
pub type B256 = [u8; 32];

/// 教学用确定性哈希（生产请用 sha3::Keccak256）
fn keccak_like(bytes: &[u8]) -> B256 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut out = [0u8; 32];
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    out[..8].copy_from_slice(&h.finish().to_le_bytes());
    out
}

fn hex8(b: &B256) -> String {
    b.iter().take(4).map(|x| format!("{:02x}", x)).collect()
}

// ============================================================================
// 场景 1：Merkle Tree（Vec 分层塌缩）
// ============================================================================
/// **生产问题**：交易/receipt root、空投白名单、L2 状态承诺。
/// 叶子任意数量，奇数时复制最后一个配对。
///
/// **数据结构**：每层 `Vec<B256>`，循环 `chunks(2)` 向上塌缩。
pub mod merkle_tree {
    use super::*;

    pub fn merkle_root(mut layer: Vec<B256>) -> B256 {
        if layer.is_empty() {
            return [0u8; 32];
        }
        while layer.len() > 1 {
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
        }
        layer[0]
    }

    pub fn demonstrate() {
        println!("## 场景 1：Merkle Tree（Vec 分层塌缩）");
        let leaves: Vec<B256> = (0u8..8).map(|i| keccak_like(&[i])).collect();
        let root = merkle_root(leaves);
        println!("8 叶子 → root = 0x{}…", hex8(&root));
        println!("关键：Vec 层数组；O(n) 空间；证明路径长度 O(log n)\n");
    }
}

// ============================================================================
// 场景 2：Hex Trie 账户存储（HashMap 节点）
// ============================================================================
/// **生产问题**：以太坊 MPT 的简化版 —— 按 hex path 存 account storage。
/// 需要前缀共享、点查、前缀遍历。
///
/// **数据结构**：递归 `HashMap<u8, Box<Node>>` 或扁平 `HashMap<Vec<u8>, V>`。
pub mod hex_trie {
    use std::collections::HashMap;

    #[derive(Default)]
    struct Node {
        children: HashMap<u8, Node>,
        value: Option<Vec<u8>>,
    }

    struct HexTrie {
        root: Node,
    }

    impl HexTrie {
        fn new() -> Self {
            Self { root: Node::default() }
        }

        fn insert(&mut self, path: &[u8], value: Vec<u8>) {
            let mut node = &mut self.root;
            for &nibble in path {
                node = node.children.entry(nibble).or_default();
            }
            node.value = Some(value);
        }

        fn get(&self, path: &[u8]) -> Option<&[u8]> {
            let mut node = &self.root;
            for &nibble in path {
                node = node.children.get(&nibble)?;
            }
            node.value.as_deref()
        }

        fn collect_prefix_keys(&self, prefix: &[u8], out: &mut Vec<Vec<u8>>) {
            let mut node = &self.root;
            for &nibble in prefix {
                match node.children.get(&nibble) {
                    Some(n) => node = n,
                    None => return,
                }
            }
            Self::collect_keys(node, prefix.to_vec(), out);
        }

        fn collect_keys(node: &Node, path: Vec<u8>, out: &mut Vec<Vec<u8>>) {
            if node.value.is_some() {
                out.push(path.clone());
            }
            for (&nib, child) in &node.children {
                let mut extended = path.clone();
                extended.push(nib);
                Self::collect_keys(child, extended, out);
            }
        }
    }

    pub fn demonstrate() {
        println!("## 场景 2：Hex Trie（HashMap 子节点）");

        let mut trie = HexTrie::new();
        trie.insert(&[1, 2, 3], b"alice_balance".to_vec());
        trie.insert(&[1, 2, 4], b"bob_balance".to_vec());
        trie.insert(&[1, 5], b"charlie".to_vec());

        println!(
            "get [1,2,3] = {:?}",
            trie.get(&[1, 2, 3]).map(|v| std::str::from_utf8(v).unwrap())
        );
        let mut prefix_keys = Vec::new();
        trie.collect_prefix_keys(&[1, 2], &mut prefix_keys);
        println!("前缀 [1,2] 下 {} 个 key", prefix_keys.len());
        println!("关键：HashMap 稀疏分支；生产 MPT 用 16 叉 + RLP 编码 + 节点哈希\n");
    }
}

// ============================================================================
// 场景 3：Mempool 双索引（HashMap + BinaryHeap）
// ============================================================================
/// **生产问题**：按 tx hash O(1) 查重/替换；按 priority fee 取 top tx 装块。
/// 两棵结构维护同一批对象的不同视图。
///
/// **数据结构**：`HashMap<B256, Tx>` + `BinaryHeap<(u64, B256)>`。
pub mod mempool {
    use super::*;
    use std::collections::{BinaryHeap, HashMap};

    #[derive(Debug, Clone)]
    pub struct Tx {
        pub hash: B256,
        pub gas: u64,
        pub priority_fee: u64,
    }

    pub struct Mempool {
        by_hash: HashMap<B256, Tx>,
        by_priority: BinaryHeap<(u64, B256)>, // (priority_fee, hash)
    }

    impl Mempool {
        pub fn new() -> Self {
            Self { by_hash: HashMap::new(), by_priority: BinaryHeap::new() }
        }

        pub fn insert(&mut self, tx: Tx) {
            let hash = tx.hash;
            let fee = tx.priority_fee;
            self.by_hash.insert(hash, tx);
            self.by_priority.push((fee, hash));
        }

        pub fn contains(&self, hash: &B256) -> bool {
            self.by_hash.contains_key(hash)
        }

        /// 取最高 priority 且仍存活的 tx（lazy delete 处理 stale heap entry）
        pub fn pop_best(&mut self) -> Option<Tx> {
            while let Some((_, hash)) = self.by_priority.pop() {
                if let Some(tx) = self.by_hash.remove(&hash) {
                    return Some(tx);
                }
            }
            None
        }
    }

    pub fn demonstrate() {
        println!("## 场景 3：Mempool 双索引（HashMap + BinaryHeap）");

        let mut pool = Mempool::new();
        for (i, fee) in [(1u8, 100), (2, 300), (3, 200)] {
            let hash = keccak_like(&[i]);
            pool.insert(Tx { hash, gas: 21_000, priority_fee: fee });
        }

        while let Some(tx) = pool.pop_best() {
            println!("  pop fee={}", tx.priority_fee);
        }
        println!("关键：多索引是常态；heap lazy delete 比同步删除 O(log n) 更简单\n");
    }
}

// ============================================================================
// 场景 4：交易依赖 DAG（HashMap 邻接表）
// ============================================================================
/// **生产问题**：builder 构建 block 时，tx B 依赖 tx A 的执行结果（nonce、
/// CREATE2 地址）。必须拓扑排序，不能乱序执行。
///
/// **数据结构**：`HashMap<TxHash, Vec<TxHash>>` 邻接表 + 入度计数。
pub mod tx_dag {
    use super::*;
    use std::collections::{HashMap, VecDeque};

    pub fn topo_sort(edges: &HashMap<B256, Vec<B256>>) -> Option<Vec<B256>> {
        let mut indegree: HashMap<B256, usize> = HashMap::new();
        for (from, tos) in edges {
            indegree.entry(*from).or_default();
            for to in tos {
                *indegree.entry(*to).or_default() += 1;
            }
        }

        let mut queue: VecDeque<B256> =
            indegree.iter().filter(|(_, &d)| d == 0).map(|(&k, _)| k).collect();

        let mut order = Vec::new();
        while let Some(node) = queue.pop_front() {
            order.push(node);
            if let Some(children) = edges.get(&node) {
                for &child in children {
                    let d = indegree.get_mut(&child).unwrap();
                    *d -= 1;
                    if *d == 0 {
                        queue.push_back(child);
                    }
                }
            }
        }

        if order.len() == indegree.len() { Some(order) } else { None }
    }

    pub fn demonstrate() {
        println!("## 场景 4：Tx 依赖 DAG（HashMap 邻接表 + 拓扑排序）");

        let a = keccak_like(b"A");
        let b = keccak_like(b"B");
        let c = keccak_like(b"C");
        let d = keccak_like(b"D");

        let mut edges = HashMap::new();
        edges.insert(a, vec![b, c]); // A → B, A → C
        edges.insert(b, vec![d]);    // B → D

        let order = topo_sort(&edges).unwrap();
        println!("拓扑序 {} 个节点", order.len());
        println!("关键：邻接表适合稀疏图；稠密图换 Vec<Vec<bool>> 或 petgraph\n");
    }
}

// ============================================================================
// 场景 5：Nonce 序列化队列（HashMap<Address, BTreeMap<u64, Tx>>）
// ============================================================================
/// **生产问题**：同一 sender 的 tx 必须按 nonce 递增执行。Mempool 里
/// nonce=5 到了但 nonce=4 还没到 → 不能进 block，但要能快速查 gap。
///
/// **数据结构**：`HashMap<Address, BTreeMap<u64, Tx>>` —— 地址分桶，nonce 有序。
pub mod nonce_tracker {
    use super::*;
    use std::collections::{BTreeMap, HashMap};

    #[derive(Debug, Clone)]
    pub struct Tx {
        pub hash: B256,
        pub nonce: u64,
    }

    pub struct NoncePool {
        by_sender: HashMap<Address, BTreeMap<u64, Tx>>,
        next_nonce: HashMap<Address, u64>,
    }

    impl NoncePool {
        pub fn new() -> Self {
            Self { by_sender: HashMap::new(), next_nonce: HashMap::new() }
        }

        pub fn insert(&mut self, sender: Address, tx: Tx) {
            self.by_sender.entry(sender).or_default().insert(tx.nonce, tx);
        }

        /// 取出从 next_nonce 起连续可用的 tx
        pub fn drain_ready(&mut self, sender: Address) -> Vec<Tx> {
            let mut ready = Vec::new();
            let expected = *self.next_nonce.get(&sender).unwrap_or(&0);
            let Some(queue) = self.by_sender.get_mut(&sender) else {
                return ready;
            };

            let mut nonce = expected;
            while let Some(tx) = queue.remove(&nonce) {
                ready.push(tx);
                nonce += 1;
            }
            if !ready.is_empty() {
                self.next_nonce.insert(sender, nonce);
            }
            ready
        }
    }

    pub fn demonstrate() {
        println!("## 场景 5：Nonce 序列（HashMap + BTreeMap）");

        let alice = [0xab; 20];
        let mut pool = NoncePool::new();
        pool.insert(alice, Tx { hash: keccak_like(b"t0"), nonce: 0 });
        pool.insert(alice, Tx { hash: keccak_like(b"t2"), nonce: 2 }); // 缺 nonce 1

        let blocked = pool.drain_ready(alice);
        println!("缺 nonce 1 时仅就绪 {} 笔", blocked.len());

        pool.insert(alice, Tx { hash: keccak_like(b"t1"), nonce: 1 });
        let unblocked = pool.drain_ready(alice);
        println!("补齐 nonce 1 后再就绪 {} 笔", unblocked.len());
        println!("关键：BTreeMap 保证 nonce 有序；gap 检测 = 首 key != expected\n");
    }
}

// ============================================================================
// 场景 6：Bloom + HashSet 两级去重
// ============================================================================
/// **生产问题**：P2P 层每秒收到大量 tx hash announcement。全用 HashSet
/// 内存爆炸；全用 Bloom 有误杀。两级过滤是标准做法。
///
/// **数据结构**：Bloom filter（廉价预筛）+ HashSet（精确确认）。
pub mod bloom_dedup {
    use std::collections::HashSet;

    pub struct Bloom {
        bits: Vec<u64>,
        k: u32,
    }

    impl Bloom {
        pub fn new(words: usize, k: u32) -> Self {
            Self { bits: vec![0u64; words], k }
        }

        fn idx(&self, hash: u64, i: u32) -> (usize, u64) {
            let h = hash.wrapping_add((i as u64).wrapping_mul(0x9e3779b97f4a7c15));
            let total = (self.bits.len() * 64) as u64;
            let bit = h % total;
            ((bit / 64) as usize, 1u64 << (bit % 64))
        }

        fn maybe_contains(&self, hash: u64) -> bool {
            (0..self.k).all(|i| {
                let (w, m) = self.idx(hash, i);
                self.bits[w] & m != 0
            })
        }

        fn insert(&mut self, hash: u64) {
            for i in 0..self.k {
                let (w, m) = self.idx(hash, i);
                self.bits[w] |= m;
            }
        }
    }

    pub struct TwoTierDedup {
        bloom: Bloom,
        exact: HashSet<u64>,
    }

    impl TwoTierDedup {
        pub fn new() -> Self {
            Self { bloom: Bloom::new(1024, 4), exact: HashSet::new() }
        }

        pub fn is_novel(&mut self, hash: u64) -> bool {
            if !self.bloom.maybe_contains(hash) {
                self.bloom.insert(hash);
                self.exact.insert(hash);
                return true;
            }
            if self.exact.contains(&hash) {
                return false;
            }
            // Bloom 假阳：精确集说不存在 → 真是新的
            self.exact.insert(hash);
            true
        }
    }

    pub fn demonstrate() {
        println!("## 场景 6：Bloom + HashSet 两级去重");

        let mut dedup = TwoTierDedup::new();
        let first: Vec<bool> = (0..20u64).map(|h| dedup.is_novel(h)).collect();
        let second: Vec<bool> = (0..25u64).map(|h| dedup.is_novel(h)).collect();

        let new_first = first.iter().filter(|&&x| x).count();
        let new_second = second.iter().filter(|&&x| x).count();
        println!("第一批新 tx = {}，第二批新 tx = {}（20..25 是新的）", new_first, new_second);
        println!("关键：Bloom 挡 99% 重复；HashSet 消除假阳；exact 集可 LRU 淘汰\n");
    }
}

pub fn demonstrate() {
    merkle_tree::demonstrate();
    hex_trie::demonstrate();
    mempool::demonstrate();
    tx_dag::demonstrate();
    nonce_tracker::demonstrate();
    bloom_dedup::demonstrate();
}
