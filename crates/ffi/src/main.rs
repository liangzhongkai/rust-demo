//! FFI 深度实践入口
//!
//! 模块组织：
//!   - hft          高频交易：Rust → C vendor 封装
//!   - web3         Web3：输出缓冲 + opaque session
//!   - pitfalls     5 个常见陷阱（正确 vs 陷阱代码对照）
//!   - strategies   泛化的应对策略矩阵

use ffi::{hft, pitfalls, strategies, web3};

fn main() {
    println!("=== FFI 深度实践 ===\n");

    println!("--- 1. HFT 生产场景 ---");
    hft::demonstrate();

    println!("--- 2. Web3 生产场景 ---");
    web3::demonstrate();

    println!("--- 3. 常见陷阱 ---");
    pitfalls::demonstrate();

    println!("--- 4. 泛化的应对策略 ---");
    strategies::demonstrate();
}
