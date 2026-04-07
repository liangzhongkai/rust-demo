use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

pub async fn run_demos() {
    println!("══════════ 五、结构化并发与任务生命周期 ══════════\n");
    demo_joinset_error_handling().await;
    println!();
    demo_select_cancel_safety().await;
    println!();
    demo_try_join_fail_fast().await;
}

// ─────────────────────────────────────────────────────────────
// 题 5.1: JoinSet 收集 spawn 任务的错误与 panic
//
// fire-and-forget（丢弃 JoinHandle）→ 错误/panic 静默丢失。
// JoinSet 持有全部句柄，逐个收割结果。
// ─────────────────────────────────────────────────────────────
async fn demo_joinset_error_handling() {
    println!("【Demo 5.1】JoinSet 收集 spawn 任务的错误与 panic\n");

    // 临时屏蔽 panic 输出，保持 demo 输出整洁
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));

    let mut set = tokio::task::JoinSet::new();

    set.spawn(async { Ok::<_, String>("task-A: 正常完成".into()) });

    set.spawn(async { Err::<String, _>("task-B: 余额不足".into()) });

    set.spawn(async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        Ok("task-C: 延迟后完成".into())
    });

    set.spawn(async {
        panic!("task-D: 数组越界");
    });

    println!("    已 spawn 4 个任务：成功 / 业务错误 / 延迟成功 / panic\n");

    while let Some(result) = set.join_next().await {
        match result {
            Ok(Ok(val)) => println!("    ✅ 成功: {val}"),
            Ok(Err(e)) => println!("    ❌ 业务错误: {e}"),
            Err(e) if e.is_panic() => println!("    💥 panic 被捕获: {e}"),
            Err(e) => println!("    ⚠️  任务被取消: {e}"),
        }
    }

    std::panic::set_hook(default_hook);
    println!("\n    ↑ JoinSet 收集了所有结果（包括 panic），没有静默丢失");
}

// ─────────────────────────────────────────────────────────────
// 题 5.3: select! 丢弃分支的副作用
//
// branch_a 执行多步事务操作（每步 200ms），
// branch_b 在 300ms 后触发。select! 选择 b，a 被 drop。
// a 的 step 1 和 step 2 已执行，但 step 3（提交）未执行。
// ─────────────────────────────────────────────────────────────
async fn demo_select_cancel_safety() {
    println!("【Demo 5.3】select! 丢弃分支的副作用\n");

    let log = Arc::new(Mutex::new(Vec::<String>::new()));

    let log_a = log.clone();
    let branch_a = async move {
        log_a.lock().await.push("step 1: 开始事务".into());
        tokio::time::sleep(Duration::from_millis(200)).await;
        log_a.lock().await.push("step 2: 写入数据".into());
        tokio::time::sleep(Duration::from_millis(200)).await;
        // ↓ 这一行在 300ms 时 branch_b 赢了之后永远不会执行
        log_a.lock().await.push("step 3: 提交事务".into());
        "branch_a: 事务完成"
    };

    let branch_b = async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        "branch_b: 超时触发"
    };

    let winner = tokio::select! {
        v = branch_a => v,
        v = branch_b => v,
    };

    println!("    select! 胜者: {winner}");
    println!("    branch_a 实际执行的步骤:");
    for step in log.lock().await.iter() {
        println!("      ✓ {step}");
    }
    println!("      ✗ step 3: 提交事务  ← 未执行！Future 被 drop");
    println!("\n    ↑ 数据已写入但未提交 → 不一致状态");
    println!("    对策：事务操作不应在 select! 中拆分；或 spawn 隔离副作用");
}

// ─────────────────────────────────────────────────────────────
// 题 5.4: try_join! fail-fast vs join! 等全部
//
// 三个任务：A(500ms ok), B(100ms err), C(500ms ok)
// join! 等到 500ms 才返回全部结果。
// try_join! 在 100ms B 失败时立即返回，drop 掉 A 和 C。
// ─────────────────────────────────────────────────────────────
async fn demo_try_join_fail_fast() {
    println!("【Demo 5.4】try_join! fail-fast vs join! 等全部\n");

    // ❌ join!: 等所有完成才返回
    let start = Instant::now();
    let (r1, r2, r3) = tokio::join!(
        async {
            tokio::time::sleep(Duration::from_millis(500)).await;
            Ok::<String, String>("A ok".into())
        },
        async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            Err::<String, String>("B failed!".into())
        },
        async {
            tokio::time::sleep(Duration::from_millis(500)).await;
            Ok::<String, String>("C ok".into())
        },
    );
    println!("  ❌ join!  总耗时 {:?}", start.elapsed());
    println!("     A={r1:?}  B={r2:?}  C={r3:?}");
    println!("     ↑ B 在 100ms 就失败了，但还是等了 500ms\n");

    // ✅ try_join!: 第一个 Err 立即返回
    let start = Instant::now();
    let result = tokio::try_join!(
        async {
            tokio::time::sleep(Duration::from_millis(500)).await;
            Ok::<String, String>("A ok".into())
        },
        async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            Err::<String, String>("B failed!".into())
        },
        async {
            tokio::time::sleep(Duration::from_millis(500)).await;
            Ok::<String, String>("C ok".into())
        },
    );
    println!("  ✅ try_join! 总耗时 {:?}", start.elapsed());
    match result {
        Ok((a, b, c)) => println!("     全部成功: {a}, {b}, {c}"),
        Err(e) => println!("     首个错误即返回: {e}"),
    }
    println!("     ↑ ~100ms 就返回，节省了 400ms 等待");
}
