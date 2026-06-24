//! 跨模块复用的 profiling / benchmarking 工具。
//!
//! 生产环境配合 perf、criterion、Prometheus histogram；
//! 本 crate 用纯 std 实现可运行的教学替身。

#![allow(dead_code)]

use std::time::{Duration, Instant};

/// 固定宽度分桶直方图（纳秒），HFT P99 / Web3 gas 分布通用。
#[derive(Debug, Clone)]
pub struct LatencyHistogram {
    pub buckets: Vec<u64>,
    pub width_ns: u64,
    pub overflow: u64,
}

impl LatencyHistogram {
    pub fn new(num_buckets: usize, width_ns: u64) -> Self {
        Self {
            buckets: vec![0; num_buckets],
            width_ns,
            overflow: 0,
        }
    }

    pub fn record(&mut self, latency_ns: u64) {
        let idx = latency_ns / self.width_ns;
        if idx < self.buckets.len() as u64 {
            self.buckets[idx as usize] += 1;
        } else {
            self.overflow += 1;
        }
    }

    pub fn total(&self) -> u64 {
        self.buckets.iter().sum::<u64>() + self.overflow
    }

    /// 从直方图近似 P99（分桶粒度 = width_ns）。
    pub fn p99_ns(&self) -> u64 {
        let target = (self.total() as f64 * 0.99).ceil() as u64;
        let mut acc = 0u64;
        for (i, &cnt) in self.buckets.iter().enumerate() {
            acc += cnt;
            if acc >= target {
                return (i as u64 + 1) * self.width_ns;
            }
        }
        if self.overflow > 0 {
            return self.buckets.len() as u64 * self.width_ns;
        }
        0
    }

    pub fn mean_ns(&self) -> u64 {
        let total = self.total();
        if total == 0 {
            return 0;
        }
        let sum: u64 = self
            .buckets
            .iter()
            .enumerate()
            .map(|(i, &c)| c * (i as u64 * self.width_ns + self.width_ns / 2))
            .sum();
        sum / total
    }
}

/// 从原始样本算精确 P99（比直方图更准，适合离线分析）。
pub fn p99_from_samples(samples: &[u64]) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let idx = ((sorted.len() as f64 * 0.99).ceil() as usize).saturating_sub(1);
    sorted[idx]
}

pub fn mean_from_samples(samples: &[u64]) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    samples.iter().sum::<u64>() / samples.len() as u64
}

/// 简易微基准：warmup + N 次迭代，返回 (min_ns, mean_ns)。
pub fn bench_ns<F: FnMut()>(warmup: usize, iters: usize, mut f: F) -> (u64, u64) {
    for _ in 0..warmup {
        f();
    }
    let mut samples: Vec<u64> = Vec::with_capacity(iters);
    for _ in 0..iters {
        let start = Instant::now();
        f();
        samples.push(start.elapsed().as_nanos() as u64);
    }
    let min = *samples.iter().min().unwrap_or(&0);
    let mean = mean_from_samples(&samples);
    (min, mean)
}

/// 按操作量归一化：返回每条操作的纳秒数。
pub fn bench_per_op_ns<F: FnMut()>(warmup: usize, iters: usize, ops_per_iter: u64, mut f: F) -> (u64, u64) {
    let (min, mean) = bench_ns(warmup, iters, || f());
    (min / ops_per_iter, mean / ops_per_iter)
}

/// 多阶段耗时分解（模拟 RPC / bundle 流水线 profiling）。
#[derive(Debug, Clone, Default)]
pub struct StageTimer {
    pub stages: Vec<(String, Duration)>,
}

impl StageTimer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn time<F: FnOnce() -> T, T>(&mut self, name: &str, f: F) -> T {
        let start = Instant::now();
        let out = f();
        self.stages.push((name.to_string(), start.elapsed()));
        out
    }

    pub fn total(&self) -> Duration {
        self.stages.iter().map(|(_, d)| *d).sum()
    }

    pub fn print_breakdown(&self) {
        let total = self.total();
        for (name, d) in &self.stages {
            let pct = if total.is_zero() {
                0.0
            } else {
                d.as_secs_f64() / total.as_secs_f64() * 100.0
            };
            println!("  {name}: {:.2?} ({pct:.1}%)", d);
        }
    }
}

/// 热点计数器：教学用采样 profiler 替身（生产用 perf DWARF stack）。
#[derive(Debug, Default, Clone)]
pub struct HotspotCounter {
    pub hits: std::collections::HashMap<String, u64>,
}

impl HotspotCounter {
    pub fn hit(&mut self, label: &str) {
        *self.hits.entry(label.to_string()).or_insert(0) += 1;
    }

    pub fn top(&self, n: usize) -> Vec<(String, u64)> {
        let mut v: Vec<_> = self.hits.iter().map(|(k, &c)| (k.clone(), c)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        v.truncate(n);
        v
    }
}

/// 分配计数：用 Vec 容量变化近似 heap churn（生产用 dhat / heaptrack）。
#[derive(Debug, Default, Clone, Copy)]
pub struct AllocCounter {
    pub allocs: u64,
    pub bytes: u64,
}

impl AllocCounter {
    pub fn track_vec_push<T>(&mut self, v: &mut Vec<T>, item: T) {
        let cap_before = v.capacity();
        v.push(item);
        if v.capacity() > cap_before {
            self.allocs += 1;
            self.bytes += (v.capacity() - cap_before) as u64 * std::mem::size_of::<T>() as u64;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn histogram_p99() {
        let mut h = LatencyHistogram::new(100, 1000);
        for _ in 0..980 {
            h.record(500);
        }
        for _ in 0..20 {
            h.record(50_000);
        }
        assert!(h.p99_ns() >= 50_000);
        assert!(h.mean_ns() < 5000);
    }

    #[test]
    fn p99_samples() {
        let samples: Vec<u64> = (1..=100).collect();
        assert_eq!(p99_from_samples(&samples), 99);
    }
}
