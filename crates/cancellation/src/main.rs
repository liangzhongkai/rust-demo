//! # Cancellation（取消）— 可运行示例
//!
//! 覆盖八类生产级取消场景：
//! 语义基线、传播与 supervision、副作用回滚、竞态终态、
//! cancel-safety、spawn_blocking、优雅停机、有界并发。
//!
//! ## 运行
//!
//! ```bash
//! cargo run -p cancellation              # 全部章节
//! cargo run -p cancellation -- 1         # 仅第一节
//! cargo run -p cancellation -- 7         # 仅第七节（优雅停机）
//! cargo run -p cancellation -- all       # 顺序跑 1–8
//! ```

mod exam;

#[tokio::main]
async fn main() {
    let section = std::env::args().nth(1).unwrap_or_default();

    match section.as_str() {
        "1" => exam::run_section_1_semantics().await,
        "2" => exam::run_section_2_propagation().await,
        "3" => exam::run_section_3_side_effects().await,
        "4" => exam::run_section_4_races_testing().await,
        "5" => exam::run_section_5_cancel_safety().await,
        "6" => exam::run_section_6_spawn_blocking().await,
        "7" => exam::run_section_7_graceful_shutdown().await,
        "8" => exam::run_section_8_bounded_concurrency().await,
        "all" | "" => exam::run_all_sections().await,
        other => {
            eprintln!("未知参数: {other}");
            eprintln!();
            print_usage();
            std::process::exit(1);
        }
    }
}

fn print_usage() {
    println!("用法: cargo run -p cancellation [section]");
    println!();
    println!("  (缺省)  运行全部（等同 all）");
    println!("  1       语义：协作式取消、drop JoinHandle、timeout vs token");
    println!("  2       传播：JoinSet + 子 token，一处失败取消 peer");
    println!("  3       副作用：TxGuard 回滚、DropGuard 传播取消");
    println!("  4       竞态：完成与取消互斥终态（单一出口）");
    println!("  5       Cancel-safety：select 分支内累积状态 = 丢数据");
    println!("  6       spawn_blocking 的协作取消（is_cancelled 轮询）");
    println!("  7       优雅停机：宽限期 drain + 超时 abort_all");
    println!("  8       Semaphore 有界并发 + 可取消 permit 获取");
    println!("  all     按顺序运行 1–8");
}
