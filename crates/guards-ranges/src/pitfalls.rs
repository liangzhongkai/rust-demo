//! # 守卫与范围：常见陷阱
//!
//! 生产事故里 guard/range 相关的 8 个高频坑：
//! - 边界 off-by-one
//! - arm 顺序颠倒导致优先级错误
//! - guard 重复求值 / 副作用
//! - range 留缝（gap）
//! - 有符号 / 无符号 range 混用
//! - guard 不能替代穷尽性检查
//! - 浮点 range（禁止）
//! - 重叠 guard 导致死代码

#![allow(dead_code)]

// ============================================================================
// 陷阱 1：range 边界 off-by-one
// ============================================================================
pub mod range_off_by_one {
    /// ❌ `1..5` 不含 5；tick 恰好在边界时归错桶。
    fn bucket_bad(px: i64) -> &'static str {
        match px {
            0..=99 => "a",
            100..199 => "b", // 199 是 b 的上界；200 才进下一档
            _ => "c",
        }
    }

    fn bucket_good(px: i64) -> &'static str {
        match px {
            ..=-1 => "invalid",
            0..=99 => "a",
            100..=199 => "b",
            200.. => "c",
        }
    }

    pub fn demonstrate() {
        println!("## 陷阱 1：range 边界 off-by-one");
        for px in [99, 100, 199, 200] {
            println!("  px={} bad={} good={}", px, bucket_bad(px), bucket_good(px));
        }
        println!("规则：边界值写进测试；闭区间用 `..=`\n");
    }
}

// ============================================================================
// 陷阱 2：arm 顺序颠倒 —— 宽松 guard 吞掉严格条件
// ============================================================================
pub mod arm_order {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Level {
        Ok,
        Warn,
        Kill,
    }

    fn classify_bad(pnl: i64) -> Level {
        match pnl {
            p if p < 0 => Level::Warn, // ❌ 先匹配所有负数
            p if p < -100_000 => Level::Kill, // 死代码！永远到不了
            _ => Level::Ok,
        }
    }

    fn classify_good(pnl: i64) -> Level {
        match pnl {
            p if p < -100_000 => Level::Kill,
            p if p < 0 => Level::Warn,
            _ => Level::Ok,
        }
    }

    pub fn demonstrate() {
        println!("## 陷阱 2：guard arm 顺序颠倒");
        let pnl = -200_000i64;
        println!("  pnl={} bad={:?} good={:?}", pnl, classify_bad(pnl), classify_good(pnl));
        println!("规则：最严格 guard 放最前\n");
    }
}

// ============================================================================
// 陷阱 3：guard 副作用 / 重复求值
// ============================================================================
pub mod guard_side_effects {
    fn expensive(id: u64) -> bool {
        id % 2 == 0
    }

    fn route_bad(id: u64) -> &'static str {
        match id {
            x if expensive(x) && expensive(x) => "fast", // 可能算两次
            _ => "slow",
        }
    }

    fn route_good(id: u64) -> &'static str {
        let ok = expensive(id);
        match id {
            _ if ok => "fast",
            _ => "slow",
        }
    }

    pub fn demonstrate() {
        println!("## 陷阱 3：guard 重复求值");
        println!("  route_good(100) → {}", route_good(100));
        println!("规则：昂贵检查提到 match 外；guard 保持纯函数\n");
    }
}

// ============================================================================
// 陷阱 4：range 留缝 —— 整数域未覆盖
// ============================================================================
pub mod range_gaps {
    fn fee_tier_bad(gwei: u64) -> &'static str {
        match gwei {
            0..=10 => "low",
            20..=50 => "mid", // ❌ 11..=19 落洞
            _ => "high",
        }
    }

    fn fee_tier_good(gwei: u64) -> &'static str {
        match gwei {
            0..=10 => "low",
            11..=50 => "mid",
            _ => "high",
        }
    }

    pub fn demonstrate() {
        println!("## 陷阱 4：range 留缝");
        println!("  15 gwei bad={} good={}", fee_tier_bad(15), fee_tier_good(15));
        println!("规则：画数轴检查相邻 range 是否无缝衔接\n");
    }
}

// ============================================================================
// 陷阱 5：有符号 range 与负数
// ============================================================================
pub mod signed_range {
    fn pnl_band_bad(pnl: i64) -> &'static str {
        match pnl {
            0..=100 => "profit", // ❌ 负数全落 _
            _ => "other",
        }
    }

    fn pnl_band_good(pnl: i64) -> &'static str {
        match pnl {
            ..=-1 => "loss",
            0..=100 => "profit",
            _ => "big_profit",
        }
    }

    pub fn demonstrate() {
        println!("## 陷阱 5：有符号 range 漏负数");
        println!("  pnl=-50 bad={} good={}", pnl_band_bad(-50), pnl_band_good(-50));
        println!("规则：i64 先处理 `..=-1` 或 `..0`\n");
    }
}

// ============================================================================
// 陷阱 6：guard 不能替代穷尽性 —— 新增 variant 静默漏处理
// ============================================================================
pub mod guard_not_exhaustive {
    #[derive(Debug, Clone, Copy)]
    enum Status {
        Open,
        Filled,
        // 将来加 Cancelled
    }

    fn label_with_guard(s: Status) -> &'static str {
        match s {
            Status::Open => "open",
            Status::Filled => "filled",
            // 新增 Cancelled 时此处编译失败 —— guard 救不了漏 arm
        }
    }

    pub fn demonstrate() {
        println!("## 陷阱 6：guard 不替代穷尽 match");
        println!("  Open → {}", label_with_guard(Status::Open));
        println!("规则：enum 不用 `_` 兜底；guard 只过滤，不掩盖漏 arm\n");
    }
}

// ============================================================================
// 陷阱 7：浮点 range —— 禁止
// ============================================================================
pub mod float_range {
    /// 浮点区间只能用 if，不能用 range pattern：
    /// ```compile_fail
    /// match x { 0.0..=1.0 => "low", _ => "high" }
    /// ```
    fn classify_good(x: f64) -> &'static str {
        if (0.0..=1.0).contains(&x) {
            "low"
        } else {
            "high"
        }
    }

    pub fn demonstrate() {
        println!("## 陷阱 7：浮点不能用 range pattern");
        println!("  0.5 → {}", classify_good(0.5));
        println!("  生产做法：价格用 i64 定点；或 `if (lo..=hi).contains(&x)`\n");
    }
}

// ============================================================================
// 陷阱 8：@ 绑定后 guard 误用 moved 字段
// ============================================================================
pub mod at_binding_move {
    #[derive(Debug)]
    struct Fill {
        id: u64,
        qty: i64,
    }

    fn log_bad(f @ Fill { qty, .. }: Fill) -> i64 {
        let _ = (f, qty); // @ 绑定后若再 match 解构会 partial move
        qty
    }

    fn log_good(f: &Fill) -> i64 {
        match f {
            Fill { qty, .. } if *qty > 0 => *qty,
            _ => 0,
        }
    }

    pub fn demonstrate() {
        println!("## 陷阱 8：@ 绑定 + partial move");
        let fill = Fill { id: 1, qty: 10 };
        println!("  qty={}", log_good(&fill));
        println!("  id 仍可用 = {}", fill.id);
        println!("规则：需要保留整体时用 `match &x` 或 `ref` 模式\n");
    }
}

pub fn demonstrate() {
    range_off_by_one::demonstrate();
    arm_order::demonstrate();
    guard_side_effects::demonstrate();
    range_gaps::demonstrate();
    signed_range::demonstrate();
    guard_not_exhaustive::demonstrate();
    float_range::demonstrate();
    at_binding_move::demonstrate();
}
