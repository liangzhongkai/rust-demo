//! # 泛化：从 HFT/Web3 场景到通用应对策略
//!
//! 把前两章具体业务里的模式匹配套路抽象出来，得到一张
//! **「问题类型 → 推荐套路」决策矩阵**：
//!
//! | 问题类型           | 标志特征                  | 首选套路                          |
//! |--------------------|---------------------------|-----------------------------------|
//! | 1. 协议分发        | 异构消息 / opcode         | 穷尽 enum match                   |
//! | 2. 状态机          | (state, event) 合法转移   | 二元 match + 守卫                 |
//! | 3. 嵌套解码        | 外层类型 + 内层字段       | 嵌套 struct/enum 解构             |
//! | 4. 分层路由        | 多条件优先级              | 守卫 + 顺序 arm                   |
//! | 5. 捕获 + 解构     | 既要整体又要字段          | `@` 绑定                          |
//! | 6. 布尔谓词        | filter / assert           | `matches!` 宏                     |
//! | 7. 早返回          | 解析失败 / 前置条件       | `let-else` / `if let` chain       |
//! | 8. 错误矩阵        | Result × 上下文           | `(Result, ctx)` match 或 `?`      |
//!
//! 下面 8 个策略各有一个 *通用模板函数*，签名上不带任何业务名词。

#![allow(dead_code)]

// ============================================================================
// 策略 1：穷尽 enum 分发 —— 协议 / 消息类型
// ============================================================================
/// 问题：收到异构消息，每种类型处理逻辑完全不同。
/// 模式：顶层 `match msg`，每个 variant 一个 arm，禁止 `_` 兜底。
///
/// HFT: 见 hft::md_dispatch
/// Web3: 见 web3::tx_type_dispatch
pub mod exhaustive_dispatch {
    #[derive(Debug, Clone, Copy)]
    pub enum Message {
        Ping,
        Data(u32),
        Close,
    }

    pub fn handle(m: Message) -> &'static str {
        match m {
            Message::Ping => "pong",
            Message::Data(n) if n == 0 => "empty",
            Message::Data(_) => "payload",
            Message::Close => "bye",
        }
    }

    pub fn demonstrate() {
        println!("## 策略 1：穷尽 enum 分发");
        for m in [Message::Ping, Message::Data(0), Message::Close] {
            println!("  {:?} → {}", m, handle(m));
        }
        println!();
    }
}

// ============================================================================
// 策略 2：二元状态机 —— (state, event)
// ============================================================================
/// 问题：有限状态机，事件驱动转移。
/// 模式：`match (state, event)`，终止态用守卫统一拒绝。
///
/// HFT: 见 hft::order_fsm
/// Web3: 见 web3::reorg_handler
pub mod state_event_matrix {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum State {
        Idle,
        Running,
        Done,
    }

    #[derive(Debug, Clone, Copy)]
    pub enum Event {
        Start,
        Finish,
        Cancel,
    }

    pub fn step(s: State, e: Event) -> Result<State, &'static str> {
        match (s, e) {
            (State::Idle, Event::Start) => Ok(State::Running),
            (State::Running, Event::Finish) => Ok(State::Done),
            (State::Running, Event::Cancel) => Ok(State::Idle),
            (State::Running, Event::Start) => Err("already running"),
            (State::Done, _) => Err("already done"),
            (State::Idle, Event::Finish | Event::Cancel) => Err("not started"),
        }
    }

    pub fn demonstrate() {
        println!("## 策略 2：(state, event) 二元 match");
        println!("  (Idle, Start) → {:?}", step(State::Idle, Event::Start));
        println!("  (Done, Start) → {:?}", step(State::Done, Event::Start));
        println!();
    }
}

// ============================================================================
// 策略 3：嵌套解构 —— 协议解码
// ============================================================================
/// 问题：外层 envelope 决定内层怎么解析。
/// 模式：嵌套 match 或一层 match 里解构多层 struct。
///
/// HFT: 见 hft::venue_multiplex
/// Web3: 见 web3::event_log_decode
pub mod nested_decode {
    #[derive(Debug, Clone, Copy)]
    pub enum Envelope {
        V1 { tag: u8, body: u32 },
        V2 { flags: u8, body: u64 },
    }

    pub fn payload(envelope: Envelope) -> u64 {
        match envelope {
            Envelope::V1 { body, .. } => body as u64,
            Envelope::V2 { body, .. } => body,
        }
    }

    pub fn demonstrate() {
        println!("## 策略 3：嵌套解构");
        let e = Envelope::V2 {
            flags: 1,
            body: 999,
        };
        println!("  payload = {}\n", payload(e));
    }
}

// ============================================================================
// 策略 4：分层守卫路由 —— 优先级决策
// ============================================================================
/// 问题：多个条件，优先级不同（如 Kill > Reject > Warn > Pass）。
/// 模式：把最严格条件放最前 arm，用 guard 过滤。
///
/// HFT: 见 hft::risk_gate, hft::circuit_breaker
/// Web3: 见 web3::reorg_handler
pub mod tiered_guards {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Level {
        Ok,
        Warn,
        Critical,
    }

    pub fn classify(score: i32, latency_ms: u32) -> Level {
        match (score, latency_ms) {
            (s, _) if s < -100 => Level::Critical,
            (_, l) if l > 1000 => Level::Critical,
            (s, _) if s < 0 => Level::Warn,
            _ => Level::Ok,
        }
    }

    pub fn demonstrate() {
        println!("## 策略 4：分层守卫（高优先级 arm 在前）");
        println!("  (-200, 10) → {:?}", classify(-200, 10));
        println!("  (10, 2000) → {:?}", classify(10, 2000));
        println!();
    }
}

// ============================================================================
// 策略 5：@ 绑定 —— 整体 + 字段同时要
// ============================================================================
/// 问题：日志要打整体 id，计算要用字段。
/// 模式：`val @ Struct { field, .. }` 一次绑定。
///
/// HFT: 见 hft::exec_algo_router 里的 OrderReq
/// Web3: 见 web3::account_transition
pub mod at_binding {
    #[derive(Debug, Clone, Copy)]
    pub struct Packet {
        pub id: u32,
        pub len: u16,
    }

    pub fn describe(p @ Packet { len, .. }: Packet) -> String {
        format!("packet#{} len={}", p.id, len)
    }

    pub fn demonstrate() {
        println!("## 策略 5：@ 绑定");
        let p = Packet { id: 7, len: 128 };
        println!("  {}\n", describe(p));
    }
}

// ============================================================================
// 策略 6：matches! 谓词 —— 过滤 / 断言
// ============================================================================
/// 问题：只关心「是否符合某模式」，不需要绑定值。
/// 模式：`matches!(x, Pattern if guard)`，零成本 bool。
///
/// HFT: 见 hft::order_fsm 终止态检测
/// Web3: 见 web3::mev_bundle_classify 里的 iter().all
pub mod matches_predicate {
    #[derive(Debug, Clone, Copy)]
    pub enum Status {
        Active,
        Draining,
        Down,
    }

    pub fn accepts_traffic(s: Status) -> bool {
        matches!(s, Status::Active | Status::Draining)
    }

    pub fn demonstrate() {
        println!("## 策略 6：matches! 谓词");
        for s in [Status::Active, Status::Down] {
            println!("  {:?} accepts? {}", s, accepts_traffic(s));
        }
        println!();
    }
}

// ============================================================================
// 策略 7：let-else 早返回 —— 解析 / 前置条件
// ============================================================================
/// 问题：函数开头要把 Option/Result 拆包，失败则 return。
/// 模式：`let Some(x) = opt else { return };`（Rust 1.65+）。
///
/// HFT: 见 hft::tick_bucket::split_batch 的调用方
/// Web3: 见 indexer 里 `let [t0, ..] = topics else { return None }`
pub mod let_else {
    pub fn first_byte(buf: &[u8]) -> Option<u8> {
        let [head, ..] = buf else {
            return None;
        };
        Some(*head)
    }

    pub fn demonstrate() {
        println!("## 策略 7：let-else 早返回");
        println!("  [1,2,3] → {:?}", first_byte(&[1, 2, 3]));
        println!("  [] → {:?}", first_byte(&[]));
        println!();
    }
}

// ============================================================================
// 策略 8：Result 矩阵 —— 错误分类
// ============================================================================
/// 问题：同一错误类型在不同上下文要不同处理（重试 / 降级 / 上报）。
/// 模式：`match (result, context)` 或先 match Result 再 match 错误 kind。
///
/// HFT: gateway Reject code 分类
/// Web3: 见 web3::account_transition 的 ApplyError
pub mod result_matrix {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Ctx {
        HotPath,
        Batch,
    }

    #[derive(Debug)]
    pub enum ApiError {
        Timeout,
        BadRequest,
    }

    pub fn policy<T>(ctx: Ctx, r: Result<T, ApiError>) -> &'static str {
        match (ctx, r) {
            (Ctx::HotPath, Err(ApiError::Timeout)) => "drop",
            (Ctx::Batch, Err(ApiError::Timeout)) => "retry",
            (_, Err(ApiError::BadRequest)) => "alert",
            (_, Ok(_)) => "ok",
        }
    }

    pub fn demonstrate() {
        println!("## 策略 8：(context, Result) 错误矩阵");
        println!(
            "  hot timeout → {}",
            policy::<()>(Ctx::HotPath, Err(ApiError::Timeout))
        );
        println!(
            "  batch timeout → {}",
            policy::<()>(Ctx::Batch, Err(ApiError::Timeout))
        );
        println!();
    }
}

// ============================================================================
// 反向：什么时候 *不要* 用 match
// ============================================================================
pub mod when_not_to_match {
    pub fn demonstrate() {
        println!("## 反例：什么时候不要用 match");
        println!("  - 简单两分支 bool → `if` 更短");
        println!("  - 连续相等比较 → `match` 换 `HashMap` 查表（opcode 上百个时）");
        println!("  - 动态类型插件系统 → trait object / visitor，enum 爆炸");
        println!("  - 单字段访问 → `.field` 或解构 let，不必 match");
        println!("  - 需要 fallthrough / 共享大段逻辑 → 先 match 到小 enum 再函数分发\n");
    }
}

pub fn demonstrate() {
    exhaustive_dispatch::demonstrate();
    state_event_matrix::demonstrate();
    nested_decode::demonstrate();
    tiered_guards::demonstrate();
    at_binding::demonstrate();
    matches_predicate::demonstrate();
    let_else::demonstrate();
    result_matrix::demonstrate();
    when_not_to_match::demonstrate();
}
