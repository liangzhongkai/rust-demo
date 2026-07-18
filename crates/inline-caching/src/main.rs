//! Inline Caching 深度实践入口
//!
//! 模块组织：
//!   - basics     单态 / 多态 / megamorphic / generation guard
//!   - hft        高频交易 7 个生产 IC 场景
//!   - web3       Web3 / 区块链 6 个生产 IC 场景
//!   - pitfalls   8 个常见陷阱
//!   - strategies 泛化的应对策略矩阵

mod basics;
mod hft;
mod pitfalls;
mod strategies;
mod web3;

fn main() {
    println!("=== Inline Caching 深度实践 ===\n");
    println!("核心：在调用点旁缓存「上次成功路径」，用廉价 guard 命中后直达结果\n");

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
