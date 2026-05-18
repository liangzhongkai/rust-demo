//! # 自定义分配器基础：`GlobalAlloc` 与可观测性
//!
//! - **HFT / Web3 共性**：线上 OOM、延迟尖刺、碎片往往来自「看不见」的全局堆行为。
//! - **第一步**：在测试/压测里包一层 `GlobalAlloc`，把 **调用次数、活跃字节趋势** 拉出来。

#![allow(dead_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};

/// 包装任意 `GlobalAlloc`，统计 alloc / dealloc 次数与「当前估算活跃字节」。
///
/// **注意**：未覆盖 `realloc` / `alloc_zeroed` 的精细字节差分时，长寿命 `Vec`  growth
/// 可能让「活跃字节」估算偏离真实值——生产里应改用 `#[global_allocator]` + jemalloc/mimalloc 的 profiling，
/// 或 ebpf/USDT。此处用于 **教学：** 证明「热点路径是否真的在捅全局分配器」。
pub struct StatsAllocator<A: GlobalAlloc> {
    pub inner: A,
    pub alloc_calls: AtomicU64,
    pub dealloc_calls: AtomicU64,
    /// best-effort：按 `layout.size()` 增减，不把 realign 计入。
    pub live_bytes_estimate: AtomicU64,
}

impl<A: GlobalAlloc> StatsAllocator<A> {
    pub const fn new(inner: A) -> Self {
        Self {
            inner,
            alloc_calls: AtomicU64::new(0),
            dealloc_calls: AtomicU64::new(0),
            live_bytes_estimate: AtomicU64::new(0),
        }
    }

    pub fn snapshot(&self) -> (u64, u64, u64) {
        (
            self.alloc_calls.load(Ordering::Relaxed),
            self.dealloc_calls.load(Ordering::Relaxed),
            self.live_bytes_estimate.load(Ordering::Relaxed),
        )
    }
}

unsafe impl<A: GlobalAlloc> GlobalAlloc for StatsAllocator<A> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.alloc_calls.fetch_add(1, Ordering::Relaxed);
        self.live_bytes_estimate
            .fetch_add(layout.size() as u64, Ordering::Relaxed);
        self.inner.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.dealloc_calls.fetch_add(1, Ordering::Relaxed);
        self.live_bytes_estimate
            .fetch_sub(layout.size() as u64, Ordering::Relaxed);
        self.inner.dealloc(ptr, layout);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let out = GlobalAlloc::realloc(&self.inner, ptr, layout, new_size);
        if !out.is_null() {
            let old = layout.size() as i64;
            let new = new_size as i64;
            let diff = new - old;
            if diff > 0 {
                self.live_bytes_estimate.fetch_add(diff as u64, Ordering::Relaxed);
            } else if diff < 0 {
                self.live_bytes_estimate
                    .fetch_sub((-diff) as u64, Ordering::Relaxed);
            }
        }
        out
    }
}

/// SIMD / 伪共享：`Layout` **对齐约束**不满足时 `global`/`alloc` 会 panic 或对齐错误。
///
/// **HFT**：把热区结构体对齐到缓存行（常为 64）可降低 false sharing；
/// **Web3**：哈希/签名中间缓冲常要求 32/64 字节对齐以便于 intrinsics。
pub fn demo_aligned_layout(elem_size: usize, align: usize) -> Result<Layout, std::alloc::LayoutError> {
    Layout::from_size_align(elem_size, align)
}

pub fn demonstrate() {
    println!("### basics：StatsAllocator<System>（未挂 global，仅占位演示 API）");

    let stats = StatsAllocator::new(System);
    unsafe {
        let layout = Layout::new::<u64>();
        let p = stats.alloc(layout);
        assert!(!p.is_null());
        stats.dealloc(p, layout);
    }
    let (a, d, live) = stats.snapshot();
    println!("  alloc={a} dealloc={d} live_bytes_est={live}");
    println!("### basics：对齐布局");
    let l72 = demo_aligned_layout(72, 64).expect("aligned layout");
    println!("  72 bytes @ align 64 => size={}, align={}", l72.size(), l72.align());
}
