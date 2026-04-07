use std::sync::Arc;
use std::time::{Duration, Instant};

pub async fn run_demos() {
    println!("══════════ 一/二、执行模型、Send 与阻塞 ══════════\n");

    tokio::task::spawn_blocking(demo_blocking_stalls_executor)
        .await
        .unwrap();

    println!();
    demo_mutex_across_await().await;
}

// ─────────────────────────────────────────────────────────────
// 题 1.2 + 2.1: std::thread::sleep 阻塞 executor 线程
//
// 在单线程 runtime 中，一个 task 的阻塞调用会卡住同线程
// 上所有其他 task。spawn_blocking 将工作移到独立线程池。
// ─────────────────────────────────────────────────────────────
fn demo_blocking_stalls_executor() {
    println!("【Demo 1.2/2.1】阻塞调用卡住 executor vs spawn_blocking\n");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    // ❌ 在 async task 中直接 std::thread::sleep
    let (cpu_bad, io_bad) = rt.block_on(async {
        let start = Instant::now();

        let cpu = tokio::spawn(async move {
            std::thread::sleep(Duration::from_millis(500)); // 阻塞 executor 线程
            start.elapsed()
        });
        let io = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await; // 应该 50ms 就完成
            start.elapsed()
        });

        let (c, i) = tokio::join!(cpu, io);
        (c.unwrap(), i.unwrap())
    });

    println!("  ❌ 直接在 async 中 std::thread::sleep（单线程 runtime）:");
    println!("     CPU task 完成于 {cpu_bad:?}");
    println!("     I/O task 完成于 {io_bad:?}  ← 预期 ~50ms，被拖到 ~500ms!");
    println!();

    // ✅ 使用 spawn_blocking 隔离阻塞调用
    let (cpu_good, io_good) = rt.block_on(async {
        let start = Instant::now();

        let cpu = tokio::task::spawn_blocking(move || {
            std::thread::sleep(Duration::from_millis(500)); // 在独立线程池运行
            start.elapsed()
        });
        let io = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            start.elapsed()
        });

        let (c, i) = tokio::join!(cpu, io);
        (c.unwrap(), i.unwrap())
    });

    println!("  ✅ 使用 spawn_blocking 隔离:");
    println!("     CPU task 完成于 {cpu_good:?}");
    println!("     I/O task 完成于 {io_good:?}  ← 不受阻塞影响，~50ms");
}

// ─────────────────────────────────────────────────────────────
// 题 2.3: Mutex guard 跨 await 导致并发度下降
//
// std::sync::Mutex 不跨 await → 短暂持锁，I/O 并行
// tokio::sync::Mutex 跨 await → 安全但 I/O 串行化
// ─────────────────────────────────────────────────────────────
async fn demo_mutex_across_await() {
    println!("【Demo 2.3】Mutex guard 跨 await 导致并发度下降\n");

    // ✅ 不跨 await 持锁：I/O 在锁外并行
    let counter = Arc::new(std::sync::Mutex::new(0u64));
    let start = Instant::now();
    let mut handles = Vec::new();
    for i in 0..5u64 {
        let counter = counter.clone();
        handles.push(tokio::spawn(async move {
            // I/O 阶段不持锁 → 5 个 task 并行 sleep
            tokio::time::sleep(Duration::from_millis(100)).await;
            let result = i * 10;
            // 短暂持锁写入
            *counter.lock().unwrap() += result;
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    let elapsed_parallel = start.elapsed();
    let val = *counter.lock().unwrap();
    println!("  ✅ 不跨 await 持锁: {elapsed_parallel:?}, counter = {val}");
    println!("     ↑ 5 个 task 并行 sleep 100ms ≈ 总 ~100ms\n");

    // ⚠️ tokio::sync::Mutex 跨 await：I/O 被串行化
    let counter = Arc::new(tokio::sync::Mutex::new(0u64));
    let start = Instant::now();
    let mut handles = Vec::new();
    for i in 0..5u64 {
        let counter = counter.clone();
        handles.push(tokio::spawn(async move {
            let mut guard = counter.lock().await; // 获得锁
            tokio::time::sleep(Duration::from_millis(100)).await; // 持锁做 I/O！
            *guard += i * 10;
            // guard drop 后下一个 task 才能拿锁
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    let elapsed_serial = start.elapsed();
    let val = *counter.lock().await;
    println!("  ⚠️  tokio::Mutex 跨 await: {elapsed_serial:?}, counter = {val}");
    println!("     ↑ 锁跨 await → I/O 串行化 ≈ 总 ~500ms");
    println!("     教训：即使用 async mutex，也应尽量缩短持锁区间");
}
