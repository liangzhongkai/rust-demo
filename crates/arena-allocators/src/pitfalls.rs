//! # Arena 生产事故模式（提前预防）
//!
//! 真实故障往往不是「arena 慢」，而是 **语义误用** —— 尤其在把 GC 语言习惯带到 Rust 时。

#![allow(dead_code)]

use bumpalo::Bump;

pub fn demonstrate() {
    drop_order_gotcha();
    thread_pinning_gotcha();
    reset_invalidates_pointers();
    overuse_when_heap_is_fine();
}

/// bump 上 `alloc(T)` **不会** 在丢弃 arena 时调用 `T::drop`（除非用 `bumpalo::boxed::Box`）。
/// **生产症状**：FD/套接字/锁 guard 放进 arena → 「泄漏」或重复 close。
fn drop_order_gotcha() {
    println!("## 陷阱 1：`Drop` 类型默认不在 bump 上析构");
    println!(
        "若把 `File`、`TcpStream`、锁 guard 放进 bump，离开作用域时 **不会** 自动 RAII 清理。\n\
         对策：只有 POD/纯数据进 bump；需析构的用全局 `Box` / 池化句柄 / `bumpalo::boxed::Box::new_in`。\n"
    );
}

/// `Bump` **不是** `Sync`：两个线程并发 `alloc` 会产生数据竞争。
/// **生产症状**：把同一个 `&Bump` 塞进 `rayon`/`tokio` worker → MI / flaky。
fn thread_pinning_gotcha() {
    println!("## 陷阱 2：单 arena 不跨线程");
    println!(
        "模式：每线程 TLS 一个 `Bump`，或每任务 `Bump::new()`；跨线程传递 **只送数据，不送 bump 句柄**。\n\
         `Sync` 替代：`bumpalo::Bump` + 外部 `Mutex` 会直接把热路径锁死 —— HFT 几乎从不这么干。\n"
    );
}

/// `unsafe Bump::reset()`（若使用）会使 **所有** 指向该 arena 的引用立刻悬空。
/// **生产症状**：跨 `await` 保存了 `&ArenaObj` → 下一轮 reactor 复位 bump → UAF。
fn reset_invalidates_pointers() {
    println!("## 陷阱 3：`reset` / 整块释放 = 所有借用一次性作废");
    println!(
        "规则：arena 借用的生命周期 ≤ 本次 `process_*` 调用栈。禁止存进长期 `struct`。\n\
         异步：Future 状态机里持 `&Bump` 产出值 —— `await` 点之前必须降到 `'static` 或 `Vec`。\n"
    );
}

/// 不是「有短命对象就该 arena」：对象很少、分配不在热路径时，直接用 `Vec` 更清晰。
fn overuse_when_heap_is_fine() {
    println!("## 陷阱 4：小批/冷路径滥用 arena");
    let n = 3_usize;
    let bump = Bump::new();
    let xs = bump.alloc_slice_copy(&[1_i64, 2, 3][..n]);
    println!(
        "例：仅 {} 个 `i64` 也进 bump —— 调试负担 > 收益。冷路径优先可读性与工具链（heaptrack）。\n\
         经验阈值（启发式）：热路径每 tick >256 次小分配，或**可证明** allocator 争用，再上 arena。\n\
         演示 slice = {:?}\n",
        n, xs
    );
}
