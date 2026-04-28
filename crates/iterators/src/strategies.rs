//! # 泛化：从 HFT/Web3 场景到通用应对策略
//!
//! 把前两章具体业务里的迭代器套路抽象出来，得到一张
//! **「问题类型 → 推荐套路」决策矩阵**：
//!
//! | 问题类型           | 标志特征                  | 首选套路                          |
//! |--------------------|---------------------------|-----------------------------------|
//! | 1. 流式 ETL        | 过滤 + 变换 + 解码        | `filter_map` 链 + 终端 `for_each` |
//! | 2. 状态聚合        | 累加器、增量更新          | `fold` / `reduce` / `try_fold`    |
//! | 3. 顺序状态机      | 上一步影响下一步          | `scan` / 自定义 Iterator          |
//! | 4. 窗口 / 分组     | 按 size 或 key 分桶       | `chunks_exact` / `peekable + key` |
//! | 5. 多源归并        | 多个有序流合一            | k-way merge（Vec<Peekable>）      |
//! | 6. 错误短路        | 任意元素失败即放弃        | `try_fold` / `collect::<Result>`  |
//! | 7. 容量 / 资源约束 | 累计变量超阈值即停        | `scan` 返回 `None` / `take_while` |
//! | 8. 副作用 + 状态   | 一边过滤一边记录          | `filter` 闭包内修改外部 `&mut`    |
//!
//! 下面 8 个策略各有一个 *通用模板函数*，签名上不带任何业务名词，
//! 任何项目都可以直接抄走当工具函数。

#![allow(dead_code)]

// ============================================================================
// 策略 1：流式 ETL —— filter_map 链
// ============================================================================
/// 问题：对一个流做「过滤 + 变换 + 解码」。
/// 模式：用 `filter_map` 一次性表达「丢掉它 / 保留并变换它」。
/// 与 `filter().map().filter()` 的区别：
///   - filter_map 只调用一次闭包，便于重用昂贵的解析中间结果
///   - 编译器更容易把整条链 inline 成一个循环
///
/// HFT: 见 hft::zero_alloc_parser（解析 + 价格过滤合并）
/// Web3: 见 web3::erc20_scanner（topic 过滤 + ABI 解码合并）
pub mod streaming_etl {
    /// 通用：把任意 `&[u8]` 切成定长 chunk，对每块运行可能失败的 decoder。
    /// 失败的 chunk 被静默丢弃 —— 调用方决定是否需要 logging。
    pub fn parse_records<'a, T, F>(
        buf: &'a [u8],
        record_size: usize,
        mut decode: F,
    ) -> impl Iterator<Item = T> + 'a
    where
        F: FnMut(&[u8]) -> Option<T> + 'a,
    {
        buf.chunks_exact(record_size).filter_map(move |c| decode(c))
    }

    pub fn demonstrate() {
        println!("## 策略 1：流式 ETL（filter_map 链）");
        let buf: Vec<u8> = (0u8..32).collect();
        let evens: Vec<u32> = parse_records(&buf, 4, |c| {
            let v = u32::from_le_bytes(c.try_into().ok()?);
            (v % 2 == 0).then_some(v)
        })
        .collect();
        println!("解码 + 过滤后 = {:?}\n", evens);
    }
}

// ============================================================================
// 策略 2：状态聚合 —— fold / reduce
// ============================================================================
/// 问题：把一串数据归约成一个累加器值。
/// 模式：`fold(init, |acc, x| ...)`。比起 mut 循环：
///   - 累加器初值显式 → 更难写错初始化
///   - `fold` 表达式有返回值 → 可以直接赋值到 `let`
///   - LLVM 容易识别为 reduce，自动 SIMD
///
/// HFT: 见 hft::latency_histogram（buckets fold）
/// Web3: Merkle 每一层都是一次 fold（chunks(2).map.collect 是 fold 的特例）
pub mod stateful_reduce {
    /// 通用：求 (min, max, sum, count) 一次遍历完成。
    pub fn stats<I: IntoIterator<Item = i64>>(it: I) -> (i64, i64, i64, u64) {
        it.into_iter().fold(
            (i64::MAX, i64::MIN, 0i64, 0u64),
            |(mn, mx, s, c), x| (mn.min(x), mx.max(x), s + x, c + 1),
        )
    }

    pub fn demonstrate() {
        println!("## 策略 2：fold 一次遍历多指标");
        let r = stats([3i64, -1, 7, 2, 5]);
        println!("(min,max,sum,count) = {:?}", r);
        println!("一次循环 4 个指标：编译器会把它们融合成一条 SIMD 友好的 reduce\n");
    }
}

// ============================================================================
// 策略 3：顺序状态机 —— scan
// ============================================================================
/// 问题：当前输出依赖之前所有输入（运行总和、累计 PnL、状态演进）。
/// 模式：`scan(init, |state, x| Some(emit))`。返回 `None` 即终止流。
/// 与 fold 的区别：scan **每步都产出**（流），fold 只产出最终累加器。
///
/// HFT: 见 hft::vwap_rolling（增量价量加权）
/// Web3: 见 web3::sandwich_sim（pool 状态演进）、web3::mempool_pack（累计 gas）
pub mod ordered_state_machine {
    /// 通用：对任意数值流求「运行最大值」（类似 max-so-far）。
    pub fn running_max<I: IntoIterator<Item = i64>>(it: I) -> impl Iterator<Item = i64> {
        it.into_iter().scan(i64::MIN, |best, x| {
            if x > *best {
                *best = x;
            }
            Some(*best)
        })
    }

    pub fn demonstrate() {
        println!("## 策略 3：scan 表达运行总量");
        let v: Vec<_> = running_max([3i64, 1, 4, 1, 5, 9, 2, 6]).collect();
        println!("running_max = {:?}", v);
        println!("scan 是 stateful map；状态藏在闭包外的累加器里\n");
    }
}

// ============================================================================
// 策略 4：窗口 / 分组 —— chunks_exact / peekable + key
// ============================================================================
/// 问题：把流按固定 size 或动态 key 切成段。
/// 模式：
///   - 固定大小：`slice::chunks_exact(N)` —— 0 分配，SIMD 友好
///   - 动态 key（time bucket / contiguous group）：自定义 Iterator + peekable
///
/// HFT: 见 hft::ohlcv_bars（按时间桶分组）
/// Web3: 同样的 pattern 用在按 block_number 分组事件
pub mod windowing {
    /// 通用：对一个 *按 key 升序* 的流，把相邻同 key 的元素聚到一起。
    /// 常用于：按区块号聚合事件、按订单 id 聚合 fills、按用户聚合操作。
    pub struct GroupBy<I: Iterator, K: Eq, F: FnMut(&I::Item) -> K> {
        inner: std::iter::Peekable<I>,
        keyfn: F,
    }

    impl<I: Iterator, K: Eq, F: FnMut(&I::Item) -> K> GroupBy<I, K, F> {
        pub fn new(it: I, keyfn: F) -> Self {
            Self { inner: it.peekable(), keyfn }
        }
    }

    impl<I: Iterator, K: Eq, F: FnMut(&I::Item) -> K> Iterator for GroupBy<I, K, F> {
        type Item = (K, Vec<I::Item>);
        fn next(&mut self) -> Option<Self::Item> {
            let first = self.inner.next()?;
            let key = (self.keyfn)(&first);
            let mut group = vec![first];
            while let Some(next_item) = self.inner.peek() {
                if (self.keyfn)(next_item) == key {
                    group.push(self.inner.next().unwrap());
                } else {
                    break;
                }
            }
            Some((key, group))
        }
    }

    pub fn demonstrate() {
        println!("## 策略 4：peekable + key 实现流式 group_by");
        let events = vec![(1u64, "a"), (1, "b"), (2, "c"), (3, "d"), (3, "e")];
        for (block, items) in GroupBy::new(events.into_iter(), |(b, _)| *b) {
            println!("  block {} → {} 个事件", block, items.len());
        }
        println!("注意：要求输入按 key 升序；乱序请先 sort_by_key 或换 HashMap 累加\n");
    }
}

// ============================================================================
// 策略 5：多源归并 —— k-way merge
// ============================================================================
/// 问题：N 个有序流合成一个有序流（多链区块、多对手簿订单、多 shard 日志）。
/// 模式：`Vec<Peekable<I>>`，每次输出 peek 最小那一路。
/// 当 N 很大（>16）时换成 BinaryHeap<Peek<Iter>> 来 O(log N) 选最小。
///
/// HFT: 多 venue 行情按时间合并
/// Web3: 见 web3::multichain_merge
pub mod kway_merge {
    use std::iter::Peekable;

    pub struct KMerge<T: Ord, I: Iterator<Item = T>> {
        streams: Vec<Peekable<I>>,
    }

    impl<T: Ord, I: Iterator<Item = T>> KMerge<T, I> {
        pub fn new<S: IntoIterator<Item = I>>(streams: S) -> Self {
            Self { streams: streams.into_iter().map(|s| s.peekable()).collect() }
        }
    }

    impl<T: Ord, I: Iterator<Item = T>> Iterator for KMerge<T, I> {
        type Item = T;
        fn next(&mut self) -> Option<T> {
            let (idx, _) = self
                .streams
                .iter_mut()
                .enumerate()
                .filter_map(|(i, s)| s.peek().map(|v| (i, v)))
                .min_by(|a, b| a.1.cmp(b.1))?;
            self.streams[idx].next()
        }
    }

    pub fn demonstrate() {
        println!("## 策略 5：通用 k-way merge");
        let a = vec![1, 4, 7];
        let b = vec![2, 5, 8];
        let c = vec![3, 6, 9, 10];
        let merged: Vec<_> =
            KMerge::new(vec![a.into_iter(), b.into_iter(), c.into_iter()]).collect();
        println!("merged = {:?}", merged);
        println!("N>16 路时改用 BinaryHeap 把 next 从 O(N) 降到 O(log N)\n");
    }
}

// ============================================================================
// 策略 6：错误短路 —— try_fold / collect::<Result>
// ============================================================================
/// 问题：链式校验/解析，任意一项失败就放弃整体。
///
/// 模式：
///   - 累加结果但短路：`try_fold(init, |acc, x| -> Result<_, _>)`
///   - 收集 Vec 但短路：`collect::<Result<Vec<_>, _>>()`
///
/// 关键：try_fold 是 Rust 标准库里实现 ?-传播的核心机制。
///
/// HFT: 见 hft::pretrade_risk（多级风控链）
/// Web3: 多步签名/解码任一失败即整条 tx 拒绝
pub mod error_shortcircuit {
    /// 通用：对一串可失败操作累加，遇到 Err 立即返回。
    pub fn checked_sum<I: IntoIterator<Item = Result<u64, &'static str>>>(
        it: I,
    ) -> Result<u64, &'static str> {
        it.into_iter().try_fold(0u64, |acc, x| Ok(acc + x?))
    }

    pub fn demonstrate() {
        println!("## 策略 6：try_fold 错误短路");
        let ok = vec![Ok(1u64), Ok(2), Ok(3)];
        let bad = vec![Ok(1u64), Err("boom"), Ok(99999)];
        println!("ok  → {:?}", checked_sum(ok));
        println!("bad → {:?}（99999 永远不会被加）", checked_sum(bad));
        println!();
    }
}

// ============================================================================
// 策略 7：容量约束 —— scan + None 终止
// ============================================================================
/// 问题：「累加到某阈值就停」（block gas、batch byte size、内存预算）。
/// 模式：scan 把累计值物化到流，超限则返回 `None` 终止。
/// 比 take_while 强：take_while 拿不到累加器，而 scan 可以。
///
/// HFT: 累计 notional / 持仓金额触顶停止下单
/// Web3: 见 web3::mempool_pack（累计 gas 装块）
pub mod budgeted_take {
    /// 通用：取至总 cost 不超过 budget 为止。
    pub fn take_within_budget<T, I, F>(it: I, budget: u64, mut cost: F) -> impl Iterator<Item = T>
    where
        I: IntoIterator<Item = T>,
        F: FnMut(&T) -> u64,
    {
        it.into_iter().scan(0u64, move |used, x| {
            let c = cost(&x);
            if *used + c > budget {
                None
            } else {
                *used += c;
                Some(x)
            }
        })
    }

    pub fn demonstrate() {
        println!("## 策略 7：预算约束的 take");
        // 比如：每条消息有 size，要装到 1KB 的 packet
        let msgs = vec![("a", 300u64), ("b", 400), ("c", 500), ("d", 100)];
        let packed: Vec<_> = take_within_budget(msgs, 1000, |&(_, s)| s).collect();
        println!("装入 {} 条（其中 c=500 超限被丢，但 d 因为已经 None 终止也没机会）", packed.len());
        println!("注意：这是『首次超限即停』；如果想跳过超限继续找小的，要换 filter\n");
    }
}

// ============================================================================
// 策略 8：副作用 + 状态 —— filter 内修改 &mut
// ============================================================================
/// 问题：一边迭代一边把元素登记到外部数据结构（去重、统计、缓存）。
/// 模式：`filter(move |x| { side_effect(state, x); decision })` —— Rust
/// 的可变借用规则保证只有迭代器活着时 state 不会被别人动。
///
/// HFT: 价格变动时同步更新内部簿
/// Web3: 见 web3::bloom_dedupe（去重 + 写入 bloom）
pub mod side_effect_filter {
    use std::collections::HashSet;

    /// 通用：去重，保持出现顺序。比 collect → HashSet → Vec 更省一次重排。
    pub fn dedup_in_order<T, I>(it: I) -> impl Iterator<Item = T>
    where
        T: std::hash::Hash + Eq + Clone,
        I: IntoIterator<Item = T>,
    {
        let mut seen: HashSet<T> = HashSet::new();
        it.into_iter().filter(move |x| seen.insert(x.clone()))
    }

    pub fn demonstrate() {
        println!("## 策略 8：filter 内 &mut state（在线去重）");
        let v: Vec<i32> = dedup_in_order([1, 2, 2, 3, 1, 4, 3]).collect();
        println!("去重保序: {:?}", v);
        println!("HashSet::insert 返回 bool，正好作为 filter 的判定 + 副作用\n");
    }
}

// ============================================================================
// 反向：什么时候 *不要* 用迭代器
// ============================================================================
/// 不是所有场景都该写 fluent chain。明确的反例：
///
/// 1. **复杂的 break / continue / 多重 return**：写出来比 for 还难读，
///    while 循环 + 显式 break 反而更清楚。
/// 2. **协调多个 mut 状态**：迭代器闭包只能借一个，超出就要么 Cell 要么 for。
/// 3. **大量 try_fold + ? 嵌套**：阅读者要倒着读，可读性差，用 for + ? 即可。
/// 4. **需要在迭代过程中拿索引、上一项、下一项的复杂关系**：peekable +
///    手写 while 比 windows + slice 更直接。
/// 5. **异步流式**：`Iterator` 不支持 await；要用 `futures::Stream` /
///    `async fn next`，否则会被迫 `block_on` 把 async 退化成 sync。
pub mod when_not_to_use {
    pub fn demonstrate() {
        println!("## 反例：什么时候不要用迭代器");
        println!("  - 复杂控制流（多重 break / 早返回）→ 写 for");
        println!("  - 多个 &mut 状态需要协调 → 写 for");
        println!("  - try_fold 嵌套读不懂 → 写 for + ? 操作符");
        println!("  - 异步流 → 用 futures::Stream，不要 block_on");
        println!("  - 算法需要回退 / 重置 → 写循环 + 索引\n");
    }
}

pub fn demonstrate() {
    streaming_etl::demonstrate();
    stateful_reduce::demonstrate();
    ordered_state_machine::demonstrate();
    windowing::demonstrate();
    kway_merge::demonstrate();
    error_shortcircuit::demonstrate();
    budgeted_take::demonstrate();
    side_effect_filter::demonstrate();
    when_not_to_use::demonstrate();
}
