//! Data Structures 深度实践入口
//!
//! 模块组织：
//!   - basics       数据结构底层机制与选型原则
//!   - hft          高频交易生产场景（7 个）
//!   - web3         Web3 / 区块链生产场景（6 个）
//!   - pitfalls     常见陷阱（8 个）
//!   - strategies   泛化的应对策略矩阵（8 个）

mod basics;
mod hft;
mod pitfalls;
mod strategies;
mod web3;

fn main() {
    println!("=== Data Structures 深度实践 ===\n");

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
