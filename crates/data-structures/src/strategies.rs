//! # 泛化：从 HFT/Web3 场景到通用应对策略
//!
//! 把前两章具体业务里的数据结构选型抽象出来，得到一张
//! **「问题类型 → 推荐结构」决策矩阵**：
//!
//! | 问题类型           | 标志特征                  | 首选结构                          |
//! |--------------------|---------------------------|-----------------------------------|
//! | 1. O(1) 点查       | id → object               | HashMap                           |
//! | 2. 有序 / 范围     | range / prefix / 后继     | BTreeMap                          |
//! | 3. Top-K / 调度    | 取极值、定时触发          | BinaryHeap                        |
//! | 4. FIFO 分桶       | key 内公平排队            | HashMap<K, VecDeque<V>>           |
//! | 5. 滑动窗口        | 固定容量流                | Ring buffer / VecDeque            |
//! | 6. 去重            | 见过/没见过               | HashSet / Bloom+HashSet           |
//! | 7. 关系 / 依赖     | A 依赖 B                  | HashMap 邻接表                    |
//! | 8. 密码学承诺      | 成员证明 / 状态根         | Merkle Tree / Trie                |
//!
//! 下面 8 个策略各有一个 *通用模板*，签名上不带业务名词。

#![allow(dead_code)]

// ============================================================================
// 策略 1：O(1) 点查 —— HashMap
// ============================================================================
/// 问题：id → object 的 CRUD，不需要顺序。
/// HFT: clordid_index | Web3: mempool by_hash
pub mod point_lookup {
    use std::collections::HashMap;

    pub struct Index<K: Eq + std::hash::Hash, V> {
        map: HashMap<K, V>,
    }

    impl<K: Eq + std::hash::Hash, V> Index<K, V> {
        pub fn with_capacity(n: usize) -> Self {
            Self { map: HashMap::with_capacity(n) }
        }

        pub fn get(&self, k: &K) -> Option<&V> {
            self.map.get(k)
        }

        pub fn upsert(&mut self, k: K, v: V) {
            self.map.insert(k, v);
        }
    }

    pub fn demonstrate() {
        println!("## 策略 1：HashMap 点查索引");
        let mut idx = Index::with_capacity(1024);
        idx.upsert(42u64, "order");
        println!("get 42 = {:?}", idx.get(&42));
        println!();
    }
}

// ============================================================================
// 策略 2：有序 / 范围 —— BTreeMap
// ============================================================================
/// 问题：按 key 排序遍历、range 查询、前缀 successor。
/// HFT: band_risk | Web3: nonce_tracker 内层
pub mod ordered_range {
    use std::collections::BTreeMap;

    pub fn sum_in_range(map: &BTreeMap<i64, i64>, lo: i64, hi: i64) -> i64 {
        map.range(lo..=hi).map(|(_, v)| v).sum()
    }

    pub fn demonstrate() {
        println!("## 策略 2：BTreeMap 范围查询");
        let mut m = BTreeMap::new();
        for i in 0..10 {
            m.insert(i * 10, i);
        }
        println!("range [20,50] sum = {}", sum_in_range(&m, 20, 50));
        println!();
    }
}

// ============================================================================
// 策略 3：Top-K / 调度 —— BinaryHeap
// ============================================================================
/// 问题：反复取最大/最小、定时器、优先级队列。
/// HFT: timed_orders | Web3: mempool by_priority
pub mod priority_queue {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;

    pub struct MinQueue<T: Ord> {
        heap: BinaryHeap<Reverse<T>>,
    }

    impl<T: Ord> MinQueue<T> {
        pub fn new() -> Self {
            Self { heap: BinaryHeap::new() }
        }

        pub fn push(&mut self, item: T) {
            self.heap.push(Reverse(item));
        }

        pub fn pop(&mut self) -> Option<T> {
            self.heap.pop().map(|Reverse(t)| t)
        }
    }

    pub fn demonstrate() {
        println!("## 策略 3：BinaryHeap 最小优先级队列");
        let mut q = MinQueue::<i32>::new();
        for x in [30, 10, 20] {
            q.push(x);
        }
        let mut out = Vec::new();
        while let Some(x) = q.pop() {
            out.push(x);
        }
        println!("pop 顺序 = {:?}\n", out);
    }
}

// ============================================================================
// 策略 4：FIFO 分桶 —— HashMap + VecDeque
// ============================================================================
/// 问题：同一 key 下公平排队（订单簿档位、per-symbol 队列）。
/// HFT: order_book 内层
pub mod fifo_buckets {
    use std::collections::{HashMap, VecDeque};

    pub struct BucketQueue<K: Eq + std::hash::Hash, V> {
        buckets: HashMap<K, VecDeque<V>>,
    }

    impl<K: Eq + std::hash::Hash, V> BucketQueue<K, V> {
        pub fn new() -> Self {
            Self { buckets: HashMap::new() }
        }

        pub fn push(&mut self, key: K, val: V) {
            self.buckets.entry(key).or_default().push_back(val);
        }

        pub fn pop(&mut self, key: &K) -> Option<V> {
            self.buckets.get_mut(key)?.pop_front()
        }
    }

    pub fn demonstrate() {
        println!("## 策略 4：HashMap + VecDeque FIFO 分桶");
        let mut q = BucketQueue::<&str, u64>::new();
        q.push("BTC", 1);
        q.push("BTC", 2);
        println!("FIFO pop = {:?}\n", q.pop(&"BTC"));
    }
}

// ============================================================================
// 策略 5：滑动窗口 —— Ring buffer / VecDeque
// ============================================================================
/// 问题：固定容量最近 N 条记录，双端 O(1)。
/// HFT: ring_buffer | Web3: 最近 N 个 block hash
pub mod sliding_window {
    use std::collections::VecDeque;

    pub struct Window<T> {
        inner: VecDeque<T>,
        cap: usize,
    }

    impl<T> Window<T> {
        pub fn new(cap: usize) -> Self {
            Self { inner: VecDeque::with_capacity(cap), cap }
        }

        pub fn push(&mut self, item: T) {
            if self.inner.len() == self.cap {
                self.inner.pop_front();
            }
            self.inner.push_back(item);
        }

        pub fn len(&self) -> usize {
            self.inner.len()
        }
    }

    pub fn demonstrate() {
        println!("## 策略 5：VecDeque 滑动窗口");
        let mut w = Window::new(3);
        for x in 1..=5 {
            w.push(x);
        }
        println!("窗口内 {} 个元素\n", w.len());
    }
}

// ============================================================================
// 策略 6：去重 —— HashSet / Bloom 两级
// ============================================================================
/// 问题：快速判断元素是否见过。
/// Web3: bloom_dedup | HFT: 已处理 tick sequence
pub mod dedup {
    use std::collections::HashSet;

    pub fn dedup_in_order<T: Eq + std::hash::Hash + Clone, I: IntoIterator<Item = T>>(
        it: I,
    ) -> impl Iterator<Item = T> {
        let mut seen = HashSet::new();
        it.into_iter().filter(move |x| seen.insert(x.clone()))
    }

    pub fn demonstrate() {
        println!("## 策略 6：HashSet 在线去重");
        let v: Vec<i32> = dedup_in_order([1, 2, 2, 3, 1, 4]).collect();
        println!("去重保序 = {:?}\n", v);
    }
}

// ============================================================================
// 策略 7：关系 / 依赖 —— HashMap 邻接表
// ============================================================================
/// 问题：实体间有向依赖，需要拓扑排序或 BFS。
/// Web3: tx_dag | HFT: 多 leg 订单依赖
pub mod adjacency_graph {
    use std::collections::{HashMap, VecDeque};

    pub fn has_cycle(edges: &HashMap<u64, Vec<u64>>) -> bool {
        let mut indegree: HashMap<u64, usize> = HashMap::new();
        for (from, tos) in edges {
            indegree.entry(*from).or_default();
            for to in tos {
                *indegree.entry(*to).or_default() += 1;
            }
        }
        let mut queue: VecDeque<u64> =
            indegree.iter().filter(|(_, &d)| d == 0).map(|(&k, _)| k).collect();
        let mut visited = 0;
        while let Some(n) = queue.pop_front() {
            visited += 1;
            if let Some(children) = edges.get(&n) {
                for &c in children {
                    let d = indegree.get_mut(&c).unwrap();
                    *d -= 1;
                    if *d == 0 {
                        queue.push_back(c);
                    }
                }
            }
        }
        visited != indegree.len()
    }

    pub fn demonstrate() {
        println!("## 策略 7：邻接表 + 拓扑排序");
        let mut edges = HashMap::new();
        edges.insert(1, vec![2]);
        edges.insert(2, vec![3]);
        println!("有环? {}", has_cycle(&edges));
        edges.insert(3, vec![1]); // 加环
        println!("加 3→1 后有环? {}\n", has_cycle(&edges));
    }
}

// ============================================================================
// 策略 8：密码学承诺 —— Merkle Tree
// ============================================================================
/// 问题：大量叶子需要一个 compact root + 成员证明。
/// Web3: merkle_tree | HFT: 审计日志 tamper-evident batch
pub mod merkle_commit {
    pub fn fold_layer(mut layer: Vec<u64>) -> u64 {
        if layer.is_empty() {
            return 0;
        }
        while layer.len() > 1 {
            layer = layer
                .chunks(2)
                .map(|p| {
                    let (a, b) = match p {
                        [x, y] => (*x, *y),
                        [x] => (*x, *x),
                        _ => unreachable!(),
                    };
                    a.wrapping_add(b).wrapping_mul(31)
                })
                .collect();
        }
        layer[0]
    }

    pub fn demonstrate() {
        println!("## 策略 8：Merkle 分层塌缩");
        let leaves: Vec<u64> = (1..=8).collect();
        println!("root = {}\n", fold_layer(leaves));
    }
}

// ============================================================================
// 反向：什么时候该换专用结构 / 外部 crate
// ============================================================================
pub mod when_to_upgrade {
    pub fn demonstrate() {
        println!("## 反例：什么时候 std 容器不够");
        println!("  - 并发读写 → dashmap / scc::HashMap / sharded lock");
        println!("  - 超大规模 LRU → moka / lru crate");
        println!("  - 整数稀疏 key → roaring / btree + 压缩");
        println!("  - 订单簿极致延迟 → 自定义 arena skiplist（见 lock-free-orderbook）");
        println!("  - 以太坊 MPT → 专用 Patricia trie（见 merkle-patricia）");
        println!("  - 先 std 原型验证语义，热路径 profiling 后再换\n");
    }
}

pub fn demonstrate() {
    point_lookup::demonstrate();
    ordered_range::demonstrate();
    priority_queue::demonstrate();
    fifo_buckets::demonstrate();
    sliding_window::demonstrate();
    dedup::demonstrate();
    adjacency_graph::demonstrate();
    merkle_commit::demonstrate();
    when_to_upgrade::demonstrate();
}
