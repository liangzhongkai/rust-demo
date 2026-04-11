//! Lock-Free Order Book for High-Frequency Trading
//!
//! 展示 Rust 在 HFT 中的经典特性组合：
//! - AtomicU64/AtomicU32 无锁编程
//! - Arena 分配器 (bumpalo) 减少内存碎片
//! - Unsafe Rust 与内存布局控制
//! - Cache line 对齐避免 false sharing
//! - Unsafe 用于实现无锁链表
//!
//! 适用场景：交易所订单簿、撮合引擎、高频交易系统

use bumpalo::Bump;
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicU32, AtomicU64, Ordering};

/// Cache line 大小 (x86_64)
#[allow(dead_code)]
const CACHE_LINE_SIZE: usize = 64;

/// 订单方向
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Side {
    Buy = 0,
    Sell = 1,
}

/// 订单状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum OrderStatus {
    Open = 0,
    Filled = 1,
    Cancelled = 2,
}

/// 确保类型按缓存行对齐 - 避免 false sharing
#[repr(C, align(64))]
struct AlignedAtomicU64 {
    value: AtomicU64,
    _pad: [u8; 64 - 8], // 填充到缓存行大小
}

/// 订单 - 使用紧凑的内存布局
#[repr(C)]
struct Order {
    /// 订单 ID
    id: u64,
    /// 价格 (缩放以避免浮点数)
    price: u64,
    /// 数量
    qty: u32,
    /// 方向
    side: Side,
    /// 状态
    status: AtomicU32, // 使用原子操作支持无锁更新
    /// 下一个订单 (链表)
    next: AtomicPtr<Order>,
}

impl Order {
    fn new(id: u64, price: u64, qty: u32, side: Side) -> Self {
        Self {
            id,
            price,
            qty,
            side,
            status: AtomicU32::new(OrderStatus::Open as u32),
            next: AtomicPtr::new(ptr::null_mut()),
        }
    }

    fn set_status(&self, status: OrderStatus) {
        self.status.store(status as u32, Ordering::Release);
    }

    fn get_status(&self) -> OrderStatus {
        match self.status.load(Ordering::Acquire) {
            0 => OrderStatus::Open,
            1 => OrderStatus::Filled,
            _ => OrderStatus::Cancelled,
        }
    }
}

/// 价格层级 - 使用跳表风格的节点
#[repr(C)]
struct PriceLevel {
    price: u64,
    total_qty: AtomicU64,
    order_count: AtomicU64,
    /// Arena 分配器中的订单列表头指针
    orders_head: AtomicPtr<Order>,
    next: AtomicPtr<PriceLevel>,
}

impl PriceLevel {
    fn new(price: u64) -> Self {
        Self {
            price,
            total_qty: AtomicU64::new(0),
            order_count: AtomicU64::new(0),
            orders_head: AtomicPtr::new(ptr::null_mut()),
            next: AtomicPtr::new(ptr::null_mut()),
        }
    }
}

/// 无锁订单簿
struct OrderBook {
    /// Arena 分配器 - 所有订单集中分配，减少碎片
    arena: Bump,
    /// 买方价格层级 (降序)
    bids_head: AtomicPtr<PriceLevel>,
    /// 卖方价格层级 (升序)
    asks_head: AtomicPtr<PriceLevel>,
    /// 最佳买价 (缓存)
    best_bid: AlignedAtomicU64,
    /// 最佳卖价 (缓存)
    best_ask: AlignedAtomicU64,
    /// 订单 ID 计数器
    order_id_seq: AtomicU64,
    /// 总成交额
    total_volume: AlignedAtomicU64,
}

impl OrderBook {
    fn new() -> Self {
        Self {
            arena: Bump::new(),
            bids_head: AtomicPtr::new(ptr::null_mut()),
            asks_head: AtomicPtr::new(ptr::null_mut()),
            best_bid: AlignedAtomicU64 {
                value: AtomicU64::new(0),
                _pad: [0; 56],
            },
            best_ask: AlignedAtomicU64 {
                value: AtomicU64::new(u64::MAX),
                _pad: [0; 56],
            },
            order_id_seq: AtomicU64::new(1),
            total_volume: AlignedAtomicU64 {
                value: AtomicU64::new(0),
                _pad: [0; 56],
            },
        }
    }

    /// 在 arena 中分配订单
    fn allocate_order(&self, price: u64, qty: u32, side: Side) -> *mut Order {
        let id = self.order_id_seq.fetch_add(1, Ordering::Relaxed);
        let order = Order::new(id, price, qty, side);
        self.arena.alloc(order)
    }

    /// 在 arena 中分配价格层级
    fn allocate_price_level(&self, price: u64) -> *mut PriceLevel {
        let level = PriceLevel::new(price);
        self.arena.alloc(level)
    }

    /// 限制订单 - 无锁插入
    fn limit_order(&self, price: u64, qty: u32, side: Side) -> u64 {
        let order_ptr = self.allocate_order(price, qty, side);

        unsafe {
            // 尝试与现有订单撮合
            let remaining = self.try_match(order_ptr, side);

            if remaining > 0 {
                // 更新订单数量
                (*order_ptr).qty = remaining;
                // 插入到订单簿
                self.insert_order(order_ptr);
            } else {
                (*order_ptr).set_status(OrderStatus::Filled);
            }

            (*order_ptr).id
        }
    }

    /// 尝试撮合订单 - 返回剩余数量
    unsafe fn try_match(&self, order_ptr: *mut Order, side: Side) -> u32 {
        let order = &*order_ptr;
        let mut remaining = order.qty;
        let match_side = match side {
            Side::Buy => &self.asks_head,
            Side::Sell => &self.bids_head,
        };

        // 遍历对手方价格层级
        let mut level_ptr = match_side.load(Ordering::Acquire);

        while remaining > 0 && !level_ptr.is_null() {
            let level = &*level_ptr;

            // 检查价格是否匹配
            let can_match = match side {
                Side::Buy => order.price >= level.price,
                Side::Sell => order.price <= level.price,
            };

            if !can_match {
                break;
            }

            // 在此价格层级撮合
            remaining = self.match_at_level(level_ptr, remaining, side);

            // 更新最佳买卖价
            self.update_best_prices(side);

            level_ptr = (*level_ptr).next.load(Ordering::Acquire);
        }

        remaining
    }

    /// 在指定价格层级撮合
    unsafe fn match_at_level(&self, level_ptr: *mut PriceLevel, mut qty: u32, _side: Side) -> u32 {
        let level = &*level_ptr;
        let mut order_ptr = level.orders_head.load(Ordering::Acquire);

        while qty > 0 && !order_ptr.is_null() {
            let order = &*order_ptr;
            if order.get_status() != OrderStatus::Open {
                order_ptr = (*order_ptr).next.load(Ordering::Acquire);
                continue;
            }

            // 计算成交数量
            let fill_qty = qty.min(order.qty);

            // 更新订单簿数量
            level
                .total_qty
                .fetch_sub(fill_qty as u64, Ordering::Relaxed);

            // 更新成交额 (无锁)
            self.total_volume
                .value
                .fetch_add(fill_qty as u64 * level.price, Ordering::Relaxed);

            // 更新订单状态
            if fill_qty >= order.qty {
                (*order_ptr).set_status(OrderStatus::Filled);
            }

            qty -= fill_qty;
            order_ptr = (*order_ptr).next.load(Ordering::Acquire);
        }

        qty
    }

    /// 插入订单到订单簿
    unsafe fn insert_order(&self, order_ptr: *mut Order) {
        let order = &*order_ptr;
        let (head, should_descend) = match order.side {
            Side::Buy => (&self.bids_head, true),   // 买方降序
            Side::Sell => (&self.asks_head, false), // 卖方升序
        };

        // 查找或创建价格层级
        let level_ptr = self.find_or_create_level(head, order.price, should_descend);

        // 将订单添加到层级头部
        let level = &*level_ptr;
        (*order_ptr)
            .next
            .store(level.orders_head.load(Ordering::Acquire), Ordering::Release);
        level.orders_head.store(order_ptr, Ordering::Release);
        level.order_count.fetch_add(1, Ordering::Relaxed);
        level
            .total_qty
            .fetch_add(order.qty as u64, Ordering::Relaxed);

        // 更新最佳买卖价
        self.update_best_prices(order.side);
    }

    /// 查找或创建价格层级
    unsafe fn find_or_create_level(
        &self,
        head: &AtomicPtr<PriceLevel>,
        price: u64,
        descend: bool,
    ) -> *mut PriceLevel {
        let mut level_ptr = head.load(Ordering::Acquire);

        // 空订单簿
        if level_ptr.is_null() {
            let new_level = self.allocate_price_level(price);
            if head
                .compare_exchange(
                    ptr::null_mut(),
                    new_level,
                    Ordering::Release,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                return new_level;
            }
            level_ptr = head.load(Ordering::Acquire);
        }

        // 查找正确的位置
        let mut prev: *mut PriceLevel = ptr::null_mut();

        loop {
            let level = &*level_ptr;

            let should_insert = if descend {
                price >= level.price
            } else {
                price <= level.price
            };

            if should_insert {
                if price == level.price {
                    return level_ptr;
                }

                let new_level = self.allocate_price_level(price);
                (*new_level).next.store(level_ptr, Ordering::Release);

                if prev.is_null() {
                    if head
                        .compare_exchange(
                            level_ptr,
                            new_level,
                            Ordering::Release,
                            Ordering::Relaxed,
                        )
                        .is_ok()
                    {
                        return new_level;
                    }
                } else {
                    if (*prev)
                        .next
                        .compare_exchange(
                            level_ptr,
                            new_level,
                            Ordering::Release,
                            Ordering::Relaxed,
                        )
                        .is_ok()
                    {
                        return new_level;
                    }
                }

                // CAS 失败，重试
                let _ = new_level;
                level_ptr = head.load(Ordering::Acquire);
                prev = ptr::null_mut();
                continue;
            }

            prev = level_ptr;
            level_ptr = level.next.load(Ordering::Acquire);

            if level_ptr.is_null() {
                let new_level = self.allocate_price_level(price);
                (*new_level).next.store(ptr::null_mut(), Ordering::Release);

                if (*prev)
                    .next
                    .compare_exchange(
                        ptr::null_mut(),
                        new_level,
                        Ordering::Release,
                        Ordering::Relaxed,
                    )
                    .is_ok()
                {
                    return new_level;
                }

                let _ = new_level;
                level_ptr = (*prev).next.load(Ordering::Acquire);
            }
        }
    }

    /// 更新最佳买卖价缓存
    fn update_best_prices(&self, side: Side) {
        let head = match side {
            Side::Buy => &self.bids_head,
            Side::Sell => &self.asks_head,
        };

        let level_ptr = head.load(Ordering::Acquire);
        if !level_ptr.is_null() {
            unsafe {
                let price = (*level_ptr).price;
                match side {
                    Side::Buy => self.best_bid.value.store(price, Ordering::Release),
                    Side::Sell => self.best_ask.value.store(price, Ordering::Release),
                }
            }
        }
    }

    /// 获取最佳买卖价
    fn get_best_prices(&self) -> (u64, u64) {
        let bid = self.best_bid.value.load(Ordering::Acquire);
        let ask = self.best_ask.value.load(Ordering::Acquire);
        (bid, ask)
    }

    /// 获取总成交额
    fn get_total_volume(&self) -> u64 {
        self.total_volume.value.load(Ordering::Acquire)
    }

    /// 市价订单
    fn market_order(&self, qty: u32, side: Side) -> (u32, u64) {
        let order_id = self.order_id_seq.fetch_add(1, Ordering::Relaxed);

        let order_ptr = self.arena.alloc(Order {
            id: order_id,
            price: 0,
            qty,
            side,
            status: AtomicU32::new(OrderStatus::Open as u32),
            next: AtomicPtr::new(ptr::null_mut()),
        });

        unsafe {
            let remaining = self.try_match(order_ptr, side);
            let mut filled = qty - remaining;
            let mut value = 0u64;

            if side == Side::Buy {
                let mut level_ptr = self.asks_head.load(Ordering::Acquire);
                while filled < qty && !level_ptr.is_null() {
                    let level = &*level_ptr;
                    let level_qty = level.total_qty.load(Ordering::Relaxed) as u32;
                    let take = (qty - filled).min(level_qty);
                    value += take as u64 * level.price;
                    filled += take;
                    level_ptr = level.next.load(Ordering::Acquire);
                }
            }

            (filled, value)
        }
    }
}

fn main() {
    println!("=== Lock-Free Order Book ===\n");

    let book = OrderBook::new();

    // 添加一些初始订单
    println!("Adding orders...");

    // 买方订单
    book.limit_order(100, 10, Side::Buy);
    book.limit_order(105, 5, Side::Buy);
    book.limit_order(98, 20, Side::Buy);
    book.limit_order(102, 8, Side::Buy);

    // 卖方订单
    book.limit_order(101, 15, Side::Sell);
    book.limit_order(103, 10, Side::Sell);
    book.limit_order(99, 5, Side::Sell);
    book.limit_order(106, 20, Side::Sell);

    let (best_bid, best_ask) = book.get_best_prices();
    println!(
        "Best Bid: {}, Best Ask: {}, Spread: {}",
        best_bid,
        best_ask,
        best_ask - best_bid
    );

    // 市价买单
    println!("\nExecuting market buy for 25 units...");
    let (filled, value) = book.market_order(25, Side::Buy);
    println!("Filled: {} / {}, Value: {}", filled, 25, value);
    println!("Total Volume: {}", book.get_total_volume());

    let (best_bid, best_ask) = book.get_best_prices();
    println!(
        "Best Bid: {}, Best Ask: {}, Spread: {}",
        best_bid,
        best_ask,
        best_ask - best_bid
    );

    // 限价订单测试
    println!("\nAdding large limit buy at 102...");
    book.limit_order(102, 50, Side::Buy);

    println!("\n=== Order Book State ===");
    println!("Total Volume: {}", book.get_total_volume());
    let (best_bid, best_ask) = book.get_best_prices();
    println!("Best Bid: {}, Best Ask: {}", best_bid, best_ask);
    println!("Spread: {}", best_ask - best_bid);

    // 性能测试
    println!("\n=== Performance Test ===");
    use std::time::Instant;

    let iterations = 100_000;
    let start = Instant::now();

    for i in 0..iterations {
        let price = 100 + (i % 20);
        let qty = 1 + (i % 10) as u32;
        let side = if i % 2 == 0 { Side::Buy } else { Side::Sell };
        book.limit_order(price, qty, side);
    }

    let elapsed = start.elapsed();
    println!("Inserted {} orders in {:?}", iterations, elapsed);
    println!(
        "Orders/sec: {:.0}",
        iterations as f64 / elapsed.as_secs_f64()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_order() {
        let book = OrderBook::new();
        let id = book.limit_order(100, 10, Side::Buy);
        assert!(id > 0);
    }

    #[test]
    fn test_best_prices() {
        let book = OrderBook::new();

        book.limit_order(100, 10, Side::Buy);
        book.limit_order(101, 10, Side::Sell);

        let (bid, ask) = book.get_best_prices();
        assert_eq!(bid, 100);
        assert_eq!(ask, 101);
    }

    #[test]
    fn test_market_order() {
        let book = OrderBook::new();

        book.limit_order(100, 10, Side::Sell);
        let (filled, value) = book.market_order(5, Side::Buy);

        assert_eq!(filled, 5);
        assert_eq!(value, 500);
    }
}
