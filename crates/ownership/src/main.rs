//! Ownership 完整示例 - 从基础到深入
//!
//! 运行所有示例以全面理解所有权系统

mod basics;
mod advanced;
mod pitfalls;
mod real_world;

fn main() {
    println!("=== Ownership 深度实践 ===\n");

    println!("--- 1. 基础概念 ---");
    basics::demonstrate();

    println!("\n--- 2. 进阶特性 ---");
    advanced::demonstrate();

    println!("\n--- 3. 常见陷阱 ---");
    pitfalls::demonstrate();

    println!("\n--- 4. 实际应用 ---");
    real_world::demonstrate();
}
