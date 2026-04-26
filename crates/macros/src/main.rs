//! Macros in Production: HFT & Web3 Scenarios
//!
//! 本 crate 只用标准库，聚焦"宏到底解决了哪些生产问题"，而不是语法玩具。
//!
//! 为什么 HFT 和 Web3 对宏依赖特别重：
//! - HFT 追求**零成本抽象 + 编译期检查 + hot-path 可预测**，
//!   而宏是唯一能在"不付出运行期代价"的前提下同时做到这三件事的工具；
//! - Web3 里合约 ABI、事件签名、selector、revert 语义是**形式化**的，
//!   宏能把"规范"直接编译成"代码"，消除一整类"写错字段名/算错 selector"的 bug。
//!
//! 覆盖顺序：
//!   §1  HFT  hot-path 日志与延迟探针（debug 有、release 零指令）
//!   §2  HFT  FIX 字段声明式解析（tag -> 字段 -> struct 一把梭）
//!   §3  HFT  金额/价格/数量 newtype（编译期阻止单位串台）
//!   §4  HFT  branch hint（冷路径标注替代 nightly intrinsics::likely）
//!   §5  Web3 require! / revert!（Solidity 语义 → Rust Result）
//!   §6  Web3 函数 selector 分派表（4 字节 → handler，编译期生成）
//!   §7  Web3 事件 emit（字段名 + 顺序 + 类型在编译期固定）
//!   §8  泛化：三类宏、十条坑、五条选型策略

use std::time::Instant;

// ---------------------------------------------------------------------------
// §1. HFT hot-path 日志 / 延迟探针
// ---------------------------------------------------------------------------
//
// 生产问题：
//   交易热路径（收行情 → 撮合 → 下单）要求 p99 在个位数微秒。
//   普通 `log::info!` 即使 level 被关掉，**参数仍会被求值**（format_args
//   本身是懒的，但传入的表达式比如 `compute_mid_price(&book)` 会先算），
//   在热路径里这是致命的。
//
// 解决思路：
//   - 用 `#[cfg(debug_assertions)]` 或 feature flag 让整段代码在 release
//     里"消失"（不是 no-op，是字面意义上的 0 条指令）。
//   - 延迟探针用 RAII / scope macro，避免手动 `start = Instant::now()`
//     四处散落、`?` 早退时忘了打点。

/// debug 构建打印，release 构建**编译期消除**。
///
/// 对比 `if cfg!(debug_assertions) { ... }`：`cfg!` 是运行期常量，
/// 参数表达式（如 `expensive()`）仍会被类型检查并保留在 AST 里，
/// 依赖 LLVM 的 DCE；而 `#[cfg]` 是**预处理阶段**剔除，更可靠。
macro_rules! hot_log {
    ($($arg:tt)*) => {{
        #[cfg(debug_assertions)]
        {
            eprintln!("[hot] {}", format_args!($($arg)*));
        }
    }};
}

/// scope 级延迟探针：任何 `?` 早退都仍会在 Drop 里落盘。
///
/// 用法：`let _g = lat_scope!("match_engine");`
/// 出作用域自动打印耗时，**零心智负担**。
macro_rules! lat_scope {
    ($label:expr) => {
        $crate::LatGuard::new($label)
    };
}

pub struct LatGuard {
    label: &'static str,
    start: Instant,
}

impl LatGuard {
    pub fn new(label: &'static str) -> Self {
        Self { label, start: Instant::now() }
    }
}

impl Drop for LatGuard {
    fn drop(&mut self) {
        let us = self.start.elapsed().as_nanos() as f64 / 1_000.0;
        eprintln!("[lat] {:<20} {:>8.2} us", self.label, us);
    }
}

// ---------------------------------------------------------------------------
// §2. HFT FIX 字段声明式解析
// ---------------------------------------------------------------------------
//
// 生产问题：
//   FIX 协议是 `tag=value\x01` 的 KV 流。手写每条消息的 parser 既繁琐，
//   又容易在"新增字段"时漏掉一处导致线上 reject。
//
// 解决思路：
//   用一个宏，**一次声明** tag + 字段名 + 类型，自动生成
//   `struct` 本体、`parse_fix(input: &[u8])` 方法、`Display` 兼容 dump。
//   新增字段只改一行。
//
// 这是典型的"schema 驱动代码生成"——也是 `serde`, `prost`, `alloy-sol-macro`
// 的本质：宏把**规范**编译成**代码**。

macro_rules! fix_message {
    (
        struct $name:ident {
            $( ($tag:literal, $field:ident : $ty:ty) ),+ $(,)?
        }
    ) => {
        #[derive(Debug, Default, PartialEq)]
        pub struct $name {
            $( pub $field: Option<$ty>, )+
        }

        impl $name {
            /// 解析 `tag=value|tag=value|...` 格式（用 `|` 代替 SOH 方便演示）。
            /// 未知 tag 静默跳过——生产里通常要可配置严格模式。
            pub fn parse_fix(input: &[u8]) -> Self {
                let mut out = Self::default();
                for kv in input.split(|&b| b == b'|') {
                    let mut it = kv.splitn(2, |&b| b == b'=');
                    let (Some(k), Some(v)) = (it.next(), it.next()) else { continue };
                    let Ok(k) = std::str::from_utf8(k) else { continue };
                    let Ok(v) = std::str::from_utf8(v) else { continue };
                    let Ok(tag) = k.parse::<u32>() else { continue };
                    match tag {
                        $(
                            $tag => {
                                if let Ok(parsed) = v.parse::<$ty>() {
                                    out.$field = Some(parsed);
                                }
                            }
                        )+
                        _ => {}
                    }
                }
                out
            }
        }
    };
}

// 新增字段就加一行，不需要动 parser。
fix_message! {
    struct NewOrderSingle {
        (35, msg_type:      u32),  // 真实 FIX 是 char/string，这里简化
        (11, cl_ord_id:     u64),
        (54, side:          u8),   // 1=Buy 2=Sell
        (38, order_qty:     u64),
        (44, price:         u64),  // 定点数（×1e4）
        (59, time_in_force: u8),
    }
}

// ---------------------------------------------------------------------------
// §3. HFT 金额 / 价格 / 数量 newtype
// ---------------------------------------------------------------------------
//
// 生产问题：
//   `fn place(price: u64, qty: u64)` —— 你敢保证调用方不会把两个参数写反？
//   2010 年 Knight Capital 45 分钟亏 4.4 亿美金的事故，
//   其中一条成因就是"同类型参数语义混淆"。
//
// 解决思路：
//   用宏批量生成"看起来像 u64、但编译期不可互换"的 newtype。
//   比手写 N 份 newtype + 算术 impl 省 90% 代码，同时获得：
//     - 类型不同 → 传错参数直接编译错
//     - `repr(transparent)` → 运行期零开销
//     - 算术只在同类型之间合法 → 不能 `price + qty`

macro_rules! money_newtype {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[repr(transparent)]
        pub struct $name(pub u64);

        impl $name {
            pub const ZERO: Self = Self(0);
            pub const fn new(v: u64) -> Self { Self(v) }
            pub const fn raw(self) -> u64 { self.0 }
        }

        impl std::ops::Add for $name {
            type Output = Self;
            fn add(self, rhs: Self) -> Self { Self(self.0 + rhs.0) }
        }
        impl std::ops::Sub for $name {
            type Output = Self;
            fn sub(self, rhs: Self) -> Self { Self(self.0 - rhs.0) }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }
    };
}

money_newtype!(/// 定点数价格，单位 1e-4
               Price);
money_newtype!(/// 数量，整数张
               Qty);
money_newtype!(/// 金额 = Price × Qty，用第三个类型承载结果
               Notional);

/// 显式的跨类型乘法：类型系统强制你"想清楚单位"。
fn notional(p: Price, q: Qty) -> Notional {
    Notional(p.0 * q.0)
}

// ---------------------------------------------------------------------------
// §4. HFT 分支提示
// ---------------------------------------------------------------------------
//
// 生产问题：
//   订单簿里 99% 的报价变动不会穿越对手价，但编译器不知道这一点，
//   会按 50/50 概率给出 branch layout，导致 icache miss。
//
// 解决思路（stable 版本）：
//   把冷路径抽成 `#[cold] #[inline(never)]` 的小函数，
//   LLVM 的 PGO 启发式会自动把冷路径挪到函数末尾，
//   hot 路径变成直线代码，流水线友好。
//
// 这个"把冷路径抽函数"的动作写多了会忘记加属性，用宏固化它。

macro_rules! cold_path {
    ($body:block) => {{
        #[cold]
        #[inline(never)]
        fn __cold() { }
        __cold();
        $body
    }};
}

/// 类似 `likely!(cond)`，在 stable 上通过"冷路径打标"实现。
/// 不会改变结果，只影响代码布局。
macro_rules! unlikely {
    ($cond:expr) => {{
        let __c = $cond;
        if __c { cold_path!({}); }
        __c
    }};
}

fn match_quote(best_bid: Price, best_ask: Price, incoming: Price) -> &'static str {
    if unlikely!(incoming >= best_ask) {
        "CROSS_ASK"           // 冷路径：穿越对手价，走风控
    } else if unlikely!(incoming <= best_bid) {
        "CROSS_BID"
    } else {
        "QUOTE"               // 热路径：99% 走这里
    }
}

// ---------------------------------------------------------------------------
// §5. Web3 require! / revert!
// ---------------------------------------------------------------------------
//
// 生产问题：
//   Rust 写的链上 agent / 模拟器 / indexer 需要精确复现 EVM 的
//   `require(cond, "msg")` 与 `revert("msg")` 语义。
//   手写成 `if !cond { return Err(...) }` 很快会变成噪音，
//   而且**返回类型**和错误路径容易不统一。
//
// 解决思路：
//   `require!(cond, Error::X)` 和 `revert!(Error::X)` 两个宏，
//   强制团队只用这两种早退方式，code review 时一眼能看见"合约级校验"。

#[derive(Debug, PartialEq)]
pub enum VmError {
    InsufficientBalance,
    Slippage,
    Reentrancy,
    Unauthorized,
}

macro_rules! require {
    ($cond:expr, $err:expr) => {
        if !($cond) {
            return Err($err);
        }
    };
}

macro_rules! revert {
    ($err:expr) => {
        return Err($err);
    };
}

/// 简化版 ERC20 transfer 语义。
fn erc20_transfer(
    balance_of: &mut std::collections::HashMap<u64, u64>,
    from: u64,
    to: u64,
    amount: u64,
) -> Result<(), VmError> {
    // `from == 0` 在 ERC20 里语义是"mint"，这里用 revert! 早退演示。
    if from == 0 {
        revert!(VmError::Unauthorized);
    }
    let bal = *balance_of.get(&from).unwrap_or(&0);
    require!(bal >= amount, VmError::InsufficientBalance);

    *balance_of.entry(from).or_insert(0) -= amount;
    *balance_of.entry(to).or_insert(0) += amount;
    Ok(())
}

// ---------------------------------------------------------------------------
// §6. Web3 函数 selector 分派表
// ---------------------------------------------------------------------------
//
// 生产问题：
//   EVM 调用前 4 字节是 keccak256(signature)[..4]。链上监听 / 模拟器
//   要根据 selector 分派到不同 handler。手写 `match` 会有两种 bug：
//     - selector 常量算错（人肉算 keccak 谁都会错）；
//     - 新增函数时忘了在 match 里加一条分支。
//
// 解决思路：
//   把 `(selector, handler)` 的映射放在一个宏里一次声明，
//   宏同时生成 `dispatch(selector, calldata)` 函数 + `SELECTORS` 常量表，
//   调试 / fuzz 时可以遍历常量表。
//
// 真正的生产代码（如 alloy-sol-macro）会在 proc-macro 里直接算 keccak，
// 这里用固定常量做演示。

#[allow(dead_code)]
type Handler = fn(&[u8]) -> Result<Vec<u8>, VmError>;

macro_rules! dispatch_table {
    (
        $( $sel:literal => $name:ident ),+ $(,)?
    ) => {
        /// 导出常量表，便于测试 / fuzz 遍历。
        pub const SELECTORS: &[(u32, &str)] = &[
            $( ($sel, stringify!($name)), )+
        ];

        pub fn dispatch(selector: u32, calldata: &[u8]) -> Result<Vec<u8>, VmError> {
            match selector {
                $( $sel => $name(calldata), )+
                _ => Err(VmError::Unauthorized),
            }
        }
    };
}

fn transfer(_cd: &[u8]) -> Result<Vec<u8>, VmError> { Ok(vec![1]) }
fn approve(_cd: &[u8]) -> Result<Vec<u8>, VmError> { Ok(vec![2]) }
fn transfer_from(_cd: &[u8]) -> Result<Vec<u8>, VmError> { Ok(vec![3]) }

dispatch_table! {
    0xa9059cbb_u32 => transfer,         // transfer(address,uint256)
    0x095ea7b3_u32 => approve,          // approve(address,uint256)
    0x23b872dd_u32 => transfer_from,    // transferFrom(address,address,uint256)
}

// ---------------------------------------------------------------------------
// §7. Web3 事件 emit：编译期强制字段顺序 / 类型
// ---------------------------------------------------------------------------
//
// 生产问题：
//   链下监听器要把 `emit Transfer(from, to, value)` 序列化成 JSON 推给下游。
//   如果"字段顺序"或"字段名"和链上 ABI 有偏差，下游整条 pipeline 崩。
//
// 解决思路：
//   为每个 event 生成一个专用宏，**模式匹配字段名**，
//   写错字段名会直接编译失败，而不是运行期才发现。

macro_rules! define_event {
    ($name:ident { $( $field:ident : $ty:ty ),+ $(,)? }) => {
        paste_like_emit!($name, $( ($field, $ty) ),+);
    };
}

// 不依赖 `paste` crate，我们直接把 define + emit 合并在一个宏里：
macro_rules! paste_like_emit {
    ($name:ident, $( ($field:ident, $ty:ty) ),+) => {
        #[derive(Debug)]
        #[allow(dead_code)]
        pub struct $name {
            $( pub $field: $ty, )+
        }

        impl $name {
            pub fn to_json(&self) -> String {
                let mut s = String::from("{");
                $(
                    s.push_str(&format!(
                        "\"{}\":\"{:?}\",",
                        stringify!($field),
                        self.$field
                    ));
                )+
                s.pop(); // 去掉最后一个逗号
                s.push('}');
                s
            }
        }
    };
}

define_event!(Transfer { from: u64, to: u64, value: u64 });
define_event!(Approval { owner: u64, spender: u64, value: u64 });

/// emit! 宏强制"字段名 + 顺序"，漏字段 / 写错名编译失败。
macro_rules! emit {
    (Transfer { from: $from:expr, to: $to:expr, value: $value:expr }) => {
        Transfer { from: $from, to: $to, value: $value }
    };
    (Approval { owner: $owner:expr, spender: $spender:expr, value: $value:expr }) => {
        Approval { owner: $owner, spender: $spender, value: $value }
    };
}

// ---------------------------------------------------------------------------
// §8. 泛化：三类宏 / 十条坑 / 五条选型策略
// ---------------------------------------------------------------------------
//
// ── 三类宏 ────────────────────────────────────────────────────────────
//   1. 声明宏 `macro_rules!`        ：本文件全部示例属于这类。
//                                    轻量、没有单独 crate，
//                                    适合"形如 DSL"的简单变换。
//   2. 过程宏 - derive              ：`#[derive(Serialize)]` 这种。
//                                    需要独立 `proc-macro = true` crate。
//                                    能读取 struct 字段元信息 → 生成 impl。
//   3. 过程宏 - 属性 / 函数宏      ：`#[tokio::main]`、`sqlx::query!`、
//                                    `alloy::sol!`。能做任意 token 变换，
//                                    可以访问文件系统（sqlx 在编译期连数据库）。
//
// ── 十条坑 ────────────────────────────────────────────────────────────
//   1. 不用 `$crate::` 前缀 → 下游用户重命名 crate 时炸掉。
//   2. 表达式不加括号 → `$a * 2` 遇到 `$a = 1 + 1` 算出 3 而不是 4。
//   3. 误用 `tt` 代替 `expr`       → 吃掉逗号、导致递归展开歧义。
//   4. 递归深度超过 128            → 需 `#![recursion_limit = "512"]`。
//   5. 忘了 hygiene 不覆盖 item 层 → 宏里生成的 `fn foo` 会污染 outer scope。
//   6. `$()*` 与分隔符混用不当     → 末尾悬挂逗号报错，加 `$(,)?`。
//   7. 用宏代替本应是泛型的东西   → 编译时间爆炸、错误信息极难读。
//   8. proc-macro 副作用           → 编译期连 DB / 发网络请求要有回退。
//   9. 宏生成的类型在 IDE 里不跳转 → 调试时用 `cargo expand` 看真身。
//  10. 宏里 panic  → 编译期就会失败，但错误信息指向宏调用点而非定义点，
//                    用 `compile_error!("msg")` 产出可读的诊断。
//
// ── 五条选型策略 ──────────────────────────────────────────────────────
//   A. 默认用**泛型 / trait**。宏是最后手段。
//   B. 要"对字段逐个处理"（如 derive(Serialize)）→ proc-macro derive。
//   C. 要"把外部规范编译成代码"（ABI / schema / SQL）→ 函数式 proc-macro。
//   D. 要"在 hot-path 消除代码"或"固化 DSL 简写"→ macro_rules!。
//   E. 团队习惯优先：如果同事都读 `macro_rules!` 吃力，
//      就用函数 + inline 属性；可维护性 > 聪明。

// ---------------------------------------------------------------------------
// main：把每个场景跑一遍
// ---------------------------------------------------------------------------

fn main() {
    println!("===== §1 HFT hot_log / lat_scope =====");
    {
        let _g = lat_scope!("outer_scope");
        hot_log!("preparing order batch size={}", 1024);
        std::thread::sleep(std::time::Duration::from_micros(300));
    } // _g Drop 时打印耗时

    println!("\n===== §2 HFT FIX =====");
    let raw = b"35=68|11=99887766|54=1|38=500|44=102500|59=0";
    let order = NewOrderSingle::parse_fix(raw);
    println!("parsed NewOrderSingle = {:?}", order);
    assert_eq!(order.cl_ord_id, Some(99887766));
    assert_eq!(order.price, Some(102500));

    println!("\n===== §3 HFT money newtype =====");
    let p = Price::new(102_500);
    let q = Qty::new(500);
    let n = notional(p, q);
    println!("{} * {} = {}", p, q, n);
    // let bad = p + q;              // ← 编译错误：类型不同不能相加
    // let worse = notional(q, p);   // ← 编译错误：参数单位错

    println!("\n===== §4 HFT branch hint =====");
    let bid = Price::new(100_000);
    let ask = Price::new(100_100);
    for incoming in [99_500, 100_050, 100_200_u64] {
        let tag = match_quote(bid, ask, Price::new(incoming));
        println!("incoming={} → {}", incoming, tag);
    }

    println!("\n===== §5 Web3 require! / revert! =====");
    let mut ledger = std::collections::HashMap::new();
    ledger.insert(1_u64, 1_000_u64);
    let ok = erc20_transfer(&mut ledger, 1, 2, 400);
    let bad = erc20_transfer(&mut ledger, 1, 2, 999_999); // 余额不足
    println!("transfer 400  → {:?}, post-ledger={:?}", ok, ledger);
    println!("transfer huge → {:?}", bad);
    assert_eq!(bad, Err(VmError::InsufficientBalance));

    println!("\n===== §6 Web3 selector dispatch =====");
    for (sel, name) in SELECTORS {
        let out = dispatch(*sel, &[]).unwrap();
        println!("selector 0x{:08x} ({:<14}) → {:?}", sel, name, out);
    }
    let unknown = dispatch(0xdead_beef, &[]);
    println!("unknown selector → {:?}", unknown);

    println!("\n===== §7 Web3 emit event =====");
    let ev = emit!(Transfer { from: 1, to: 2, value: 400 });
    println!("{}", ev.to_json());
    let ev2 = emit!(Approval { owner: 1, spender: 3, value: 100 });
    println!("{}", ev2.to_json());
    // let bad = emit!(Transfer { frm: 1, to: 2, value: 400 });
    // ^^^ 写错字段名（frm）→ 编译期直接失败，正是我们想要的

    println!("\nAll scenarios done.");
}
