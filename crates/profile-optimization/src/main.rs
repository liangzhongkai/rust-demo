//! Profile Optimization 深度实践入口
//!
//! 模块组织：
//!   - util       直方图、微基准、StageTimer、热点计数
//!   - basics     perf / criterion / P99 / warmup 底层机制
//!   - hft        高频交易 7 个生产 profiling 场景
//!   - web3       Web3 / 区块链 6 个生产 profiling 场景
//!   - pitfalls   8 个常见 profiling 陷阱
//!   - strategies 泛化的应对策略矩阵

mod basics;
mod hft;
mod pitfalls;
mod strategies;
mod util;
mod web3;

fn main() {
    println!("=== Profile Optimization 深度实践 ===\n");
    println!("提示：性能结论请用 `cargo build --release` 后 perf/criterion 验证\n");

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
