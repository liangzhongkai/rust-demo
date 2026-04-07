mod cancel_backpressure;
mod exam;
mod executor_and_blocking;
mod graceful_shutdown;
mod pin_and_future;
mod structured_concurrency;

#[tokio::main]
async fn main() {
    let section = std::env::args().nth(1).unwrap_or_default();

    match section.as_str() {
        "exam" => exam::print_all(),
        "1" | "2" => executor_and_blocking::run_demos().await,
        "3" => cancel_backpressure::run_demos().await,
        "5" => structured_concurrency::run_demos().await,
        "6" => pin_and_future::run_demos().await,
        "7" => graceful_shutdown::run_demos().await,
        _ => {
            println!("=== Async Runtime 可运行示例 ===\n");
            println!("用法: cargo run -p async-runtime -- <section>\n");
            println!("  exam  打印题库（文字版）");
            println!("  1     执行模型与阻塞");
            println!("  3     取消、超时与背压");
            println!("  5     结构化并发");
            println!("  6     Pin 与 Future 状态机");
            println!("  7     优雅关停");
            println!("  all   运行全部示例\n");

            if section == "all" {
                run_all().await;
            }
        }
    }
}

async fn run_all() {
    let sep = "=".repeat(60);
    executor_and_blocking::run_demos().await;
    println!("\n{sep}\n");
    cancel_backpressure::run_demos().await;
    println!("\n{sep}\n");
    structured_concurrency::run_demos().await;
    println!("\n{sep}\n");
    pin_and_future::run_demos().await;
    println!("\n{sep}\n");
    graceful_shutdown::run_demos().await;
}
