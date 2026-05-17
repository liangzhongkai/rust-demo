//! # 泛化：问题类型 → Arena 决策矩阵
//!
//! 把 HFT / Web3 里的具体故事抽象成 **可迁移的检查清单**：
//!
//! | 问题类型 | 标志 | 首选策略 | 典型反模式 |
//! |----------|------|----------|------------|
//! | A. 请求/帧边界清晰 | 每个 UDP 包、每个区块、每条 tx 结束即无状态 | 帧顶 `let bump = Bump::new()` | 全局 `Vec` 不清空复用导致串包 |
//! | B. 批量同寿对象 | 解码中间树、模拟调用栈、top-K 档位 | `BumpVec` + `into_bump_slice` | 每节点 `Box::new` |
//! | C. 已知上界切片 | K 档深度、固定 header | `alloc_slice_copy` | `Vec::reserve` 抖动 |
//! | D. 解析 / IR | 递归下降 AST | 子指针全 `&'a Node<'a>` | `Rc<RefCell>` |
//! | E. 多候选试算 | bundle 搜索、风控影子 book | 候选级 arena | `clone` 全局状态 |
//! | F. 跨 `await` | 异步 pipeline | **禁止**长时间借用 `&Bump` | 在 await 前 materialize 到 `Vec` / `Arc` |
//! | G. 需要 `Drop` | socket/guard | `bumpalo::boxed::Box` 或不用 arena | 期待自动 RAII |
//!
//! 下面 4 个 **泛用模板** 不依赖业务名词，可直接抽成 `util::arena` 模块。

#![allow(dead_code)]

use bumpalo::Bump;
use bumpalo::collections::Vec as BumpVec;

// =============================================================================
// 模板 1：with_bump —— 强制「边界」在类型系统里可见
// =============================================================================
/// 任何 `FnOnce(&Bump) -> R` 形式的计算包一层，避免调用方忘记 new/drop。
pub fn with_bump<R, F>(f: F) -> R
where
    F: FnOnce(&Bump) -> R,
{
    let bump = Bump::new();
    f(&bump)
}

pub mod demonstrate_with_bump {
    use super::*;

    pub fn run() {
        println!("## 策略模板 1：`with_bump` 封装边界");

        let sum = with_bump(|b| {
            let xs = b.alloc_slice_copy(&[10_i64, 20, 30]);
            xs.iter().sum::<i64>()
        });
        println!("sum = {}\n", sum);
    }
}

// =============================================================================
// 模板 2：collect_in —— `Iterator` → `&[T]` 零额外全局堆（仅在 bump 上长）
// =============================================================================
pub fn collect_in<'a, T: Copy, I: IntoIterator<Item = T>>(
    bump: &'a Bump,
    it: I,
) -> &'a [T] {
    let mut v = BumpVec::new_in(bump);
    v.extend(it);
    let s = v.into_bump_slice();
    &*s
}

pub mod demonstrate_collect_in {
    use super::*;

    pub fn run() {
        println!("## 策略模板 2：`collect_in`（热路径迭代器物化）");

        let bump = Bump::new();
        let ys = collect_in(&bump, (0..12).filter(|x| x % 3 == 0).map(|x| x as i64));
        println!("filtered = {:?}\n", ys);
    }
}

// =============================================================================
// 模板 3：scratch_vec —— `clear` 语义词法化（仍用 bump 容量）
// =============================================================================
/// 相比「复用 `Vec` 并手动 clear」，TLS bump +（可选）`reset` 让 **峰值内存可预测**。
pub fn scratch_example() {
    println!("## 策略模板 3：TLS / 线程局部 scratch bump");
    println!(
        "模式：`thread_local! {{ static BUMP: RefCell<Bump> = ... }}`\n\
         每帧 `unsafe {{ bump.reset() }}` 或干脆每帧 `Bump::new()`（小帧时后者更简单）。\n\
         HFT：Pin 线程 + 测过 reset 延迟后，再决定是否复用。\n"
    );
}

// =============================================================================
// 模板 4：物化出口 —— 在 async 边界「落地」数据
// =============================================================================
/// 从 arena 借来的 `&[T]` 若需跨越 `.await`，**拷贝** 到 `Vec<T>`（或 `Bytes`）。
pub fn materialize_if_needed<T: Copy>(short_lived: &[T], need_long: bool) -> Option<Vec<T>> {
    if need_long {
        Some(short_lived.to_vec())
    } else {
        None
    }
}

pub mod demonstrate_materialize {
    use super::*;

    pub fn run() {
        println!("## 策略模板 4：async 边界物化");

        let bump = Bump::new();
        let tmp = bump.alloc_slice_copy(&[1_u8, 2, 3]);
        let owned = materialize_if_needed(tmp, true);
        println!("跨 await 应使用: {:?}\n", owned);
    }
}

pub fn demonstrate() {
    demonstrate_with_bump::run();
    demonstrate_collect_in::run();
    scratch_example();
    demonstrate_materialize::run();

    println!("--- 总结 ---");
    println!(
        "1. **先找边界**：包/帧/块/candidate 结束是否能把对象一锅端？\n\
         2. **再量分配次数**：热路径 profiler 是否显示 `malloc` 尖刺？\n\
         3. **最后处理 Drop/async**：析构与 `.await` 是 arena 两大禁区，提前设计出口。\n"
    );
}
