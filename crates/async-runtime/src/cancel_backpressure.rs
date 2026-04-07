use std::time::{Duration, Instant};

pub async fn run_demos() {
    println!("══════════ 三、取消、超时与背压 ══════════\n");
    demo_backpressure().await;
    println!();
    demo_timeout_retry().await;
}

// ─────────────────────────────────────────────────────────────
// 题 3.2: 有界 channel 自然背压
//
// 无界 channel 生产者永不阻塞 → 消费者慢时内存持续增长。
// 有界 channel 满时 send().await 挂起，天然限流。
// ─────────────────────────────────────────────────────────────
async fn demo_backpressure() {
    println!("【Demo 3.2】有界 channel 自然背压\n");

    let (tx, mut rx) = tokio::sync::mpsc::channel::<i32>(5); // 容量 5
    let start = Instant::now();

    let producer = tokio::spawn(async move {
        for i in 0..20 {
            tx.send(i).await.unwrap(); // 满时挂起，等消费者腾出空间
            if (i + 1) % 5 == 0 {
                println!("    生产者: 已发送 {} 条, 耗时 {:?}", i + 1, start.elapsed());
            }
        }
        println!("    生产者: 全部发完, {:?}", start.elapsed());
    });

    let consumer = tokio::spawn(async move {
        let mut count = 0u32;
        while let Some(_) = rx.recv().await {
            count += 1;
            tokio::time::sleep(Duration::from_millis(50)).await; // 慢消费
        }
        count
    });

    let (_, count) = tokio::join!(producer, consumer);
    let count = count.unwrap();
    println!("    消费者: 处理 {count} 条, 总耗时 {:?}", start.elapsed());
    println!("    ↑ 生产者被有界 channel(5) 自然限速，不会无限堆积");
}

// ─────────────────────────────────────────────────────────────
// 题 3.3: 超时 + 指数退避重试
//
// 第 1 次调用：对端响应慢 → 超时
// 第 2 次调用：对端返回业务错误
// 第 3 次调用：成功
// ─────────────────────────────────────────────────────────────
async fn demo_timeout_retry() {
    println!("【Demo 3.3】超时 + 指数退避重试\n");

    let start = Instant::now();
    let mut backoff = Duration::from_millis(100);
    let max_retries = 5;

    for attempt in 1..=max_retries {
        println!("    第 {attempt} 次尝试 ({:?})...", start.elapsed());

        let result = tokio::time::timeout(
            Duration::from_millis(200),
            simulate_unreliable_service(attempt),
        )
        .await;

        match result {
            Ok(Ok(val)) => {
                println!("    ✅ 成功: {val} (总耗时 {:?})", start.elapsed());
                return;
            }
            Ok(Err(e)) => {
                println!("    ❌ 业务错误: {e}");
            }
            Err(_) => {
                println!("    ⏱️  超时 (>200ms)");
            }
        }

        if attempt < max_retries {
            println!("    退避 {backoff:?}...");
            tokio::time::sleep(backoff).await;
            backoff = std::cmp::min(backoff * 2, Duration::from_secs(5));
        }
    }
    println!("    ❌ 全部重试失败");
}

async fn simulate_unreliable_service(attempt: u32) -> Result<String, String> {
    match attempt {
        1 => {
            // 响应极慢，会被 timeout 截断
            tokio::time::sleep(Duration::from_secs(10)).await;
            Ok("too slow".into())
        }
        2 => Err("HTTP 500 Internal Server Error".into()),
        _ => {
            tokio::time::sleep(Duration::from_millis(30)).await;
            Ok("order #1024 confirmed".into())
        }
    }
}
