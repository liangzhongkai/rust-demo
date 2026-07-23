//! # 生命周期基础机制
//!
//! Rust 生命周期回答一个问题：**借用的引用在编译期被证明「活多久」**。
//! 生产代码里几乎不会手写 `'a`，但会在以下形态反复出现：
//!
//! - 零拷贝解析：`OrderView<'buf>` 字段指向 wire buffer
//! - arena 批处理：`'arena` 把一批临时对象绑在同一次 tick / block 上
//! - 边界转换：跨线程 / async 必须 `'static` 或 owned
//! - HRTB：`for<'a> Fn(&'a T)` 让回调接受任意短生命周期输入

#![allow(dead_code)]

// ============================================================================
// 1. 显式生命周期参数 —— 告诉编译器「输出引用来自哪个输入」
// ============================================================================
/// `longest` 的返回值生命周期 = `x` 和 `y` 中较短的那个（由调用方保证）。
pub fn longest<'a>(x: &'a str, y: &'a str) -> &'a str {
    if x.len() >= y.len() { x } else { y }
}

// ============================================================================
// 2. 结构体里的引用 —— 必须标注，否则无法构造「视图类型」
// ============================================================================
pub struct BookLevelView<'buf> {
    pub px: i64,
    pub qty: i64,
    pub venue_tag: &'buf [u8],
}

// ============================================================================
// 3. 生命周期省略规则（编译器自动推断的三条）
// ============================================================================
/// 每条 input 各得一个 `'a`；若只有一个 input lifetime，输出也用它。
pub fn first_word(s: &str) -> &str {
    s.split_whitespace().next().unwrap_or("")
}

/// 方法：&self 得到一个 lifetime，返回值通常绑定到 &self。
impl BookLevelView<'_> {
    pub fn tag_str(&self) -> &str {
        std::str::from_utf8(self.venue_tag).unwrap_or("")
    }
}

// ============================================================================
// 4. 多个 lifetime —— 输出只能绑定到「确实被用到」的那个
// ============================================================================
pub struct SpreadQuote<'bid, 'ask> {
    pub bid: &'bid [u8],
    pub ask: &'ask [u8],
}

pub fn demonstrate() {
    println!("## 基础 1：显式 `'a` 与结构体视图");

    let wire = b"venue=NYSE|bid=100.5|ask=100.7";
    let view = BookLevelView {
        px: 100_50,
        qty: 500,
        venue_tag: b"NYSE",
    };
    println!(
        "BookLevelView tag={} px={} (wire len={})",
        view.tag_str(),
        view.px,
        wire.len()
    );

    println!("\n## 基础 2：省略规则 — `first_word`");
    let s = "alpha beta gamma";
    println!("first_word = {:?}", first_word(s));

    println!("\n## 基础 3：`longest` — 两输入同寿");
    let a = "short";
    let b = "much longer string";
    println!("longest = {:?}", longest(a, b));

    println!("\n## 基础 4：多 lifetime 字段 — SpreadQuote");
    let bid = b"100.5";
    let ask = b"100.7";
    let q = SpreadQuote { bid, ask };
    println!("bid={:?} ask={:?}\n", q.bid, q.ask);
}
