//! # 泛化：从 HFT/Web3 到守卫与范围应对策略
//!
//! 把前两章具体业务里的 guard/range 套路抽象成决策矩阵：
//!
//! | 问题类型              | 标志特征                     | 首选套路                          |
//! |-----------------------|------------------------------|-----------------------------------|
//! | 1. 数值分桶           | 延迟 / fee / notional 档位   | 闭区间 `..=` range match          |
//! | 2. 分层优先级         | Kill > Reject > Warn > Pass  | 严格 guard arm 在前               |
//! | 3. 形状 + 条件        | 解构后再判业务谓词           | struct 解构 + `if guard`          |
//! | 4. 特殊值快路径       | 0 / 零地址 / 空 calldata     | 字面量 arm 在 range 之前          |
//! | 5. 多维阈值           | (pnl, latency, rate)         | 二元/三元 tuple + 多 guard        |
//! | 6. 边界校验           | tick size / decimals         | 取模 guard 或 precompute          |
//! | 7. 协议区间           | opcode / chain id / session  | range + 离散常量混合              |
//! | 8. 布尔过滤           | 是否允许 / 是否终止态        | `matches!(x, Pat if guard)`       |
//!
//! 下面 8 个策略各有一个 *通用模板*，签名上不带业务名词。

#![allow(dead_code)]

// ============================================================================
// 策略 1：闭区间分桶 —— 延迟 / 费用 / 规模
// ============================================================================
/// HFT: 见 hft::latency_sla, hft::notional_tier
/// Web3: 见 web3::priority_fee_tier, web3::finality_depth
pub mod inclusive_bucketing {
    pub fn bucket(score: u32) -> &'static str {
        match score {
            0..=33 => "low",
            34..=66 => "mid",
            67..=100 => "high",
            _ => "out_of_range",
        }
    }

    pub fn demonstrate() {
        println!("## 策略 1：闭区间 `..=` 分桶");
        for s in [0, 33, 34, 100, 101] {
            println!("  score={} → {}", s, bucket(s));
        }
        println!();
    }
}

// ============================================================================
// 策略 2：分层 guard —— 优先级决策
// ============================================================================
/// HFT: 见 hft::kill_switch, hft::notional_tier
/// Web3: 见 web3::finality_depth
pub mod tiered_guards {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Severity {
        Ok,
        Warn,
        Critical,
    }

    pub fn assess(value: i32, retries: u8) -> Severity {
        match (value, retries) {
            (v, _) if v < -1000 => Severity::Critical,
            (_, r) if r > 10 => Severity::Critical,
            (v, _) if v < 0 => Severity::Warn,
            _ => Severity::Ok,
        }
    }

    pub fn demonstrate() {
        println!("## 策略 2：分层 guard（Critical 在前）");
        println!("  (-2000, 0) → {:?}", assess(-2000, 0));
        println!("  (-1, 0) → {:?}", assess(-1, 0));
        println!();
    }
}

// ============================================================================
// 策略 3：解构 + guard —— 形状与条件分离
// ============================================================================
/// HFT: 见 hft::tick_lattice, hft::spread_regime
/// Web3: 见 web3::transfer_bounds, web3::blob_tx_validate
pub mod destructure_then_guard {
    #[derive(Debug, Clone, Copy)]
    pub struct Request {
        pub id: u64,
        pub payload_len: usize,
        pub priority: u8,
    }

    pub fn route(req: Request) -> &'static str {
        match req {
            Request { payload_len: 0, .. } => "reject_empty",
            Request { priority: p, .. } if p > 9 => "express",
            Request { payload_len: len, .. } if len > 1024 => "batch",
            _ => "normal",
        }
    }

    pub fn demonstrate() {
        println!("## 策略 3：解构 + guard");
        let r = Request {
            id: 1,
            payload_len: 2048,
            priority: 3,
        };
        println!("  {:?} → {}\n", r, route(r));
    }
}

// ============================================================================
// 策略 4：特殊值快路径 —— 字面量 arm 优先
// ============================================================================
/// HFT: 见 hft::tick_lattice (`px: 0`)
/// Web3: 见 web3::transfer_bounds (ZERO_ADDR)
pub mod sentinel_first {
    pub fn normalize(value: i64, scale: i64) -> Result<i64, &'static str> {
        match value {
            0 => Ok(0),
            v if v % scale != 0 => Err("not_on_grid"),
            v if v < 0 => Err("negative"),
            v => Ok(v),
        }
    }

    pub fn demonstrate() {
        println!("## 策略 4：特殊值字面量 arm 在 range/guard 前");
        println!("  0 → {:?}", normalize(0, 5));
        println!("  7 → {:?}", normalize(7, 5));
        println!();
    }
}

// ============================================================================
// 策略 5：@ 绑定 —— 审计 + 决策同时要整体
// ============================================================================
/// HFT: 见 hft::self_trade_prevention
/// Web3: 见 web3::calldata_router
pub mod at_binding_audit {
    #[derive(Debug, Clone, Copy)]
    pub struct Event {
        pub seq: u64,
        pub code: u16,
    }

    pub fn handle(ev @ Event { code, .. }: Event) -> String {
        let action = match code {
            0..=99 => "ignore",
            100..=199 if code == 150 => "alert",
            100..=199 => "log",
            _ => "escalate",
        };
        format!("seq={} code={} → {}", ev.seq, code, action)
    }

    pub fn demonstrate() {
        println!("## 策略 5：@ 绑定 + range/guard");
        let ev = Event { seq: 99, code: 150 };
        println!("  {}\n", handle(ev));
    }
}

// ============================================================================
// 策略 6：Precompute —— 昂贵 guard 提到 match 外
// ============================================================================
/// HFT: 见 hft::notional_tier (notional 预计算)
/// Web3: 见 web3::priority_fee_tier (priority 预提取)
pub mod precompute_before_guard {
    fn costly_check(n: i64) -> bool {
        n % 7 == 0
    }

    pub fn classify(values: [i64; 3]) -> &'static str {
        let sum: i64 = values.iter().sum();
        let ok = costly_check(sum);
        match sum {
            s if s < 0 => "negative",
            s if ok => "lucky",
            0..=100 => "small",
            _ => "large",
        }
    }

    pub fn demonstrate() {
        println!("## 策略 6：Precompute 再 guard");
        println!("  [7,7,7] → {}", classify([7, 7, 7]));
        println!();
    }
}

// ============================================================================
// 策略 7：离散 + range 混合 —— 协议常量表
// ============================================================================
/// HFT: 见 hft::session_window
/// Web3: 见 web3::chain_id_gate
pub mod discrete_and_range {
    pub fn opcode_class(byte: u8) -> &'static str {
        match byte {
            0x00 => "stop",
            0x01..=0x04 => "arithmetic",
            0x60..=0x7f => "push",
            _ => "other",
        }
    }

    pub fn demonstrate() {
        println!("## 策略 7：离散常量 + range 混合");
        for b in [0x00, 0x02, 0x60, 0xff] {
            println!("  0x{:02x} → {}", b, opcode_class(b));
        }
        println!();
    }
}

// ============================================================================
// 策略 8：matches! 谓词 —— 零成本 bool 过滤
// ============================================================================
/// HFT: 见 hft::kill_switch 终止态检测
/// Web3: 见 web3::blob_tx_validate 快速校验
pub mod matches_predicate {
    #[derive(Debug, Clone, Copy)]
    pub enum State {
        Active,
        Draining,
        Down,
    }

    pub fn accepts_traffic(s: State, load_pct: u8) -> bool {
        matches!(s, State::Active | State::Draining if load_pct < 90)
    }

    pub fn demonstrate() {
        println!("## 策略 8：matches! + guard");
        println!(
            "  Active@95% → {}",
            accepts_traffic(State::Active, 95)
        );
        println!(
            "  Active@50% → {}",
            accepts_traffic(State::Active, 50)
        );
        println!();
    }
}

// ============================================================================
// 反例：什么时候不用 guard/range match
// ============================================================================
pub mod when_not_to_use {
    pub fn demonstrate() {
        println!("## 反例：什么时候不用 guard/range match");
        println!("  - 连续浮点区间 → 定点整数或 if 比较");
        println!("  - 上百个离散 opcode → 查表 `GAS_COST[op as usize]`");
        println!("  - 简单两分支 → `if` 比 match 短");
        println!("  - 动态规则引擎 → 插件 / DSL，不要硬编码 match");
        println!("  - 需要 fallthrough 共享逻辑 → 先 match 到小 enum 再函数分发\n");
    }
}

pub fn demonstrate() {
    inclusive_bucketing::demonstrate();
    tiered_guards::demonstrate();
    destructure_then_guard::demonstrate();
    sentinel_first::demonstrate();
    at_binding_audit::demonstrate();
    precompute_before_guard::demonstrate();
    discrete_and_range::demonstrate();
    matches_predicate::demonstrate();
    when_not_to_use::demonstrate();
}
