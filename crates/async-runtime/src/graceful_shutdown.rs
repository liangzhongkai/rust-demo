use std::time::Duration;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

pub async fn run_demos() {
    println!("══════════ 七、优雅关停与生命周期管理 ══════════\n");
    demo_graceful_shutdown().await;
    println!();
    demo_background_flush().await;
}

// ─────────────────────────────────────────────────────────────
// 题 7.1: CancellationToken + drain 优雅关停
//
// 模拟 3 个 in-flight 请求（100ms / 300ms / 800ms），
// 在 500ms 时发 shutdown 信号。
// 请求 1,2 已完成；请求 3 被取消并执行清理。
// ─────────────────────────────────────────────────────────────
async fn demo_graceful_shutdown() {
    println!("【Demo 7.1】CancellationToken + drain 优雅关停\n");
    println!("    模拟 3 个请求: 100ms / 300ms / 800ms");
    println!("    在 500ms 时发出 shutdown 信号\n");

    let token = CancellationToken::new();
    let mut set = JoinSet::new();

    let delays = [100u64, 300, 800];
    for (i, &delay_ms) in delays.iter().enumerate() {
        let child = token.child_token();
        set.spawn(async move {
            tokio::select! {
                _ = async {
                    println!("    请求 {}: 开始处理 (需 {delay_ms}ms)", i + 1);
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    println!("    请求 {}: ✅ 正常完成", i + 1);
                } => {}
                _ = child.cancelled() => {
                    println!("    请求 {}: ⚠️  收到取消信号，清理中...", i + 1);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    println!("    请求 {}: 清理完毕", i + 1);
                }
            }
        });
    }

    // 500ms 后发 shutdown
    tokio::time::sleep(Duration::from_millis(500)).await;
    println!("\n    ═══ 500ms: 发出 shutdown 信号 ═══\n");
    token.cancel();

    // drain：等所有任务完成（含清理）
    while let Some(result) = set.join_next().await {
        result.unwrap();
    }

    println!("\n    所有任务已完成（含清理），可以安全退出");
    println!("    ↑ 请求 1,2 在信号前完成；请求 3 被取消并执行了清理");
}

// ─────────────────────────────────────────────────────────────
// 题 7.2: 后台任务感知取消 + final flush
//
// 后台任务每 200ms 批量写入一次。
// 收到取消信号后执行最终 flush 再退出，不丢数据。
// ─────────────────────────────────────────────────────────────
async fn demo_background_flush() {
    println!("【Demo 7.2】后台任务感知取消 + final flush\n");

    let token = CancellationToken::new();
    let buffer = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::<String>::new()));

    // 生产者：持续写入 buffer
    let buf_w = buffer.clone();
    let producer = tokio::spawn(async move {
        for i in 1..=20 {
            buf_w.lock().await.push(format!("event-{i}"));
            tokio::time::sleep(Duration::from_millis(80)).await;
        }
    });

    // 后台 flush 任务：每 300ms 刷一次，感知取消后做 final flush
    let buf_f = buffer.clone();
    let child = token.child_token();
    let flusher = tokio::spawn(async move {
        let mut flushed_total = 0u32;
        let mut interval = tokio::time::interval(Duration::from_millis(300));
        interval.tick().await; // 第一次立即触发，跳过

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let batch: Vec<_> = buf_f.lock().await.drain(..).collect();
                    if !batch.is_empty() {
                        flushed_total += batch.len() as u32;
                        println!("    [flush] 定期写入 {} 条 (累计 {flushed_total})", batch.len());
                    }
                }
                _ = child.cancelled() => {
                    // 最终 flush：确保不丢数据
                    let batch: Vec<_> = buf_f.lock().await.drain(..).collect();
                    flushed_total += batch.len() as u32;
                    println!("    [flush] ⚠️  取消信号！final flush {} 条 (累计 {flushed_total})", batch.len());
                    break;
                }
            }
        }
        flushed_total
    });

    // 700ms 后发 shutdown
    tokio::time::sleep(Duration::from_millis(700)).await;
    println!("    ═══ 700ms: 发出 shutdown 信号 ═══");
    token.cancel();

    // 等 flusher 完成 final flush
    let total = flusher.await.unwrap();
    producer.abort(); // 停止生产者

    println!("\n    final flush 完成，共写入 {total} 条，无数据丢失");
    println!("    ↑ CancellationToken + select! 保证了最终清理");
}
