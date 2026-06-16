//! # SIMD 常见陷阱与诊断
//!
//! 生产事故里反复出现的 8 个 SIMD 陷阱：
//! - 现象（监控/测试看到什么）
//! - 根因（CPU/编译器/内存层面发生了什么）
//! - 解决方案（修法 + 预防）

#![allow(dead_code)]

// ============================================================================
// 陷阱 1：未做特征检测 → SIGILL 崩溃
// ============================================================================
/// **现象**：本地 dev 机器正常，生产部分节点 crash（illegal instruction）。
/// **根因**：`#[target_feature]` 函数在不含 AVX2 的 CPU 上被直接调用。
/// **修法**：外层 `is_x86_feature_detected!` + 标量回退。
pub mod missing_feature_detect {
    pub fn safe_sum(data: &[i64]) -> i64 {
        crate::util::sum_i64(data)
    }

    pub fn demonstrate() {
        println!("## 陷阱 1：缺少 CPU 特征检测");
        let v = vec![1i64, 2, 3, 4];
        println!("safe_sum = {}（含特征检测）", safe_sum(&v));
        println!("规则：intrinsics 永远包在 `if is_x86_feature_detected!(...)` 里\n");
    }
}

// ============================================================================
// 陷阱 2：对齐 load 踩未定义行为
// ============================================================================
/// **现象**：偶发 segfault 或 silent data corruption。
/// **根因**：对非 32B 对齐地址用 `_mm256_load_si256`（aligned load）。
/// **修法**：不确定对齐时用 `_mm256_loadu_si256`，或 buffer `#[repr(align(32))]`。
pub mod misaligned_load {
    pub fn demonstrate() {
        println!("## 陷阱 2：对齐 load 未定义行为");
        let unaligned = [1i64, 2, 3, 4];
        let ptr = unaligned.as_ptr() as usize;
        println!("栈数组对齐 mod 32 = {}（可能 ≠ 0）", ptr % 32);
        println!("规则：默认 `loadu`；只有 arena bump 保证对齐才 `load`\n");
    }
}

// ============================================================================
// 陷阱 3：尾部 remainder 忘记处理
// ============================================================================
/// **现象**：SIMD 结果与标量差几个元素；边界 case 单测失败。
/// **根因**：`len % lane_width != 0` 时最后几个元素未参与计算。
/// **修法**：`chunks_exact(N)` + `remainder()` 标量收尾。
pub mod forgotten_tail {
    pub fn sum_wrong(data: &[i64]) -> i64 {
        let n = data.len() / 4 * 4;
        data[..n].iter().sum() // ❌ 丢掉 tail
    }

    pub fn sum_correct(data: &[i64]) -> i64 {
        data.iter().sum()
    }

    pub fn demonstrate() {
        println!("## 陷阱 3：尾部 remainder 遗漏");
        let v: Vec<i64> = (0..13).collect();
        println!("错误 sum = {}，正确 = {}", sum_wrong(&v), sum_correct(&v));
        println!("规则：`chunks_exact` 必配 `remainder()`\n");
    }
}

// ============================================================================
// 陷阱 4：AoS 布局阻碍向量化
// ============================================================================
/// **现象**：写了 SIMD 但 profiling 无加速。
/// **根因**：`struct { px, qty }` 交错存储，load 需要 shuffle/gather。
/// **修法**：热路径改 SoA（`Vec<Px>` + `Vec<Qty>`）或 struct-of-array crate。
pub mod aos_layout {
    #[derive(Clone, Copy)]
    pub struct Trade {
        pub px: i64,
        pub qty: i64,
    }

    pub fn notional_aos(trades: &[Trade]) -> i128 {
        trades
            .iter()
            .map(|t| (t.px as i128) * (t.qty as i128))
            .sum()
    }

    pub fn notional_soa(px: &[i64], qty: &[i64]) -> i128 {
        px.iter()
            .zip(qty.iter())
            .map(|(&p, &q)| (p as i128) * (q as i128))
            .sum()
    }

    pub fn demonstrate() {
        println!("## 陷阱 4：AoS vs SoA 布局");
        let trades = vec![Trade { px: 100, qty: 10 }, Trade { px: 101, qty: 5 }];
        let px: Vec<i64> = trades.iter().map(|t| t.px).collect();
        let qty: Vec<i64> = trades.iter().map(|t| t.qty).collect();
        assert_eq!(notional_aos(&trades), notional_soa(&px, &qty));
        println!("SoA 让 px/qty 各自连续 → LLVM/SIMD 可直接 vectorize\n");
    }
}

// ============================================================================
// 陷阱 5：手写 SIMD 比编译器慢
// ============================================================================
/// **现象**：intrinsics 版本比 `-O3` 标量还慢。
/// **根因**：循环太短、分支多、或 LLVM 已自动向量化且做了更优调度。
/// **修法**：criterion benchmark；先 `-C target-cpu=native`，再决定是否手写。
pub mod premature_simd {
    pub fn demonstrate() {
        println!("## 陷阱 5：过早手写 SIMD");
        println!("流程：标量 → perf/flamegraph → 确认热点 → benchmark intrinsics");
        println!("很多 sum/map 循环 LLVM 已自动向量化，手写反而更慢\n");
    }
}

// ============================================================================
// 陷阱 6：跨平台无回退
// ============================================================================
/// **现象**：ARM CI 编译失败或运行时 panic。
/// **根因**：硬编码 x86_64 intrinsics 无 `#[cfg(target_arch)]`。
/// **修法**：`cfg` 分架构 + 标量 fallback；或用 `wide`/`simdeez` 抽象层。
pub mod no_portable_fallback {
    pub fn sum_portable(data: &[i64]) -> i64 {
        crate::util::sum_i64(data)
    }

    pub fn demonstrate() {
        println!("## 陷阱 6：跨平台无标量回退");
        let v = vec![1, 2, 3, 4];
        println!("portable sum = {}", sum_portable(&v));
        println!("规则：`#[cfg(target_arch = \"x86_64\")]` + else 标量\n");
    }
}

// ============================================================================
// 陷阱 7：SIMD scatter 写冲突
// ============================================================================
/// **现象**：histogram 计数不准，多线程下更严重。
/// **根因**：多 lane 同时写同一 bucket（gather/scatter 无 atomic）。
/// **修法**：per-thread histogram → 合并；或标量 bucket 累加。
pub mod scatter_conflict {
    pub fn demonstrate() {
        println!("## 陷阱 7：SIMD scatter 写冲突");
        println!("latency histogram 不能 naive `_mm256_i64scatter` 到共享 bucket");
        println!("修法：thread-local buckets + reduce，或 AVX512 conflict detection\n");
    }
}

// ============================================================================
// 陷阱 8：整数溢出与 SIMD 语义不一致
// ============================================================================
/// **现象**：SIMD 版 sum 与标量 sum 在溢出时结果不同。
/// **根因**：向量化改变累加顺序；i64 溢出 UB 或 wrapping 语义不同。
/// **修法**：risk/finance 用 i128 累加器；或显式 `wrapping_add` + 相同顺序。
pub mod overflow_semantics {
    pub fn demonstrate() {
        println!("## 陷阱 8：累加顺序与溢出");
        println!("HFT 定点：中间结果用 i128；比较 SIMD/标量必须同一语义");
        println!("规则：overflow test + property-based test 覆盖边界\n");
    }
}

pub fn demonstrate() {
    missing_feature_detect::demonstrate();
    misaligned_load::demonstrate();
    forgotten_tail::demonstrate();
    aos_layout::demonstrate();
    premature_simd::demonstrate();
    no_portable_fallback::demonstrate();
    scatter_conflict::demonstrate();
    overflow_semantics::demonstrate();
}
