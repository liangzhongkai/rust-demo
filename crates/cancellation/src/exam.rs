//! Cancellation：生产向场景题、权衡与泛化策略
//! 结构：生产场景 → 问题表现 → 改进方向 → 权衡 → 泛化

pub fn print_header() {
    println!("=== Cancellation：场景题、Trade-off 与泛化策略 ===\n");
    println!("说明：下列题目按「生产场景 → 典型失误 → 改进方向 → 权衡 → 泛化」组织。");
    println!("可与 tokio::select!、CancellationToken、JoinHandle::abort 等 API 对照。\n");
}

// ────────────────────────────────────────────────────────────────────
// 一、语义与心智模型
// ────────────────────────────────────────────────────────────────────

pub fn print_section_1_semantics() {
    println!("────────── 一、语义与心智模型 ──────────\n");

    println!("【题 1.1】微服务里「取消下游 HTTP」与「杀掉本地线程」是一回事吗？");
    println!("  生产场景：上游请求超时，网关向本服务发取消信号；本服务正在 fan-out 三个下游。");
    println!(
        "  典型失误：以为 `task.abort()` 或 `drop(future)` 会像 SIGKILL 一样立刻停掉所有副作用。"
    );
    println!("  要点：Rust/Tokio 的取消本质是「协作式」—— Future 在 await 点才能被驱动停止；");
    println!("        已提交的同步段、未挂钩的阻塞调用、fire-and-forget 的 spawn 可能继续跑。");
    println!("  权衡：协作式取消实现简单、无数据竞争；强杀（进程级）简单但有全局副作用。");
    println!("  泛化：任何「可中断工作」都要定义检查点（await、循环内 poll、显式 token）；");
    println!(
        "        把「停止计算」与「撤销已提交的副作用」分成两阶段（compensating transaction）。\n"
    );

    println!("【题 1.2】`drop` 一个 `JoinHandle` / 不再 poll 一个 Future，工作停了吗？");
    println!(
        "  生产场景：请求处理 Future 被上层超时丢弃，但内部 `tokio::spawn` 了写审计日志的任务。"
    );
    println!("  典型失误：认为父 Future 取消后，子任务自动全部取消。");
    println!(
        "  要点：`JoinHandle` 被 drop 时，默认 detached 任务仍运行；结构化并发（join/join_set）"
    );
    println!("        或显式 `abort`、或 `CancellationToken` 传播才能对齐生命周期。");
    println!("  权衡：detach 灵活但难推理；强制 join 拖慢响应；token 传播需要库支持。");
    println!("  泛化：子任务生命周期策略三选一：继承取消、必须跑完（fire-and-forget 明确文档）、");
    println!("        或独立 SLA（单独队列）。\n");

    println!("【题 1.3】`timeout` 与 `CancellationToken` 解决的是同一类问题吗？");
    println!("  生产场景：用户关闭浏览器标签；运维滚动发布要排空连接。");
    println!("  典型失误：到处 `tokio::time::timeout`，没有统一「关停」语义。");
    println!("  要点：timeout 是「时间上限」；token 是「外部意图」—— 可组合、可层级、可测试。");
    println!("  权衡：timeout 实现快；token 适合与 HTTP disconnect、SIGTERM、配置热更对齐。");
    println!("  泛化：把「何时停止」建模为可组合信号（token OR timeout OR channel closed）。\n");
}

// ────────────────────────────────────────────────────────────────────
// 二、传播、层次与结构化并发
// ────────────────────────────────────────────────────────────────────

pub fn print_section_2_propagation() {
    println!("────────── 二、传播、层次与结构化并发 ──────────\n");

    println!(
        "【题 2.1】API 网关：一个请求触发 DB 查询 + 两个 gRPC + 缓存回填，任一失败要取消兄弟任务。"
    );
    println!("  生产场景：尾延迟敏感，不希望失败路径上其他 RPC 继续占用连接池与配额。");
    println!("  典型失误：用 `try_join!` 只聚合错误，没有在第一个 Err 上取消其他 branch。");
    println!(
        "  改进方向：`tokio::select!` biased + token，或 `FuturesUnordered` + 首个 Err 后 cancel，"
    );
    println!("        或使用支持取消的客户端（带 deadline / 可中断 stream）。");
    println!("  权衡：`try_join` 简洁但无「取消兄弟」；select 需手写；结构化并发 crate 增加依赖。");
    println!("  泛化：Fan-out 默认策略：「失败即取消未完成的 peer」或「全部跑完再合并」—— 必须在");
    println!("        API 设计里写清楚，并与资源池大小、计费模型一致。\n");

    println!("【题 2.2】长连接服务：父连接 token 取消时，子任务（心跳、读循环、写队列）谁先停？");
    println!("  生产场景：WebSocket 或 MQTT broker，一条连接上多个逻辑任务。");
    println!("  典型失误：只停读循环，写队列仍往已关闭 socket 写，或对端已走导致半开。");
    println!("  改进方向：单一 token 分叉；关停顺序：停止接受新写 → flush/drain → 关 fd；");
    println!("        或统一 `GracefulShutdown` 状态机。");
    println!("  权衡：顺序关停延迟略增；乱序可能产生日志噪音与错误码。");
    println!("  泛化：有向无环的「关停依赖图」；把顺序写进 runbook 与监控（各阶段耗时）。\n");

    println!("【题 2.3】批处理：MapReduce 式，1000 个分片，主机收到 SIGTERM 要在 30s 内尽量 checkpoint。");
    println!("  生产场景：K8s preStop、Spot 实例回收。");
    println!("  典型失误：收到信号立刻 `abort` 所有任务，无 checkpoint，从头重跑成本高。");
    println!("  改进方向：两级信号——「停止接收新分片」+「当前分片尽力完成或保存中间态」；");
    println!("        与外部协调（Kafka 位点、对象存储 multipart）。");
    println!("  权衡：优雅关停延长占用资源；硬截止避免永远挂起。");
    println!(
        "  泛化：批处理取消 = min(外部 deadline, 内部幂等与 checkpoint 策略)，不是单一 abort。\n"
    );
}

// ────────────────────────────────────────────────────────────────────
// 三、资源、事务与副作用
// ────────────────────────────────────────────────────────────────────

pub fn print_section_3_side_effects() {
    println!("────────── 三、资源、事务与副作用 ──────────\n");

    println!("【题 3.1】请求取消时，已 `BEGIN` 的数据库事务必须怎样？");
    println!("  生产场景：ORM 在 async 里开事务，客户端断开导致 Future 不再被 poll。");
    println!("  典型失误：依赖 Drop 回滚——连接归还池时事务状态未定义；或泄漏长事务。");
    println!("  改进方向：显式 `spawn` + timeout 的 rollback；或使用支持 cancel callback 的池；");
    println!("        短事务 + 幂等写，使「重试」安全。");
    println!("  权衡：每次取消都 rollback 增加负载；幂等设计前期成本高。");
    println!(
        "  泛化：「取消」不是免费—— 为每个持锁/持连接的操作定义释放路径（RAII + 显式协议）。\n"
    );

    println!("【题 3.2】上传大文件到对象存储：已传 80%，用户点取消，应如何处理？");
    println!("  生产场景：分片上传、断点续传、计费按流量。");
    println!("  典型失误：客户端停读，服务端仍读完 body，或孤儿 multipart 永远占配额。");
    println!("  改进方向：检测对端关闭/HTTP reset；中止时 `AbortMultipartUpload`；");
    println!("        生命周期规则（N 天后清理未完成 upload）。");
    println!("  权衡：尽早检测取消减少带宽 vs 实现复杂度（各云 SDK 差异）。");
    println!("  泛化：任何「跨边界的长时间操作」需要：取消检测点 + 远端清理协议 + 定期 GC。\n");

    println!("【题 3.3】消息队列消费者：`process` 里 await 很慢，收到再平衡要尽快释放分区。");
    println!("  生产场景：Kafka consumer 被 revoke，仍在处理中的消息若提交 offset 会丢或重复。");
    println!("  典型失误：只 cancel 业务 task，未与 consumer 协议交互，导致重复消费风暴或停滞。");
    println!(
        "  改进方向：协作式：revoke 信号 → token cancel → 当前消息处理完或超时 → 同步提交策略；"
    );
    println!("        与「至少一次」语义文档一致。");
    println!("  权衡：快释放分区 vs 处理完当前批；幂等消费端是长期解。");
    println!("  泛化：取消与「外部系统契约」（offset、锁、租约）绑定，不能只看本地 task。\n");
}

// ────────────────────────────────────────────────────────────────────
// 四、竞态、幂等与测试
// ────────────────────────────────────────────────────────────────────

pub fn print_section_4_races_testing() {
    println!("────────── 四、竞态、幂等与测试 ──────────\n");

    println!("【题 4.1】`cancelled` 与 `completed` 几乎同时到达，如何避免双重投递？");
    println!("  生产场景：异步任务完成后要发通知；取消路径也要发「已取消」。");
    println!("  典型失误：两个路径都 `send`，下游收到两次或顺序错乱。");
    println!("  改进方向：单一状态机（Idle → Running → Done|Cancelled）；或 `tokio::sync::Notify`");
    println!("        一次唤醒；幂等 token（request_id）。");
    println!("  权衡：状态机啰嗦；单通道合并事件更清晰。");
    println!("  泛化：完成/取消/超时是互斥终态—— 设计为单一出口（single writer to result）。\n");

    println!("【题 4.2】如何测试「取消后不再访问已释放资源」？");
    println!("  生产场景：unsafe 或 FFI 边界，取消后指针失效。");
    println!("  典型失误：只测 happy path；取消路径在压力下才崩。");
    println!("  改进方向：`tokio::time::pause` 控制时间；注入 token 在确定性时刻触发；");
    println!("        Miri/loom 对同步结构；fuzz 取消点。");
    println!("  权衡：确定性测试需要可注入时钟与 IO；loom 组合爆炸。");
    println!("  泛化：可取消代码路径与主路径同等测试权重；把取消当作一等公民场景。\n");

    println!("【题 4.3】跨语言 FFI：Rust async 取消时，C 库里的阻塞调用怎么办？");
    println!("  生产场景：调用 legacy 压缩/加密库，内部无中断点。");
    println!("  典型失误：在 async 任务里直接调阻塞 FFI，取消不了，占满 worker。");
    println!("  改进方向：`spawn_blocking` + 可中断包装（若库支持）；进程隔离；");
    println!("        或接受「取消=不等待完成」并记录泄漏风险。");
    println!("  权衡：进程隔离最重但最干净；blocking 池增大延迟。");
    println!("  泛化：取消能力取决于最慢的不可中断段—— 架构上要么缩短该段，要么隔离。\n");
}

// ────────────────────────────────────────────────────────────────────
// 五、总表：Trade-off 与泛化速查
// ────────────────────────────────────────────────────────────────────

pub fn print_section_5_summary() {
    println!("────────── 五、总表：Trade-off 与泛化速查 ──────────\n");

    println!("| 维度           | 选项 A           | 选项 B           | 何时倾向 A / B |");
    println!("|----------------|------------------|------------------|----------------|");
    println!("| 取消模型       | 协作式 await     | 强杀进程/线程    | 正确性优先 / 运维止损 |");
    println!("| 信号来源       | 超时             | Token/用户/运维  | 简单 SLA / 多维关停 |");
    println!("| 子任务         | Detach           | Join/Abort       | 后台日志 / 须一致 |");
    println!("| Fan-out 失败   | 取消兄弟         | 等全部返回       | 资源紧 / 要全量错误 |");
    println!("| 数据面         | 尽快停读         | 事务回滚+清理    | 带宽 / 一致性     |");
    println!("| 测试           | 仅 happy path    | 注入取消+时钟    | 演示 / 生产信心   |");
    println!();

    println!("泛化策略（可背）：");
    println!("  1) 明确终态：完成 | 失败 | 取消 | 超时 —— 互斥，单一写入点。");
    println!("  2) 分层 token：全局关停 → 连接级 → 请求级，避免全局一把锁。");
    println!("  3) 副作用与取消配对：每个「提交」有对应的「补偿或幂等」。");
    println!("  4) 不可取消段显式标注：FFI/阻塞/长计算 —— 隔离或缩短。");
    println!("  5) 可观测：取消原因、阶段耗时、是否触发补偿 —— 否则线上黑盒。\n");
}

pub fn print_all() {
    print_header();
    print_section_1_semantics();
    print_section_2_propagation();
    print_section_3_side_effects();
    print_section_4_races_testing();
    print_section_5_summary();
}
