//! Lifetimes 深度实践入口
//!
//! 模块组织：
//!   - basics     显式 `'a`、结构体视图、省略规则、多 lifetime
//!   - hft        高频交易 7 个生产场景
//!   - web3       Web3 / 区块链 6 个生产场景
//!   - pitfalls   8 个常见陷阱
//!   - strategies 泛化的应对策略矩阵
//!   - summary    决策备忘

mod basics;
mod hft;
mod pitfalls;
mod strategies;
mod summary;
mod web3;

fn main() {
    println!("=== Lifetimes 深度实践 ===\n");
    println!("核心：编译期证明「借用活多久」—— 零拷贝 vs 边界 owned\n");

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

    summary::print_decision_cheat_sheet();
}
