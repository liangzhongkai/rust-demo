//! # Cancellation（取消）
//!
//! 本 crate 以**文字题库**形式考察：协作式取消、传播、关停顺序、副作用与幂等。
//! 与 `async-runtime` crate 中的可运行示例（`cancel_backpressure`、`graceful_shutdown` 等）互补：
//! 此处偏重**问题表述、生产场景与权衡**，运行后打印全部题目。
//!
//! ## 运行
//!
//! ```bash
//! cargo run -p cancellation          # 打印全部
//! cargo run -p cancellation -- 1   # 仅第一节：语义与心智模型
//! cargo run -p cancellation -- 5   # 仅第五节：总表
//! ```

mod exam;

fn main() {
    let section = std::env::args().nth(1).unwrap_or_default();

    match section.as_str() {
        "1" => exam::print_section_1_semantics(),
        "2" => exam::print_section_2_propagation(),
        "3" => exam::print_section_3_side_effects(),
        "4" => exam::print_section_4_races_testing(),
        "5" => exam::print_section_5_summary(),
        _ => {
            if section.is_empty() {
                exam::print_all();
            } else {
                eprintln!("未知参数: {section}");
                eprintln!();
                print_usage();
                std::process::exit(1);
            }
        }
    }
}

fn print_usage() {
    println!("用法: cargo run -p cancellation [section]");
    println!();
    println!("  (缺省)  打印全部章节");
    println!("  1       一、语义与心智模型");
    println!("  2       二、传播、层次与结构化并发");
    println!("  3       三、资源、事务与副作用");
    println!("  4       四、竞态、幂等与测试");
    println!("  5       五、总表：Trade-off 与泛化速查");
}
