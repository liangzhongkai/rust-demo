//! # HFT 生产场景下的性能分析与优化
//!
//! 高频交易的 profiling 硬约束：
//! - **尾延迟**：P99/P999 比 mean 重要，直方图分桶要细
//! - **热路径隔离**：perf 在 feed 回放时采样，不是 synthetic noop
//! - **回归门禁**：criterion CI 防止「优化冷路径、拖慢热路径」
//!
//! 下面 7 个场景对应真实系统里「先 profile、再改、再验证」的闭环。

#![allow(dead_code)]

use crate::util::{bench_per_op_ns, AllocCounter, LatencyHistogram};

pub type Px = i64;
pub type Qty = i64;

#[derive(Debug, Clone, Copy)]
pub struct Level {
    pub px: Px,
    pub qty: Qty,
}

#[derive(Debug, Clone, Copy)]
pub struct Tick {
    pub bid: Px,
    pub ask: Px,
}

// ============================================================================
// 场景 1：Tick handler P99 回归 —— 直方图发现尾延迟尖刺
// ============================================================================
/// **生产问题**：部署后 P99 从 2μs 飙到 45μs，mean 几乎不变；
/// on-call 靠 Prometheus histogram 发现，但不知道哪段代码。
///
/// **Profiling 套路**：对比部署前后 latency trace + perf flamegraph；
/// 常见根因：热路径新增 `clone()` / `format!` / 隐式分配。
pub mod tick_handler_p99 {
    use super::*;

    #[inline(never)]
    pub fn on_tick_slow(t: Tick) -> Px {
        let s = format!("{}-{}", t.bid, t.ask);
        let _ = s.len();
        (t.bid + t.ask) / 2
    }

    #[inline(never)]
    pub fn on_tick_fast(t: Tick) -> Px {
        (t.bid + t.ask) / 2
    }

    pub fn profile_handler<F: Fn(Tick) -> Px>(label: &str, f: F, tick: Tick) -> LatencyHistogram {
        let mut hist = LatencyHistogram::new(128, 500);
        for _ in 0..2_000 {
            let start = std::time::Instant::now();
            f(tick);
            hist.record(start.elapsed().as_nanos() as u64);
        }
        println!(
            "## 场景 1：Tick handler P99 回归 —— {label} mean={}ns p99={}ns",
            hist.mean_ns(),
            hist.p99_ns()
        );
        hist
    }

    pub fn demonstrate() {
        let tick = Tick { bid: 100_00, ask: 100_05 };
        let slow = profile_handler("slow(format!)", on_tick_slow, tick);
        let fast = profile_handler("fast(整数)", on_tick_fast, tick);
        println!(
            "P99 改善 ≈ {}x；关键：尾延迟用 histogram，别只看 mean",
            slow.p99_ns() / fast.p99_ns().max(1)
        );
        println!("perf：`perf record -g` 常见热点 = format/alloc\n");
    }
}

// ============================================================================
// 场景 2：Order book 扫描 —— perf 定位 O(n) 热点后优化
// ============================================================================
/// **生产问题**：撮合前 risk 扫描 256 档，CPU 8%，perf 显示 `best_bid` 占 60%。
///
/// **Profiling 套路**：确认数据已按价排序 → 从尾扫描改 `last()` / 维护 cached best。
pub mod orderbook_scan {
    use super::*;

    pub fn best_bid_scan(levels: &[Level]) -> Option<Px> {
        levels.iter().map(|l| l.px).max()
    }

    pub fn best_bid_cached(levels: &[Level], cached: Option<Px>) -> Option<Px> {
        if let Some(c) = cached {
            if levels.last().map(|l| l.px) == Some(c) {
                return cached;
            }
        }
        levels.last().map(|l| l.px)
    }

    pub fn demonstrate() {
        println!("## 场景 2：Order book 扫描热点");
        let levels: Vec<Level> = (0..256)
            .map(|i| Level {
                px: 100_00 + i,
                qty: 10,
            })
            .collect();
        let cached = levels.last().map(|l| l.px);

        let (scan_ns, _) = bench_per_op_ns(30, 500, 1, || {
            best_bid_scan(&levels);
        });
        let (cache_ns, _) = bench_per_op_ns(30, 500, 1, || {
            best_bid_cached(&levels, cached);
        });

        println!("全表 max scan ≈ {scan_ns}ns/op，cached last ≈ {cache_ns}ns/op");
        println!(
            "加速比 ≈ {}x；profile 确认 O(n) 后再改，别假设「已经够快」",
            scan_ns / cache_ns.max(1)
        );
        println!("关键：sorted book + cached best 是 HFT 标配\n");
    }
}

// ============================================================================
// 场景 3：FIX 解析 CPU 尖峰 —— 热点计数定位 parse 瓶颈
// ============================================================================
/// **生产问题**：开盘 10 万 msg/s，某字段解析占 40% CPU。
///
/// **Profiling 套路**：插桩计数 / perf annotate → 标量字节扫描替代 split/parse。
pub mod fix_parse_cpu {
    use crate::util::HotspotCounter;

    #[inline(never)]
    pub fn parse_price_slow(msg: &[u8]) -> Option<i64> {
        let s = String::from_utf8_lossy(msg);
        for part in s.split('\x01') {
            if part.starts_with("44=") {
                return part[3..].parse().ok();
            }
        }
        None
    }

    #[inline(never)]
    pub fn parse_price_fast(msg: &[u8]) -> Option<i64> {
        let mut i = 0;
        while i + 3 < msg.len() {
            if msg[i] == b'4' && msg[i + 1] == b'4' && msg[i + 2] == b'=' {
                let start = i + 3;
                let mut end = start;
                while end < msg.len() && msg[end] != 0x01 {
                    end += 1;
                }
                return parse_i64_bytes(&msg[start..end]);
            }
            i += 1;
        }
        None
    }

    fn parse_i64_bytes(s: &[u8]) -> Option<i64> {
        let mut acc = 0i64;
        let mut neg = false;
        let mut i = 0;
        if s.first() == Some(&b'-') {
            neg = true;
            i = 1;
        }
        for &b in &s[i..] {
            if b == b'.' {
                break;
            }
            if b < b'0' || b > b'9' {
                return None;
            }
            acc = acc * 10 + (b - b'0') as i64;
        }
        Some(if neg { -acc } else { acc })
    }

    pub fn demonstrate() {
        println!("## 场景 3：FIX `44=` 解析 CPU 热点");
        let msg = b"8=FIX.4.2\x0135=D\x0111=abc\x0144=10025\x0138=500\x0110=128\x01";
        let mut counter = HotspotCounter::default();

        let (slow_ns, _) = crate::util::bench_per_op_ns(20, 300, 1, || {
            counter.hit("parse_slow");
            parse_price_slow(msg);
        });
        let (fast_ns, _) = crate::util::bench_per_op_ns(20, 300, 1, || {
            counter.hit("parse_fast");
            parse_price_fast(msg);
        });

        let tops = counter.top(2);
        println!("slow={slow_ns}ns fast={fast_ns}ns，热点计数 {:?}", tops);
        println!("关键：String/split 在 perf 里很显眼；零拷贝字节扫描是修法\n");
    }
}

// ============================================================================
// 场景 4：全局锁竞争 —— 锁等待在 profile 里表现为 syscall 栈
// ============================================================================
/// **生产问题**：多线程 quote 更新，P99 周期性尖刺；perf 显示 futex。
///
/// **Profiling 套路**：`perf lock` / 对比 per-symbol shard 后的 P99。
pub mod lock_contention {
    use super::Px;
    use std::sync::Mutex;

    #[inline(never)]
    pub fn update_global(book: &Mutex<Vec<Px>>, px: Px) {
        let mut g = book.lock().unwrap();
        g.push(px);
        if g.len() > 1024 {
            g.drain(0..512);
        }
    }

    pub fn update_sharded(shards: &[Mutex<Vec<Px>>], symbol_id: usize, px: Px) {
        let shard = &shards[symbol_id % shards.len()];
        let mut g = shard.lock().unwrap();
        g.push(px);
        if g.len() > 256 {
            g.drain(0..128);
        }
    }

    pub fn demonstrate() {
        println!("## 场景 4：全局锁 vs 分片锁");
        let global = Mutex::new(Vec::new());
        let shards: Vec<Mutex<Vec<Px>>> = (0..16).map(|_| Mutex::new(Vec::new())).collect();

        let (global_ns, _) = crate::util::bench_per_op_ns(10, 200, 1, || {
            update_global(&global, 100_00);
        });
        let (shard_ns, _) = crate::util::bench_per_op_ns(10, 200, 1, || {
            update_sharded(&shards, 7, 100_00);
        });

        println!("全局锁 ≈ {global_ns}ns/op，16 分片 ≈ {shard_ns}ns/op");
        println!("关键：perf 见 `futex` → 查锁粒度；单线程 bench 看不出竞争\n");
    }
}

// ============================================================================
// 场景 5：Quote 更新分配抖动 —— dhat/AllocCounter 抓 heap churn
// ============================================================================
/// **生产问题**：每次 tick `Vec::new` + push，分配器锁 + cache 污染导致尾延迟。
///
/// **Profiling 套路**：dhat-rs / heaptrack → 复用 `Vec` capacity / arena。
pub mod allocator_churn {
    use super::*;

    #[inline(never)]
    pub fn quote_updates_alloc(qty: Qty) -> Vec<Qty> {
        let mut buf = Vec::new();
        for i in 0..qty {
            buf.push(i);
        }
        buf
    }

    #[inline(never)]
    pub fn quote_updates_reuse(buf: &mut Vec<Qty>, qty: Qty) {
        buf.clear();
        for i in 0..qty {
            buf.push(i);
        }
    }

    pub fn demonstrate() {
        println!("## 场景 5：Quote 更新 heap churn");
        let qty = 64;
        let mut counter = AllocCounter::default();
        let mut reuse: Vec<Qty> = Vec::with_capacity(128);

        for _ in 0..500 {
            let v = quote_updates_alloc(qty);
            counter.track_vec_push(&mut Vec::new(), v);
        }

        let alloc_count = counter.allocs;
        let (alloc_ns, _) = crate::util::bench_per_op_ns(20, 200, 1, || {
            quote_updates_alloc(qty);
        });
        let (reuse_ns, _) = crate::util::bench_per_op_ns(20, 200, 1, || {
            quote_updates_reuse(&mut reuse, qty);
        });

        println!(
            "每次 new Vec 追踪到扩容 ≈ {alloc_count} 次，alloc={alloc_ns}ns reuse={reuse_ns}ns"
        );
        println!("关键：热路径 `with_capacity` + clear 复用；生产用 dhat 看 bytes allocated\n");
    }
}

// ============================================================================
// 场景 6：Risk 分支预测 —— perf stat 看 branch-misses
// ============================================================================
/// **生产问题**：risk 检查随机 pass/fail，branch miss 高，吞吐掉 30%。
///
/// **Profiling 套路**：`perf stat -e branches,branch-misses` → 排序/批处理提高可预测性。
pub mod branch_mispredict {
    #[inline(never)]
    pub fn risk_check_random(flags: &[bool]) -> u64 {
        let mut reject = 0u64;
        for &f in flags {
            if f {
                reject += 1;
            }
        }
        reject
    }

    #[inline(never)]
    pub fn risk_check_sorted(flags: &[bool]) -> u64 {
        risk_check_random(flags)
    }

    pub fn demonstrate() {
        println!("## 场景 6：Risk 分支预测");
        let random: Vec<bool> = (0..10_000).map(|i| i % 3 == 0).collect();
        let mut sorted = random.clone();
        sorted.sort_by_key(|&b| !b);

        let (rand_ns, _) = crate::util::bench_per_op_ns(10, 100, random.len() as u64, || {
            risk_check_random(&random);
        });
        let (sort_ns, _) = crate::util::bench_per_op_ns(10, 100, sorted.len() as u64, || {
            risk_check_sorted(&sorted);
        });

        println!("随机模式 ≈ {rand_ns}ns/flag，排序后 ≈ {sort_ns}ns/flag");
        println!("关键：`perf stat -e branch-misses`；批处理同源订单提高预测率\n");
    }
}

// ============================================================================
// 场景 7：Feed 回放稳态基准 —— warmup 前后的测量偏差
// ============================================================================
/// **生产问题**：冷启动 benchmark 显示 50μs/tick，实盘稳态 3μs；
/// 错误基准导致错误的优化优先级。
///
/// **Profiling 套路**：criterion warmup + 真实 replay trace 长度对齐生产。
pub mod steady_state_bench {
    static mut WARM_STATE: u64 = 0;

    #[inline(never)]
    pub fn process_tick(id: u64) -> u64 {
        unsafe {
            WARM_STATE = WARM_STATE.wrapping_add(id * 13 + 7);
            WARM_STATE % 1_000_003
        }
    }

    pub fn demonstrate() {
        println!("## 场景 7：Feed 回放稳态 vs 冷启动");
        let (cold_min, cold_mean) = crate::util::bench_ns(0, 100, || {
            process_tick(42);
        });
        let (warm_min, warm_mean) = crate::util::bench_ns(500, 100, || {
            process_tick(42);
        });

        println!("无 warmup: min={cold_min}ns mean={cold_mean}ns");
        println!("500 warmup: min={warm_min}ns mean={warm_mean}ns");
        println!("关键：HFT bench 用 production tick 文件 replay；对齐 P99 窗口长度\n");
    }
}

pub fn demonstrate() {
    tick_handler_p99::demonstrate();
    orderbook_scan::demonstrate();
    fix_parse_cpu::demonstrate();
    lock_contention::demonstrate();
    allocator_churn::demonstrate();
    branch_mispredict::demonstrate();
    steady_state_bench::demonstrate();
}
