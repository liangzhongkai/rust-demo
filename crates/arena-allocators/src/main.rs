//! Arena / bump allocator 深度实践入口
//!
//! 模块组织：
//!   - basics      Arena 语义、生命周期、与全局分配器的取舍
//!   - hft         低延迟交易中的「请求级 / 批处理级」内存池
//!   - web3        模拟、解码、递归结构中的短命对象批量回收
//!   - pitfalls    生产里最常见的误用与事故模式
//!   - strategies  从场景抽象出的决策矩阵与可复用模板

mod basics;
mod hft;
mod pitfalls;
mod strategies;
mod web3;

fn main() {
    println!("=== Arena / Bump 分配器 深度实践 ===\n");

    println!("--- 1. 底层语义与 API ---");
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
