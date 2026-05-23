//! # 模式匹配常见陷阱与诊断
//!
//! 这一章把生产事故里反复出现的 8 个模式匹配陷阱解剖清楚：
//! - 现象（用户在监控 / 测试里看到什么）
//! - 根因（编译器 / 语义层面发生了什么）
//! - 解决方案（一行修法 + 代码风格上的预防）

#![allow(dead_code)]

// ============================================================================
// 陷阱 1：非穷尽 match —— 新增 variant 静默漏处理
// ============================================================================
/// **现象**：升级依赖后某个订单状态不再被处理，订单「卡住」。
/// **根因**：match 没有覆盖全部 enum variant，旧代码靠 `_` 或 default 吞掉。
/// **修法**：去掉 `_`，让编译器 exhaustiveness 报错；或用 `#[non_exhaustive]` 强制显式 `_`。
pub mod non_exhaustive {
    #[derive(Debug, Clone, Copy)]
    enum OrderStatus {
        Open,
        Filled,
        // 将来加 Cancelled 时，没 `_` 的 match 会编译失败 —— 这是 feature
    }

    fn is_terminal(s: OrderStatus) -> bool {
        match s {
            OrderStatus::Filled => true,
            OrderStatus::Open => false,
            // 若加了 Cancelled 却忘了补 arm → 编译 error
        }
    }

    pub fn demonstrate() {
        println!("## 陷阱 1：非穷尽 match");
        println!("  Open terminal? {}", is_terminal(OrderStatus::Open));
        println!("规则：业务 enum 尽量不用 `_` 兜底，让编译器当你的 QA\n");
    }
}

// ============================================================================
// 陷阱 2：`_` 吞掉本该处理的错误
// ============================================================================
/// **现象**：Reject 原因全是 unknown，运维无法排障。
/// **根因**：`Err(_)` 或 `_ =>` 把错误细节丢弃。
/// **修法**：至少 log 或 map 到结构化错误；只在 truly impossible 分支用 `_`。
pub mod underscore_swallows {
    #[derive(Debug)]
    enum GatewayReply {
        Ack(u64),
        Reject { code: u16, reason: &'static str },
    }

    fn describe_bad(reply: GatewayReply) -> &'static str {
        match reply {
            GatewayReply::Ack(_) => "ok",
            // ❌ BUG: GatewayReply::Reject { .. } => "unknown",
            GatewayReply::Reject { reason, .. } => reason, // ✅ 保留 reason
        }
    }

    pub fn demonstrate() {
        println!("## 陷阱 2：`_` 吞掉错误细节");
        let r = GatewayReply::Reject {
            code: 99,
            reason: "price band",
        };
        println!("  reject → {}", describe_bad(r));
        println!("规则：`_` 只用于「真的不可能」或已 log 的分支\n");
    }
}

// ============================================================================
// 陷阱 3：match 里 partial move
// ============================================================================
/// **现象**：编译 error `use of partially moved value`。
/// **根因**：一个 arm move 了 struct 的某字段，其他 arm 还想用整体。
/// **修法**：用 `ref` / `ref mut` 借用；或 `@` 绑定后只 move 需要的字段。
pub mod partial_move {
    #[derive(Debug)]
    struct Fill {
        id: u64,
        px: i64,
        qty: i64,
    }

    fn log_fill(f: Fill) -> i64 {
        match f {
            Fill { qty, .. } => qty, // move 了 qty，f 其余字段也被 move
                                      // 若后面还要 f.id 就会编译失败
        }
    }

    fn log_fill_ref(f: &Fill) -> i64 {
        match f {
            Fill { qty, .. } => *qty, // 只借 qty，不 move
        }
    }

    pub fn demonstrate() {
        println!("## 陷阱 3：partial move");
        let f = Fill {
            id: 1,
            px: 100,
            qty: 10,
        };
        println!("  qty = {} (via ref match)", log_fill_ref(&f));
        println!("  原 struct 仍可用 id = {}", f.id);
        let _ = log_fill(f); // consume
        println!("规则：需要保留整体时用 `match &x` 或 `ref` 模式\n");
    }
}

// ============================================================================
// 陷阱 4：if let 链 vs match —— 漏掉互斥性
// ============================================================================
/// **现象**：两个 if let 都进了，或都没进。
/// **根因**：if let 链不检查穷尽性，条件可能重叠或遗漏。
/// **修法**：互斥分支用 match；多个 Option 组合尤其如此。
pub mod if_let_chain {
    #[derive(Debug, Clone, Copy)]
    enum Side {
        Bid,
        Ask,
    }

    fn spread_label_bad(bid: Option<i64>, ask: Option<i64>) -> &'static str {
        if let Some(b) = bid {
            if b > 100 {
                return "wide bid";
            }
        }
        if let Some(a) = ask {
            if a < 100 {
                return "tight ask";
            }
        }
        "unknown" // 很多合法组合落到这里
    }

    fn spread_label_good(bid: Option<i64>, ask: Option<i64>) -> &'static str {
        match (bid, ask) {
            (Some(b), Some(a)) if b >= a => "crossed",
            (Some(b), Some(a)) if a - b <= 5 => "tight",
            (Some(_), Some(_)) => "normal",
            (Some(_), None) => "bid only",
            (None, Some(_)) => "ask only",
            (None, None) => "empty",
        }
    }

    pub fn demonstrate() {
        println!("## 陷阱 4：if let 链漏组合");
        println!(
            "  bad  (Some(101), Some(99)) → {}",
            spread_label_bad(Some(101), Some(99))
        );
        println!(
            "  good (Some(101), Some(99)) → {}",
            spread_label_good(Some(101), Some(99))
        );
        println!("规则：二元及以上 Option/Result 优先 `(a,b) match`\n");
    }
}

// ============================================================================
// 陷阱 5：不可 refutable 的 let
// ============================================================================
/// **现象**：运行时 panic `called Option::unwrap()` on a None`。
/// **根因**：`let Some(x) = opt` 在 Rust 2024 前是 refutable，None 直接 panic。
/// **修法**：用 `if let` / `match` / `let-else` 显式处理 None。
pub mod refutable_let {
    pub fn demonstrate() {
        println!("## 陷阱 5：refutable let 隐式 panic");

        let opt: Option<i32> = None;
        // ❌ `let Some(x) = opt;` 在 None 时 panic

        // ✅ 显式分支
        let msg = match opt {
            Some(x) => format!("got {x}"),
            None => "missing".to_string(),
        };
        println!("  {}", msg);
        println!("规则：生产代码禁止对 Option/Result 裸 `let Ok/Some`\n");
    }
}

// ============================================================================
// 陷阱 6：range 模式边界漏洞
// ============================================================================
/// **现象**：价格为 tick_size 整数倍时归错桶。
/// **根因**：`1..5` 不含 5；混合 `..` 和 `..=` 时 off-by-one。
/// **修法**：统一用 `..=` 或写单元测试覆盖边界值。
pub mod range_gaps {
    pub fn bucket(px: i64) -> &'static str {
        match px {
            ..0 => "invalid",
            0..=99 => "low",
            100..=999 => "mid",
            1000.. => "high", // 1000 归 high；1000..= 才含上界语义需注意
        }
    }

    pub fn demonstrate() {
        println!("## 陷阱 6：range 边界 off-by-one");
        for px in [-1, 0, 99, 100, 999, 1000] {
            println!("  px={} → {}", px, bucket(px));
        }
        println!("规则：边界值写进测试；优先 `..=` 表达闭区间\n");
    }
}

// ============================================================================
// 陷阱 7：守卫里的副作用 / 重复计算
// ============================================================================
/// **现象**：风控守卫调用了两次 expensive 函数，延迟翻倍。
/// **根因**：guard 表达式可能被多次求值（文档注明 guard 不应有副作用）。
/// **修法**：守卫前用 `@` 绑定或 let 提前算好。
pub mod guard_side_effects {
    fn expensive_check(px: i64) -> bool {
        px % 2 == 0 // 教学替身
    }

    fn route_bad(px: i64) -> &'static str {
        match px {
            p if expensive_check(p) && expensive_check(p) => "fast", // 可能算两次
            _ => "slow",
        }
    }

    fn route_good(px: i64) -> &'static str {
        let ok = expensive_check(px);
        match px {
            p if ok => "fast",
            _ => "slow",
        }
    }

    pub fn demonstrate() {
        println!("## 陷阱 7：守卫副作用 / 重复求值");
        println!("  good route(100) → {}", route_good(100));
        println!("规则：guard 保持纯函数；昂贵检查提到 match 外\n");
    }
}

// ============================================================================
// 陷阱 8：match 引用 vs 值 —— 绑定模式混淆
// ============================================================================
/// **现象**：以为 match 到了 `&str` 实际 move 了 `String`；或改不动字段。
/// **根因**：`match x` / `match &x` / `match &mut x` 绑定模式不同。
/// **修法**：明确 match 的是值还是引用；原地修改用 `ref mut`。
pub mod binding_mode_confusion {
    #[derive(Debug)]
    struct Book {
        bid: i64,
        ask: i64,
    }

    fn widen_bad(book: Book) {
        match book {
            Book { bid, ask } => {
                // bid/ask 是 owned 副本，下面运算不会写回 book
                let (_new_bid, _new_ask) = (bid - 1, ask + 1);
            }
        }
    }

    fn widen_good(book: &mut Book) {
        match book {
            Book { bid, ask } => {
                *bid -= 1;
                *ask += 1;
            }
        }
    }

    pub fn demonstrate() {
        println!("## 陷阱 8：match 值 vs match &mut");

        let mut b = Book { bid: 100, ask: 101 };
        widen_good(&mut b);
        println!("  after widen_good: {:?}", b);

        let b2 = Book { bid: 100, ask: 101 };
        widen_bad(b2); // move，内部修改无效
        println!("规则：`match &mut x` + 默认 binding 拿到 &mut 字段\n");
    }
}

pub fn demonstrate() {
    non_exhaustive::demonstrate();
    underscore_swallows::demonstrate();
    partial_move::demonstrate();
    if_let_chain::demonstrate();
    refutable_let::demonstrate();
    range_gaps::demonstrate();
    guard_side_effects::demonstrate();
    binding_mode_confusion::demonstrate();
}
