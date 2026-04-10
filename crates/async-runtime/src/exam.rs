//! 异步运行时：生产向场景题、权衡与泛化策略
//! 每道题包含「问题代码 → 改进方案 → 权衡 → 泛化」四段式结构

pub fn print_header() {
    println!("=== Async Runtime：场景题、Trade-off 与泛化策略 ===\n");
    println!("说明：下列题目按「生产场景 → 问题代码 → 改进方案 → 权衡 → 泛化策略」组织。");
    println!("运行本 crate 即打印题库；可与 tokio/async-std 文档对照加深。\n");
}

// ────────────────────────────────────────────────────────────────────
// 一、执行模型与调度
// ────────────────────────────────────────────────────────────────────

pub fn print_section_1_executor_model() {
    println!("────────── 一、执行模型与调度 ──────────\n");

    // ── 1.1 ──
    println!("【题 1.1】网关接入层：10 万长连接，绝大多数空闲，偶发小包。");
    println!("  现象：用「一线程一连接」模型，内存与线程切换成本暴涨。");
    println!("  问：为何 async + 事件驱动更合适？代价是什么？\n");
    println!("  ❌ 问题代码（一线程一连接）:");
    println!(
        r#"
    fn handle_connections(listener: TcpListener) {{
        for stream in listener.incoming() {{
            std::thread::spawn(move || {{    // 每连接一个 OS 线程
                let mut buf = [0u8; 1024];  // 每线程 ≥8KB 栈
                loop {{
                    let n = stream.read(&mut buf).unwrap(); // 阻塞等待
                    if n == 0 {{ break; }}
                    stream.write_all(&buf[..n]).unwrap();
                }}
            }});
        }}
    }}
    // 10 万连接 = 10 万线程 ≈ 800MB 纯栈内存 + 巨大的上下文切换成本
"#
    );
    println!("  ✅ 改进方案（async + 事件驱动）:");
    println!(
        r#"
    async fn handle_connections(listener: TcpListener) {{
        loop {{
            let (stream, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {{        // 轻量级 task，≈几百字节状态机
                let mut buf = vec![0u8; 1024];
                loop {{
                    let n = stream.read(&mut buf).await.unwrap();
                    if n == 0 {{ break; }}
                    stream.write_all(&buf[..n]).await.unwrap();
                }}
            }});
        }}
    }}
    // 10 万连接 ≈ 几十 MB；OS 线程仅需 CPU 核数量级
"#
    );
    println!("  权衡：吞吐/延迟 vs 编程复杂度、调试难度、取消与背压要显式设计。");
    println!("  泛化：I/O 密集、大量并发会话 → 复用少量 OS 线程 + 非阻塞 I/O；");
    println!("        把「一个会话」建模为状态机/Future，而不是一线程。\n");

    // ── 1.2 ──
    println!("【题 1.2】同一进程内混跑「纯 CPU 图像压缩」与「HTTP 长轮询」。");
    println!("  现象：压缩占满执行线程时，轮询响应明显变慢。");
    println!("  问：根因是？如何避免把两类负载绑在同一调度语义上？\n");
    println!("  ❌ 问题代码（CPU 密集任务占住 executor 线程）:");
    println!(
        r#"
    async fn handle_upload(image: Bytes) -> Response {{
        // 直接在 async 上下文做 CPU 密集操作 —— 阻塞 executor 线程！
        let compressed = lz4::compress(&image);  // 可能跑 50ms+
        save_to_s3(compressed).await
    }}
"#
    );
    println!("  ✅ 改进方案（隔离 CPU 工作到专用线程池）:");
    println!(
        r#"
    async fn handle_upload(image: Bytes) -> Response {{
        // spawn_blocking 把 CPU 工作卸载到专用阻塞线程池
        let compressed = tokio::task::spawn_blocking(move || {{
            lz4::compress(&image)
        }}).await.unwrap();

        save_to_s3(compressed).await  // I/O 操作留在 async
    }}

    // 或者用独立的 rayon 线程池做计算：
    async fn handle_upload_v2(image: Bytes) -> Response {{
        let (tx, rx) = tokio::sync::oneshot::channel();
        rayon::spawn(move || {{
            let compressed = lz4::compress(&image);
            let _ = tx.send(compressed);
        }});
        let compressed = rx.await.unwrap();
        save_to_s3(compressed).await
    }}
"#
    );
    println!("  权衡：合作式调度友好于 I/O，但对长计算不友好；拆线程池有边界成本。");
    println!("  泛化：识别 CPU-bound 段 → 隔离到专用线程池或进程；");
    println!("        async 任务内避免长时间不占 yield 点的计算循环。\n");

    // ── 1.3 ──
    println!("【题 1.3】work-stealing 多线程 runtime 上，短任务与长任务混排。");
    println!("  现象：某些 worker 队列堆积，尾延迟升高。");
    println!("  问：调度层面可能出什么问题？除「别在 async 里算太久」外还有什么手段？\n");
    println!("  ❌ 问题代码（所有任务共享一个 runtime）:");
    println!(
        r#"
    #[tokio::main]
    async fn main() {{
        // 延迟敏感的 API 请求和批处理报表混在同一个 runtime
        tokio::spawn(api_server());         // 需要 p99 < 10ms
        tokio::spawn(batch_report_job());   // 每批处理耗时 500ms+
    }}
"#
    );
    println!("  ✅ 改进方案（按 SLA 分 runtime + 信号量限并发）:");
    println!(
        r#"
    fn main() {{
        // 延迟敏感 runtime
        let rt_api = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .thread_name("api-worker")
            .build().unwrap();

        // 批处理 runtime（独立线程池）
        let rt_batch = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("batch-worker")
            .build().unwrap();

        rt_api.spawn(api_server());
        rt_batch.spawn(batch_report_job());

        // 或者用 Semaphore 限制批处理并发度
        // let batch_permits = Arc::new(Semaphore::new(2));
    }}
"#
    );
    println!("  权衡：全局公平性 vs 局部缓存友好；分池/优先级会增复杂度和饿死风险。");
    println!("  泛化：按 SLA 分池（latency-sensitive vs batch）、限并发（信号量）、");
    println!("        对重任务用独立 executor 或批处理队列。\n");
}

// ────────────────────────────────────────────────────────────────────
// 二、Send、阻塞与跨 await 状态
// ────────────────────────────────────────────────────────────────────

pub fn print_section2_send_blocking() {
    println!("────────── 二、Send、阻塞与跨 await 状态 ──────────\n");

    // ── 2.1 ──
    println!("【题 2.1】在 async 函数里 `std::thread::sleep` 或同步读大文件。");
    println!("  现象：同线程上其它任务全部卡住。");
    println!("  问：与「在阻塞线程池里做」相比，边界在哪里？\n");
    println!("  ❌ 问题代码（在 async 里调阻塞 API）:");
    println!(
        r#"
    async fn poll_external_api() {{
        loop {{
            let data = reqwest::get("https://api.example.com").await;
            process(data);
            std::thread::sleep(Duration::from_secs(5)); // 阻塞整个 executor 线程 5 秒！
        }}
    }}
"#
    );
    println!("  ✅ 改进方案:");
    println!(
        r#"
    async fn poll_external_api() {{
        loop {{
            let data = reqwest::get("https://api.example.com").await;
            process(data);
            tokio::time::sleep(Duration::from_secs(5)).await;  // 异步 sleep，不阻塞
        }}
    }}

    // 对于不可避免的阻塞调用（如同步数据库驱动、JNI）：
    async fn read_legacy_db(query: String) -> Result<Data> {{
        tokio::task::spawn_blocking(move || {{
            legacy_sync_db::execute(&query)  // 阻塞操作在专用线程池里
        }}).await?
    }}
"#
    );
    println!("  权衡：简单阻塞 API vs 占用 executor 线程；线程池有线程数与排队上限。");
    println!("  泛化：在 async 上下文只用非阻塞/异步 API；");
    println!("        不可避免的阻塞 → 专用 blocking 池并设超时与队列监控。\n");

    // ── 2.2 ──
    println!("【题 2.2】`Rc<RefCell<T>>` 在 async 里跨 `.await` 持有。");
    println!("  现象：编译报错或（若勉强通过）运行时数据竞争风险。");
    println!("  问：`Send` 约束在「多线程 runtime」里解决什么问题？\n");
    println!("  ❌ 问题代码（!Send 类型跨 await）:");
    println!(
        r#"
    use std::rc::Rc;
    use std::cell::RefCell;

    async fn update_cache(cache: Rc<RefCell<HashMap<String, String>>>) {{
        let snapshot = cache.borrow().clone();   // Rc 是 !Send
        let new_data = fetch_remote().await;     // ← .await 后 task 可能在别的线程恢复
        cache.borrow_mut().extend(new_data);     // ← 此时 Rc 已被移到别的线程 → UB!
    }}
    // 编译器报错：future is not `Send` — cannot be spawned on multi-thread runtime
"#
    );
    println!("  ✅ 改进方案:");
    println!(
        r#"
    // 方案 A：用 Arc + tokio::sync::Mutex（跨 await 安全）
    async fn update_cache(cache: Arc<tokio::sync::Mutex<HashMap<String, String>>>) {{
        let new_data = fetch_remote().await;
        cache.lock().await.extend(new_data); // async-aware 锁，不阻塞 executor
    }}

    // 方案 B：不跨 await 持有，缩短临界区
    async fn update_cache_v2(cache: Arc<std::sync::Mutex<HashMap<String, String>>>) {{
        let new_data = fetch_remote().await;     // 先完成 I/O
        cache.lock().unwrap().extend(new_data);  // 再短暂持锁写入（无 await）
    }}

    // 方案 C：!Send 逻辑在单线程 runtime 里运行
    let local = tokio::task::LocalSet::new();
    local.run_until(async {{
        tokio::task::spawn_local(update_cache_with_rc(rc_cache)).await;
    }}).await;
"#
    );
    println!("  权衡：`Arc<Mutex<T>>` / `tokio::sync::Mutex` vs `Send` 全链路成本。");
    println!("  泛化：跨 await 的状态须满足「可在线程间迁移」或固定在单线程任务域；");
    println!("        `!Send` 逻辑用 `LocalSet`/单线程 runtime 或消息传递到 owning 任务。\n");

    // ── 2.3 ──
    println!("【题 2.3】持 `std::sync::Mutex` guard 调用 `.await`。");
    println!("  现象：死锁或极差的并发度（视 runtime 与锁竞争而定）。");
    println!("  问：与 async-aware mutex 的差异？\n");
    println!("  ❌ 问题代码（std Mutex guard 跨 await）:");
    println!(
        r#"
    async fn bad_update(state: Arc<std::sync::Mutex<State>>) {{
        let mut guard = state.lock().unwrap();  // 获取 std::sync::MutexGuard
        let enriched = fetch_enrichment(guard.id).await; // guard 跨 await 持有！
        //                                       ^^^^^
        // 此线程被阻塞在 await → 其它 task 若在同线程试图 lock → 死锁
        // 即使不死锁，guard 在整个 I/O 期间持有 → 极差并发度
        guard.data = enriched;
    }}
"#
    );
    println!("  ✅ 改进方案:");
    println!(
        r#"
    // 方案 A：缩短持锁区间，不跨 await
    async fn good_update(state: Arc<std::sync::Mutex<State>>) {{
        let id = state.lock().unwrap().id;      // 短暂持锁，拷贝所需数据
        // guard 在此已 drop
        let enriched = fetch_enrichment(id).await;  // I/O 期间无锁
        state.lock().unwrap().data = enriched;       // 再短暂持锁写回
    }}

    // 方案 B：必须跨 await 时用 tokio::sync::Mutex
    async fn update_with_async_mutex(state: Arc<tokio::sync::Mutex<State>>) {{
        let mut guard = state.lock().await;   // async-aware，等待时不阻塞线程
        let enriched = fetch_enrichment(guard.id).await;
        guard.data = enriched;
        // 注意：持锁期间其他 task 的 .lock().await 会排队，但不死锁
    }}
"#
    );
    println!("  权衡：std 锁轻但在 async 里易踩坑；async 锁可 await 但有额外调度成本。");
    println!("  泛化：临界区尽量不含 await；必须跨 await 共享 → 用 async mutex 或 actor；");
    println!("        缩短持锁时间，避免在锁内等 I/O。\n");
}

// ────────────────────────────────────────────────────────────────────
// 三、取消、超时与背压
// ────────────────────────────────────────────────────────────────────

pub fn print_section3_cancellation_backpressure() {
    println!("────────── 三、取消、超时与背压 ──────────\n");

    // ── 3.1 ──
    println!("【题 3.1】客户端断开 TCP，但服务端任务仍在读下游、占连接池槽位。");
    println!("  问：如何与「取消」绑定？取消后资源清理顺序要注意什么？\n");
    println!("  ❌ 问题代码（客户端断开后任务继续跑）:");
    println!(
        r#"
    async fn handle_request(req: Request) -> Response {{
        let db_result = db.query("SELECT ...").await;    // 客户端已走，仍占 DB 连接
        let enriched = call_downstream(db_result).await; // 继续调下游，浪费资源
        Response::ok(enriched)                           // 返回值无人接收
    }}
"#
    );
    println!("  ✅ 改进方案（取消传播 + Drop 资源清理）:");
    println!(
        r#"
    async fn handle_request(req: Request, cancel: CancellationToken) -> Response {{
        // select! 在客户端断开（cancel）时自动 drop 未完成分支
        tokio::select! {{
            result = async {{
                let db_result = db.query("SELECT ...").await?;
                let enriched = call_downstream(db_result).await?;
                Ok(Response::ok(enriched))
            }} => result.unwrap_or_else(|e| Response::error(e)),

            _ = cancel.cancelled() => {{
                tracing::info!("client disconnected, aborting");
                Response::cancelled()
            }}
        }}
    }}

    // 关键资源用 Drop guard 保证清理：
    struct DbConnGuard {{ conn: PooledConnection }}
    impl Drop for DbConnGuard {{
        fn drop(&mut self) {{
            self.conn.return_to_pool(); // 无论 cancel 还是正常完成都归还
        }}
    }}
"#
    );
    println!("  权衡：协作式取消（poll 间检查）vs 强制中止；后者易留下不一致状态。");
    println!("  泛化：用 `select!`/drop future/显式 cancel token 传播；");
    println!(
        "        `JoinHandle::abort` 只作最后手段；关键资源用 `Drop`/`scopeguard` 保证释放。\n"
    );

    // ── 3.2 ──
    println!("【题 3.2】无界 channel 接收慢、生产端 `send` 永不阻塞。");
    println!("  现象：内存持续上涨，最终 OOM。");
    println!("  问：背压应落在哪一层？有界 channel 的 trade-off？\n");
    println!("  ❌ 问题代码（无界 channel 无背压）:");
    println!(
        r#"
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    // 生产者：高速写入，永不阻塞
    tokio::spawn(async move {{
        loop {{
            let event = receive_from_kafka().await;
            tx.send(event).unwrap();  // unbounded → 永不失败，内存无上限
        }}
    }});

    // 消费者：慢速处理
    tokio::spawn(async move {{
        while let Some(event) = rx.recv().await {{
            expensive_process(event).await;  // 处理速度 < 生产速度 → 队列无限增长
        }}
    }});
"#
    );
    println!("  ✅ 改进方案（有界 channel + 背压）:");
    println!(
        r#"
    // 有界 channel：生产者满了会 await，天然背压
    let (tx, mut rx) = tokio::sync::mpsc::channel(1000);  // 容量 1000

    tokio::spawn(async move {{
        loop {{
            let event = receive_from_kafka().await;
            if tx.send(event).await.is_err() {{  // 队列满时背压到生产端
                tracing::warn!("consumer gone, stopping producer");
                break;
            }}
        }}
    }});

    // 或者用 Semaphore 做任务级并发控制：
    let sem = Arc::new(Semaphore::new(100)); // 最多 100 个并发处理任务
    while let Some(event) = rx.recv().await {{
        let permit = sem.clone().acquire_owned().await.unwrap();
        tokio::spawn(async move {{
            expensive_process(event).await;
            drop(permit);  // 完成后释放许可
        }});
    }}
"#
    );
    println!("  权衡：反压到上游（慢下来）vs 丢包/采样；有界队列会阻塞或需处理 `send` 失败。");
    println!("  泛化：默认可背压边界（有界队列、semaphore、rate limit）；");
    println!("        监控队列深度与任务数，把「无限缓冲」视为设计缺陷。\n");

    // ── 3.3 ──
    println!("【题 3.3】对外部 API 调用无超时。");
    println!("  现象：线程/任务堆积，雪崩。");
    println!("  问：超时与取消如何配合重试与幂等？\n");
    println!("  ❌ 问题代码（无超时 + 无限重试）:");
    println!(
        r#"
    async fn call_payment(order: &Order) -> Result<Receipt> {{
        loop {{
            match payment_client.charge(order).await {{  // 无超时！下游卡住就永远等
                Ok(receipt) => return Ok(receipt),
                Err(_) => continue,  // 无限重试，无 backoff → 雪崩放大
            }}
        }}
    }}
"#
    );
    println!("  ✅ 改进方案（超时 + 指数退避 + 幂等）:");
    println!(
        r#"
    async fn call_payment(order: &Order) -> Result<Receipt> {{
        let mut backoff = Duration::from_millis(100);
        let max_retries = 3;

        for attempt in 0..=max_retries {{
            let result = tokio::time::timeout(
                Duration::from_secs(5),               // 每次调用设 deadline
                payment_client.charge_idempotent(      // 幂等接口：同一 idempotency_key
                    order,                             // 重试不会重复扣款
                    &order.idempotency_key,
                ),
            ).await;

            match result {{
                Ok(Ok(receipt)) => return Ok(receipt),
                Ok(Err(e)) if !e.is_retryable() => return Err(e),  // 不可重试错误
                _ => {{
                    if attempt == max_retries {{ return Err(anyhow!("max retries")); }}
                    let jitter = rand::random::<u64>() % backoff.as_millis() as u64;
                    tokio::time::sleep(backoff + Duration::from_millis(jitter)).await;
                    backoff = std::cmp::min(backoff * 2, Duration::from_secs(30));
                }}
            }}
        }}
        unreachable!()
    }}
"#
    );
    println!("  权衡：过早超时误杀 vs 过长超时拖垮系统；重试放大负载。");
    println!("  泛化：每层 I/O 设 deadline；重试带 jitter 与上限；幂等键与熔断。\n");
}

// ────────────────────────────────────────────────────────────────────
// 四、运行时选型与生态
// ────────────────────────────────────────────────────────────────────

pub fn print_section4_runtime_choice() {
    println!("────────── 四、运行时选型与生态 ──────────\n");

    // ── 4.1 ──
    println!("【题 4.1】库作者：是否应在库内 `#[tokio::main]` 或隐式启动 runtime？");
    println!("  问：对二进制作者与测试的影响？\n");
    println!("  ❌ 问题代码（库里偷偷起 runtime）:");
    println!(
        r#"
    // my_lib/src/lib.rs
    pub fn fetch_config() -> Config {{
        // 库自己创建 runtime → 用户的 runtime 里调用会 panic（嵌套 runtime）
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {{
            reqwest::get("https://config.example.com").await.unwrap().json().await.unwrap()
        }})
    }}
"#
    );
    println!("  ✅ 改进方案（库暴露 async fn，由应用控制 runtime）:");
    println!(
        r#"
    // my_lib/src/lib.rs — 库只暴露 async 接口
    pub async fn fetch_config() -> Result<Config> {{
        let resp = reqwest::get("https://config.example.com").await?;
        Ok(resp.json().await?)
    }}

    // 应用侧选择 runtime
    #[tokio::main]
    async fn main() {{
        let config = my_lib::fetch_config().await.unwrap();
    }}

    // 测试用 current_thread 即可
    #[tokio::test]
    async fn test_fetch_config() {{
        let config = my_lib::fetch_config().await.unwrap();
        assert_eq!(config.version, 1);
    }}
"#
    );
    println!("  权衡：开箱即用 vs 与用户选定 runtime 冲突、嵌套 runtime。");
    println!("  泛化：库暴露 `async fn` + 可选 rt 特性；由应用统一选 executor；");
    println!("        测试可用 `current_thread` 或用户提供的 handle。\n");

    // ── 4.2 ──
    println!("【题 4.2】嵌入式或严格 no_std 场景需要异步。");
    println!("  问：完整 futures + 生态 runtime 与「自研最小 executor」如何选？\n");
    println!("  代码示例（最小 executor：单线程轮询）:");
    println!(
        r#"
    // 不依赖 tokio/async-std，仅用 core::future + 手写 executor
    use core::future::Future;
    use core::task::{{Context, Poll, RawWaker, RawWakerVTable, Waker}};
    use core::pin::Pin;

    fn block_on<F: Future>(mut future: F) -> F::Output {{
        // 构造一个 no-op waker（单线程轮询不需要唤醒机制）
        fn dummy_raw_waker() -> RawWaker {{
            fn no_op(_: *const ()) {{}}
            fn clone(_: *const ()) -> RawWaker {{ dummy_raw_waker() }}
            let vtable = &RawWakerVTable::new(clone, no_op, no_op, no_op);
            RawWaker::new(core::ptr::null(), vtable)
        }}
        let waker = unsafe {{ Waker::from_raw(dummy_raw_waker()) }};
        let mut cx = Context::from_waker(&waker);
        let mut future = unsafe {{ Pin::new_unchecked(&mut future) }};

        loop {{
            match future.as_mut().poll(&mut cx) {{
                Poll::Ready(val) => return val,
                Poll::Pending => core::hint::spin_loop(),  // 忙等，适合裸机
            }}
        }}
    }}
"#
    );
    println!("  权衡：功能与安全审计成本 vs 维护负担与边界情况。");
    println!("  泛化：先明确并发模型（单线程轮询即可？）；");
    println!("        再选最小依赖集；文档写明 `Send`/时钟/IO 资源假设。\n");

    // ── 4.3 ──
    println!("【题 4.3】团队在 tokio 与 async-std 之间摇摆，两套生态的 crate 混用。");
    println!("  现象：Timer、Spawn、IO trait 不兼容；集成测试需要两套 setup。");
    println!("  问：混用 runtime 为何危险？依赖冲突的根源在哪里？\n");
    println!("  ❌ 问题代码（跨 runtime 调度）:");
    println!(
        r#"
    // tokio runtime 里用 async-std 的 sleep —— timer 永远不会触发！
    #[tokio::main]
    async fn main() {{
        async_std::task::sleep(Duration::from_secs(1)).await;
        //                     ^^^^^^^^^^^^^^^^^^^^^^^^
        // async-std 的 timer 注册到 async-std 的 reactor
        // 但此时跑的是 tokio 的 reactor → timer 永远 Pending
        println!("this may never print");
    }}
"#
    );
    println!("  ✅ 改进方案:");
    println!(
        r#"
    // 统一使用一个 runtime 的 API
    #[tokio::main]
    async fn main() {{
        tokio::time::sleep(Duration::from_secs(1)).await;  // tokio runtime → tokio timer
        println!("works correctly");
    }}

    // 若必须用跨 runtime 的库，用 async-compat 垫片（理解其代价）：
    // use async_compat::CompatExt;
    // some_async_std_future.compat().await;  // 在 tokio 上下文运行 async-std future
"#
    );
    println!("  权衡：「全站一 runtime」简单但锁定生态；抽象层（如 async-compat）增间接。");
    println!("  泛化：选定一个主 runtime 并在 CI 锁死；");
    println!("        对第三方库看它依赖哪个 runtime trait——不兼容时优先换库而非垫片。\n");
}

// ────────────────────────────────────────────────────────────────────
// 五、结构化并发与任务生命周期
// ────────────────────────────────────────────────────────────────────

pub fn print_section5_structured_concurrency() {
    println!("────────── 五、结构化并发与任务生命周期 ──────────\n");

    // ── 5.1 ──
    println!("【题 5.1】fire-and-forget：tokio::spawn 后无人持有 JoinHandle。");
    println!("  现象：任务里 panic 了，日志里只有 runtime 层一行 warning，主逻辑浑然不知。");
    println!("  问：谁负责观察子任务的结果？不 await JoinHandle 会丢失什么？\n");
    println!("  ❌ 问题代码（fire-and-forget）:");
    println!(
        r#"
    async fn handle_order(order: Order) {{
        // JoinHandle 被丢弃，子任务的 panic/error 无人收集
        tokio::spawn(send_notification(order.user_id));  // 如果 panic → 静默丢失
        tokio::spawn(update_analytics(order.id));         // 如果出错 → 无人知晓
        // 主逻辑继续，不知道后台任务是否成功
    }}
"#
    );
    println!("  ✅ 改进方案（JoinSet 收集 + 监控）:");
    println!(
        r#"
    async fn handle_orders(mut rx: Receiver<Order>) {{
        let mut tasks = tokio::task::JoinSet::new();

        loop {{
            tokio::select! {{
                Some(order) = rx.recv() => {{
                    tasks.spawn(async move {{
                        send_notification(order.user_id).await?;
                        update_analytics(order.id).await?;
                        Ok::<_, anyhow::Error>(())
                    }});
                }}
                // 持续收割完成的任务，处理错误
                Some(result) = tasks.join_next() => {{
                    match result {{
                        Ok(Ok(())) => {{ /* 成功 */ }}
                        Ok(Err(e)) => tracing::error!("task failed: {{e}}"),
                        Err(e) => tracing::error!("task panicked: {{e}}"),  // JoinError
                    }}
                }}
                else => break,
            }}
        }}
        // 优雅关停：等待所有剩余任务完成
        while let Some(result) = tasks.join_next().await {{
            if let Err(e) = result {{ tracing::error!("shutdown: task error: {{e}}"); }}
        }}
    }}
"#
    );
    println!("  权衡：spawn 解耦、响应快 vs 错误黑洞、泄漏无约束任务数。");
    println!("  泛化：凡 spawn 必须有人 join 或监控；");
    println!("        用 JoinSet / TaskTracker 收集句柄；设 panic hook 保底上报。\n");

    // ── 5.2 ──
    println!("【题 5.2】需要并发请求 50 个下游，结果全部到齐后聚合返回。");
    println!("  现象：用 `futures::future::join_all` 一次发 50 个 Future。");
    println!("  问：join_all vs spawn+JoinSet 差异？一个子任务 panic/超时如何影响整体？\n");
    println!("  方案对比:");
    println!(
        r#"
    // 方案 A：join_all — 在同一 task 内轮询所有 future
    async fn fan_out_join_all(urls: Vec<String>) -> Vec<Response> {{
        let futures: Vec<_> = urls.iter()
            .map(|url| reqwest::get(url))
            .collect();
        futures::future::join_all(futures).await  // 全在一个 task 内
        // 优点：简单、无 spawn 开销
        // 缺点：一个 future panic → 整个 task panic；无法单独取消/超时
    }}

    // 方案 B：spawn + JoinSet + Semaphore 限并发
    async fn fan_out_joinset(urls: Vec<String>) -> Vec<Result<Response>> {{
        let sem = Arc::new(Semaphore::new(10));  // 最多 10 并发
        let mut set = JoinSet::new();

        for url in urls {{
            let sem = sem.clone();
            set.spawn(async move {{
                let _permit = sem.acquire().await.unwrap();
                tokio::time::timeout(
                    Duration::from_secs(5),
                    reqwest::get(&url),
                ).await
            }});
        }}

        let mut results = Vec::new();
        while let Some(res) = set.join_next().await {{
            results.push(res.map_err(|e| anyhow!(e)).and_then(|r| r.map_err(Into::into)));
        }}
        results
    }}

    // 方案 C：stream + buffer_unordered（流式限并发）
    use futures::stream::{{self, StreamExt}};
    async fn fan_out_stream(urls: Vec<String>) -> Vec<Response> {{
        stream::iter(urls)
            .map(|url| reqwest::get(url))
            .buffer_unordered(10)   // 最多 10 个同时执行
            .collect().await
    }}
"#
    );
    println!("  权衡：join_all 在同一 task 里轮询——无并行上限、难以单独取消；");
    println!("        spawn 每个得到独立 task，但句柄管理成本上升。");
    println!("  泛化：少量无 I/O 依赖的 Future → join_all 简洁；");
    println!("        大量或需独立取消/限并发 → spawn + Semaphore + JoinSet；");
    println!("        用 `FuturesUnordered` 配合 `buffer_unordered` 做流式限并发。\n");

    // ── 5.3 ──
    println!("【题 5.3】`select!` 选择最快分支，但被丢弃的分支已执行了一半副作用。");
    println!("  场景：分支 A 已向下游发了写请求，但 B 先完成，A 被 drop。");
    println!("  问：被 drop 的 Future 停在哪？副作用回滚谁负责？\n");
    println!("  ❌ 问题代码（select 非 cancel-safe 的 Future）:");
    println!(
        r#"
    async fn write_with_timeout(db: &Database, data: Data) -> Result<()> {{
        tokio::select! {{
            result = async {{
                db.begin_transaction().await?;       // 已开始事务
                db.insert(data).await?;              // 已写入数据
                db.commit().await                    // ← 如果 timeout 先到，
            }} => result,                            //    commit 不执行，事务悬挂！
            _ = tokio::time::sleep(Duration::from_secs(5)) => {{
                Err(anyhow!("timeout"))
                // 事务 begin 了但没 commit/rollback → 连接泄漏或锁持有
            }}
        }}
    }}
"#
    );
    println!("  ✅ 改进方案（spawn 隔离副作用 + oneshot 回传）:");
    println!(
        r#"
    // 方案 A：把有副作用的操作 spawn 出去，保证完成
    async fn write_with_timeout(db: Database, data: Data) -> Result<()> {{
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {{
            // spawn 里的 future 不会被 select! drop
            let result = async {{
                db.begin_transaction().await?;
                db.insert(data).await?;
                db.commit().await
            }}.await;
            let _ = tx.send(result);
        }});

        match tokio::time::timeout(Duration::from_secs(5), rx).await {{
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(anyhow!("task dropped")),
            Err(_) => Err(anyhow!("timeout")),  // 任务还在跑，会自行完成
        }}
    }}

    // 方案 B：只 select cancel-safe 操作，事务逻辑不切分
    async fn write_with_timeout_v2(db: &Database, data: Data) -> Result<()> {{
        tokio::time::timeout(Duration::from_secs(5), async {{
            db.begin_transaction().await?;
            db.insert(data).await?;
            db.commit().await
        }}).await.map_err(|_| anyhow!("timeout"))?
        // timeout 会 drop 整个 future，但数据库连接的 Drop impl 应保证 rollback
    }}
"#
    );
    println!("  权衡：select! 提供延迟最优路径，代价是 cancel-safety 要求。");
    println!("  泛化：只 select! cancel-safe 的 Future（无不可逆副作用直到完成）；");
    println!("        写操作需幂等或先在 spawn 里跑完、用 oneshot 回传结果；");
    println!("        文档标注哪些 async fn 是 cancel-safe，哪些不是。\n");

    // ── 5.4 ──
    println!("【题 5.4】服务启动时并行初始化 DB 连接池、缓存预热、配置拉取。");
    println!("  现象：其中一个失败后，其余继续跑完才返回错误，浪费时间。");
    println!("  问：如何做到「一失败全停」的 fail-fast 语义？\n");
    println!("  ❌ 问题代码（join_all 不 fail-fast）:");
    println!(
        r#"
    async fn init_all() -> Result<()> {{
        let results = futures::future::join_all(vec![
            init_db_pool(),         // 成功，但要等其他全完成
            init_cache(),           // 失败！但 join_all 不提前退出
            fetch_remote_config(),  // 也要跑完才返回
        ]).await;
        // 所有都完成后才能看到 init_cache 的错误
        for r in results {{ r?; }}
        Ok(())
    }}
"#
    );
    println!("  ✅ 改进方案（try_join! fail-fast / JoinSet abort_all）:");
    println!(
        r#"
    // 方案 A：tokio::try_join! — 第一个 Err 即返回，其余被 drop
    async fn init_all() -> Result<()> {{
        tokio::try_join!(
            init_db_pool(),
            init_cache(),           // 这个失败 → 其余立刻被 drop
            fetch_remote_config(),
        )?;
        Ok(())
    }}

    // 方案 B：JoinSet + 显式 abort（需要清理逻辑时）
    async fn init_all_with_cleanup() -> Result<()> {{
        let mut set = JoinSet::new();
        set.spawn(init_db_pool());
        set.spawn(init_cache());
        set.spawn(fetch_remote_config());

        while let Some(result) = set.join_next().await {{
            match result {{
                Ok(Ok(())) => continue,
                Ok(Err(e)) => {{
                    tracing::error!("init failed: {{e}}, aborting remaining");
                    set.abort_all();  // 取消所有还在跑的 init 任务
                    return Err(e);
                }}
                Err(join_err) => {{
                    set.abort_all();
                    return Err(anyhow!("init task panicked: {{join_err}}"));
                }}
            }}
        }}
        Ok(())
    }}
"#
    );
    println!("  权衡：try_join! 失败即返回但会 drop 其余 Future；spawn 需显式 abort。");
    println!("  泛化：用 `try_join!` / `try_join_all` 做 fail-fast；");
    println!("        需要清理逻辑时用 JoinSet + 收到首个 Err 后 abort_all()；");
    println!("        启动阶段失败应 fast-fail 而非静默降级（除非有 fallback 策略）。\n");
}

// ────────────────────────────────────────────────────────────────────
// 六、Pin、Future 状态机与性能
// ────────────────────────────────────────────────────────────────────

pub fn print_section6_pin_future_perf() {
    println!("────────── 六、Pin、Future 状态机与性能 ──────────\n");

    // ── 6.1 ──
    println!("【题 6.1】一个 async fn 内部声明了 16 KB 的栈上缓冲区并跨 await 持有。");
    println!("  现象：Future 本体超大，spawn 时堆分配压力上升；嵌套调用导致 Future 指数膨胀。");
    println!("  问：async fn 编译后的状态机大小由什么决定？如何缩减？\n");
    println!("  ❌ 问题代码（大缓冲区跨 await）:");
    println!(
        r#"
    async fn process_file(path: &str) -> Result<()> {{
        let mut buf = [0u8; 16384];  // 16KB 栈上数组 → 编入 Future 状态机
        let file = File::open(path).await?;
        file.read(&mut buf).await?;  // buf 跨越 .await → Future 至少 16KB
        transform(&buf).await?;      // 再跨一个 await
        Ok(())
    }}
    // 嵌套调用 process_file → 外层 Future 包含内层 16KB → 递归放大
    println!("size = {{}}", std::mem::size_of_val(&process_file("a.txt")));
    // 可能打印：size = 16544（远超预期）
"#
    );
    println!("  ✅ 改进方案:");
    println!(
        r#"
    async fn process_file(path: &str) -> Result<()> {{
        // 方案 A：用 Vec 分配到堆上，Future 只存一个指针（8 bytes）
        let mut buf = vec![0u8; 16384];
        let file = File::open(path).await?;
        file.read(&mut buf).await?;
        transform(&buf).await?;
        Ok(())
    }}

    async fn process_file_v2(path: &str) -> Result<()> {{
        // 方案 B：缩小跨 await 作用域 — 不跨 await 的量不计入状态机
        let data = {{
            let mut buf = [0u8; 16384];
            let file = File::open(path).await?;
            let n = file.read(&mut buf).await?;
            buf[..n].to_vec()          // 拷贝出需要的部分
        }};                             // buf 在此 drop，不跨后续 await
        transform(&data).await?;
        Ok(())
    }}

    // 排查工具：
    let fut = process_file("test.txt");
    println!("Future size: {{}} bytes", std::mem::size_of_val(&fut));
"#
    );
    println!("  权衡：栈上缓冲零分配 vs Future 体积；Box 缓冲多一次分配但 Future 瘦小。");
    println!("  泛化：跨 await 持有的局部变量全部计入状态机大小；");
    println!("        大缓冲区用 Vec / Box<[u8]>；把不跨 await 的局部量提到独立作用域；");
    println!("        用 `std::mem::size_of_val(&future)` 排查异常膨胀。\n");

    // ── 6.2 ──
    println!("【题 6.2】写了 `async fn traverse(node: &Node) {{ traverse(child).await }}` 递归。");
    println!("  现象：编译报错「recursive async fn has infinite size」。");
    println!("  问：为什么同步递归可以但 async 递归不行？`Box::pin` 在这里做什么？\n");
    println!("  ❌ 问题代码（直接 async 递归）:");
    println!(
        r#"
    struct Node {{
        val: i32,
        children: Vec<Node>,
    }}

    // 编译报错！Future 大小 = 自身 + 子 Future → 无穷递归
    async fn sum_tree(node: &Node) -> i32 {{
        let mut total = node.val;
        for child in &node.children {{
            total += sum_tree(child).await;  // 递归 → Future 嵌套自身
        }}
        total
    }}
    // error[E0733]: recursion in an async fn requires boxing
"#
    );
    println!("  ✅ 改进方案:");
    println!(
        r#"
    // 方案 A：Box::pin 打断编译期大小推导
    fn sum_tree(node: &Node) -> Pin<Box<dyn Future<Output = i32> + '_>> {{
        Box::pin(async move {{
            let mut total = node.val;
            for child in &node.children {{
                total += sum_tree(child).await;  // 每层一次堆分配
            }}
            total
        }})
    }}

    // 方案 B：改迭代 + 显式栈（零额外分配）
    fn sum_tree_iter(root: &Node) -> i32 {{
        let mut stack = vec![root];
        let mut total = 0;
        while let Some(node) = stack.pop() {{
            total += node.val;
            stack.extend(node.children.iter());  // 复用一个 Vec 做栈
        }}
        total
    }}

    // 原理图解：
    // async fn 编译为枚举状态机：
    //   enum SumTreeFuture {{
    //       State0 {{ node, total }},
    //       State1 {{ node, total, child_future: SumTreeFuture }},  // 自引用！无穷大
    //   }}
    // Box::pin 后：
    //   State1 {{ node, total, child_future: Pin<Box<dyn Future>> }}  // 固定 8 字节
"#
    );
    println!("  权衡：Box::pin 每层一次堆分配；改迭代+显式栈无分配但实现复杂。");
    println!("  泛化：async 递归 → `Box::pin(async move {{ ... }})` 打断编译期大小推导；");
    println!("        深递归场景优先改为迭代 + 手动栈；");
    println!("        Future 大小 = 所有分支最大变体之 max，递归导致无穷链。\n");

    // ── 6.3 ──
    println!("【题 6.3】把多个不同 async fn 装进同一个 Vec 做调度。");
    println!("  现象：`Vec<impl Future>` 不合法，需要 `Vec<Pin<Box<dyn Future>>>`。");
    println!("  问：类型擦除 + Pin + Box 三层嵌套解决什么问题？各自的成本？\n");
    println!("  代码对比:");
    println!(
        r#"
    // ❌ 不合法：每个 async fn 返回不同大小的匿名类型
    // let tasks: Vec<impl Future<Output = ()>> = vec![
    //     fetch_users(),    // 类型 A
    //     sync_inventory(), // 类型 B
    //     send_report(),    // 类型 C
    // ];

    // ✅ 方案 A：Pin<Box<dyn Future>> 类型擦除（动态分发）
    let tasks: Vec<Pin<Box<dyn Future<Output = ()> + Send>>> = vec![
        Box::pin(fetch_users()),      // Box: 堆分配放置不同大小的 Future
        Box::pin(sync_inventory()),   // Pin: 保证 Future 地址不变（自引用安全）
        Box::pin(send_report()),      // dyn: 运行时 vtable 分发 poll()
    ];
    // 每个 task = 1 次堆分配 + 1 个 vtable 指针（16 字节胖指针）

    // ✅ 方案 B：enum dispatch（静态分发，零额外分配）
    enum MyTask {{
        FetchUsers(FetchUsersFuture),
        SyncInventory(SyncInventoryFuture),
        SendReport(SendReportFuture),
    }}
    impl Future for MyTask {{
        type Output = ();
        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {{
            match self.get_mut() {{  // 编译期确定分支，可内联
                MyTask::FetchUsers(f) => Pin::new(f).poll(cx),
                MyTask::SyncInventory(f) => Pin::new(f).poll(cx),
                MyTask::SendReport(f) => Pin::new(f).poll(cx),
            }}
        }}
    }}

    // ✅ 方案 C：FuturesUnordered（复用内部 slab，减少分配频率）
    let mut futs = FuturesUnordered::new();
    futs.push(fetch_users());
    futs.push(sync_inventory());  // 需要同类型 → 通常配合 Box::pin
    while let Some(()) = futs.next().await {{}}
"#
    );
    println!("  权衡：静态分发（enum 枚举）零开销但封闭；dyn 灵活但有 vtable + 堆分配。");
    println!("  泛化：已知变体集合 → 手写 enum Future 消除分配；");
    println!("        变体开放/插件化 → `Pin<Box<dyn Future + Send>>`；");
    println!("        hot path 高频分配 → 用 FuturesUnordered 复用 slab。\n");

    // ── 6.4 ──
    println!("【题 6.4】手动实现 `Future` trait 时 `self: Pin<&mut Self>` 的含义。");
    println!("  场景：包装一个内部持有自引用（如 tokio::time::Sleep）的组合 Future。");
    println!("  问：Pin 防止什么操作？如果误用 `std::mem::swap` 会怎样？\n");
    println!("  代码示例（手写组合 Future + pin-project）:");
    println!(
        r#"
    use pin_project_lite::pin_project;
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{{Context, Poll}};

    pin_project! {{
        /// 给任意 Future 加上超时功能的组合 Future
        struct Timeout<F> {{
            #[pin]                // 标记为 structurally pinned
            future: F,            // 内部 future 可能含自引用 → 不可移动
            #[pin]
            delay: tokio::time::Sleep,
        }}
    }}

    impl<F: Future> Future for Timeout<F> {{
        type Output = Result<F::Output, &'static str>;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {{
            let this = self.project();      // pin-project 安全投影出字段

            // 先检查内部 future 是否完成
            if let Poll::Ready(val) = this.future.poll(cx) {{
                return Poll::Ready(Ok(val));
            }}
            // 再检查超时
            if let Poll::Ready(()) = this.delay.poll(cx) {{
                return Poll::Ready(Err("timeout"));
            }}
            Poll::Pending
        }}
    }}

    // ❌ 如果没有 Pin 保护：
    // let mut a = Timeout {{ ... }};
    // let mut b = Timeout {{ ... }};
    // std::mem::swap(&mut a, &mut b); // 内部 Sleep 有自引用指针
    //                                  // swap 后指针指向旧地址 → UB！
    // Pin 防止的正是这种 move 操作
"#
    );
    println!("  权衡：Pin 保证 !Unpin 类型的内存地址不变，安全包装自引用；");
    println!("        但 API 表面复杂，pin-project 宏简化字段投影。");
    println!("  泛化：绝大多数应用代码用 async/await 自动获得正确 Pin 语义；");
    println!("        手写 poll 时用 `pin-project-lite`；");
    println!("        只有底层 IO / Timer 原语才需要理解裸 Pin 投影。\n");
}

// ────────────────────────────────────────────────────────────────────
// 七、优雅关停与生命周期管理
// ────────────────────────────────────────────────────────────────────

pub fn print_section7_graceful_shutdown() {
    println!("────────── 七、优雅关停与生命周期管理 ──────────\n");

    // ── 7.1 ──
    println!("【题 7.1】Ctrl+C 到来，但有 200 个 in-flight HTTP 请求正在处理。");
    println!("  现象：直接 `std::process::exit` → 客户端收到 RST，数据库事务回滚。");
    println!("  问：如何做到「不再接新请求 + 等存量完成 + 超时强杀」三步走？\n");
    println!("  ✅ 完整实现（三阶段 graceful shutdown）:");
    println!(
        r#"
    use tokio_util::sync::CancellationToken;

    async fn run_server() {{
        let token = CancellationToken::new();
        let tracker = tokio_util::task::TaskTracker::new();

        // 1) 启动 HTTP listener
        let listener = TcpListener::bind("0.0.0.0:8080").await.unwrap();

        // 2) 监听 Ctrl+C 信号
        let shutdown_token = token.clone();
        tokio::spawn(async move {{
            tokio::signal::ctrl_c().await.unwrap();
            tracing::info!("shutdown signal received");
            shutdown_token.cancel();  // 通知所有持有 token 的任务
        }});

        // 3) 主循环：接新连接 or 停止
        loop {{
            tokio::select! {{
                Ok((stream, _)) = listener.accept() => {{
                    let token = token.child_token();
                    tracker.spawn(async move {{
                        handle_connection(stream, token).await;
                    }});
                }}
                _ = token.cancelled() => {{
                    tracing::info!("stop accepting new connections");
                    break;                   // ← 不再 accept
                }}
            }}
        }}

        // 4) drain：等所有 in-flight 请求完成，但设 deadline
        tracker.close();  // 不再允许新 spawn
        if tokio::time::timeout(Duration::from_secs(30), tracker.wait())
            .await
            .is_err()
        {{
            tracing::warn!("drain timeout, aborting remaining tasks");
            // 超时 → 强制退出（剩余任务被 runtime drop）
        }}
        tracing::info!("shutdown complete");
    }}
"#
    );
    println!("  权衡：等待时间太长影响重启速度 vs 太短丢弃有效工作。");
    println!("  泛化：信号 → 标记停止监听 → drain 现有连接（设 deadline）→ 超时 abort；");
    println!("        Kubernetes：preStop hook + `terminationGracePeriodSeconds` 对齐。\n");

    // ── 7.2 ──
    println!("【题 7.2】后台任务（定时刷缓存、心跳上报）在关停时需要 flush。");
    println!("  现象：直接取消后台任务，缓存丢失最后一批写入。");
    println!("  问：后台任务如何感知「该退出了」并执行最终清理？\n");
    println!("  ❌ 问题代码（取消即丢数据）:");
    println!(
        r#"
    async fn background_flush(buffer: Arc<Mutex<Vec<Event>>>) {{
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        loop {{
            interval.tick().await;            // 被 abort 时直接死在这里
            let events = buffer.lock().await.drain(..).collect::<Vec<_>>();
            db.batch_insert(&events).await;   // 最后一批永远不会被 flush
        }}
    }}
"#
    );
    println!("  ✅ 改进方案（感知取消 + final flush）:");
    println!(
        r#"
    async fn background_flush(
        buffer: Arc<Mutex<Vec<Event>>>,
        cancel: CancellationToken,
    ) {{
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        loop {{
            tokio::select! {{
                _ = interval.tick() => {{
                    flush_buffer(&buffer).await;
                }}
                _ = cancel.cancelled() => {{
                    tracing::info!("shutting down, performing final flush");
                    flush_buffer(&buffer).await;  // 最终清理
                    break;
                }}
            }}
        }}
    }}

    async fn flush_buffer(buffer: &Arc<Mutex<Vec<Event>>>) {{
        let events: Vec<_> = buffer.lock().await.drain(..).collect();
        if !events.is_empty() {{
            if let Err(e) = db.batch_insert(&events).await {{
                // flush 失败 → 写入 WAL / 本地文件做兜底
                tracing::error!("flush failed, writing to WAL: {{e}}");
                wal.append(&events).await;
            }}
        }}
    }}

    // 主函数：确保后台任务完成 flush 后再退出
    async fn main() {{
        let cancel = CancellationToken::new();
        let bg_handle = tokio::spawn(background_flush(buffer, cancel.child_token()));
        // ... 等待 shutdown 信号 ...
        cancel.cancel();
        bg_handle.await.unwrap();  // 等 final flush 完成
    }}
"#
    );
    println!("  权衡：select! + cancel token 简单，但需要 cancel-safe；");
    println!("        用 drop guard + oneshot 可以保证 flush 但增复杂度。");
    println!("  泛化：后台任务持有 CancellationToken，每个 tick 检查；");
    println!("        对重要数据用 WAL / 持久化队列，不依赖内存 flush。\n");

    // ── 7.3 ──
    println!("【题 7.3】滚动部署：新旧 Pod 同时运行，旧 Pod 接到 SIGTERM。");
    println!("  现象：LB 还在发流量给旧 Pod（健康检查间隔 10s），但旧 Pod 已关端口。");
    println!("  问：从「收到信号」到「进程退出」之间应做什么？\n");
    println!("  ✅ K8s 友好的 shutdown 时序:");
    println!(
        r#"
    // Kubernetes Pod spec:
    // terminationGracePeriodSeconds: 60
    // lifecycle:
    //   preStop:
    //     exec:
    //       command: ["sleep", "15"]  # 等 LB 摘除（≥健康检查间隔）

    async fn run_server() {{
        let is_healthy = Arc::new(AtomicBool::new(true));

        // 健康检查端点
        let health = is_healthy.clone();
        let health_route = warp::path("health").map(move || {{
            if health.load(Ordering::Relaxed) {{
                warp::reply::with_status("OK", StatusCode::OK)
            }} else {{
                warp::reply::with_status("draining", StatusCode::SERVICE_UNAVAILABLE)
            }}
        }});

        // 收到 SIGTERM 后的时序
        tokio::signal::ctrl_c().await.unwrap();

        // Step 1: 标记不健康 → LB 下一次探测将摘除此 Pod
        is_healthy.store(false, Ordering::Relaxed);
        tracing::info!("marked unhealthy, waiting for LB to drain");

        // Step 2: 等一个探测周期，确保 LB 已停止发新流量
        tokio::time::sleep(Duration::from_secs(15)).await;

        // Step 3: drain 存量请求（deadline = terminationGracePeriod - preStop - buffer）
        // 60s - 15s(preStop) - 15s(等探测) - 5s(buffer) = 25s
        tokio::time::timeout(
            Duration::from_secs(25),
            drain_in_flight_requests(),
        ).await.ok();

        tracing::info!("shutdown complete");
    }}
"#
    );
    println!("  权衡：先下健康检查再 drain vs 直接断开；取决于 LB 实现与超时配置。");
    println!("  泛化：SIGTERM → 健康检查返回 unhealthy → 等一个检查周期 → drain →");
    println!("        超时退出；readiness 探针与 shutdown 状态联动；");
    println!("        preStop sleep > LB 摘除延迟，减少连接重置。\n");
}

// ────────────────────────────────────────────────────────────────────
// 八、Async Trait、抽象边界与生态兼容
// ────────────────────────────────────────────────────────────────────

pub fn print_section8_async_trait_abstraction() {
    println!("────────── 八、Async Trait、抽象边界与生态兼容 ──────────\n");

    // ── 8.1 ──
    println!("【题 8.1】定义 trait Storage {{ async fn get(...); }}，需要 dyn dispatch。");
    println!("  现象：Rust 1.75+ 原生支持 async fn in trait，但 `dyn Storage` 报错。");
    println!("  问：AFIT（Async Fn In Trait）在什么条件下可用 dyn dispatch？\n");
    println!("  代码对比:");
    println!(
        r#"
    // ❌ AFIT 不支持 dyn dispatch
    trait Storage {{
        async fn get(&self, key: &str) -> Option<Vec<u8>>;
    }}
    // fn use_storage(s: &dyn Storage) {{ ... }}
    // error: the trait `Storage` cannot be made into an object

    // ✅ 方案 A：泛型静态分发（零成本，但不能做 trait object）
    async fn read_config<S: Storage>(store: &S) -> Config {{
        let bytes = store.get("config").await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }}

    // ✅ 方案 B：手动 boxing wrapper 支持 dyn
    trait StorageDyn: Send + Sync {{
        fn get(&self, key: &str) -> Pin<Box<dyn Future<Output = Option<Vec<u8>>> + Send + '_>>;
    }}
    // 为所有实现了 Storage 的类型自动实现 StorageDyn
    impl<T: Storage + Send + Sync> StorageDyn for T {{
        fn get(&self, key: &str) -> Pin<Box<dyn Future<Output = Option<Vec<u8>>> + Send + '_>> {{
            Box::pin(<Self as Storage>::get(self, key))
        }}
    }}
    // 现在可以用 dyn：
    async fn read_config_dyn(store: &dyn StorageDyn) -> Config {{
        let bytes = store.get("config").await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }}

    // ✅ 方案 C：enum dispatch（已知实现集合时最优）
    enum AnyStorage {{
        Redis(RedisStorage),
        Postgres(PgStorage),
        InMemory(MemStorage),
    }}
    impl AnyStorage {{
        async fn get(&self, key: &str) -> Option<Vec<u8>> {{
            match self {{
                Self::Redis(s) => s.get(key).await,
                Self::Postgres(s) => s.get(key).await,
                Self::InMemory(s) => s.get(key).await,
            }}
        }}
    }}
"#
    );
    println!("  权衡：静态 `impl Storage` 无堆分配但不能做 trait object；");
    println!("        boxing wrapper 灵活但有堆分配；enum dispatch 兼顾性能与灵活。");
    println!("  泛化：内部实现用泛型 `impl Storage` → 零成本；");
    println!("        需要 dyn 的边界（插件、测试 mock）→ 包一层 boxing wrapper；");
    println!("        评估是否真需要 dyn：enum dispatch 往往足够且更快。\n");

    // ── 8.2 ──
    println!("【题 8.2】中间件/拦截器链：每层是 async fn，需要动态组合。");
    println!("  场景：HTTP middleware: logging → auth → rate-limit → handler，层数运行时决定。");
    println!("  问：tower `Service` trait 的设计为何不用 async fn？\n");
    println!("  代码示例:");
    println!(
        r#"
    // tower Service trait（简化版）— 用关联类型 Future 实现静态分发
    trait Service<Request> {{
        type Response;
        type Error;
        type Future: Future<Output = Result<Self::Response, Self::Error>>;

        fn call(&mut self, req: Request) -> Self::Future;
    }}

    // 泛型嵌套（编译期已知层数）→ 零分配、可内联
    // 类型签名：Logging<Auth<RateLimit<MyHandler>>>
    let svc = ServiceBuilder::new()
        .layer(LoggingLayer)
        .layer(AuthLayer::new(secret))
        .layer(RateLimitLayer::new(100))
        .service(MyHandler);
    // 编译后 svc 是一个具体类型，poll 可内联穿透所有层

    // 运行时动态层数 → BoxCloneService 类型擦除
    type BoxedService = BoxCloneService<Request, Response, Error>;
    fn build_pipeline(layers: &[LayerConfig]) -> BoxedService {{
        let mut svc: BoxedService = BoxCloneService::new(MyHandler);
        for layer_cfg in layers.iter().rev() {{
            svc = match layer_cfg {{
                LayerConfig::Logging => BoxCloneService::new(LoggingLayer.layer(svc)),
                LayerConfig::Auth(s) => BoxCloneService::new(AuthLayer::new(s).layer(svc)),
                LayerConfig::RateLimit(n) =>
                    BoxCloneService::new(RateLimitLayer::new(*n).layer(svc)),
            }};
        }}
        svc  // 每层一次 Box 分配 + vtable；hot path 无法内联
    }}
"#
    );
    println!("  权衡：tower 用关联类型 Future → 静态分发、零分配；");
    println!("        但类型签名极长，Box 化简化类型但丧失内联优化。");
    println!("  泛化：层数编译期已知 → 泛型嵌套（自动单态化）；");
    println!("        层数运行时决定 / 插件系统 → `BoxCloneService`；");
    println!("        hot path 避免 dyn；cold path（启动、配置）dyn 无所谓。\n");

    // ── 8.3 ──
    println!("【题 8.3】在 async 代码中使用泛型 `F: Future<Output = T>` 参数。");
    println!("  现象：调用方传入的 Future 可能是 `!Send`，spawn 时编译失败。");
    println!("  问：何时该加 `+ Send + 'static`？加了之后哪些合法 Future 被排除？\n");
    println!("  代码对比:");
    println!(
        r#"
    // ❌ 过于宽松：无法 spawn 到多线程 runtime
    async fn run_task<F: Future<Output = ()>>(f: F) {{
        tokio::spawn(f);
        // error: `F` cannot be sent between threads safely
        // 因为 F 可能是 !Send（比如持有 Rc）
    }}

    // ✅ 对外 API：加 Send + 'static 保证可 spawn
    async fn run_task<F>(f: F)
    where
        F: Future<Output = ()> + Send + 'static,  // 排除了 !Send 和含引用的 Future
    {{
        tokio::spawn(f);  // OK
    }}

    // ✅ 内部组合子（不 spawn，无需 Send）：可以宽松
    async fn map_future<F, T, U>(f: F, transform: impl Fn(T) -> U) -> U
    where
        F: Future<Output = T>,  // 不要求 Send → 可接受 !Send Future
    {{
        let result = f.await;   // 在当前 task 内 await，不跨线程
        transform(result)
    }}

    // 对比：Send + 'static 排除了什么？
    async fn uses_rc() {{
        let data = Rc::new(42);        // Rc 是 !Send
        tokio::time::sleep(Duration::from_secs(1)).await;
        println!("{{}}", data);         // 此 Future 是 !Send → 不能 spawn
    }}
    async fn uses_local_ref(s: &str) {{
        tokio::time::sleep(Duration::from_secs(1)).await;
        println!("{{}}", s);           // 此 Future 含引用 → 不是 'static → 不能 spawn
    }}
    // 以上两种 Future 只能在当前 task await 或用 spawn_local
"#
    );
    println!("  权衡：`Send + 'static` 使 spawn 安全但禁止借用局部引用和 !Send 类型；");
    println!("        放宽到 `spawn_local` 或 `LocalSet` 可兼容但限制了多线程。");
    println!("  泛化：对外 API 的 Future 参数默认 `Send + 'static`（兼容最多调用方式）；");
    println!("        内部不 spawn 的组合子可宽松些；");
    println!("        文档显式标注 Send 要求，避免用户试错。\n");
}

// ────────────────────────────────────────────────────────────────────
// 九、可观测性、调试与确定性测试
// ────────────────────────────────────────────────────────────────────

pub fn print_section9_observability_testing() {
    println!("────────── 九、可观测性、调试与确定性测试 ──────────\n");

    // ── 9.1 ──
    println!("【题 9.1】线上 async 服务出现间歇性慢请求，但 CPU / IO 指标正常。");
    println!("  现象：p99 延迟偶尔飙到几百 ms，p50 正常；日志里无异常。");
    println!("  问：怀疑是「某个 task 占住 executor 线程太久」，如何验证？\n");
    println!("  ✅ 排查手段与代码:");
    println!(
        r#"
    // 1) 用 tracing 标注 span，定位慢 poll
    use tracing::{{info_span, Instrument}};
    async fn handle_request(req: Request) -> Response {{
        async {{
            let data = fetch_db(req.id).await;
            let enriched = enrich(data).await;
            Response::ok(enriched)
        }}
        .instrument(info_span!("handle_request", req_id = %req.id))
        .await
    }}

    // 2) 配置 tracing subscriber 输出慢 span
    tracing_subscriber::fmt()
        .with_span_events(FmtSpan::CLOSE)  // span 关闭时打印耗时
        .init();
    // 输出示例：handle_request{{req_id=42}} close time.busy=3.2ms time.idle=150ms

    // 3) tokio-console（开发环境实时看 task 调度）
    // Cargo.toml: tokio = {{ features = ["tracing"] }}
    // 编译：RUSTFLAGS="--cfg tokio_unstable" cargo build
    // 安装：cargo install tokio-console
    // 运行后在另一个终端：tokio-console
    // 可看到每个 task 的 poll 次数、busy time、idle time、waker 统计

    // 4) 程序化检测阻塞（runtime metrics）
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .on_thread_park(|| {{
            // worker 线程空闲时调用 → 如果很少调用说明线程忙
            metrics::counter!("runtime.thread_park").increment(1);
        }})
        .build().unwrap();
"#
    );
    println!("  权衡：tracing instrumentation 有采样成本；tokio-console 仅 dev 环境可用。");
    println!("  泛化：接入 `tracing` + `tracing-opentelemetry` 标注 span → 定位 slow poll；");
    println!("        开启 runtime metrics 看 worker busy/idle；");
    println!("        怀疑阻塞 → `tokio_unstable` + `tokio-console` 排查 task 调度。\n");

    // ── 9.2 ──
    println!("【题 9.2】async 代码 panic 后的栈回溯看不到业务调用链，全是 runtime 内部帧。");
    println!("  问：为什么 async 栈回溯与同步代码不同？如何恢复因果链？\n");
    println!("  问题示意:");
    println!(
        r#"
    // 同步代码 panic 的栈回溯 — 完整调用链：
    // thread 'main' panicked at 'oh no'
    //   main::process_order    ← 业务代码清晰可见
    //   main::validate_payment
    //   main::charge_card
    //   main::main

    // async 代码 panic 的栈回溯 — 只有 runtime 帧：
    // thread 'tokio-runtime-worker' panicked at 'oh no'
    //   std::panicking::begin_panic
    //   my_crate::charge_card::{{{{async}}}}  ← 只有最终出错的 async fn
    //   <core::future::from_generator::GenFuture as core::future::Future>::poll
    //   tokio::runtime::task::harness::poll  ← 全是 runtime 内部
    //   tokio::runtime::scheduler::multi_thread::worker::Context::run_task
    // 看不到 process_order → validate_payment → charge_card 的因果链！
"#
    );
    println!("  ✅ 用 tracing span 链恢复因果关系:");
    println!(
        r#"
    use tracing::{{info_span, Instrument}};

    async fn process_order(order: Order) {{
        validate_payment(&order)
            .instrument(info_span!("validate_payment", order_id = %order.id))
            .await;
    }}

    async fn validate_payment(order: &Order) {{
        charge_card(order.card_id)
            .instrument(info_span!("charge_card", card_id = %order.card_id))
            .await;
    }}

    // tracing 输出 → 完整因果链：
    // ERROR charge_card{{card_id=42}} > validate_payment{{order_id=7}} > process_order
    //   panicked at 'insufficient funds'

    // spawn 时也要传递 span：
    async fn handle_request(req: Request) {{
        let span = info_span!("request", id = %req.id);
        tokio::spawn(
            async move {{
                process_order(req.into_order()).await;
            }}
            .instrument(span)  // 子 task 继承父 span 上下文
        );
    }}
"#
    );
    println!("  权衡：保留完整 async backtrace 有内存/性能成本；生产通常只保留 span 上下文。");
    println!(
        "  泛化：用 `tracing::Instrument` 给每个 spawn 附 span → 用 span 链代替 stack trace；"
    );
    println!("        关键路径 `.instrument(info_span!(\"request\", id = %req_id))`。\n");

    // ── 9.3 ──
    println!("【题 9.3】单元测试要验证「超时 → 重试 → 成功」的完整流程。");
    println!("  现象：测试里 `tokio::time::sleep` 真的等了 30 秒，CI 慢到不可接受。");
    println!("  问：如何在测试中控制时间流逝？\n");
    println!("  ✅ 两种方案:");
    println!(
        r#"
    // 方案 A：tokio 内置时间控制
    #[tokio::test(start_paused = true)]  // 启动时暂停时间
    async fn test_retry_on_timeout() {{
        let mut attempts = 0;
        let client = MockClient::new(vec![
            Err(Timeout),    // 第 1 次超时
            Err(Timeout),    // 第 2 次超时
            Ok(Response),    // 第 3 次成功
        ]);

        let result = retry_with_backoff(&client, 3).await;
        assert!(result.is_ok());
        assert_eq!(client.call_count(), 3);
        // 测试瞬间完成！sleep 和 timeout 在 paused 模式下自动推进
    }}

    // 方案 B：注入 Clock trait（与 runtime 解耦）
    trait Clock: Send + Sync {{
        fn now(&self) -> Instant;
        fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send>>;
    }}

    struct TokioClock;
    impl Clock for TokioClock {{
        fn now(&self) -> Instant {{ Instant::now() }}
        fn sleep(&self, dur: Duration) -> Pin<Box<dyn Future<Output = ()> + Send>> {{
            Box::pin(tokio::time::sleep(dur))
        }}
    }}

    struct FakeClock {{
        current: Arc<Mutex<Instant>>,
    }}
    impl Clock for FakeClock {{
        fn now(&self) -> Instant {{ *self.current.lock().unwrap() }}
        fn sleep(&self, dur: Duration) -> Pin<Box<dyn Future<Output = ()> + Send>> {{
            // 立即完成，但推进时钟
            let current = self.current.clone();
            Box::pin(async move {{
                *current.lock().unwrap() += dur;
            }})
        }}
    }}

    // 业务代码面向 trait 编程：
    async fn retry_with_backoff<C: Clock>(client: &Client, clock: &C) -> Result<()> {{
        for i in 0..3 {{
            match client.call().await {{
                Ok(r) => return Ok(r),
                Err(_) => clock.sleep(Duration::from_secs(1 << i)).await,
            }}
        }}
        Err(anyhow!("max retries"))
    }}
"#
    );
    println!("  权衡：`tokio::time::pause()` 简单但与真实 I/O 混用时有陷阱。");
    println!("  泛化：测试用 `#[tokio::test(start_paused = true)]` + `tokio::time::advance`；");
    println!("        把时钟抽象为 trait `Clock` 方便注入；");
    println!("        不依赖挂钟的逻辑用 Instant mock 或 fake timer。\n");

    // ── 9.4 ──
    println!("【题 9.4】集成测试需要确定性重放并发 bug（race condition 复现）。");
    println!("  现象：测试偶尔失败，本地跑 100 次也不稳定复现。");
    println!("  问：async 代码的非确定性源头有哪些？如何约束？\n");
    println!("  ✅ 三层防御:");
    println!(
        r#"
    // 层 1：收窄共享状态 → 消除 race 根因
    // ❌ 多 task 通过 Arc<Mutex<>> 竞争写 counter
    let counter = Arc::new(Mutex::new(0));
    for _ in 0..10 {{
        let c = counter.clone();
        tokio::spawn(async move {{ *c.lock().await += 1; }});
    }}

    // ✅ 改为 channel 串行化
    let (tx, mut rx) = mpsc::channel(10);
    tokio::spawn(async move {{
        let mut counter = 0;
        while let Some(()) = rx.recv().await {{
            counter += 1;  // 单一 owner，无 race
        }}
    }});

    // 层 2：用 current_thread runtime 减少调度非确定性
    #[tokio::test(flavor = "current_thread")]
    async fn test_ordering() {{
        // 单线程 → task 切换只在 .await 点
        // 更容易推理执行顺序
    }}

    // 层 3：用 loom 做低层原语穷举测试
    #[cfg(loom)]
    #[test]
    fn test_concurrent_counter() {{
        loom::model(|| {{
            let counter = loom::sync::Arc::new(loom::sync::atomic::AtomicUsize::new(0));
            let threads: Vec<_> = (0..2).map(|_| {{
                let c = counter.clone();
                loom::thread::spawn(move || {{
                    c.fetch_add(1, Ordering::SeqCst);
                }})
            }}).collect();
            for t in threads {{ t.join().unwrap(); }}
            assert_eq!(counter.load(Ordering::SeqCst), 2);
        }});
        // loom 会自动探索所有可能的线程交错顺序
    }}
"#
    );
    println!(
        "  权衡：完全确定性调度（如 loom）只适用小范围；大系统靠 property testing + 大量迭代。"
    );
    println!("  泛化：用 `current_thread` runtime 减少线程交错；");
    println!("        用 `loom` 做低层并发原语的穷举测试；");
    println!("        业务层 race condition → 收窄共享状态 + channel 串行化关键路径。\n");
}

// ────────────────────────────────────────────────────────────────────
// 决策备忘
// ────────────────────────────────────────────────────────────────────

pub fn print_cheat_sheet() {
    println!("────────── 决策备忘（可扫一眼）──────────\n");
    println!("  【执行模型】");
    println!("  大量并发 I/O、会话 mostly idle     → async + 非阻塞 I/O；忌一线程一会话");
    println!("  长计算 / 阻塞 syscall              → spawn_blocking / rayon；勿占 executor");
    println!("  尾延迟抖动                         → 分 runtime、限并发、避免 async 内长临界区\n");
    println!("  【Send 与锁】");
    println!(
        "  跨 .await 持有状态                 → Arc + Send 或 LocalSet/单线程；忌 Rc 跨 await"
    );
    println!("  临界区含 .await                    → tokio::sync::Mutex 或 actor 模式\n");
    println!("  【取消与背压】");
    println!("  客户端消失仍干活                   → CancellationToken + select! + Drop guard");
    println!("  生产快于消费                       → 有界 channel / Semaphore / 显式反压");
    println!("  对外调用无超时                     → timeout() 包裹 + 指数退避 + 幂等键\n");
    println!("  【结构化并发】");
    println!("  spawn 后不 join                    → JoinSet/TaskTracker 收集；不允许错误黑洞");
    println!(
        "  select! 丢弃分支                   → 只 select cancel-safe Future；写操作 spawn 隔离"
    );
    println!("  大扇出请求                         → Semaphore 限并发 + buffer_unordered 流式\n");
    println!("  【Future 性能】");
    println!("  async fn 状态机过大                → 大缓冲用 Vec；局部量缩短跨 await 作用域");
    println!("  递归 async fn                      → Box::pin 打断；深递归改迭代 + 手动栈");
    println!("  异构 Future 集合                   → enum dispatch 优先；需动态 → Pin<Box<dyn>>\n");
    println!("  【优雅关停】");
    println!("  SIGTERM / Ctrl+C                   → 停接新 → drain(deadline) → abort");
    println!("  后台任务 flush                     → CancellationToken + final flush + WAL 兜底\n");
    println!("  【抽象边界】");
    println!("  库 vs 应用                         → 库暴露 async fn，不私起 runtime");
    println!("  async trait + dyn                  → 静态泛型优先；dyn 边界用 boxing wrapper");
    println!("  泛型 Future 参数                   → 对外 Send + 'static；内部组合子可宽松\n");
    println!("  【可观测性】");
    println!("  间歇慢请求                         → tracing span(time.busy) + tokio-console");
    println!("  async 栈回溯无业务帧               → .instrument(span) 给每个 spawn 附 span");
    println!("  测试控制时间                       → start_paused + Clock trait 注入");
    println!("  并发 bug 复现                      → current_thread + loom + 收窄共享状态");
    println!("────────────────────────────────────────\n");
}

pub fn print_all() {
    print_header();
    print_section_1_executor_model();
    print_section2_send_blocking();
    print_section3_cancellation_backpressure();
    print_section4_runtime_choice();
    print_section5_structured_concurrency();
    print_section6_pin_future_perf();
    print_section7_graceful_shutdown();
    print_section8_async_trait_abstraction();
    print_section9_observability_testing();
    print_cheat_sheet();
}
