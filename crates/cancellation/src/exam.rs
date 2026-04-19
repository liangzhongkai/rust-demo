//! 可运行的取消相关示例（Tokio + `tokio_util::sync::CancellationToken`）。
//!
//! 覆盖八类生产级场景：
//! 1. 协作式取消的语义基线
//! 2. 层次传播 + 结构化 supervision（`JoinSet`）
//! 3. 副作用、回滚与 `DropGuard` 传播
//! 4. 完成 vs 取消的单一终态
//! 5. Cancel-safety：哪些 future 能安全放进 `select!`
//! 6. `spawn_blocking` / CPU 密集型的协作取消
//! 7. 优雅停机（宽限期 → 强制 abort）
//! 8. 有界并发（`Semaphore`）+ 可取消的 permit 获取

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex, Notify, Semaphore};
use tokio::task::JoinSet;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

// ─── 一、语义与心智模型 ─────────────────────────────────────────

pub async fn run_section_1_semantics() {
    println!("────────── 一、语义与心智模型 ──────────\n");
    demo_cooperative_cancel_at_await().await;
    println!();
    demo_join_handle_drop_is_not_abort().await;
    println!();
    demo_timeout_vs_cancellation_token().await;
}

/// 协作式取消：只在 `await` 边界停下；循环里需配合 `select!` 或轮询 token。
async fn demo_cooperative_cancel_at_await() {
    println!("【1.1】协作式取消：work 与 `child.cancelled()` 竞速\n");

    let root = CancellationToken::new();
    let child = root.child_token();

    let worker = tokio::spawn(async move {
        for step in 1..=5 {
            tokio::select! {
                // biased：先检查取消，避免 work 分支饿死 cancel。
                biased;
                _ = child.cancelled() => {
                    println!("    work: 在 step {step} 附近收到取消，停止协作");
                    return;
                }
                _ = tokio::time::sleep(Duration::from_millis(120)) => {
                    println!("    work: step {step}/5");
                }
            }
        }
        println!("    work: 全部完成（未收到取消）");
    });

    tokio::time::sleep(Duration::from_millis(350)).await;
    println!("    main: 350ms 后发出取消\n");
    root.cancel();

    worker.await.unwrap();
    println!("\n    ↑ 取消不是「立刻杀掉线程」，而是在 await 点汇合后退出");
}

/// `JoinHandle` 被 `drop` 时任务变为 detached，默认不会停止。
async fn demo_join_handle_drop_is_not_abort() {
    println!("【1.2】`drop(JoinHandle)`：任务继续跑（除非显式 `abort`）\n");

    let (tx, rx) = tokio::sync::oneshot::channel::<()>();

    let handle = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(80)).await;
        println!("    detached: 仍在运行（父句柄已 drop）");
        let _ = tx.send(());
    });

    drop(handle);
    println!("    main: 已 drop JoinHandle，等待 150ms…");
    tokio::time::sleep(Duration::from_millis(150)).await;
    rx.await.unwrap();
    println!("\n    ↑ 需要 `abort()` / token / 结构化 join 才能对齐生命周期");
}

/// `timeout` 是时间上限；`CancellationToken` 表达外部意图，二者可组合。
async fn demo_timeout_vs_cancellation_token() {
    println!("【1.3】`timeout`（上限）与 `token`（意图）组合在 `select!` 里\n");

    let token = CancellationToken::new();

    tokio::select! {
        biased;
        _ = token.cancelled() => {
            println!("    分支: token 取消");
        }
        _ = tokio::time::sleep(Duration::from_millis(200)) => {
            println!("    分支: 200ms 超时（模拟）");
        }
        _ = tokio::time::sleep(Duration::from_secs(10)) => {
            println!("    分支: 长任务完成（本 demo 不会走到）");
        }
    }

    println!("\n    ↑ 生产里常写成：`select! {{ biased; token, timeout, actual_work }}`");
}

// ─── 二、传播、层次与结构化并发 ───────────────────────────────

pub async fn run_section_2_propagation() {
    println!("────────── 二、传播：fan-out 失败则取消兄弟 ──────────\n");
    demo_fanout_cancel_siblings().await;
}

/// 三个 peer 并行；任意一个报告失败后 `root.cancel()`，其余在下一个 await 点停下。
/// 使用 `JoinSet` 做结构化 supervision：一次 `join_next` 循环吃完所有结果。
async fn demo_fanout_cancel_siblings() {
    println!("【2.1】三路子任务 + 子 token + `JoinSet`：一路 Err → 取消其余\n");

    let root = CancellationToken::new();
    let mut set: JoinSet<Result<char, String>> = JoinSet::new();

    for id in ['A', 'B', 'C'] {
        let child = root.child_token();
        set.spawn(async move {
            let ms: u64 = match id {
                'A' => 100,
                'B' => 220,
                'C' => 400,
                _ => 100,
            };
            tokio::select! {
                biased;
                _ = child.cancelled() => {
                    println!("    peer {id}: ⛔ 被兄弟失败取消");
                    Ok(id)
                }
                _ = tokio::time::sleep(Duration::from_millis(ms)) => {
                    if id == 'B' {
                        Err(format!("peer {id}: 下游 500"))
                    } else {
                        println!("    peer {id}: ✅ 成功");
                        Ok(id)
                    }
                }
            }
        });
    }

    // 结构化聚合：第一条 Err 触发整棵子树取消，之后继续 drain。
    let mut first_err: Option<String> = None;
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => {
                if first_err.is_none() {
                    println!("\n    orchestrator: 收到 {e}，取消整棵子树\n");
                    first_err = Some(e);
                    root.cancel();
                }
            }
            Err(join_err) => {
                // 任务 panic 或被 abort。
                eprintln!("    orchestrator: join error = {join_err}");
            }
        }
    }

    println!(
        "\n    ↑ `try_join!` 只聚合错误；`JoinSet` + 子 token = 第一错误立即省下游占用"
    );
}

// ─── 三、资源、事务与副作用 ───────────────────────────────────

pub async fn run_section_3_side_effects() {
    println!("────────── 三、副作用：取消路径上的释放 / 回滚 ──────────\n");
    demo_tx_guard_rollback_on_cancel().await;
    println!();
    demo_drop_guard_propagates_cancel().await;
}

struct TxGuard {
    label: &'static str,
    committed: bool,
}

impl TxGuard {
    fn new(label: &'static str) -> Self {
        println!("    {label}: BEGIN");
        Self {
            label,
            committed: false,
        }
    }

    fn commit(&mut self) {
        self.committed = true;
        println!("    {}: COMMIT", self.label);
    }
}

impl Drop for TxGuard {
    fn drop(&mut self) {
        if !self.committed {
            println!("    {}: ROLLBACK（Future 被 drop / 路径未提交）", self.label);
        }
    }
}

/// `select!` 输掉的分支被 drop → `TxGuard` 在未 `commit` 时自动回滚。
async fn demo_tx_guard_rollback_on_cancel() {
    println!("【3.1】Drop 里补偿：模拟事务守卫 + `select!` 取消分支\n");

    let fast_ok = async {
        let mut tx = TxGuard::new("tx-fast");
        tokio::time::sleep(Duration::from_millis(60)).await;
        tx.commit();
        "fast"
    };

    let slow_tx = async {
        let mut tx = TxGuard::new("tx-slow");
        tokio::time::sleep(Duration::from_millis(200)).await;
        tx.commit();
        "slow"
    };

    tokio::select! {
        v = fast_ok => println!("    select 赢家: {v}"),
        v = slow_tx => println!("    select 赢家: {v}"),
    }

    println!("\n    ↑ 真实系统里 Drop 回滚可能不够（async 释放需额外通道，见连接池）");
}

/// `DropGuard`：父 Future 被 drop 时自动 `cancel()` token，避免子任务泄漏。
/// 生产模式：父 await 被 `select!`/`timeout` 取消时，子树能确定性地收到信号。
async fn demo_drop_guard_propagates_cancel() {
    println!("【3.2】`DropGuard`：父 Future 被 drop 时自动取消子 token\n");

    let root = CancellationToken::new();
    let child = root.child_token();

    let child_task = tokio::spawn(async move {
        child.cancelled().await;
        println!("    child: 父 Future 被 drop → 我收到了取消");
    });

    let parent_work = async {
        // guard 的存在期 = parent_work 的生命周期；
        // 它被 drop 时 root.cancel() 必然触发，无论是正常退出还是被 select 淘汰。
        let _guard = root.clone().drop_guard();
        tokio::time::sleep(Duration::from_secs(10)).await;
    };

    // 父 Future 被 timeout 抛弃，drop 走 DropGuard。
    let _ = timeout(Duration::from_millis(100), parent_work).await;
    child_task.await.unwrap();
    println!("\n    ↑ 结构化并发里，`DropGuard` 是「生命周期闭合」的最后一道保险");
}

// ─── 四、竞态、幂等与测试 ─────────────────────────────────────

pub async fn run_section_4_races_testing() {
    println!("────────── 四、完成 vs 取消：互斥终态 ──────────\n");
    demo_single_terminal_state().await;
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Terminal {
    Running = 0,
    Completed = 1,
    Cancelled = 2,
}

/// 完成与取消只应有一个成功 `compare_exchange`，下游只消费一次。
/// Ordering 选择：`AcqRel` 保证成功 CAS 前的写对其他线程可见；`Acquire` 用于失败分支读回最新值。
async fn demo_single_terminal_state() {
    println!("【4.1】`AtomicU8` 单一终态 + `Notify` 一次唤醒\n");

    let state = Arc::new(AtomicU8::new(Terminal::Running as u8));
    let notify = Arc::new(Notify::new());
    let log = Arc::new(Mutex::new(Vec::<String>::new()));

    let s1 = state.clone();
    let n1 = notify.clone();
    let log1 = log.clone();
    let completer = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(80)).await;
        let won = s1
            .compare_exchange(
                Terminal::Running as u8,
                Terminal::Completed as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok();
        if won {
            log1.lock().await.push("notify: completed".into());
            // Notify 有 permit 语义：即便 `notified()` 尚未注册，也能唤醒下一个。
            n1.notify_one();
        } else {
            log1.lock().await.push("complete: lost race".into());
        }
    });

    let s2 = state.clone();
    let n2 = notify.clone();
    let log2 = log.clone();
    let canceller = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(80)).await;
        let won = s2
            .compare_exchange(
                Terminal::Running as u8,
                Terminal::Cancelled as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok();
        if won {
            log2.lock().await.push("notify: cancelled".into());
            n2.notify_one();
        } else {
            log2.lock().await.push("cancel: lost race".into());
        }
    });

    notify.notified().await;
    completer.await.unwrap();
    canceller.await.unwrap();

    let entries = log.lock().await.clone();
    for line in &entries {
        println!("    {line}");
    }

    let terminal = state.load(Ordering::Acquire);
    println!(
        "\n    最终状态: {}",
        match terminal {
            x if x == Terminal::Completed as u8 => "Completed",
            x if x == Terminal::Cancelled as u8 => "Cancelled",
            _ => "Running",
        }
    );
    println!("\n    ↑ 把「通知下游」放在同一 CAS 的成功分支，避免双重投递");
}

// ─── 五、Cancel-safety：哪些 future 能安全放进 select! ───────

pub async fn run_section_5_cancel_safety() {
    println!("────────── 五、Cancel-safety：丢弃分支 = 丢弃局部状态 ──────────\n");
    demo_cancel_unsafe_partial_progress().await;
    println!();
    demo_cancel_safe_loop().await;
}

/// 反例：在 `select!` 的一个分支里累积状态，取消时整个 future 连同局部 `Vec` 一起被 drop。
async fn demo_cancel_unsafe_partial_progress() {
    println!("【5.1】反例：select 分支里累积状态 → 取消即丢数据\n");

    let (tx, mut rx) = mpsc::channel::<u32>(8);
    let token = CancellationToken::new();

    // 生产者：慢慢喂 5 条消息。
    let producer = tokio::spawn(async move {
        for i in 0..5 {
            tokio::time::sleep(Duration::from_millis(40)).await;
            if tx.send(i).await.is_err() {
                return;
            }
        }
    });

    let tok_for_cancel = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(110)).await;
        tok_for_cancel.cancel();
    });

    // 把累积塞进 select 分支：取消时 items 随 future 被 drop，数据全部丢失。
    let collect_unsafe = async {
        let mut items = Vec::new();
        while let Some(x) = rx.recv().await {
            items.push(x);
            if items.len() == 5 {
                break;
            }
        }
        items
    };

    tokio::select! {
        biased;
        _ = token.cancelled() => {
            println!("    ❌ 取消：累积在分支内的 Vec 随 future drop，进度归零");
        }
        items = collect_unsafe => {
            println!("    ✅ 完成：{items:?}");
        }
    }

    producer.abort();
    let _ = producer.await;
}

/// 正例：把累积状态抬到 `select!` 之外，每次只 await 一个 **cancel-safe** 的 future（`mpsc::recv`）。
async fn demo_cancel_safe_loop() {
    println!("【5.2】正例：状态放 select 之外，每次只 await 一个 cancel-safe 动作\n");

    let (tx, mut rx) = mpsc::channel::<u32>(8);
    let token = CancellationToken::new();

    let producer = tokio::spawn(async move {
        for i in 0..5 {
            tokio::time::sleep(Duration::from_millis(40)).await;
            if tx.send(i).await.is_err() {
                return;
            }
        }
    });

    let tok_for_cancel = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(110)).await;
        tok_for_cancel.cancel();
    });

    let mut items: Vec<u32> = Vec::new();
    loop {
        tokio::select! {
            biased;
            _ = token.cancelled() => break,
            maybe = rx.recv() => match maybe {
                Some(x) => {
                    items.push(x);
                    if items.len() == 5 {
                        break;
                    }
                }
                None => break,
            }
        }
    }
    println!("    ✅ 即便中途被取消，已收集部分仍保留：{items:?}");

    producer.abort();
    let _ = producer.await;

    println!(
        "\n    ↑ 规则：`mpsc::recv` / `Notify::notified` / `CancellationToken::cancelled` \n       是 cancel-safe；自写的累积 future 通常不是。把状态抬出分支"
    );
}

// ─── 六、spawn_blocking 与 CPU 密集型协作取消 ────────────────

pub async fn run_section_6_spawn_blocking() {
    println!("────────── 六、`spawn_blocking` 的协作取消 ──────────\n");
    demo_blocking_cooperative_cancel().await;
}

/// `tokio::task::spawn_blocking` 里没有 async 调度点，`cancelled().await` 不适用。
/// 用 `token.is_cancelled()`（原子 load）在热循环里周期性检查。
async fn demo_blocking_cooperative_cancel() {
    println!("【6.1】CPU 密集型循环里每 N 次迭代检查一次 `is_cancelled()`\n");

    let token = CancellationToken::new();
    let token_in_blocking = token.clone();

    // 模拟一个 CPU-bound 计算。
    let handle = tokio::task::spawn_blocking(move || {
        let mut sum: u64 = 0;
        // 粒度：每 1M 次迭代检查一次。粒度决定响应性 vs 检查开销的平衡。
        const CHECK_EVERY: u64 = 1 << 20;
        for i in 0..u64::MAX {
            if i & (CHECK_EVERY - 1) == 0 && token_in_blocking.is_cancelled() {
                return (sum, i);
            }
            sum = sum.wrapping_add(i);
        }
        (sum, u64::MAX)
    });

    tokio::time::sleep(Duration::from_millis(30)).await;
    println!("    main: 发出取消");
    token.cancel();

    let (sum, iters) = handle.await.unwrap();
    println!(
        "    blocking: 协作退出 @ iters={iters}，部分累加={sum}"
    );
    println!(
        "\n    ↑ `JoinHandle::abort()` 对 spawn_blocking 无效；必须让代码主动轮询取消标志"
    );
}

// ─── 七、优雅停机：宽限期 → 强制 abort ───────────────────────

pub async fn run_section_7_graceful_shutdown() {
    println!("────────── 七、优雅停机（graceful → forced）──────────\n");
    demo_graceful_shutdown_with_deadline().await;
}

/// 典型信号处理：`cancel()` 广播停机 → 给宽限期 drain → 超过死线 `abort_all()`。
async fn demo_graceful_shutdown_with_deadline() {
    println!("【7.1】cancel + 宽限期 + 超时兜底 abort_all\n");

    let token = CancellationToken::new();
    let mut set: JoinSet<String> = JoinSet::new();

    // 三个「守规矩」的 worker：收到取消后 flush 不同时长再退。
    for id in 0..3u64 {
        let c = token.child_token();
        set.spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = c.cancelled() => {
                        // 模拟 drain：持久化、回放 buffer 等。
                        let drain_ms = 120 + 80 * id;
                        tokio::time::sleep(Duration::from_millis(drain_ms)).await;
                        return format!("worker-{id} drained in {drain_ms}ms");
                    }
                    _ = tokio::time::sleep(Duration::from_millis(50)) => {}
                }
            }
        });
    }

    // 一个「不守规矩」的 worker：忽略 token，只能被强杀。
    set.spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        "misbehaving finished (should not happen in this demo)".to_string()
    });

    // 模拟 ctrl-c。
    tokio::time::sleep(Duration::from_millis(60)).await;
    println!("    signal: SIGTERM → cancel() 广播停机");
    token.cancel();

    let grace = Duration::from_millis(400);
    let drain = async {
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok(msg) => println!("    ↳ {msg}"),
                Err(e) if e.is_cancelled() => println!("    ↳ (aborted)"),
                Err(e) => println!("    ↳ join error: {e}"),
            }
        }
    };

    match timeout(grace, drain).await {
        Ok(()) => println!("\n    ✅ 全部在宽限期内 drain 完成"),
        Err(_) => {
            println!(
                "\n    ⚠ 宽限期 {}ms 超时，强制 abort_all() 清场",
                grace.as_millis()
            );
            set.abort_all();
            while let Some(joined) = set.join_next().await {
                match joined {
                    Ok(msg) => println!("    ↳ late: {msg}"),
                    Err(e) if e.is_cancelled() => println!("    ↳ aborted"),
                    Err(e) => println!("    ↳ join error: {e}"),
                }
            }
            println!("    ✅ 强制停机完成");
        }
    }
}

// ─── 八、有界并发 + 可取消的 permit 获取 ─────────────────────

pub async fn run_section_8_bounded_concurrency() {
    println!("────────── 八、Semaphore 有界并发 + 可取消 permit ──────────\n");
    demo_bounded_concurrency_cancel().await;
}

/// 排队等 permit 本身是一个 await 点。如果 `acquire().await` 不套 `select!`，
/// 则取消期间大量任务仍会排到信号量再原路返回，浪费调度。
async fn demo_bounded_concurrency_cancel() {
    println!("【8.1】max_in_flight=3，10 个任务，100ms 后取消\n");

    let sem = Arc::new(Semaphore::new(3));
    let token = CancellationToken::new();
    let mut set: JoinSet<String> = JoinSet::new();

    for id in 0..10u32 {
        let sem = sem.clone();
        let child = token.child_token();
        set.spawn(async move {
            // 取 permit 的阶段也要可取消，避免取消后仍然排队。
            let permit = tokio::select! {
                biased;
                _ = child.cancelled() => {
                    return format!("task-{id:02}: cancelled before permit");
                }
                p = sem.acquire_owned() => match p {
                    Ok(p) => p,
                    Err(_) => return format!("task-{id:02}: semaphore closed"),
                }
            };

            // 做工过程中也要可取消；permit 在作用域结束时 drop 自动归还。
            let outcome = tokio::select! {
                biased;
                _ = child.cancelled() => format!("task-{id:02}: cancelled mid-work"),
                _ = tokio::time::sleep(Duration::from_millis(120)) => {
                    format!("task-{id:02}: done")
                }
            };
            drop(permit);
            outcome
        });
    }

    tokio::time::sleep(Duration::from_millis(100)).await;
    println!("    orchestrator: 100ms 后 cancel()\n");
    token.cancel();

    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(msg) => println!("    {msg}"),
            Err(e) => eprintln!("    join error: {e}"),
        }
    }

    println!("\n    ↑ 规则：`acquire().await` 放进 `select!`；permit 靠作用域 Drop 归还");
}

// ─── 全部 ─────────────────────────────────────────────────────

pub async fn run_all_sections() {
    println!("=== Cancellation：可运行示例（按章节）===\n");
    run_section_1_semantics().await;
    println!("\n{}\n", "=".repeat(60));
    run_section_2_propagation().await;
    println!("\n{}\n", "=".repeat(60));
    run_section_3_side_effects().await;
    println!("\n{}\n", "=".repeat(60));
    run_section_4_races_testing().await;
    println!("\n{}\n", "=".repeat(60));
    run_section_5_cancel_safety().await;
    println!("\n{}\n", "=".repeat(60));
    run_section_6_spawn_blocking().await;
    println!("\n{}\n", "=".repeat(60));
    run_section_7_graceful_shutdown().await;
    println!("\n{}\n", "=".repeat(60));
    run_section_8_bounded_concurrency().await;
}
