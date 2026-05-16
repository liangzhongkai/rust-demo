//! # FFI 常见陷阱（生产复盘高频）
//!
//! 这里不放「刻意触发 UB」的示例代码 —— 只列出 **签名层面就该避免的坑**，
//! 与 `strategies::demonstrate` 里的矩阵交叉引用。

#![allow(dead_code)]

pub fn demonstrate() {
    println!("## 陷阱 1：`Vec` / `String` 增长导致悬挂指针");
    println!(
        "若把 `vec.as_mut_ptr()` 交给 native 侧 **长期保存**，下一次 `push` 可能 relocation。\n\
         对策：`'static` slab、`Pin`、或 native 回调只在调用栈生命周期内生效。\n"
    );

    println!("## 陷阱 2：panic 跨过 FFI");
    println!(
        "默认 unwind 进入 C/C++ 是未定义行为。\n\
         对策：`panic = abort`（whole crate / edge crate）；或在边界 `catch_unwind` 后映射为错误码。\n"
    );

    println!("## 陷阱 3：`CString::new` 含有 inner NUL → `Err`");
    println!(
        "路径／用户名一类字段若直接抛给只能读 `char*` 的老库，会先踩 Rust API。\n\
         对策：在边界明确编码（percent-encode / replace NUL），并把失败变成业务错误。\n"
    );

    println!("## 陷阱 4：`bool` / `enum` / `usize` ABI");
    println!(
        "`bool` 在 C 并非普适 1 字节；`enum` 除非 `#[repr(C)]` 否则别过边；\n\
         `usize` 与 `size_t` 通常一致但仍建议在 header 层 typedef 明示。\n"
    );

    println!("## 陷阱 5：线程亲和与 `Send`");
    println!(
        "vendor handle 可能是线程局部的；Rust `Arc` + 多线程 poll 并不等于安全。\n\
         对策：**每个 FFI handle 绑定单独线程**或读写锁封装 vendor 文档约束。\n"
    );
}
