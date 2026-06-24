//! # Profiling 常见陷阱与诊断
//!
//! 生产事故里反复出现的 8 个 profiling 陷阱：
//! - 现象（监控/测试看到什么）
//! - 根因（测量层面发生了什么）
//! - 解决方案（修法 + 预防）

#![allow(dead_code)]

// ============================================================================
// 陷阱 1：无 profile 直接优化 —— 优化了非热点
// ============================================================================
/// **现象**：改了一周 SIMD，整体延迟无变化。
/// **根因**：热点在 I/O 或锁，CPU 算子不是瓶颈。
/// **修法**：先 flamegraph，确认 >5% CPU 再动手。
pub mod optimize_without_profile {
    #[inline(never)]
    pub fn real_bottleneck() -> u64 {
        std::thread::sleep(std::time::Duration::from_micros(10));
        1
    }

    #[inline(never)]
    pub fn fake_hotspot() -> u64 {
        (0..1000).map(|i| i as u64).sum()
    }

    pub fn demonstrate() {
        println!("## 陷阱 1：无 profile 直接优化");
        let (io_ns, _) = crate::util::bench_ns(2, 5, || {
            real_bottleneck();
        });
        let (cpu_ns, _) = crate::util::bench_ns(20, 50, || {
            fake_hotspot();
        });
        println!("I/O 等待 ≈ {io_ns}ns，CPU 循环 ≈ {cpu_ns}ns");
        println!("规则：先 profile，热点 <5% CPU 不值得 SIMD\n");
    }
}

// ============================================================================
// 陷阱 2：debug build 测性能
// ============================================================================
/// **现象**：bench 显示某优化无效，release 下其实有效。
/// **根因**：debug 无优化，inline/LTO 行为完全不同。
/// **修法**：性能结论只来自 `--release`。
pub mod debug_build_bench {
    pub fn demonstrate() {
        println!("## 陷阱 2：debug build 测性能");
        println!("当前 profile = {:?}（教学默认 debug）", std::env::var("PROFILE").unwrap_or_default());
        println!("规则：cargo build --release && perf/criterion on release binary\n");
    }
}

// ============================================================================
// 陷阱 3：无 warmup 的冷启动测量
// ============================================================================
/// **现象**：第一次调用 100x 慢，误以为回归。
/// **根因**：icache、branch predictor、lazy static 未预热。
/// **修法**：criterion warmup ≥100 iter；生产看 steady-state P99。
pub mod no_warmup {
    static mut STATE: u64 = 0;

    #[inline(never)]
    pub fn tick() -> u64 {
        unsafe {
            STATE = STATE.wrapping_add(1);
            STATE
        }
    }

    pub fn demonstrate() {
        println!("## 陷阱 3：无 warmup");
        let first = std::time::Instant::now();
        tick();
        let cold = first.elapsed().as_nanos();
        let (warm_min, _) = crate::util::bench_ns(200, 50, || {
            tick();
        });
        println!("首次调用 ≈ {cold}ns，warmup 后 min ≈ {warm_min}ns");
        println!("规则：手写 bench 必须 warmup\n");
    }
}

// ============================================================================
// 陷阱 4：只看 mean 忽略 P99
// ============================================================================
/// **现象**：SLA 报警 P99 超标，mean 指标绿灯。
/// **根因**：长尾被平均稀释。
/// **修法**：histogram + P99/P999 recording rule。
pub mod mean_over_p99 {
    pub fn demonstrate() {
        println!("## 陷阱 4：只看 mean");
        let mut samples = vec![1000u64; 9990];
        samples.extend([200_000u64; 10]);
        let mean = crate::util::mean_from_samples(&samples);
        let p99 = crate::util::p99_from_samples(&samples);
        println!("mean={mean}ns 正常，P99={p99}ns 超标");
        println!("规则：HFT SLA 写 P99；mean 仅作粗筛\n");
    }
}

// ============================================================================
// 陷阱 5：微基准不代表生产负载
// ============================================================================
/// **现象**：bench 快 10x，上线无提升。
/// **根因**：数据规模、并发、cache 状态与生产不同。
/// **修法**：用 production trace replay + 同硬件 profile。
pub mod unrepresentative_microbench {
    pub fn demonstrate() {
        println!("## 陷阱 5：微基准 ≠ 生产负载");
        println!("反例：bench 1k 元素 vs 生产 256 档 + 16 venue 并发");
        println!("规则：replay 真实 tick 文件；bench 标注数据规模\n");
    }
}

// ============================================================================
// 陷阱 6：样本太少 / 噪声大
// ============================================================================
/// **现象**：两次 bench 差 15%，随机选快的。
/// **根因**：OS jitter、频率缩放、邻居进程。
/// **修法**：criterion 多样本 + 剔除异常；pin CPU、isolcpus。
pub mod insufficient_samples {
    pub fn demonstrate() {
        println!("## 陷阱 6：样本不足");
        let mut mins: Vec<u64> = Vec::new();
        for _ in 0..5 {
            let (min, _) = crate::util::bench_ns(10, 20, || {
                (0..5000).sum::<u64>();
            });
            mins.push(min);
        }
        let spread = *mins.iter().max().unwrap() - *mins.iter().min().unwrap();
        println!("5 轮 bench min 波动 spread={spread}ns");
        println!("规则：≥200 samples；生产用 pinned core + isolcpus\n");
    }
}

// ============================================================================
// 陷阱 7：优化冷路径拖慢热路径
// ============================================================================
/// **现象**：加日志/检查提升边缘 case，热路径多 2 条分支。
/// **根因**：代码膨胀 + 分支增加影响预测。
/// **修法**：热冷分离；`#[cold]` / 单独模块。
pub mod slow_hot_path {
    #[inline(never)]
    pub fn hot_with_cold_check(x: u64, rare: bool) -> u64 {
        if rare {
            return cold_path(x);
        }
        x * 2
    }

    #[inline(never)]
    pub fn hot_only(x: u64) -> u64 {
        x * 2
    }

    #[cold]
    #[inline(never)]
    fn cold_path(x: u64) -> u64 {
        (0..100).map(|i| i as u64 + x).sum()
    }

    pub fn demonstrate() {
        println!("## 陷阱 7：冷路径污染热路径");
        let (mixed_ns, _) = crate::util::bench_ns(20, 200, || {
            hot_with_cold_check(42, false);
        });
        let (pure_ns, _) = crate::util::bench_ns(20, 200, || {
            hot_only(42);
        });
        println!("热路径+冷分支检查 ≈ {mixed_ns}ns，纯热路径 ≈ {pure_ns}ns");
        println!("规则：冷路径 `#[cold]` 或拆文件；profile 验证热路径未膨胀\n");
    }
}

// ============================================================================
// 陷阱 8：测试环境与生产硬件不一致
// ============================================================================
/// **现象**：笔记本 bench 优秀，机房 AVX-512 节点行为不同。
/// **根因**：CPU 代际、cache 大小、NUMA、网卡 offload 差异。
/// **修法**：staging 同构硬件 profile；feature detect + 标量回退。
pub mod env_mismatch {
    pub fn demonstrate() {
        println!("## 陷阱 8：环境不一致");
        #[cfg(target_arch = "x86_64")]
        {
            let avx2 = std::arch::is_x86_feature_detected!("avx2");
            println!("本机 AVX2 = {avx2}");
        }
        println!("规则：staging 与 prod 同 CPU 代；cloud 小实例 profile 无参考价值\n");
    }
}

pub fn demonstrate() {
    optimize_without_profile::demonstrate();
    debug_build_bench::demonstrate();
    no_warmup::demonstrate();
    mean_over_p99::demonstrate();
    unrepresentative_microbench::demonstrate();
    insufficient_samples::demonstrate();
    slow_hot_path::demonstrate();
    env_mismatch::demonstrate();
}
