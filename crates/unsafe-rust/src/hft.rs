//! # HFT：`unsafe` 典型生产落脚点
//!
//! - **对齐 / 伪共享**：热路径结构与 cache line。
//! - **零拷贝**：在已校验长度的字节切片上解析定长头；默认用 `from_ne_bytes` 避免未对齐 UB。
//!   若缓冲区有对齐保证，可用 `&*(p as *const T)` 做视图级零拷贝（须在 `SAFETY` 中写清契约）。
//! - **SPSC 环形缓冲**：单生产者单消费者，跨线程无锁；通过 **`SpscProducer` / `SpscConsumer`** 在类型层面拆开角色，`Sync` 仍由手工证明承载在共享 `Arc` 内层上。
//!
//! 现实系统里还会遇到：NUMA pinning、`memory_ordering::*`、HugeTLB、内核旁路 NIC
//! 等；这里把 **Rust 所有权与内存安全边界** 压到最小可运行切片。

#![allow(dead_code)]

use std::cell::UnsafeCell;
use std::mem::size_of;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// 1) 对齐：避免 false sharing（生产里常配合 `#[repr(align(64))]`）
// ---------------------------------------------------------------------------

/// 订单簿某档位的紧凑槽位；按 64 字节对齐，减少与其他热数据同 cache line。
#[repr(C, align(64))]
pub struct AlignedPriceLevel {
    pub price_ticks: i64,
    pub qty_lots: u64,
}

impl AlignedPriceLevel {
    #[inline]
    pub fn new(price_ticks: i64, qty_lots: u64) -> Self {
        Self {
            price_ticks,
            qty_lots,
        }
    }
}

#[test]
fn aligned_level_is_cache_line_sized_typically() {
    // 语义：至少不会因为结构体过小与邻接字段争用同一 line；
    // 具体 64 vs 128 取决于目标 CPU。
    assert!(size_of::<AlignedPriceLevel>() >= 64);
}

// ---------------------------------------------------------------------------
// 2) 零拷贝：「已验证长度」的二进制视图（类似 FIX / 专有 UDP 载荷头）
// ---------------------------------------------------------------------------

/// Fixed 头部 + payload 全部是 `buf` 的子切片。
#[repr(C)]
pub struct WireBookDeltaHeader {
    pub seq: u32,
    pub symbol_id: u32,
}

/// 从任意对齐的 `&[u8]` 解析头部；用 `from_ne_bytes` 避免未对齐指针解引用 UB。
///
/// 若缓冲区 **已按头对齐保证**（如 `#[repr(align(...))]` 的 slab），可手写
/// `unsafe { &*(p as *const WireBookDeltaHeader) }` 做真正零拷贝视图 —— `SAFETY` 必须写明对齐契约。
pub fn try_split_book_delta(buf: &[u8]) -> Option<(WireBookDeltaHeader, &[u8])> {
    let header_len = size_of::<WireBookDeltaHeader>();
    if buf.len() < header_len {
        return None;
    }
    let (head, rest) = buf.split_at(header_len);
    Some((
        WireBookDeltaHeader {
            seq: u32::from_ne_bytes(head[0..4].try_into().ok()?),
            symbol_id: u32::from_ne_bytes(head[4..8].try_into().ok()?),
        },
        rest,
    ))
}

#[test]
fn zero_copy_roundtrip_slice() {
    let mut raw = Vec::new();
    raw.extend_from_slice(&1u32.to_ne_bytes());
    raw.extend_from_slice(&42u32.to_ne_bytes());
    raw.extend_from_slice(&[9u8, 9, 9]);
    let (h, payload) = try_split_book_delta(&raw).expect("layout");
    assert_eq!(h.seq, 1);
    assert_eq!(h.symbol_id, 42);
    assert_eq!(payload, &[9, 9, 9]);
}

// ---------------------------------------------------------------------------
// 3) SPSC ring：`UnsafeCell` + `unsafe impl Sync` + `Arc` 分拆句柄
// ---------------------------------------------------------------------------

struct SpscU64Inner {
    mask: usize,
    buffer: Box<[UnsafeCell<u64>]>,
    head: AtomicUsize,
    tail: AtomicUsize,
}

// SAFETY：与单生产者／单消费者的内存序约定同上；任一 `Arc<SpscU64Inner>` 只通过
// `push`/`pop` 访问槽位对应端（见 Producer/Consumer 文档）。
unsafe impl Sync for SpscU64Inner {}

impl SpscU64Inner {
    fn new(power_of_two_len: usize) -> Option<Self> {
        if power_of_two_len < 2 || !power_of_two_len.is_power_of_two() {
            return None;
        }
        let mut v = Vec::with_capacity(power_of_two_len);
        for _ in 0..power_of_two_len {
            v.push(UnsafeCell::new(0u64));
        }
        Some(Self {
            mask: power_of_two_len - 1,
            buffer: v.into_boxed_slice(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        })
    }
}

/// 仅生产者线程可调 `push`。
pub struct SpscProducer {
    inner: Arc<SpscU64Inner>,
}

/// 仅消费者线程可调 `pop`。
pub struct SpscConsumer {
    inner: Arc<SpscU64Inner>,
}

/// 拆成两半：各自 `clone`/`Send` 到策略线程。**禁止**在两个句柄上对调角色。
pub fn spsc_u64_pair(cap_pow2: usize) -> Option<(SpscProducer, SpscConsumer)> {
    let inner = Arc::new(SpscU64Inner::new(cap_pow2)?);
    Some((SpscProducer {
        inner: Arc::clone(&inner),
    }, SpscConsumer { inner }))
}

impl SpscProducer {
    pub fn push(&self, v: u64) -> Result<(), ()> {
        let i = self.inner.as_ref();
        let tail = i.tail.load(Ordering::Relaxed);
        let head = i.head.load(Ordering::Acquire);
        let cap = i.buffer.len();
        if tail.wrapping_sub(head) >= cap {
            return Err(());
        }
        let idx = tail & i.mask;
        unsafe {
            *i.buffer[idx].get() = v;
        }
        i.tail.store(tail.wrapping_add(1), Ordering::Release);
        Ok(())
    }
}

impl SpscConsumer {
    pub fn pop(&self) -> Option<u64> {
        let i = self.inner.as_ref();
        let head = i.head.load(Ordering::Relaxed);
        let tail = i.tail.load(Ordering::Acquire);
        if head == tail {
            return None;
        }
        let idx = head & i.mask;
        let v = unsafe { *i.buffer[idx].get() };
        i.head.store(head.wrapping_add(1), Ordering::Release);
        Some(v)
    }
}

#[test]
fn spsc_ring_fill_and_full() {
    let (p, c) = spsc_u64_pair(4).expect("ring");
    for k in [10u64, 20, 30, 40] {
        assert!(p.push(k).is_ok());
    }
    assert!(p.push(99).is_err());
    for want in [10, 20, 30, 40] {
        assert_eq!(c.pop(), Some(want));
    }
    assert_eq!(c.pop(), None);
}

#[test]
fn spsc_pair_cross_thread_scope() {
    let (p, c) = spsc_u64_pair(8).unwrap();
    std::thread::scope(|s| {
        let h1 = s.spawn(|| {
            p.push(1).unwrap();
            p.push(2).unwrap();
        });
        let h2 = s.spawn(|| {
            let mut seen = Vec::new();
            for _ in 0..128 {
                if let Some(x) = c.pop() {
                    seen.push(x);
                    if seen.len() == 2 {
                        break;
                    }
                }
                std::thread::yield_now();
            }
            assert_eq!(seen, vec![1, 2]);
        });
        h1.join().unwrap();
        h2.join().unwrap();
    });
}

pub fn demonstrate() {
    println!("## HFT · 对齐热路径槽位");
    let lvl = AlignedPriceLevel::new(100_050, 12);
    println!(
        "`AlignedPriceLevel` size_hint = {} bytes (cache-line-ish padding)",
        size_of::<AlignedPriceLevel>()
    );
    println!("  price_ticks={}, qty_lots={}\n", lvl.price_ticks, lvl.qty_lots);

    println!("## HFT · 零拷贝二进制头拆分");
    let mut buf = Vec::new();
    buf.extend_from_slice(&7u32.to_ne_bytes());
    buf.extend_from_slice(&99u32.to_ne_bytes());
    buf.extend_from_slice(&[1, 2, 3, 4]);
    if let Some((hdr, payload)) = try_split_book_delta(&buf) {
        println!(
            "  header.seq={}, symbol_id={}, payload_len={}",
            hdr.seq,
            hdr.symbol_id,
            payload.len()
        );
    }
    println!("  先把长度与布局校验完，再在子切片上做 `reinterpret`。\n");

    println!("## HFT · SPSC ring（UnsafeCell + Producer/Consumer）");
    let (producer, consumer) = spsc_u64_pair(4).unwrap();
    assert!(producer.push(100).is_ok());
    assert!(producer.push(200).is_ok());
    println!("  push 两笔后 pop: {:?}", consumer.pop());
    println!("  第二笔: {:?}", consumer.pop());
    println!(
        "  `SpscProducer`/`SpscConsumer` 分拆句柄：`Sync` **仍由内层 Ring + 角色约定**承载；类型上避免同线程误双写。\n"
    );
}
