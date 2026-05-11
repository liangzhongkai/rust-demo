//! # Unsafe Rust — HFT / Web3 生产场景与泛化策略
//!
//! 演示常见 **生产问题如何通过「受限的 unsafe + 文档化不变量」** 解决，
//! 并映射到一般性工程设计原则。
//!
//! 运行：`cargo run -p unsafe-rust` · 测试：`cargo test -p unsafe-rust`
//!
//! 说明：unsafe 仅限于子模块内有文档的不变式封装；crate 根部不直接使用 unsafe。

pub mod hft;
pub mod pitfalls;
pub mod strategies;
pub mod web3;

/// 控制台演示入口（`main.rs` 转发至此）。
pub fn run_all_demos() {
    println!("=== Unsafe Rust 深度实践（HFT · Web3 · 泛化）===\n");

    println!("--- 1. HFT 生产场景 ---");
    hft::demonstrate();

    println!("--- 2. Web3 生产场景 ---");
    web3::demonstrate();

    println!("--- 3. 常见陷阱（对照） ---");
    pitfalls::demonstrate();

    println!("--- 4. 泛化的应对策略 ---");
    strategies::demonstrate();
}
