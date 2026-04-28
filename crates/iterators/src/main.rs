//! Iterators 深度实践入口
//!
//! 模块组织：
//!   - basics       迭代器底层机制
//!   - hft          高频交易生产场景
//!   - web3         Web3 / 区块链生产场景
//!   - pitfalls     常见陷阱
//!   - strategies   泛化的应对策略矩阵

mod basics;
mod hft;
mod pitfalls;
mod strategies;
mod web3;

fn main() {
    println!("=== Iterators 深度实践 ===\n");

    println!("--- 1. 底层机制 ---");
    basics::demonstrate();

    println!("--- 2. HFT 生产场景 ---");
    hft::demonstrate();

    println!("--- 3. Web3 生产场景 ---");
    web3::demonstrate();

    println!("--- 4. 常见陷阱 ---");
    pitfalls::demonstrate();

    println!("--- 5. 泛化的应对策略 ---");
    strategies::demonstrate();
}
