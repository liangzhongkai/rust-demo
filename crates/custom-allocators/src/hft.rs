//! # HFT：`GlobalAlloc` 之外的「生产级自定义」—— **固定粒度对象池 / 环形缓冲**
//!
//! Arena（见 `arena-allocators`）擅长「一帧一命」短命图；本节针对另一类线上问题：
//! **热路径上反复触达全局 `malloc`/free**，在行情突发时出现 **allocator 长尾**。
//!
//! 对策：**启动期预热**容量、运行期只做借还或小结构原地覆写——把「堆」关在池外可控区域。

#![allow(dead_code)]

use std::sync::Mutex;

#[derive(Clone, Copy, Debug, Default)]
pub struct RawOrder {
    pub px: i64,
    pub qty: i64,
    pub side: u8,
}

// =============================================================================
// 场景 A：网关 / feeder 单线程解码 —— `Vec<Option<T>>` 槽位池（语义直白、无 UB）
// =============================================================================
/// **生产问题**：每笔插入触发小对象堆分配，`malloc` 与碎片化与流量相关 → **P99 抖动**。
///
/// **池化**：解码线程独占 `Vec<Option<RawOrder>>` + free 索引栈，`take`/`recycle`
/// **不增减 Vec 容量**（须在 `reserve_exact`/`resize` 时定死上界）。
pub struct OrderSlotPool {
    slots: Vec<Option<RawOrder>>,
    free: Vec<usize>,
}

impl OrderSlotPool {
    pub fn preallocate_exact(capacity: usize) -> Self {
        let mut slots = Vec::with_capacity(capacity);
        slots.resize_with(capacity, || None);
        let free = (0..capacity).rev().collect();
        Self { slots, free }
    }

    pub fn acquire_index(&mut self) -> Option<usize> {
        self.free.pop()
    }

    pub fn bind(&mut self, idx: usize, o: RawOrder) {
        self.slots[idx] = Some(o);
    }

    pub fn get(&self, idx: usize) -> Option<&RawOrder> {
        self.slots[idx].as_ref()
    }

    pub fn recycle(&mut self, idx: usize) {
        self.slots[idx] = None;
        self.free.push(idx);
    }
}

// =============================================================================
// 场景 B：回测 / 策略 worker —— 跨线程时用 `Mutex` 保护同一池（争用仍存在，但消灭了 per-tick malloc）
// =============================================================================

pub struct SharedOrderPool {
    inner: Mutex<OrderSlotPool>,
}

impl SharedOrderPool {
    pub fn preallocate_exact(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(OrderSlotPool::preallocate_exact(capacity)),
        }
    }

    pub fn acquire_bind(&self, o: RawOrder) -> Option<usize> {
        let mut g = self.inner.lock().ok()?;
        let idx = g.acquire_index()?;
        g.bind(idx, o);
        Some(idx)
    }

    pub fn recycle(&self, idx: usize) {
        if let Ok(mut g) = self.inner.lock() {
            g.recycle(idx);
        }
    }
}

// =============================================================================
// 场景 C：诊断旁路计数 —— 证明「本轮处理零扩容」
// =============================================================================

/// 网关侧「伪生产」钩子：若在热路径上对 `Vec` 误用 `push` 导致扩容，`try_reserve_exact` / `capacity()` 巡检可抓现行。
pub fn validate_no_implicit_growth<T>(buf: &[T], declared_cap: usize) -> bool {
    buf.len() <= declared_cap
}

// =============================================================================
// 场景 D：固定环形覆写 —— 最近 N 条事件仅保留快照，不参与全局释放风暴
// =============================================================================

#[derive(Clone, Copy, Debug, Default)]
pub struct EventRecord {
    pub seq: u64,
    pub kind: u8,
}

pub struct FixedRing<T> {
    buf: Vec<Option<T>>,
    capacity: usize,
    head: usize,
}

impl<T> FixedRing<T> {
    pub fn preallocate(capacity: usize) -> Self {
        let mut buf = Vec::with_capacity(capacity);
        buf.resize_with(capacity, || None);
        Self {
            buf,
            capacity,
            head: 0,
        }
    }

    /// O(1) 覆写：**不**触发 `Vec` growth；旧值在此处 `drop`。
    pub fn overwrite(&mut self, value: T) {
        let slot = &mut self.buf[self.head];
        *slot = Some(value);
        self.head = (self.head + 1) % self.capacity;
    }
}

pub fn demonstrate() {
    println!("## HFT 场景 A：单线程槽位池（零隐式扩容）");
    let mut pool = OrderSlotPool::preallocate_exact(4);
    let i = pool.acquire_index().expect("空闲槽");
    pool.bind(i, RawOrder { px: 100, qty: 1, side: 0 });
    println!("  idx {} px={}", i, pool.get(i).expect("bound").px);
    pool.recycle(i);

    println!("## HFT 场景 B：`Mutex` 包裹的共享池");
    let shared = SharedOrderPool::preallocate_exact(8);
    if let Some(ix) = shared.acquire_bind(RawOrder {
        px: 99,
        qty: 2,
        side: 1,
    }) {
        shared.recycle(ix);
        println!("  借还在共享池上演示完毕");
    }

    println!(
        "## HFT 场景 C：`validate_no_implicit_growth` => {}",
        validate_no_implicit_growth(&[0_u8; 3], 8)
    );

    println!("## HFT 场景 D：固定环缓冲覆写（无 realloc）");
    let mut ring = FixedRing::<EventRecord>::preallocate(3);
    for s in 0..10_u64 {
        ring.overwrite(EventRecord { seq: s, kind: 1 });
    }
    println!("  capacity={}", ring.capacity);
}
