//! # 迭代器常见陷阱与诊断
//!
//! 这一章把生产事故里反复出现的 8 个迭代器陷阱解剖清楚：
//! - 现象（用户在监控里看到什么）
//! - 根因（编译器/运行时层面发生了什么）
//! - 解决方案（一行修法 + 代码风格上的预防）
//!
//! 每个例子都能 `cargo check` 通过；故意写错的用注释标 `// ❌ BUG` 解释。

#![allow(dead_code)]

// ============================================================================
// 陷阱 1：忘记终端操作 → 副作用消失
// ============================================================================
/// **现象**：日志/metrics 没有上报，但代码看起来「写了」。
/// **根因**：`map` 是惰性的，没人 `next()` 它就一行不跑。
/// **修法**：用 `for` 表达副作用，或显式 `.for_each` / `.count()` / `collect()`。
pub mod forgot_consumer {
    pub fn demonstrate() {
        println!("## 陷阱 1：忘记终端操作");

        let txs = vec![1u64, 2, 3];

        // ❌ BUG：counter 永远是 0，因为 map 从未被消费
        let mut counter = 0u64;
        let _ = txs.iter().map(|t| {
            counter += 1; // 这行不会执行
            t * 10
        });
        println!("（错误写法）counter = {}（期望 3）", counter);

        // ✅ 修法：用 for_each / for 循环表达副作用
        let mut counter = 0u64;
        txs.iter().for_each(|_| counter += 1);
        println!("（正确写法）counter = {}", counter);

        // 经验：`map` 用于「值变换」，副作用一律用 `for_each`/`for`
        // Clippy 有 `unused_must_use` lint 在某些场景能抓到\n
        println!();
    }
}

// ============================================================================
// 陷阱 2：中间 `collect` 偷偷堆分配
// ============================================================================
/// **现象**：热路径出现莫名其妙的 alloc/free spike。
/// **根因**：把流物化成 `Vec` 再立刻消费 = 一次 O(n) 堆分配 + 缓存失效。
/// **修法**：保持迭代器形态，链到底再终端消费。
pub mod hidden_alloc {
    pub fn slow(prices: &[u64]) -> u64 {
        // ❌ 中间 collect 完全没必要
        let filtered: Vec<u64> = prices.iter().copied().filter(|&p| p > 100).collect();
        filtered.iter().map(|p| p * 2).sum()
    }

    pub fn fast(prices: &[u64]) -> u64 {
        // ✅ 一根管道到底
        prices.iter().copied().filter(|&p| p > 100).map(|p| p * 2).sum()
    }

    pub fn demonstrate() {
        println!("## 陷阱 2：中间 collect 偷偷堆分配");
        let v: Vec<u64> = (0..1000).collect();
        assert_eq!(slow(&v), fast(&v));
        println!("两种写法等价；`fast` 0 次堆分配，`slow` 多 1 次");
        println!("规则：只有最终消费者才该 collect；中间步骤一律 lazy\n");
    }
}

// ============================================================================
// 陷阱 3：迭代时修改容器
// ============================================================================
/// **现象**：编译错误 `cannot borrow `v` as mutable because it is also borrowed`.
/// **根因**：`iter()` 持有不可变借用，与 `push` 的可变借用互斥。
/// **修法**：先收集变更到临时 Vec，迭代结束后批量应用；或用索引循环。
pub mod mutate_during_iter {
    pub fn demonstrate() {
        println!("## 陷阱 3：迭代时修改容器");

        let mut v = vec![1, 2, 3];

        // ❌ 编译错误（注释里展示）：
        // for &x in v.iter() {
        //     if x == 2 { v.push(99); }
        //     // ^^ 第二个可变借用与上面的不可变借用冲突
        // }

        // ✅ 修法 A：先收集要追加的元素，再 extend
        let to_add: Vec<i32> = v.iter().filter(|&&x| x == 2).map(|&x| x + 97).collect();
        v.extend(to_add);

        // ✅ 修法 B：drain_filter / retain 这类「内省式」修改 API
        v.retain(|&x| x != 1); // 删除所有 1

        println!("迭代时修改后的 v = {:?}", v);
        println!("规则：把『读取 + 修改』拆成两阶段；或用 retain/drain\n");
    }
}

// ============================================================================
// 陷阱 4：clone 偷偷分配（cloned vs copied vs by-ref）
// ============================================================================
/// **现象**：在 String/Vec 流上做 `.cloned()` 让吞吐量崩盘。
/// **根因**：`cloned()` 对每个元素调 `Clone::clone`；对 String/Vec 是 O(n) 内存。
/// **修法**：能 `&T` 就别要 `T`；纯 Copy 类型用 `copied()` 表达意图（更易懂）。
pub mod clone_in_chain {
    pub fn demonstrate() {
        println!("## 陷阱 4：cloned() vs copied() vs &T");

        let symbols: Vec<String> = vec!["BTC".into(), "ETH".into(), "SOL".into()];

        // ❌ 无谓 clone：每个 String 都堆分配一份
        let _bad: Vec<String> = symbols.iter().cloned().collect();

        // ✅ 改用借用：直接收集 &String / &str
        let refs: Vec<&str> = symbols.iter().map(|s| s.as_str()).collect();
        println!("零分配 refs: {:?}", refs);

        // 数字类型：用 copied 比 cloned 更显意图（编译器都能优化）
        let nums = vec![1u64, 2, 3];
        let s: u64 = nums.iter().copied().sum();
        println!("copied().sum() = {}", s);

        // 特别警告：闭包里 `move` 后调 clone() 是隐藏分配重灾区
        // 比如 `streams.iter().map(move |s| heavy.clone()).collect()`
        // 每次迭代都 clone heavy ——
        // 改成 `let heavy = &heavy; streams.iter().map(|s| ...)` 即可避免\n
        println!();
    }
}

// ============================================================================
// 陷阱 5：无界迭代器 + 用户输入 = DoS
// ============================================================================
/// **现象**：服务被一个请求打到 OOM 或 CPU 100%。
/// **根因**：`(0..n).collect()` 里 n 来自外部；`repeat()` / `cycle()` 没 take。
/// **修法**：永远对外部输入设上限 `take(MAX)`，并把 `collect` 加容量提示。
pub mod unbounded_dos {
    pub fn dangerous(user_n: u64) -> Vec<u64> {
        // ❌ user_n = u64::MAX 直接 OOM
        (0..user_n).collect()
    }

    pub const MAX_RESULTS: usize = 10_000;

    pub fn safe(user_n: u64) -> Vec<u64> {
        // ✅ 永远 take，永远预估容量
        let n = (user_n as usize).min(MAX_RESULTS);
        let mut out = Vec::with_capacity(n);
        out.extend((0..user_n).take(MAX_RESULTS));
        out
    }

    pub fn demonstrate() {
        println!("## 陷阱 5：无界迭代器 + 用户输入");
        let r = safe(u64::MAX);
        println!("即使 user_n = u64::MAX，也只返回 {} 条", r.len());
        println!("规则：信任边界处一定要 take(MAX); 永远不让用户控制集合大小\n");
    }
}

// ============================================================================
// 陷阱 6：count() vs len() —— 复杂度差千倍
// ============================================================================
/// **现象**：监控显示某条「计数」请求 P99 突然飙升。
/// **根因**：用 `iter().filter(...).count()` 替代了 `len()`；前者 O(n) 全遍历。
/// **修法**：能 `len()` 就别 `count()`；分桶维护增量计数。
pub mod count_vs_len {
    pub fn demonstrate() {
        println!("## 陷阱 6：count() 是 O(n)，len() 是 O(1)");
        let v: Vec<u64> = (0..1_000_000).collect();

        // O(1)：ExactSizeIterator 直接读 self.len
        let _a = v.len();
        let _b = v.iter().len(); // 也是 O(1)，因为 slice::Iter 实现了 ExactSizeIterator

        // O(n)：count 必须真的 next 到底
        let _c = v.iter().count();

        // 在 filter 之后 ExactSize 丢失，count 是唯一选择 —— 此时要意识到代价
        let _d = v.iter().filter(|&&x| x % 2 == 0).count();

        println!("看到 .count() 时永远问一遍：『能不能改成 len() 或维护增量？』\n");
    }
}

// ============================================================================
// 陷阱 7：consume-by-value 的迭代器只能用一次
// ============================================================================
/// **现象**：`use of moved value` 编译错误，或 lazy 流第二次 `for` 啥也没有。
/// **根因**：`Iterator` 的 `next` 取 `&mut self`；很多适配器是 by-value 构造。
/// **修法**：clone 源容器、`peekable` 缓存、或重新构造迭代器。
pub mod single_shot {
    pub fn demonstrate() {
        println!("## 陷阱 7：迭代器是 one-shot");

        let v = vec![1, 2, 3];

        // ❌ 演示：以下两次 for 等价，但如果中间是 `into_iter()` 就编译错
        // let it = v.into_iter();
        // for x in it { ... } // 第一次消耗了 v
        // for x in it { ... } // ❌ use of moved value

        // ✅ 修法 A：每次重建迭代器
        let sum1: i32 = v.iter().sum();
        let max1 = v.iter().max();

        // ✅ 修法 B：先 collect 到 Vec，再多次 iter
        let cached: Vec<_> = (0..5).map(|x| x * x).collect();
        let _a: i32 = cached.iter().sum();
        let _b: i32 = cached.iter().product();

        println!("sum = {}, max = {:?}", sum1, max1);
        println!("规则：迭代器 ≈ generator；要复用必须物化\n");
    }
}

// ============================================================================
// 陷阱 8：collect::<Result<Vec<_>, _>>() 的双重语义
// ============================================================================
/// **现象**：原本期望「错误就停」的逻辑，结果错了一项也继续跑。
/// **根因**：`collect()` 的 `FromIterator<Result<T, E>>` 实现 *会* 短路；
/// 但 `collect::<Vec<Result<T, E>>>()` 不会 —— 全收集。两者只差一个类型注解。
/// **修法**：明确写出 turbofish 或类型注解，并加单元测试覆盖错误路径。
pub mod collect_result_ambiguity {
    pub fn demonstrate() {
        println!("## 陷阱 8：collect::<Result<Vec<_>, _>>() 的双重语义");

        let raw = vec!["1", "2", "oops", "4"];

        // 写法 A：短路 —— 在 "oops" 处停下，整体返回 Err
        let short_circuit: Result<Vec<i32>, _> = raw.iter().map(|s| s.parse::<i32>()).collect();
        println!("短路语义: {:?}", short_circuit);

        // 写法 B：全收集 —— 每个元素是独立的 Result
        let all: Vec<Result<i32, _>> = raw.iter().map(|s| s.parse::<i32>()).collect();
        let ok_count = all.iter().filter(|r| r.is_ok()).count();
        println!("全收集后 ok = {} / {}", ok_count, all.len());

        println!("规则：写 Result 流时，类型注解不可省 —— 它是控制流的一部分\n");
    }
}

pub fn demonstrate() {
    forgot_consumer::demonstrate();
    hidden_alloc::demonstrate();
    mutate_during_iter::demonstrate();
    clone_in_chain::demonstrate();
    unbounded_dos::demonstrate();
    count_vs_len::demonstrate();
    single_shot::demonstrate();
    collect_result_ambiguity::demonstrate();
}
