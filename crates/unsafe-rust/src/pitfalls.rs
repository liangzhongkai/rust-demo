//! # 常见陷阱（对照表 —— 不要复制到生产）
//!
//! 这里 **只打印说明**，把典型错误模式列清楚，与 `hft` / `web3` 中的安全封装对照。

pub fn demonstrate() {
    println!("## 陷阱 1：未初始化就读");
    println!("  误：`let x: u64 = std::mem::uninitialized();`");
    println!("  正：`MaybeUninit` + 证明初始化后再 `assume_init`。\n");

    println!("## 陷阱 2：先 `transmute` / 指针强转再检查长度");
    println!("  误：`let h = &*(p as *const Header)`，之后才检查 len。");
    println!("  正：先用 `split_at(size_of::<T>())` 再解码（`from_ne_bytes`/`read_unaligned`）；仅在证明对齐后对原始缓冲使用 `&*ptr`。\n");

    println!("## 陷阱 3：误给 `Arc`/`Rc` 做 `unsafe impl Send`");
    println!("  误：为了塞进 `tokio::spawn` 而盲目 impl。");
    println!("  正：单所有者跨线程请用 **`Send` 类型组成的结构**；或 `Arc<Mutex<T>>`。\n");

    println!("## 陷阱 4：SPSC ring 的角色约束被破坏");
    println!("  误：两个线程同时 `push`，或把 `SpscConsumer` 传到两个线程。");
    println!("  正：`SpscProducer`/`SpscConsumer` 拆分 + 评审约定 **每端单线程**。\n");

    println!("## 陷阱 5：忽略 FFI 生命线");
    println!("  误：把 `&RustStruct` 裸指针塞进 C，回调回来时 Rust 侧已 drop。");
    println!("  正：`Box::into_raw` + 配套 `destroy` FFI，或由 C 侧拥有内存。\n");

    println!("## 陷阱 6：opaque 指针 double-free / 泄露");
    println!("  误：C 回调里 `destroy` 两次，或从未 `from_raw`。");
    println!("  正：`session_create`/`session_destroy` 成对文档；计数所有权可用 `Arc` 留在 Rust 侧。\n");
}
