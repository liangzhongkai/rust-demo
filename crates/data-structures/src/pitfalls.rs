//! # 数据结构常见陷阱与诊断
//!
//! 生产事故里反复出现的 8 个选型/用法陷阱：
//! - 现象（监控里看到什么）
//! - 根因（编译器/运行时层面发生了什么）
//! - 解决方案（一行修法 + 预防）

#![allow(dead_code)]

// ============================================================================
// 陷阱 1：LinkedList 误用 —— 以为 O(1) 插入就更快
// ============================================================================
/// **现象**：订单簿 FIFO 用 LinkedList，P99 延迟比 VecDeque 高一个数量级。
/// **根因**：每个节点独立堆分配，cache miss + TLB miss。
/// **修法**：VecDeque 或 arena + index linked list。
pub mod linked_list_trap {
    use std::collections::{LinkedList, VecDeque};

    pub fn demonstrate() {
        println!("## 陷阱 1：LinkedList 在热路径上是反模式");

        let n = 50_000usize;
        let mut dq = VecDeque::with_capacity(n);
        let mut ll = LinkedList::new();
        for i in 0..n {
            dq.push_back(i);
            ll.push_back(i);
        }

        let sum_dq: u64 = dq.iter().map(|&x| x as u64).sum();
        let sum_ll: u64 = ll.iter().map(|&x| x as u64).sum();
        assert_eq!(sum_dq, sum_ll);

        println!("VecDeque / LinkedList 求和相同，但 VecDeque cache-friendly");
        println!("规则：Rust 生产代码几乎不用 LinkedList\n");
    }
}

// ============================================================================
// 陷阱 2：HashMap 默认容量 → rehash 尖峰
// ============================================================================
/// **现象**：P99 延迟周期性尖峰，profiler 显示 hashbrown resize。
/// **根因**：默认容量 0，增长过程多次 *2 rehash。
/// **修法**：`HashMap::with_capacity(estimated)`。
pub mod rehash_spike {
    use std::collections::HashMap;

    pub fn demonstrate() {
        println!("## 陷阱 2：HashMap rehash 尖峰");

        let n = 100_000usize;
        let mut cold = HashMap::new();
        let mut hot = HashMap::with_capacity(n);

        for i in 0..n {
            cold.insert(i, i);
            hot.insert(i, i);
        }
        println!("cold 经历 ~{} 次 rehash，hot 通常 0-1 次", (n as f64).log2() as u32);
        println!("规则：知道上界就 with_capacity；监控 load factor > 0.7 预警\n");
    }
}

// ============================================================================
// 陷阱 3：String 做 HashMap key → 隐藏分配
// ============================================================================
/// **现象**：热路径 insert/get 触发 malloc，吞吐上不去。
/// **根因**：每次 `insert(symbol.to_string(), ...)` 都堆分配。
/// **修法**：整数 ID、`&str` + 生命周期、或 `CompactString` / intern table。
pub mod string_key_alloc {
    use std::collections::HashMap;

    pub fn demonstrate() {
        println!("## 陷阱 3：String key 的隐藏堆分配");

        let symbols = ["BTCUSDT", "ETHUSDT", "SOLUSDT"];

        // ❌ 每次 to_string() 都 alloc
        let mut by_string: HashMap<String, u64> = HashMap::new();
        for s in &symbols {
            by_string.insert(s.to_string(), 1);
        }

        // ✅ 静态 str 或整数 symbol id
        let mut by_str: HashMap<&str, u64> = HashMap::new();
        for s in &symbols {
            by_str.insert(*s, 1);
        }

        println!("String key len={}, &str key len={}", by_string.len(), by_str.len());
        println!("HFT：symbol 预注册为 u16 id；Web3：address 用 [u8;20] 或 U256\n");
    }
}

// ============================================================================
// 陷阱 4：该用 BTreeMap 却用了 HashMap —— range 变全表扫描
// ============================================================================
/// **现象**：风控「价格带内 notional」CPU 100%，订单量线性增长时延迟爆炸。
/// **根因**：HashMap 没有 range API，只能 iter 全表 filter。
/// **修法**：需要 range / 前缀 / 有序遍历 → BTreeMap。
pub mod missing_range {
    use std::collections::{BTreeMap, HashMap};

    pub fn notional_hash(levels: &HashMap<i64, i64>, lo: i64, hi: i64) -> i128 {
        // ❌ O(n) 全表扫描
        levels
            .iter()
            .filter(|(&px, _)| px >= lo && px <= hi)
            .map(|(&px, &qty)| px as i128 * qty as i128)
            .sum()
    }

    pub fn notional_btree(levels: &BTreeMap<i64, i64>, lo: i64, hi: i64) -> i128 {
        // ✅ O(log n + k)
        levels.range(lo..=hi).map(|(&px, &qty)| px as i128 * qty as i128).sum()
    }

    pub fn demonstrate() {
        println!("## 陷阱 4：HashMap 做不了 range");

        let mut hm = HashMap::new();
        let mut bt = BTreeMap::new();
        for i in 0..10_000i64 {
            hm.insert(i * 100, 10);
            bt.insert(i * 100, 10);
        }

        let lo = 450_000;
        let hi = 550_000;
        assert_eq!(notional_hash(&hm, lo, hi), notional_btree(&bt, lo, hi));
        println!("结果相同，但 HashMap 扫描 {} 档，BTreeMap 只扫带内", hm.len());
        println!("规则：先列操作集合 —— 有 range 就必须 BTreeMap\n");
    }
}

// ============================================================================
// 陷阱 5：该用 HashMap 却用了 BTreeMap —— 无谓的 log n
// ============================================================================
/// **现象**：ClOrdID 点查 P99 比同事系统慢 3x。
/// **根因**：BTreeMap O(log n) 比较 + 指针追逐，HashMap O(1) 均摊。
/// **修法**：纯点查、不需要顺序 → HashMap。
pub mod btree_overkill {
    use std::collections::{BTreeMap, HashMap};

    pub fn demonstrate() {
        println!("## 陷阱 5：BTreeMap 做点查是大材小用");

        let n = 100_000u64;
        let mut hm: HashMap<u64, u64> = HashMap::with_capacity(n as usize);
        let mut bt: BTreeMap<u64, u64> = BTreeMap::new();
        for i in 0..n {
            hm.insert(i, i * 2);
            bt.insert(i, i * 2);
        }

        let target = 99_999;
        assert_eq!(hm.get(&target), bt.get(&target));
        println!("get({}) 结果相同", target);
        println!("BTreeMap 每次 get 走 O(log n) 比较；HashMap 均摊 O(1)\n");
    }
}

// ============================================================================
// 陷阱 6：堆/索引不同步 —— lazy delete 遗漏
// ============================================================================
/// **现象**：Mempool pop 返回已取消的 tx，或 panic。
/// **根因**：HashMap 删了但 BinaryHeap 里还有 stale entry。
/// **修法**：pop 时 lazy validate；或维护 heap index 反向映射。
pub mod stale_heap {
    use std::collections::{BinaryHeap, HashMap};

    pub fn demonstrate() {
        println!("## 陷阱 6：多索引结构的不同步");

        let mut live: HashMap<u64, u64> = HashMap::new();
        let mut heap = BinaryHeap::new();

        for (id, fee) in [(1, 100), (2, 200), (3, 150)] {
            live.insert(id, fee);
            heap.push((fee, id));
        }

        // 用户取消 id=2，但 heap 里 (200, 2) 还在
        live.remove(&2);

        // ✅ lazy delete：pop 时检查 live
        let mut best = None;
        while let Some((fee, id)) = heap.pop() {
            if live.contains_key(&id) {
                best = Some((fee, id));
                break;
            }
        }
        println!("pop_best = {:?}（跳过了 stale id=2）", best);
        println!("规则：多视图结构必须定义『哪个是 source of truth』\n");
    }
}

// ============================================================================
// 陷阱 7：无界增长 —— Mempool / Cache 吃掉全部内存
// ============================================================================
/// **现象**：节点 OOM，mempool 几百万笔 stale tx。
/// **根因**：只 insert 不 evict；没有 TTL / cap / fee threshold。
/// **修法**：LRU cap、按 nonce gap 淘汰、最低 fee 截断。
pub mod unbounded_growth {
    use std::collections::HashMap;

    pub const MAX_POOL: usize = 10_000;

    pub struct BoundedPool {
        map: HashMap<u64, u64>,
    }

    impl BoundedPool {
        pub fn insert(&mut self, id: u64, fee: u64) {
            if self.map.len() >= MAX_POOL {
                // 简化：删最低 fee（生产用更精细的 eviction）
                if let Some(worst) = self.map.iter().min_by_key(|(_, f)| *f).map(|(&k, _)| k) {
                    self.map.remove(&worst);
                }
            }
            self.map.insert(id, fee);
        }
    }

    pub fn demonstrate() {
        println!("## 陷阱 7：无界 HashMap 增长");

        let mut pool = BoundedPool { map: HashMap::new() };
        for i in 0..MAX_POOL as u64 + 500 {
            pool.insert(i, i);
        }
        println!("插入 {} 笔后，池大小 capped ≈ {}", MAX_POOL + 500, pool.map.len());
        println!("规则：任何 in-memory 索引都必须有 eviction 策略\n");
    }
}

// ============================================================================
// 陷阱 8：i64 价格 key 的符号/溢出陷阱
// ============================================================================
/// **现象**：跨零 spread 计算错误；极端价格比较反转。
/// **根因**：把价格转 f64 丢精度；或负数 key 的 range 边界写错。
/// **修法**：全程定点整数；range 用 `lo..=hi` 显式闭区间。
pub mod price_key_trap {
    pub fn spread_wrong(bid: i64, ask: i64) -> f64 {
        // ❌ 转 f64 丢精度，且负数行为怪异
        ask as f64 - bid as f64
    }

    pub fn spread_correct(bid: i64, ask: i64) -> i64 {
        // ✅ 整数减法
        ask - bid
    }

    pub fn demonstrate() {
        println!("## 陷阱 8：价格 key 的类型选择");

        let bid = 99_999_999_999i64;
        let ask = 100_000_000_001i64;

        println!("整数 spread = {}", spread_correct(bid, ask));
        println!("f64 spread  = {}", spread_wrong(bid, ask));
        println!("规则：价格/数量永远定点整数；f64 只出现在展示层\n");
    }
}

pub fn demonstrate() {
    linked_list_trap::demonstrate();
    rehash_spike::demonstrate();
    string_key_alloc::demonstrate();
    missing_range::demonstrate();
    btree_overkill::demonstrate();
    stale_heap::demonstrate();
    unbounded_growth::demonstrate();
    price_key_trap::demonstrate();
}
