//! # 自定义分配 / 池化里的「生产事故」形态（**带 Runnable 最小反例**）
//!
//! 这些点多与 **所有权 + 容量不变量 + 观测盲区** 相关；下面每一段都是 **可复制到单测里的逻辑骨架**，
//! 不是「纯文档注释」。

#![allow(dead_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::VecDeque;
use std::ffi::CString;
use std::thread;

use crate::basics::StatsAllocator;
use crate::hft::{RawOrder, SharedOrderPool};

pub fn demonstrate() {
    dirty_buffer_length_after_pool();
    unbounded_vec_pool_explodes_rss_outline();
    cstring_ownership_across_abi();
    stats_hook_vs_real_heap_residency();
    shared_mutex_pool_contention_outline();
}

// =============================================================================
/// **坑 1：池化 `Vec<u8>` 归还时忘记 `clear` / `truncate(0)`** → 下一次「以为空缓冲区」却仍带旧 `len`。
fn dirty_buffer_length_after_pool() {
    println!("## 陷阱 1：池中取出但未 clear —— `len` 仍为旧请求长度");

    fn parse_as_if_empty(mut buf: Vec<u8>) {
        println!(
            "  逻辑认为「空白 buffer」，实测 len={}, capacity={}",
            buf.len(),
            buf.capacity()
        );
        let marker = [0_u8];
        buf.extend_from_slice(&marker[..]);
        // 若不 clear：`extend` 会从旧尾部之后写——上一请求前缀仍可见
        println!("  marker 写入后前缀（前 min(8,len)）：{:?}", &buf[..buf.len().min(8)]);
    }

    let mut pooled = Vec::with_capacity(64);
    pooled.extend_from_slice(b"RESP_A");

    println!("错误路径：simulate 放回池但没 clear，直接递给下一个 handler");
    parse_as_if_empty(pooled.clone()); // deliberate bad

    pooled.clear();
    println!("对照：放回前 `clear` 再复用同一 capacity");
    parse_as_if_empty(pooled);
}

// =============================================================================
/// **坑 2：无上限回收池**：流量尖峰时把成千上万个大块 `Vec` 卡在 `deque` 里，**RSS/MMS 永远不回落**。
fn unbounded_vec_pool_explodes_rss_outline() {
    println!("## 陷阱 2：无界缓冲池——峰值后即「永久性驻留」");

    let mut pool: VecDeque<Vec<u8>> = VecDeque::new();
    let burst_sessions = 2_048_usize;

    // 每一段 session 把一个「够用的大 buffer」放回池——却没有 MAX_POOLED
    for _ in 0..burst_sessions {
        let mut resp = Vec::with_capacity(8 * 1024);
        resp.extend_from_slice(b"{\"logs\":[],\"status\":\"OK\"}");
        pool.push_back(resp);
    }

    println!(
        "  演示：单次尖峰塞进池中的 buffer 个数 = {}（生产中即常驻 capacity 量级）",
        pool.len(),
    );

    println!("  **对策：** `VecDeque.len() < MAX_POOLED` 才 recycle，否则直接 `drop`；或在线程私池上分桶。");

    drop(pool); // demo 结束前释放内存
}

// =============================================================================
/// **坑 3：跨 FFI 「谁分配谁释放」**：典型是 C `malloc` 的指针被 Rust `Box::from_raw` / `CString` 不匹配。
///
/// Safe 子集演示：`CString::into_raw` ←→ `CString::from_raw` 必须严格 **一一配对**，不能混入 `libc::free`。
fn cstring_ownership_across_abi() {
    println!("## 陷阱 3：跨边界所有权——`CString::into_raw` 必须拿回 `CString::from_raw`");

    let s = CString::new("simulate_pass_to_c_abi").unwrap();
    let ptr = s.into_raw();
    println!("  `into_raw` 后 Rust 不再自动 Drop，下一行模拟从 C「回调Rust」拿回所有权……");

    unsafe {
        let _revived = CString::from_raw(ptr);
    }

    println!(
        "  **错误心智**：假设 `ptr` 是 `malloc`/`free`，却用 Rust deallocator——或反向——都会 heap corruption。\n\
         **规则**：对齐双方 ABI 文档：`GlobalAlloc`、`malloc`/`free`、`jemalloc` 等成套使用。"
    );

    // 刻意不演示：`CString::from_raw` 对同一 `ptr` 只能用一次，否则会 double-free / UB。
}

// =============================================================================
/// **坑 4：`StatsAllocator` 类钩子只认得 `Layout::size`**，拿不到分配器粒度、tcache、`realloc` 内部路径 → **指标≠会计**。
fn stats_hook_vs_real_heap_residency() {
    println!("## 陷阱 4：统计钩子≠真实常驻——对齐、元数据、分配器缓存不可见");

    let stats = StatsAllocator::new(System);
    unsafe {
        let needle = Layout::from_size_align(1, 1).unwrap();
        let p = stats.alloc(needle);
        let (_, _, live_est) = stats.snapshot();
        println!(
            "  申请 layout size={}，`live_bytes_estimate`≈ {}",
            needle.size(),
            live_est,
        );

        stats.dealloc(p, needle);
    }

    println!("  OS `RSS`/`VmRSS`、jemalloc profiling、heaptrack 才能回答「我到底占了多少」。");
}

// =============================================================================
/// **坑 5：全局 `Mutex` 池**：多 worker 争抢同一把锁，**延迟尾部**可能比省下的 `malloc` 更糟。
fn shared_mutex_pool_contention_outline() {
    println!("## 陷阱 5：Mutex 保护共享池——争用时替代了 malloc contention");

    let pool = SharedOrderPool::preallocate_exact(4096);

    thread::scope(|s| {
        for _ in 0..16 {
            s.spawn(|| {
                for seq in 0..256_u64 {
                    if let Some(idx) = pool.acquire_bind(RawOrder {
                        px: seq as i64,
                        qty: 1,
                        side: 0,
                    }) {
                        pool.recycle(idx);
                    }
                }
            });
        }
    });

    println!(
        "  已完成 16×256 借还演示（仅在说明「所有线程共抢 `SharedOrderPool` 内核」）；\n\
         **对策**：线程局部池 + 批尾上交，或 shard by thread / connection key。"
    );

    // 「纯注释」的补充：这里没有测时；生产用 histogram/tracing/coarsetime，对比私池路径。
}
