//! # 栈 vs 堆常见陷阱与诊断
//!
//! 这一章把生产事故里反复出现的 8 个内存布局陷阱解剖清楚：
//! - 现象（监控里看到什么）
//! - 根因（编译器/运行时发生了什么）
//! - 解决方案（一行修法 + 预防风格）

#![allow(dead_code)]

use crate::util::AllocCounter;

// ============================================================================
// 陷阱 1：热路径上不必要的 String / format!
// ============================================================================
/// **现象**：alloc rate 与 QPS 线性相关；P99 尖刺。
/// **根因**：`format!` / `to_string()` 每次 malloc + fmt 开销。
/// **修法**：热路径用 `write!` 到预分配 buffer，或延迟到 IO 边界。
pub mod format_in_hot_path {
    use crate::util::AllocCounter;

    pub fn slow(id: u64) -> String {
        format!("order_{id}")
    }

    pub fn fast(id: u64, buf: &mut String) {
        buf.clear();
        use std::fmt::Write;
        let _ = write!(buf, "order_{id}");
    }

    pub fn demonstrate() {
        println!("## 陷阱 1：热路径 format! 堆分配");

        let mut counter = AllocCounter::default();
        for i in 0..1000u64 {
            let s = slow(i);
            counter.allocs += 1;
            let _ = s;
        }
        println!("  format! ×1000: ~{} alloc", counter.allocs);

        let mut buf = String::with_capacity(32);
        let counter2 = AllocCounter::default();
        for i in 0..1000u64 {
            fast(i, &mut buf);
        }
        println!("  reuse String buffer: ~{} realloc", counter2.allocs);
        println!("  规则：format! 默认禁止出现在 >1k/s 的循环里\n");
    }
}

// ============================================================================
// 陷阱 2：中间 collect / to_vec 偷偷堆分配
// ============================================================================
/// **现象**：flamegraph 里 `alloc` 占比异常高，逻辑看起来只是 map/filter。
/// **根因**：链式中间步骤 `collect()` 物化成 Vec。
/// **修法**：保持 iterator 或 slice 形态到底。
pub mod hidden_collect {
    pub fn slow(data: &[u64]) -> u64 {
        let filtered: Vec<u64> = data.iter().copied().filter(|&x| x > 100).collect();
        filtered.iter().sum()
    }

    pub fn fast(data: &[u64]) -> u64 {
        data.iter().copied().filter(|&x| x > 100).sum()
    }

    pub fn demonstrate() {
        println!("## 陷阱 2：中间 collect 堆分配");
        let v: Vec<u64> = (0..1000).collect();
        assert_eq!(slow(&v), fast(&v));
        println!("  两种写法结果相同；fast 少 1 次 O(n) malloc");
        println!("  规则：只有最终消费者才 collect\n");
    }
}

// ============================================================================
// 陷阱 3：大数组放栈 → stack overflow
// ============================================================================
/// **现象**：release 环境偶发 SIGSEGV / stack overflow；debug 正常。
/// **根因**：`[u8; 1_000_000]` 在栈上 ≈ 1MB，默认线程栈 8MB，嵌套调用即爆。
/// **修法**：大 buffer 放堆（Vec）或 mmap；栈数组保持 < few KB。
pub mod stack_overflow {
    pub fn demonstrate() {
        println!("## 陷阱 3：栈上大数组");

        const OK: usize = 4096;
        let _small: [u8; OK] = [0; OK]; // ~4KB，通常安全
        println!("  [u8; 4096] = {} bytes 栈 — OK", OK);

        // ❌ 不要这样做（注释展示）：
        // let huge: [u8; 1_000_000] = [0; 1_000_000]; // 1MB 栈，危险

        let safe_heap = vec![0u8; 1_000_000]; // 堆上 1MB
        println!("  Vec 1MB len = {} — 大对象上堆", safe_heap.len());
        println!("  规则：栈数组 < 4-8KB；更大用 Vec/mmap/arena\n");
    }
}

// ============================================================================
// 陷阱 4：返回栈上数据的引用（悬垂引用）
// ============================================================================
/// **现象**：编译错误（Rust 优势）或 unsafe 代码 UB。
/// **根因**：函数返回 `&T` 但 T 在栈帧内，return 后地址失效。
/// **修法**：返回值（Copy/Clone）、Box、或 `'a` 绑定输入生命周期。
pub mod dangling_stack_ref {
    pub fn demonstrate() {
        println!("## 陷阱 4：返回栈引用（编译器拦截）");

        // ❌ 编译错误：
        // fn bad() -> &str {
        //     let s = String::from("hello");
        //     &s
        // }

        // ✅ 返回 owned
        fn good() -> String {
            String::from("hello")
        }

        // ✅ 借用输入
        fn borrow_input(input: &str) -> &str {
            input
        }

        let owned = good();
        let borrowed = borrow_input("world");
        println!("  owned = {}, borrowed = {}", owned, borrowed);
        println!("  规则：输出生命周期不能超过栈帧；用 owned 或 tie to input\n");
    }
}

// ============================================================================
// 陷阱 5：递归过深栈溢出
// ============================================================================
/// **现象**：深 trie / 深 JSON 解析 SIGSEGV。
/// **根因**：每层递归消耗栈帧；Rust 默认无尾调用优化保证。
/// **修法**：显式栈（Vec 模拟）或 trampoline；限制深度。
pub mod deep_recursion {
    pub fn recurse(n: u32) -> u32 {
        if n == 0 {
            0
        } else {
            1 + recurse(n - 1)
        }
    }

    pub fn iterate(n: u32) -> u32 {
        let mut acc = 0;
        for _ in 0..n {
            acc += 1;
        }
        acc
    }

    pub fn demonstrate() {
        println!("## 陷阱 5：深递归 vs 迭代");

        assert_eq!(recurse(100), iterate(100));
        println!("  recurse(100) = iterate(100) = 100");
        println!("  深 trie walk：用显式 Vec<(Node, depth)> 栈，别无限递归");
        println!("  规则：深度无界 → 堆上显式栈；有界且浅 → 可递归\n");
    }
}

// ============================================================================
// 陷阱 6：不必要的 Box / Arc "以防万一"
// ============================================================================
/// **现象**：到处都是 `Box<T>` / `Arc<T>`，cache miss 上升，代码难读。
/// **根因**：误以为「堆 = 灵活 = 更好」。
/// **修法**：默认栈 + owned；只在需要共享所有权或递归类型时用 Box/Arc。
pub mod unnecessary_box {
    #[derive(Debug, Clone, Copy)]
    struct Point {
        x: i64,
        y: i64,
    }

    fn use_stack(p: Point) -> i64 {
        p.x + p.y
    }

    fn use_box(p: Box<Point>) -> i64 {
        p.x + p.y
    }

    pub fn demonstrate() {
        println!("## 陷阱 6：不必要的 Box");

        let p = Point { x: 1, y: 2 };
        assert_eq!(use_stack(p), use_box(Box::new(p)));
        println!("  Point size = {} — Copy，无需 Box", std::mem::size_of::<Point>());
        println!("  Box 适用：递归类型（链表）、超大单对象、trait object");
        println!("  规则：Box 是显式决策，不是默认包装\n");
    }
}

// ============================================================================
// 陷阱 7：Vec 默认容量导致连续 realloc
// ============================================================================
/// **现象**：启动后前几秒延迟高，之后稳定 —— "warmup" 其实是 realloc。
/// **根因**：`Vec::new()` 从 0 开始倍增扩容。
/// **修法**：`with_capacity` / `reserve`；复用 `clear()` buffer。
pub mod vec_realloc {
    use super::*;

    pub fn demonstrate() {
        println!("## 陷阱 7：Vec 默认增长");

        let n = 10_000;
        let mut c1 = AllocCounter::default();
        let mut v1 = Vec::new();
        for i in 0..n {
            c1.track_vec_push(&mut v1, i);
        }

        let mut c2 = AllocCounter::default();
        let mut v2 = Vec::with_capacity(n);
        for i in 0..n {
            c2.track_vec_push(&mut v2, i);
        }

        println!("  Vec::new push {}: {} realloc", n, c1.allocs);
        println!("  with_capacity: {} realloc", c2.allocs);
        println!("  规则：知道上界就 reserve；不知道就估一个 P99 并 reuse\n");
    }
}

// ============================================================================
// 陷阱 8：Clone 大堆对象而非借用
// ============================================================================
/// **现象**：内存带宽打满，CPU 在 memcpy；火焰图 `_clone` 显著。
/// **根因**：函数签名取 `String`/`Vec` by value 或 `.clone()` 习惯。
/// **修法**：参数改 `&str` / `&[T]`；只在需要 ownership 转移时 move。
pub mod clone_vs_borrow {
    #[derive(Clone)]
    struct Payload {
        data: Vec<u8>,
    }

    fn process_borrow(p: &Payload) -> usize {
        p.data.len()
    }

    fn process_clone(p: Payload) -> usize {
        p.data.len()
    }

    pub fn demonstrate() {
        println!("## 陷阱 8：Clone 大对象 vs 借用");

        let p = Payload {
            data: vec![0u8; 1_000_000],
        };
        assert_eq!(process_borrow(&p), process_clone(p.clone()));
        println!("  borrow: 8 byte 指针传递");
        println!("  clone:  1MB memcpy + 可能的 free");
        println!("  规则：>64B 且只读 → 借用；写或跨线程 ownership 才 move/clone\n");
    }
}

pub fn demonstrate() {
    format_in_hot_path::demonstrate();
    hidden_collect::demonstrate();
    stack_overflow::demonstrate();
    dangling_stack_ref::demonstrate();
    deep_recursion::demonstrate();
    unnecessary_box::demonstrate();
    vec_realloc::demonstrate();
    clone_vs_borrow::demonstrate();
}
