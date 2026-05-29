//! # HFT 生产场景下的数据结构
//!
//! 高频交易的硬约束：
//! - **延迟**：热路径 O(1) 或 O(log n)，禁止不可预测的 rehash / alloc
//! - **正确**：价格用定点整数，订单簿 FIFO 公平性
//! - **吞吐**：cache-friendly 连续内存，避免 pointer chasing
//!
//! 下面 7 个场景是真实交易系统里的高频写法。每个场景都标注：
//! - 用了什么数据结构
//! - 解决什么生产问题
//! - 选错结构会踩什么坑

#![allow(dead_code)]

pub type Px = i64;
pub type Qty = i64;
pub type TsNs = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Clone)]
pub struct Order {
    pub id: u64,
    pub side: Side,
    pub px: Px,
    pub qty: Qty,
}

// ============================================================================
// 场景 1：L2 订单簿（BTreeMap + VecDeque）
// ============================================================================
/// **生产问题**：维护 bid/ask 两棵价格树，每档价格内 FIFO 排队。
/// 需要：最优价 O(1)、插入 O(log P + 1)、按价区扫描。
///
/// **数据结构**：`BTreeMap<Px, VecDeque<Order>>` —— 外层有序，内层 FIFO。
/// 卖盘用升序 BTreeMap，买盘用 `Reverse<Px>` 或单独维护 max。
pub mod order_book {
    use super::*;
    use std::collections::{BTreeMap, VecDeque};
    use std::cmp::Reverse;

    pub struct Book {
        /// ask: 价格升序 → first_key_value = best ask
        asks: BTreeMap<Px, VecDeque<Order>>,
        /// bid: 用 Reverse 让 BTreeMap 升序 = 价格降序 → first = best bid
        bids: BTreeMap<Reverse<Px>, VecDeque<Order>>,
    }

    impl Book {
        pub fn new() -> Self {
            Self { asks: BTreeMap::new(), bids: BTreeMap::new() }
        }

        pub fn add(&mut self, o: Order) {
            match o.side {
                Side::Buy => self.bids.entry(Reverse(o.px)).or_default().push_back(o),
                Side::Sell => self.asks.entry(o.px).or_default().push_back(o),
            }
        }

        pub fn best_bid(&self) -> Option<Px> {
            self.bids.keys().next().map(|Reverse(p)| *p)
        }

        pub fn best_ask(&self) -> Option<Px> {
            self.asks.keys().next().copied()
        }

        pub fn spread(&self) -> Option<Px> {
            Some(self.best_ask()? - self.best_bid()?)
        }

        /// 消费最优 ask 的一小部分（撮合模拟）
        pub fn take_from_best_ask(&mut self, qty: Qty) -> Qty {
            let px = match self.asks.keys().next() {
                Some(px) => *px,
                None => return 0,
            };
            let level = match self.asks.get_mut(&px) {
                Some(level) => level,
                None => return 0,
            };
            let mut filled = 0;
            while filled < qty {
                let Some(front) = level.front_mut() else {
                    break;
                };
                let take = (qty - filled).min(front.qty);
                front.qty -= take;
                filled += take;
                if front.qty == 0 {
                    level.pop_front();
                }
            }
            if level.is_empty() {
                self.asks.remove(&px);
            }
            filled
        }
    }

    pub fn demonstrate() {
        println!("## 场景 1：L2 订单簿（BTreeMap + VecDeque FIFO）");

        let mut book = Book::new();
        book.add(Order { id: 1, side: Side::Sell, px: 101_00, qty: 5 });
        book.add(Order { id: 2, side: Side::Sell, px: 100_00, qty: 8 });
        book.add(Order { id: 3, side: Side::Buy, px: 99_00, qty: 10 });
        book.add(Order { id: 4, side: Side::Buy, px: 100_00, qty: 3 });

        println!("best bid = {:?}, best ask = {:?}", book.best_bid(), book.best_ask());
        println!("spread = {:?}", book.spread());

        let filled = book.take_from_best_ask(10);
        println!("taker 吃 ask 10 手 → 成交 {}", filled);
        println!("剩余 best ask = {:?}", book.best_ask());
        println!("关键：BTreeMap 给 range + 最优价；VecDeque 给 O(1) FIFO\n");
    }
}

// ============================================================================
// 场景 2：ClOrdID 索引（HashMap 双向映射）
// ============================================================================
/// **生产问题**：交易所回报带 ClOrdID，OMS 要在微秒内找到内部订单做
/// amend/cancel/fill 更新。字符串 key 会分配，生产用整数或预哈希 ID。
///
/// **数据结构**：`HashMap<u64, OrderState>` + 可选 `HashMap<u64, Px>` 反向索引。
pub mod clordid_index {
    use super::*;
    use std::collections::HashMap;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Status {
        Live,
        Partial,
        Filled,
        Cancelled,
    }

    #[derive(Debug, Clone)]
    pub struct OrderState {
        pub id: u64,
        pub px: Px,
        pub leaves_qty: Qty,
        pub status: Status,
    }

    pub struct OmsIndex {
        by_clord: HashMap<u64, OrderState>,
        /// 价格 → 该价位上的 clord id 集合（撤单时快速定位档位）
        by_price: HashMap<Px, Vec<u64>>,
    }

    impl OmsIndex {
        pub fn with_capacity(n: usize) -> Self {
            Self {
                by_clord: HashMap::with_capacity(n),
                by_price: HashMap::with_capacity(n / 4),
            }
        }

        pub fn insert(&mut self, state: OrderState) {
            self.by_price.entry(state.px).or_default().push(state.id);
            self.by_clord.insert(state.id, state);
        }

        pub fn on_fill(&mut self, clord: u64, fill_qty: Qty) -> Option<&OrderState> {
            let st = self.by_clord.get_mut(&clord)?;
            st.leaves_qty -= fill_qty;
            st.status = if st.leaves_qty == 0 { Status::Filled } else { Status::Partial };
            Some(st)
        }

        pub fn cancel_at_price(&mut self, px: Px) -> Vec<u64> {
            self.by_price.remove(&px).unwrap_or_default()
        }
    }

    pub fn demonstrate() {
        println!("## 场景 2：ClOrdID 索引（HashMap O(1) 点查）");

        let mut oms = OmsIndex::with_capacity(4096);
        oms.insert(OrderState { id: 1001, px: 100_00, leaves_qty: 50, status: Status::Live });
        oms.insert(OrderState { id: 1002, px: 100_00, leaves_qty: 30, status: Status::Live });

        if let Some(st) = oms.on_fill(1001, 20) {
            println!("fill 后 1001: leaves={}, status={:?}", st.leaves_qty, st.status);
        }

        let cancelled = oms.cancel_at_price(100_00);
        println!("价位 100.00 批量撤单: {:?}", cancelled);
        println!("关键：整数 key + with_capacity；辅助索引支持批量操作\n");
    }
}

// ============================================================================
// 场景 3：SPSC 环形缓冲区（固定数组 + 原子索引）
// ============================================================================
/// **生产问题**：行情网关 → 策略线程的单生产者单消费者队列。
/// 不能 malloc，不能锁。经典 ring buffer。
///
/// **数据结构**：`[T; N]` + read/write 索引（单线程版用 usize，多线程用 Atomic）。
pub mod ring_buffer {
    #[derive(Debug, Clone, Copy, Default)]
    struct Trade {
        ts_ns: u64,
        px: i64,
        qty: i64,
    }

    pub struct SpscRing<T: Copy + Default, const N: usize> {
        buf: [T; N],
        head: usize, // 消费者读
        tail: usize, // 生产者写
    }

    impl<T: Copy + Default, const N: usize> SpscRing<T, N> {
        pub fn new() -> Self {
            Self { buf: [T::default(); N], head: 0, tail: 0 }
        }

        /// 容量 N-1（留一个空位区分满/空）
        pub fn push(&mut self, item: T) -> bool {
            let next = (self.tail + 1) % N;
            if next == self.head {
                return false; // full —— 生产事故：要么丢弃要么覆盖最旧
            }
            self.buf[self.tail] = item;
            self.tail = next;
            true
        }

        pub fn pop(&mut self) -> Option<T> {
            if self.head == self.tail {
                return None;
            }
            let item = self.buf[self.head];
            self.head = (self.head + 1) % N;
            Some(item)
        }

        pub fn len(&self) -> usize {
            if self.tail >= self.head {
                self.tail - self.head
            } else {
                N - self.head + self.tail
            }
        }
    }

    pub fn demonstrate() {
        println!("## 场景 3：SPSC 环形缓冲区（固定数组，0 alloc）");

        let mut ring: SpscRing<Trade, 8> = SpscRing::new();
        for i in 0..7 {
            assert!(ring.push(Trade { ts_ns: i, px: 100_00 + i as i64, qty: 1 }));
        }
        assert!(!ring.push(Trade { ts_ns: 99, px: 0, qty: 0 })); // 满

        let mut count = 0;
        while ring.pop().is_some() {
            count += 1;
        }
        println!("写入 7 笔、第 8 笔被拒，读出 {} 笔", count);
        println!("关键：预分配 + 模运算；HFT 进阶换 cache-line 对齐的原子索引\n");
    }
}

// ============================================================================
// 场景 4：延迟触发订单（BinaryHeap 定时器）
// ============================================================================
/// **生产问题**：GTD / 定时单 / TWAP slice 要在指定时间触发。
/// 每秒数千次 insert，需要 O(log n) 取最近到期。
///
/// **数据结构**：`BinaryHeap<Reverse<(TsNs, OrderId)>>` —— 最小堆按时间。
pub mod timed_orders {
    use super::*;
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;

    pub struct TimerWheel {
        heap: BinaryHeap<Reverse<(TsNs, u64)>>,
    }

    impl TimerWheel {
        pub fn new() -> Self {
            Self { heap: BinaryHeap::new() }
        }

        pub fn schedule(&mut self, trigger_ns: TsNs, order_id: u64) {
            self.heap.push(Reverse((trigger_ns, order_id)));
        }

        /// 弹出所有 <= now 的到期订单
        pub fn poll(&mut self, now_ns: TsNs) -> Vec<u64> {
            let mut fired = Vec::new();
            while let Some(&Reverse((ts, id))) = self.heap.peek() {
                if ts > now_ns {
                    break;
                }
                self.heap.pop();
                fired.push(id);
            }
            fired
        }
    }

    pub fn demonstrate() {
        println!("## 场景 4：定时触发（BinaryHeap 最小堆）");

        let mut tw = TimerWheel::new();
        tw.schedule(1_000_000, 101);
        tw.schedule(500_000, 102);
        tw.schedule(2_000_000, 103);

        let fired = tw.poll(1_500_000);
        println!("@1.5ms 触发: {:?}", fired);
        println!("关键：Reverse 把 max-heap 变 min-heap；peek 不弹出 → 批量 poll\n");
    }
}

// ============================================================================
// 场景 5：Symbol 元数据 LRU（HashMap + 侵入式链表模拟）
// ============================================================================
/// **生产问题**：每个 tick 都要查 contract multiplier、tick size、limit band。
/// 全量 HashMap 太大，热 symbol 才值得缓存。
///
/// **数据结构**：`HashMap` + `VecDeque` 维护 LRU 顺序（教学简化版）。
/// 生产常用 `lru` crate 或自定义 intrusive list。
pub mod symbol_lru {
    use std::collections::{HashMap, VecDeque};

    #[derive(Debug, Clone, Copy)]
    pub struct Spec {
        pub tick: i64,
        pub multiplier: i64,
    }

    pub struct LruCache<K: Eq + std::hash::Hash + Clone> {
        map: HashMap<K, Spec>,
        order: VecDeque<K>,
        cap: usize,
    }

    impl<K: Eq + std::hash::Hash + Clone> LruCache<K> {
        pub fn new(cap: usize) -> Self {
            Self { map: HashMap::with_capacity(cap), order: VecDeque::with_capacity(cap), cap }
        }

        pub fn get(&mut self, key: &K) -> Option<Spec> {
            if !self.map.contains_key(key) {
                return None;
            }
            // 移到 MRU 端
            self.order.retain(|k| k != key);
            self.order.push_back(key.clone());
            self.map.get(key).copied()
        }

        pub fn put(&mut self, key: K, spec: Spec) {
            if self.map.len() >= self.cap && !self.map.contains_key(&key) {
                if let Some(evict) = self.order.pop_front() {
                    self.map.remove(&evict);
                }
            }
            self.order.retain(|k| k != &key);
            self.order.push_back(key.clone());
            self.map.insert(key, spec);
        }
    }

    pub fn demonstrate() {
        println!("## 场景 5：Symbol LRU 缓存（HashMap + VecDeque 顺序）");

        let mut cache = LruCache::new(3);
        for (sym, tick) in [("BTC", 1), ("ETH", 1), ("SOL", 1), ("AVAX", 1)] {
            cache.put(sym, Spec { tick, multiplier: 1 });
        }
        println!("cap=3，插入 4 个 → BTC 被驱逐");
        println!("get BTC = {:?}", cache.get(&"BTC"));
        println!("get SOL = {:?}", cache.get(&"SOL"));
        println!("关键：热路径 get 必须是 O(1)；LRU 顺序可以 O(n) 简化或换 intrusive\n");
    }
}

// ============================================================================
// 场景 6：价格带风控扫描（BTreeMap::range）
// ============================================================================
/// **生产问题**：风控要查「某 symbol 在 [mid - band, mid + band] 内的总 notional」。
/// 不能遍历全部挂单。
///
/// **数据结构**：`BTreeMap<Px, Qty>` 聚合档位 + `range()` O(log n + k)。
pub mod band_risk {
    use super::*;
    use std::collections::BTreeMap;

    pub struct NotionalBook {
        levels: BTreeMap<Px, Qty>,
    }

    impl NotionalBook {
        pub fn new() -> Self {
            Self { levels: BTreeMap::new() }
        }

        pub fn upsert(&mut self, px: Px, qty: Qty) {
            if qty == 0 {
                self.levels.remove(&px);
            } else {
                self.levels.insert(px, qty);
            }
        }

        pub fn notional_in_band(&self, mid: Px, band: Px) -> i128 {
            let lo = mid - band;
            let hi = mid + band;
            self.levels
                .range(lo..=hi)
                .map(|(&px, &qty)| px as i128 * qty as i128)
                .sum()
        }
    }

    pub fn demonstrate() {
        println!("## 场景 6：价格带 notional（BTreeMap::range）");

        let mut nb = NotionalBook::new();
        for (px, qty) in [(98_00, 10), (99_00, 20), (100_00, 15), (105_00, 50)] {
            nb.upsert(px, qty);
        }
        let mid = 100_00;
        let band = 2_00;
        let exposure = nb.notional_in_band(mid, band);
        println!("mid={}, band=±2 → 带内 notional = {}", mid, exposure);
        println!("关键：HashMap 无法 range；这是 BTreeMap 的杀手级 API\n");
    }
}

// ============================================================================
// 场景 7：订单对象池（Vec 空闲栈 + HashMap 活跃集）
// ============================================================================
/// **生产问题**：每秒新建/销毁数万 Order 对象 → allocator 压力、碎片、延迟尖峰。
/// 解法：预分配 pool，用完归还，热路径零 alloc。
///
/// **数据结构**：`Vec<OrderSlot>` + `Vec<usize>` 空闲索引栈 + `HashMap<id, usize>`。
pub mod order_pool {
    use super::*;
    use std::collections::HashMap;

    #[derive(Debug, Clone)]
    struct OrderSlot {
        id: u64,
        px: Px,
        qty: Qty,
        alive: bool,
    }

    pub struct OrderPool {
        slots: Vec<OrderSlot>,
        free: Vec<usize>,
        active: HashMap<u64, usize>,
    }

    impl OrderPool {
        pub fn with_capacity(n: usize) -> Self {
            let mut slots = Vec::with_capacity(n);
            let mut free = Vec::with_capacity(n);
            for i in 0..n {
                slots.push(OrderSlot { id: 0, px: 0, qty: 0, alive: false });
                free.push(n - 1 - i); // 栈：后进先出
            }
            Self { slots, free, active: HashMap::with_capacity(n) }
        }

        pub fn acquire(&mut self, id: u64, px: Px, qty: Qty) -> Option<usize> {
            let idx = self.free.pop()?;
            self.slots[idx] = OrderSlot { id, px, qty, alive: true };
            self.active.insert(id, idx);
            Some(idx)
        }

        pub fn release(&mut self, id: u64) -> bool {
            let Some(idx) = self.active.remove(&id) else {
                return false;
            };
            self.slots[idx].alive = false;
            self.free.push(idx);
            true
        }

        pub fn live_count(&self) -> usize {
            self.active.len()
        }
    }

    pub fn demonstrate() {
        println!("## 场景 7：订单对象池（Vec 栈 + HashMap 索引）");

        let mut pool = OrderPool::with_capacity(4);
        pool.acquire(1, 100_00, 10);
        pool.acquire(2, 101_00, 5);
        pool.release(1);
        pool.acquire(3, 99_00, 8);

        println!("活跃订单 = {}", pool.live_count());
        println!("free 栈剩余 = {}", 4 - pool.live_count());
        println!("关键：acquire/release O(1)；HFT 进阶用 bump arena + 索引代替 Box\n");
    }
}

pub fn demonstrate() {
    order_book::demonstrate();
    clordid_index::demonstrate();
    ring_buffer::demonstrate();
    timed_orders::demonstrate();
    symbol_lru::demonstrate();
    band_risk::demonstrate();
    order_pool::demonstrate();
}
