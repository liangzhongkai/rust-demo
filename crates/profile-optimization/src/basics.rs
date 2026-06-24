//! # Profiling 底层机制
//!
//! 性能优化不是「猜」出来的，而是测量驱动的迭代：
//!
//! 1. **采样 profiler**（perf）：低开销，看 CPU 热点栈
//! 2. **插桩 profiler**（dhat）：精确分配，但扰动大
//! 3. **微基准**（criterion）：隔离函数级回归
//! 4. **生产指标**（P99 histogram）：真实流量下的尾延迟
//! 5. **硬件计数器**（perf stat）：cache miss、branch miss
//! 6. **火焰图**（flamegraph）：把 perf 输出可视化

#![allow(dead_code)]

// ============================================================================
// 1. perf 采样 profiler 工作流
// ============================================================================
pub mod perf_workflow {
    pub fn demonstrate() {
        println!("## 1. perf 采样 profiler 工作流");
        println!("  cargo build --release -p profile-optimization");
        println!("  perf record -g --call-graph dwarf -F 997 ./target/release/profile-optimization");
        println!("  perf script | inferno-collapse-perf | inferno-flamegraph > flame.svg");
        println!("要点：release + DWARF 栈 + 997Hz 避免与 timer 共振");
        println!("HFT：在真实 feed 回放时 profile，不要用 synthetic 1M noop\n");
    }
}

// ============================================================================
// 2. criterion 微基准
// ============================================================================
pub mod criterion_bench {
  #[inline(never)]
    pub fn work(n: usize) -> u64 {
        (0..n).map(|i| i as u64).sum()
    }

    pub fn demonstrate() {
        println!("## 2. criterion 微基准（本 crate 用 util::bench_ns 替身）");
        let (min, mean) = crate::util::bench_ns(50, 200, || {
            work(10_000);
        });
        println!("work(10k) min={min}ns mean={mean}ns");
        println!("criterion 额外提供：warmup、样本剔除、HTML 报告、回归检测");
        println!("规则：bench 函数与生产路径同编译选项（--release）\n");
    }
}

// ============================================================================
// 3. P99 vs mean —— 尾延迟才是 SLA
// ============================================================================
pub mod p99_vs_mean {
    pub fn demonstrate() {
        println!("## 3. P99 vs mean");
        let mut samples: Vec<u64> = Vec::new();
        for _ in 0..9_800 {
            samples.push(800);
        }
        for _ in 0..200 {
            samples.push(120_000);
        }
        let mean = samples.iter().sum::<u64>() / samples.len() as u64;
        let p99 = crate::util::p99_from_samples(&samples);
        println!("9.8k×800ns + 200×120μs → mean={mean}ns, P99={p99}ns");
        println!("mean 尚可，P99 已超 HFT 10μs SLA");
        println!("生产：Prometheus histogram + recording rule 算 P99\n");
    }
}

// ============================================================================
// 4. 直方图分桶策略
// ============================================================================
pub mod histogram_buckets {
    pub fn demonstrate() {
        println!("## 4. 直方图分桶");
        let mut h = crate::util::LatencyHistogram::new(64, 1_000);
        for lat in [500, 1_200, 800, 2_000, 50_000] {
            h.record(lat);
        }
        println!(
            "width=1μs buckets=64 → mean={}ns p99={}ns total={}",
            h.mean_ns(),
            h.p99_ns(),
            h.total()
        );
        println!("HFT：1μs 桶覆盖 0~64μs；Web3 gas：按 gas unit 分桶\n");
    }
}

// ============================================================================
// 5. release vs debug 测量失真
// ============================================================================
pub mod release_vs_debug {
    #[inline(never)]
    pub fn hot_loop(n: usize) -> u64 {
        let mut acc = 0u64;
        for i in 0..n {
            acc += (i as u64 * 7 + 3) % 97;
        }
        acc
    }

    pub fn demonstrate() {
        println!("## 5. release vs debug");
        let (min, _) = crate::util::bench_ns(20, 100, || {
            hot_loop(50_000);
        });
        println!("debug build hot_loop(50k) ≈ {min}ns（含未优化开销）");
        println!("务必 `cargo build --release` 后再 perf / criterion");
        println!("debug 适合功能正确性，不适合性能结论\n");
    }
}

// ============================================================================
// 6. warmup 与稳态
// ============================================================================
pub mod warmup_steady_state {
    static mut COUNTER: u64 = 0;

    #[inline(never)]
    pub fn touch_cache() -> u64 {
        unsafe {
            COUNTER += 1;
            COUNTER
        }
    }

    pub fn demonstrate() {
        println!("## 6. warmup 与 JIT/缓存稳态");
        let (no_warm_min, _) = crate::util::bench_ns(0, 50, || {
            touch_cache();
        });
        let (warm_min, _) = crate::util::bench_ns(100, 50, || {
            touch_cache();
        });
        println!("无 warmup min={no_warm_min}ns，100 warmup min={warm_min}ns");
        println!("criterion 默认 warmup；手写 bench 别忘预热 icache + branch predictor\n");
    }
}

pub fn demonstrate() {
    perf_workflow::demonstrate();
    criterion_bench::demonstrate();
    p99_vs_mean::demonstrate();
    histogram_buckets::demonstrate();
    release_vs_debug::demonstrate();
    warmup_steady_state::demonstrate();
}
