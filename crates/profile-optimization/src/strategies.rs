//! # 泛化：从 HFT/Web3 场景到通用 profiling 策略
//!
//! 把前两章具体业务里的 profiling 套路抽象成决策矩阵：
//!
//! | 问题类型              | 标志特征                     | 首选套路                           |
//! |-----------------------|------------------------------|------------------------------------|
//! | 1. 建立基线           | 无 before 数字               | release bench + 存档 commit hash   |
//! | 2. CPU 热点           | 高 CPU、未知函数             | perf record + flamegraph           |
//! | 3. 尾延迟             | P99 超标、mean 正常          | histogram + trace 对齐             |
//! | 4. 分配抖动           | 周期性 latency spike         | dhat / heaptrack / AllocCounter    |
//! | 5. 锁/等待            | perf 见 futex、吞吐随核数不升| perf lock + shard 验证             |
//! | 6. 流水线分解         | RPC/bundle 多阶段            | StageTimer / tracing span          |
//! | 7. 回归门禁           | PR 引入性能回退              | criterion in CI + threshold        |
//! | 8. 生产持续 profile   | 线上偶发慢请求               | continuous profiler + 采样         |
//!
//! 下面 8 个策略各有一个通用模板。

#![allow(dead_code)]

// ============================================================================
// 策略 1：建立基线 —— before 数字 + git sha
// ============================================================================
/// 问题：优化无对照组，无法证明收益。
/// 模式：release build → bench → 记录 ns/op + commit + 数据规模。
///
/// HFT: hft::steady_state_bench
/// Web3: web3::block_replay_throughput
pub mod establish_baseline {
    pub struct Baseline {
        pub label: String,
        pub ns_per_op: u64,
        pub ops: u64,
    }

    pub fn measure(label: &str, ops: u64, mut f: impl FnMut()) -> Baseline {
        let (min, _) = crate::util::bench_ns(50, 100, || {
            f();
        });
        Baseline {
            label: label.to_string(),
            ns_per_op: min / ops.max(1),
            ops,
        }
    }

    pub fn demonstrate() {
        println!("## 策略 1：建立基线");
        let b = measure("sum_1k", 1000, || {
            (0..1000).sum::<u64>();
        });
        println!("{label}: {ns}ns/op (ops={ops})", label = b.label, ns = b.ns_per_op, ops = b.ops);
        println!("存档：commit + PROFILE=release + 数据规模\n");
    }
}

// ============================================================================
// 策略 2：CPU 热点 —— perf + flamegraph
// ============================================================================
/// 问题：不知道 CPU 时间花在哪。
/// 模式：`perf record -g` → collapse → flamegraph → 找最宽栈。
///
/// HFT: hft::fix_parse_cpu
/// Web3: web3::merkle_rebuild
pub mod cpu_hotspot {
    pub fn demonstrate() {
        println!("## 策略 2：CPU 热点 flamegraph");
        println!("  perf record -g --call-graph dwarf -F 997 ./app");
        println!("  perf script | inferno-collapse-perf | inferno-flamegraph > fg.svg");
        println!("规则：最宽帧 = 优先优化；<5% 忽略\n");
    }
}

// ============================================================================
// 策略 3：尾延迟 —— P99 histogram
// ============================================================================
/// 问题：偶发慢请求，mean 看不出来。
/// 模式：固定宽度 histogram → P99/P999 → 与 trace id 关联。
///
/// HFT: hft::tick_handler_p99
/// Web3: web3::bundle_simulation_budget
pub mod tail_latency {
    pub fn record_samples(samples: &[u64], width_ns: u64) -> crate::util::LatencyHistogram {
        let mut h = crate::util::LatencyHistogram::new(128, width_ns);
        for &s in samples {
            h.record(s);
        }
        h
    }

    pub fn demonstrate() {
        println!("## 策略 3：尾延迟 P99");
        let samples: Vec<u64> = (0..1000).map(|i| if i < 990 { 800 } else { 80_000 }).collect();
        let h = record_samples(&samples, 1000);
        println!("mean={}ns p99={}ns", h.mean_ns(), h.p99_ns());
        println!("规则：SLA 写 P99；告警用 histogram 而非 gauge mean\n");
    }
}

// ============================================================================
// 策略 4：分配 profiling —— heap churn
// ============================================================================
/// 问题：周期性 latency spike，CPU 不高。
/// 模式：dhat-rs 跑一轮 → bytes allocated / realloc 排序。
///
/// HFT: hft::allocator_churn
/// Web3: mempool 大 calldata clone
pub mod allocation_profile {
    pub fn demonstrate() {
        println!("## 策略 4：分配 profiling");
        println!("  RUSTFLAGS='-C force-frame-pointers=yes' cargo run --features dhat-heap");
        println!("规则：热路径零 alloc；Vec reuse / arena / bump\n");
    }
}

// ============================================================================
// 策略 5：锁竞争 —— perf lock + shard
// ============================================================================
/// 问题：加核不提速，P99 尖刺。
/// 模式：perf lock record → 对比 shard 后 P99 + 吞吐。
///
/// HFT: hft::lock_contention
/// Web3: global nonce manager
pub mod lock_contention {
    pub fn demonstrate() {
        println!("## 策略 5：锁竞争");
        println!("  perf lock record -g ./app");
        println!("规则：futex 栈 → 缩小锁粒度 / sharded lock / lock-free\n");
    }
}

// ============================================================================
// 策略 6：流水线分解 —— stage timer
// ============================================================================
/// 问题：端到端慢，不知哪段。
/// 模式：StageTimer / tracing span → 最大 stage 优先。
///
/// HFT: order entry pipeline
/// Web3: web3::rpc_eth_call_breakdown
pub mod pipeline_breakdown {
    use std::time::Duration;

    pub fn run_pipeline() -> Duration {
        let mut t = crate::util::StageTimer::new();
        t.time("stage_a", || (0..10_000).sum::<u64>());
        t.time("stage_b", || (0..50_000).sum::<u64>());
        t.total()
    }

    pub fn demonstrate() {
        println!("## 策略 6：流水线分解");
        let total = run_pipeline();
        println!("pipeline total ≈ {:.2?}", total);
        println!("规则：最大 stage 优先；对齐 timeout budget\n");
    }
}

// ============================================================================
// 策略 7：回归 CI —— criterion threshold
// ============================================================================
/// 问题：PR 悄悄引入 10% 回归。
/// 模式：benches/ + criterion + CI fail on >5% regression。
///
/// HFT/Web3: 所有优化 PR 必备
pub mod regression_ci {
    pub fn demonstrate() {
        println!("## 策略 7：回归 CI");
        println!("  cargo bench -p my-crate -- --save-baseline main");
        println!("  cargo bench -p my-crate -- --baseline main");
        println!("规则：threshold 5%；bench 与 prod 同 RUSTFLAGS\n");
    }
}

// ============================================================================
// 策略 8：生产持续 profiling —— 低采样在线
// ============================================================================
/// 问题：staging 无法复现线上偶发慢请求。
/// 模式：continuous profiler（1% 采样）+ 慢请求自动提升采样率。
///
/// HFT: 开盘尖刺
/// Web3: 主网拥堵时 bundle 超时
pub mod continuous_profile {
    pub fn demonstrate() {
        println!("## 策略 8：生产持续 profiling");
        println!("工具：parca / pyroscope / Google-Wide Profiling");
        println!("规则：常开低采样；P99 告警时 pull 1min flamegraph\n");
    }
}

pub fn demonstrate() {
    establish_baseline::demonstrate();
    cpu_hotspot::demonstrate();
    tail_latency::demonstrate();
    allocation_profile::demonstrate();
    lock_contention::demonstrate();
    pipeline_breakdown::demonstrate();
    regression_ci::demonstrate();
    continuous_profile::demonstrate();
}
