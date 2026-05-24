//! # 守卫与范围：底层机制
//!
//! Rust `match` 在「形状匹配」之上还有两层表达能力：
//!
//! 1. **Range 模式** —— 对整数 / char 做区间归类（`0..=9`、`100..`）
//! 2. **Match guard** —— `if condition` 在形状匹配后再过滤
//!
//! 这一节回答四个根本问题，后面 HFT/Web3 的生产案例都建立在这之上。

#![allow(dead_code)]

// ============================================================================
// 1. Range 模式：闭区间 vs 半开区间
// ============================================================================
/// `a..b` 含左不含右；`a..=b` 两端都含。
/// 生产代码里 **价格档位 / 延迟分桶 / gas tier** 几乎总是 `..=`。
pub mod range_inclusive {
    pub fn latency_bucket(us: u64) -> &'static str {
        match us {
            0..=50 => "ultra",
            51..=200 => "fast",
            201..=1000 => "slow",
            _ => "timeout",
        }
    }

    pub fn demonstrate() {
        println!("## 1. Range 模式：闭区间 `..=`");

        for us in [0, 50, 51, 200, 201, 5000] {
            println!("  {}μs → {}", us, latency_bucket(us));
        }
        println!("规则：边界值 50/51 必须落在不同桶；写单元测试钉死边界\n");
    }
}

// ============================================================================
// 2. Match guard：形状 + 业务条件分离
// ============================================================================
/// 模式先绑定变量，guard 再写业务谓词。
/// 这比 `if px > 0 && px < band` 嵌套更可读，且 arm 顺序即优先级。
pub mod match_guard {
    #[derive(Debug, Clone, Copy)]
    pub struct Quote {
        pub px: i64,
        pub qty: i64,
    }

    pub fn classify(q: Quote, ref_px: i64) -> &'static str {
        let band = ref_px / 20; // ±5%
        match q {
            Quote { px: 0, .. } => "invalid_px",
            Quote { qty, .. } if qty <= 0 => "invalid_qty",
            Quote { px, .. } if px < ref_px - band || px > ref_px + band => "out_of_band",
            _ => "ok",
        }
    }

    pub fn demonstrate() {
        println!("## 2. Match guard：解构后再 `if`");

        let ref_px = 100_00i64;
        for q in [
            Quote { px: 100_00, qty: 10 },
            Quote { px: 120_00, qty: 10 },
            Quote { px: 100_00, qty: 0 },
        ] {
            println!("  {:?} → {}", q, classify(q, ref_px));
        }
        println!("规则：最严格 arm 放最前；guard 保持纯函数\n");
    }
}

// ============================================================================
// 3. @ 绑定：既要整体又要字段
// ============================================================================
/// `val @ Pattern { field, .. }` 同时保留整体引用和字段绑定。
pub mod at_binding {
    #[derive(Debug, Clone, Copy)]
    pub struct Order {
        pub id: u64,
        pub px: i64,
        pub qty: i64,
    }

    pub fn audit(o @ Order { px, qty, .. }: Order, limit: i64) -> String {
        let tag = match o {
            Order { px: p, .. } if p > limit => "fat_finger",
            Order { qty: q, .. } if q > 1_000 => "large",
            _ => "normal",
        };
        format!("order#{} px={} qty={} → {}", o.id, px, qty, tag)
    }

    pub fn demonstrate() {
        println!("## 3. @ 绑定 + guard");

        let o = Order {
            id: 42,
            px: 150_00,
            qty: 50,
        };
        println!("  {}\n", audit(o, 110_00));
    }
}

// ============================================================================
// 4. 多维度：(a, b) + range + guard 组合
// ============================================================================
pub mod tuple_range_guard {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Action {
        Ok,
        Warn,
        Block,
    }

    pub fn check(score: i32, retries: u8) -> Action {
        match (score, retries) {
            (s, _) if s < -100 => Action::Block,
            (_, r) if r > 5 => Action::Block,
            (s, _) if s < 0 => Action::Warn,
            _ => Action::Ok,
        }
    }

    pub fn demonstrate() {
        println!("## 4. 二元组 + 多 guard 优先级");

        for (s, r) in [(-200, 0), (10, 10), (-1, 2), (5, 1)] {
            println!("  (score={}, retries={}) → {:?}", s, r, check(s, r));
        }
        println!("规则：第一个匹配的 arm 胜出；Block 条件必须排在 Warn 之前\n");
    }
}

pub fn demonstrate() {
    range_inclusive::demonstrate();
    match_guard::demonstrate();
    at_binding::demonstrate();
    tuple_range_guard::demonstrate();
}
