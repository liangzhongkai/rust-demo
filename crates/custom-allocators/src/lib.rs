//! 自定义分配：围绕 `GlobalAlloc`、对齐、字节池化与 **生产可观测性** 的范例集合。
//! 二进制入口见 `main.rs`：`cargo run -p custom-allocators`。

pub mod basics;
pub mod hft;
pub mod pitfalls;
pub mod strategies;
pub mod web3;

/// 顺序执行所有小节演示输出。
pub fn run_all_demonstrations() {
    println!("=== 自定义分配器 / 池化 / GlobalAlloc 实践 ===\n");

    println!("--- 1. GlobalAlloc 基础与观测 ---");
    basics::demonstrate();

    println!("--- 2. HFT：槽位池 / 固定环 / 零隐式扩容 ---");
    hft::demonstrate();

    println!("--- 3. Web3：RPC buffer 池 / 预分配 / 批重置 ---");
    web3::demonstrate();

    println!("--- 4. 常见陷阱 ---");
    pitfalls::demonstrate();

    println!("--- 5. 泛化策略 ---");
    strategies::demonstrate();
}
