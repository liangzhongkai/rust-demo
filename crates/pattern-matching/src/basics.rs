//! # 模式匹配底层机制
//!
//! 这一节回答四个根本问题，后面 HFT/Web3 章节里所有套路都建立在这之上：
//!
//! 1. `match` 与 `if`/`if let` 的本质区别是什么？
//! 2. 解构（destructure）如何同时「拆包 + 绑定 + 过滤」？
//! 3. `ref` / `ref mut` / `@` 绑定各解决什么问题？
//! 4. 编译器的穷尽性检查（exhaustiveness）如何帮你挡生产事故？

#![allow(dead_code)]

/// `match` 是 *决策树*：每个 arm 同时做三件事——
/// 1. **模式**：描述「长什么样」
/// 2. **绑定**：把匹配到的值赋给变量
/// 3. **守卫**（可选）：在模式之上再加条件
///
/// 与 `if` 链的区别：`match` 一次扫描就能穷尽所有分支，
/// 编译器会检查是否漏掉 variant；`if/else if` 不会。
pub mod match_as_decision_tree {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Side {
        Buy,
        Sell,
    }

    #[derive(Debug, Clone, Copy)]
    struct Quote {
        px: i64,
        qty: i64,
    }

    /// 用 match 表达「买价必须低于卖价才合法」——模式 + 守卫合一。
    fn spread_ok(bid: Quote, ask: Quote) -> bool {
        matches!(
            (bid, ask),
            (Quote { px: b, .. }, Quote { px: a, .. }) if b < a
        )
    }

    pub fn demonstrate() {
        println!("## 1. match 是带穷尽检查的决策树");

        let bid = Quote { px: 99_50, qty: 10 };
        let ask = Quote { px: 100_00, qty: 5 };
        println!("spread_ok = {}", spread_ok(bid, ask));

        let side = Side::Buy;
        let label = match side {
            Side::Buy => "taker buys",
            Side::Sell => "taker sells",
        };
        println!("side = {:?} → {}", side, label);
        println!();
    }
}

/// 解构的四种常见形态：tuple / struct / enum / slice。
/// 生产代码里 80% 的 match 都在做「从 ADT 里拿出字段」。
pub mod destructuring_forms {
    #[derive(Debug)]
    enum FeedMsg {
        Heartbeat { seq: u64 },
        Snapshot { bids: Vec<i64>, asks: Vec<i64> },
        Delta { side: u8, px: i64, qty: i64 },
    }

    fn summarize(msg: &FeedMsg) -> &'static str {
        match msg {
            FeedMsg::Heartbeat { .. } => "ping",
            FeedMsg::Snapshot { bids, asks } if bids.is_empty() && asks.is_empty() => "empty book",
            FeedMsg::Snapshot { .. } => "full snapshot",
            FeedMsg::Delta { qty, .. } if *qty == 0 => "delete level",
            FeedMsg::Delta { .. } => "update level",
        }
    }

    pub fn demonstrate() {
        println!("## 2. 解构：tuple / struct / enum / slice");

        let msgs = [
            FeedMsg::Heartbeat { seq: 1 },
            FeedMsg::Snapshot {
                bids: vec![100],
                asks: vec![101],
            },
            FeedMsg::Delta {
                side: 0,
                px: 100,
                qty: 0,
            },
        ];
        for m in &msgs {
            println!("  {:?} → {}", m, summarize(m));
        }
        println!();
    }
}

/// 绑定模式：`ref` 借出、`ref mut` 可变借出、`@` 同时保留整体和字段。
pub mod binding_modes {
    #[derive(Debug, Clone)]
    struct Order {
        id: u64,
        px: i64,
        qty: i64,
    }

    /// `@` 绑定：既要整体 id 做日志，又要字段做计算。
    fn fee_basis(Order { px, qty, .. }: &Order) -> i128 {
        (*px as i128) * (*qty as i128)
    }

    /// `ref mut`：在 match arm 里原地改，不 move 整个 struct。
    fn halve_qty(order: &mut Order) {
        let Order { qty, .. } = order;
        *qty /= 2;
    }

    pub fn demonstrate() {
        println!("## 3. ref / ref mut / @ 绑定");

        let o = Order {
            id: 42,
            px: 100,
            qty: 10,
        };
        println!("fee_basis(order#{} ) = {}", o.id, fee_basis(&o));

        let mut partial = o.clone();
        halve_qty(&mut partial);
        println!("halve_qty 后 qty = {}", partial.qty);
        println!();
    }
}

/// `matches!` 宏：把 match 压缩成 bool 谓词，适合 filter / assert / 守卫复用。
pub mod matches_macro {
    #[derive(Debug, Clone, Copy)]
    enum Status {
        Live,
        Stale,
        Dead,
    }

    fn is_actionable(s: Status) -> bool {
        matches!(s, Status::Live | Status::Stale)
    }

    pub fn demonstrate() {
        println!("## 4. matches! 宏 = 零开销 bool 谓词");
        for s in [Status::Live, Status::Stale, Status::Dead] {
            println!("  {:?} actionable? {}", s, is_actionable(s));
        }
        println!("编译后等价于手写 match，无额外分配\n");
    }
}

pub fn demonstrate() {
    match_as_decision_tree::demonstrate();
    destructuring_forms::demonstrate();
    binding_modes::demonstrate();
    matches_macro::demonstrate();
}
