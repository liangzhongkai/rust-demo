//! # 数据结构底层机制
//!
//! 这一节回答四个根本问题，后面 HFT/Web3 章节里所有选型都建立在这之上：
//!
//! 1. Rust 标准库提供了哪些「容器协议」？
//! 2. 哈希表 vs 有序树：什么时候该用哪个？
//! 3. 连续内存（Vec）vs 指针追逐（链表）：缓存局部性如何决定延迟？
//! 4. 复合结构（HashMap + VecDeque）为什么是生产系统的默认形态？

#![allow(dead_code)]

// ============================================================================
// 1. 容器协议：不是「数据结构」，而是「操作契约」
// ============================================================================
/// `HashMap<K,V>` 的契约：均摊 O(1) 的 get/insert/remove，*不保证* 顺序。
/// `BTreeMap<K,V>` 的契约：O(log n) 的 get/insert/remove，*保证* 按 key 有序。
/// `BinaryHeap<T>` 的契约：O(log n) push/pop，永远 O(1) peek 最大（或最小）。
///
/// 选容器 = 选操作契约。先列出热路径上的操作集合，再反推结构。
pub mod container_contracts {
    use std::collections::{BTreeMap, BinaryHeap, HashMap, VecDeque};

    pub fn demonstrate() {
        println!("## 1. 容器协议 = 操作契约");

        // HashMap：O(1) 点查
        let mut orders: HashMap<u64, i64> = HashMap::with_capacity(1024);
        orders.insert(1001, 50_000);
        println!("ClOrdID 1001 → qty {}", orders.get(&1001).unwrap());

        // BTreeMap：O(log n) 点查 + O(log n + k) 范围扫描
        let mut book: BTreeMap<i64, i64> = BTreeMap::new();
        book.insert(100_00, 10);
        book.insert(101_00, 5);
        book.insert(102_00, 8);
        let band: Vec<_> = book.range(100_50..=101_50).collect();
        println!("价格带 [100.50, 101.50] 档位 = {:?}", band);

        // BinaryHeap：O(log n) 取极值
        let heap = BinaryHeap::from([3, 1, 4, 1, 5]);
        println!("堆顶 = {}", heap.peek().unwrap());

        // VecDeque：O(1) 双端 push/pop —— 滑动窗口的标准底座
        let mut window = VecDeque::with_capacity(64);
        window.push_back(1);
        window.push_back(2);
        if window.len() > 1 {
            window.pop_front();
        }
        println!("滑动窗口 = {:?}", window);
        println!();
    }
}

// ============================================================================
// 2. HashMap vs BTreeMap 决策树
// ============================================================================
/// | 需求                         | 选 HashMap | 选 BTreeMap |
/// |------------------------------|------------|-------------|
/// | 只要 O(1) 点查               | ✅         |             |
/// | 需要 range / 前缀 / 后继     |            | ✅          |
/// | key 是整数且范围小           | ✅         |             |
/// | key 需要稳定迭代顺序         |            | ✅          |
/// | 热路径 P99 延迟敏感          | ✅（更快） |             |
/// | 内存紧凑、无 rehash 尖峰     |            | ✅          |
pub mod hash_vs_tree {
    use std::collections::{BTreeMap, HashMap};

    /// 用 u64 做 key 时，HashMap 不会分配 String，也没有 rehash 的不可预测性
    /// 如果 key 空间是连续整数 [0..N)，Vec 索引比 HashMap 更快。
    pub fn vec_index_vs_hashmap(n: usize) -> (u64, u64) {
        let mut by_vec = vec![0u64; n];
        let mut by_map: HashMap<usize, u64> = HashMap::with_capacity(n);

        for i in 0..n {
            by_vec[i] = i as u64 * 10;
            by_map.insert(i, i as u64 * 10);
        }

        let sum_vec: u64 = by_vec.iter().sum();
        let sum_map: u64 = by_map.values().sum();
        (sum_vec, sum_map)
    }

    pub fn demonstrate() {
        println!("## 2. HashMap vs BTreeMap vs Vec 索引");

        // 整数 ID 稠密 → Vec 最快
        let ids = vec![0usize, 1, 2, 3];
        let slots = vec![100, 200, 300, 400];
        let qty = slots[ids[2]]; // O(1)，无哈希、无比较
        println!("Vec 索引 slot[2] = {}", qty);

        // 整数 ID 稀疏 → HashMap
        let mut sparse: HashMap<u64, i64> = HashMap::new();
        sparse.insert(9_999_999, 42);
        println!("稀疏 HashMap get = {:?}", sparse.get(&9_999_999));

        // 需要按 key 排序遍历 → BTreeMap
        let mut sorted: BTreeMap<i64, &str> = BTreeMap::new();
        sorted.insert(102_00, "ask_2");
        sorted.insert(100_00, "ask_0");
        sorted.insert(101_00, "ask_1");
        println!("BTreeMap 升序: {:?}", sorted.values().collect::<Vec<_>>());
        println!();
    }
}

// ============================================================================
// 3. 缓存局部性：Vec 碾压 LinkedList
// ============================================================================
/// Rust 标准库的 `LinkedList` 几乎从不出现在生产代码里。
/// 原因：每个节点是独立堆分配，遍历时 cache miss 率极高。
/// HFT 里连 `Box` 链表都很少见，更常见的是 arena + 数组索引。
pub mod cache_locality {
    use std::collections::{LinkedList, VecDeque};

    pub fn demonstrate() {
        println!("## 3. 缓存局部性：VecDeque >> LinkedList");

        let n = 10_000usize;

        // VecDeque：环形缓冲区，元素在连续 chunk 里
        let mut dq: VecDeque<u64> = VecDeque::with_capacity(n);
        for i in 0..n {
            dq.push_back(i as u64);
        }
        let sum_dq: u64 = dq.iter().sum();

        // LinkedList：每个节点独立分配，遍历时 pointer chasing
        let mut ll: LinkedList<u64> = LinkedList::new();
        for i in 0..n {
            ll.push_back(i as u64);
        }
        let sum_ll: u64 = ll.iter().sum();

        assert_eq!(sum_dq, sum_ll);
        println!("VecDeque sum = {}, LinkedList sum = {}（结果相同）", sum_dq, sum_ll);
        println!("VecDeque 遍历 cache-friendly；LinkedList 每个节点一次 cache miss");
        println!("规则：除非需要 O(1) 中间 splice 且元素巨大，否则不用 LinkedList\n");
    }
}

// ============================================================================
// 4. 复合结构：HashMap + VecDeque 是订单簿的 DNA
// ============================================================================
/// 单一容器很少够用。生产系统的模式是：
/// - 外层 HashMap/BTreeMap：按 key（价格、地址、symbol）分桶
/// - 内层 VecDeque/Vec：桶内 FIFO 或顺序存储
/// - 辅助 HashMap：反向索引（id → location）
pub mod composite_pattern {
    use std::collections::{HashMap, VecDeque};

    #[derive(Debug, Clone)]
    struct Order {
        id: u64,
        qty: i64,
    }

    /// 最简单的「价格 → 订单队列」复合结构
    struct PriceLevelBook {
        levels: HashMap<i64, VecDeque<Order>>,
    }

    impl PriceLevelBook {
        fn new() -> Self {
            Self { levels: HashMap::new() }
        }

        fn add(&mut self, px: i64, order: Order) {
            self.levels.entry(px).or_default().push_back(order);
        }

        fn best_bid_qty(&self, px: i64) -> i64 {
            self.levels.get(&px).map(|q| q.iter().map(|o| o.qty).sum()).unwrap_or(0)
        }
    }

    pub fn demonstrate() {
        println!("## 4. 复合结构：HashMap<Price, VecDeque<Order>>");

        let mut book = PriceLevelBook::new();
        book.add(100_00, Order { id: 1, qty: 5 });
        book.add(100_00, Order { id: 2, qty: 3 });
        book.add(101_00, Order { id: 3, qty: 8 });

        println!("@100.00 总量 = {}", book.best_bid_qty(100_00));
        println!("模式：外层分桶 O(1)，内层 FIFO O(1) push/pop");
        println!("HFT 进阶：外层换 BTreeMap 支持 range；内层换 arena 索引避免堆分配\n");
    }
}

// ============================================================================
// 5. with_capacity 与预分配：避免热路径 realloc
// ============================================================================
pub mod preallocation {
    use std::collections::HashMap;

    pub fn demonstrate() {
        println!("## 5. with_capacity 避免 rehash 尖峰");

        // ❌ 默认容量 0，插入过程中多次 *2 扩容 + rehash
        let mut cold = HashMap::new();
        for i in 0..10_000u64 {
            cold.insert(i, i);
        }

        // ✅ 预估最终大小，一次分配
        let mut hot: HashMap<u64, u64> = HashMap::with_capacity(10_000);
        for i in 0..10_000u64 {
            hot.insert(i, i);
        }

        println!("cold len = {}, hot len = {}", cold.len(), hot.len());
        println!("HFT：rehash 发生在不可预测的时刻 → P99 尖峰");
        println!("规则：知道上界就 with_capacity；不知道就监控 load factor\n");
    }
}

// ============================================================================
// 6. Entry API：一次哈希，两种结局
// ============================================================================
pub mod entry_api {
    use std::collections::HashMap;

    pub fn demonstrate() {
        println!("## 6. Entry API：get + insert 合并为一次查找");

        let mut counts: HashMap<&str, u64> = HashMap::new();

        // ❌ 两次哈希
        // let c = counts.get("BTC").unwrap_or(&0) + 1;
        // counts.insert("BTC", c);

        // ✅ 一次哈希
        for sym in ["BTC", "ETH", "BTC", "SOL", "BTC"] {
            *counts.entry(sym).or_insert(0) += 1;
        }
        println!("成交计数: {:?}", counts);
        println!("`entry().or_insert()` / `or_default()` 是热路径标配\n");
    }
}

pub fn demonstrate() {
    container_contracts::demonstrate();
    hash_vs_tree::demonstrate();
    cache_locality::demonstrate();
    composite_pattern::demonstrate();
    preallocation::demonstrate();
    entry_api::demonstrate();
}
