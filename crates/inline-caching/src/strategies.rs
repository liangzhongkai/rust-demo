//! # 泛化：从 HFT/Web3 场景到通用 Inline Cache 策略
//!
//! 把前两章具体业务里的 IC 套路抽象成决策矩阵：
//!
//! | 问题类型              | 标志特征                          | 首选套路                              |
//! |-----------------------|-----------------------------------|---------------------------------------|
//! | 1. 单 key 粘滞        | 同 id 连打 >90%                   | 单态 IC（last_key + value）           |
//! | 2. 少量交替 shape     | 2~4 种局部交替                    | 多态 IC（PIC，MRU 小表）              |
//! | 3. 形状爆炸           | 调用点 >N 种 shape                | Megamorphic 字典 / 拆分调用点         |
//! | 4. 可失效配置         | 热更新、升级、reorg               | Guard = identity + generation         |
//! | 5. 多线程热路径       | 每核独立消费                      | 线程局部 IC + 共享只读表              |
//! | 6. 编译期已确定       | 部署后一种实现                    | 静态分发，不用 IC                     |
//! | 7. 不知有无局部性     | 新服务、无画像                    | 先计数 locality，再决定是否加 IC      |
//! | 8. 正确性优先         | 资损/错账风险                     | 命中仍比 guard；写路径必失效          |
//!
//! 下面 8 个策略各有一个 *通用模板*，签名上不带业务名词。

#![allow(dead_code)]

// ============================================================================
// 策略 1：单态 IC —— last key 特化
// ============================================================================
/// 问题：完整查找昂贵，且 key 高度粘滞。
/// 模式：`Option<(K, V)>` 调用点缓存；相等则返回 V。
///
/// HFT: hft::symbol_meta_ic
/// Web3: web3::token_meta_ic
pub mod mono_last_key {
    #[derive(Clone, Copy)]
    pub struct MonoCache<K, V> {
        slot: Option<(K, V)>,
        pub hits: u64,
        pub misses: u64,
    }

    impl<K: PartialEq + Copy, V: Copy> MonoCache<K, V> {
        pub fn new() -> Self {
            Self {
                slot: None,
                hits: 0,
                misses: 0,
            }
        }

        pub fn get_or_insert(&mut self, key: K, resolve: impl FnOnce() -> V) -> V {
            if let Some((k, v)) = self.slot {
                if k == key {
                    self.hits += 1;
                    return v;
                }
            }
            self.misses += 1;
            let v = resolve();
            self.slot = Some((key, v));
            v
        }

        pub fn invalidate(&mut self) {
            self.slot = None;
        }
    }

    pub fn demonstrate() {
        println!("## 策略 1：单态 last-key IC");
        let mut c = MonoCache::new();
        for _ in 0..10 {
            let _ = c.get_or_insert(7u32, || 100u32);
        }
        let value = c.get_or_insert(7, || 100);
        println!("hits={} value={}\n", c.hits, value);
    }
}

// ============================================================================
// 策略 2：多态 PIC —— 小 N MRU
// ============================================================================
/// 问题：少量 shape 交替，单态来回 thrash。
/// 模式：`[Option<(K,V)>; N]` + 命中提升到槽 0。
///
/// HFT: hft::book_update_pic
pub mod poly_mru {
    pub struct PolyCache<K, V, const N: usize> {
        pub(super) slots: [Option<(K, V)>; N],
        pub(super) len: usize,
        pub hits: u64,
        pub misses: u64,
    }

    impl<K: PartialEq + Copy, V: Copy, const N: usize> PolyCache<K, V, N> {
        pub fn new() -> Self {
            Self {
                slots: [None; N],
                len: 0,
                hits: 0,
                misses: 0,
            }
        }

        pub fn get_or_insert(&mut self, key: K, resolve: impl FnOnce() -> V) -> V {
            for i in 0..self.len {
                if let Some((k, v)) = self.slots[i] {
                    if k == key {
                        self.hits += 1;
                        if i > 0 {
                            self.slots.swap(0, i);
                        }
                        return v;
                    }
                }
            }
            self.misses += 1;
            let v = resolve();
            if self.len < N {
                self.slots[self.len] = Some((key, v));
                self.len += 1;
            } else {
                self.slots[N - 1] = Some((key, v));
            }
            v
        }
    }

    pub fn demonstrate() {
        println!("## 策略 2：PIC MRU（N=2）");
        let mut c: PolyCache<u8, u16, 2> = PolyCache::new();
        for k in [1u8, 2, 1, 2, 1] {
            let _ = c.get_or_insert(k, || k as u16 * 10);
        }
        println!("hits={} misses={}\n", c.hits, c.misses);
    }
}

// ============================================================================
// 策略 3：Megamorphic 回退 —— 诚实的字典
// ============================================================================
/// 问题：shape 过多，PIC 命中率崩。
/// 模式：检测 miss 风暴 → 切换 HashMap；或编译期拆调用点。
pub mod mega_fallback {
    use std::collections::HashMap;

    pub enum CacheFront<K, V, const N: usize> {
        Poly(super::poly_mru::PolyCache<K, V, N>),
        Dict(HashMap<K, V>),
    }

    impl<K: PartialEq + Eq + Copy + std::hash::Hash, V: Copy, const N: usize> CacheFront<K, V, N> {
        pub fn new() -> Self {
            Self::Poly(super::poly_mru::PolyCache::new())
        }

        pub fn get_or_insert(&mut self, key: K, resolve: impl FnOnce() -> V) -> V {
            match self {
                Self::Poly(p) => {
                    let v = p.get_or_insert(key, resolve);
                    // 简易启发式：miss 多且已满 → 升级
                    if p.misses > 8 && p.len == N {
                        let mut map = HashMap::new();
                        for i in 0..p.len {
                            if let Some((k, val)) = p.slots[i] {
                                map.insert(k, val);
                            }
                        }
                        map.insert(key, v);
                        *self = Self::Dict(map);
                    }
                    v
                }
                Self::Dict(map) => {
                    if let Some(v) = map.get(&key) {
                        *v
                    } else {
                        let v = resolve();
                        map.insert(key, v);
                        v
                    }
                }
            }
        }
    }

    pub fn demonstrate() {
        println!("## 策略 3：PIC → 字典升级");
        let mut c: CacheFront<u32, u32, 2> = CacheFront::new();
        for k in 0..12u32 {
            let _ = c.get_or_insert(k, || k * 3);
        }
        let kind = match &c {
            CacheFront::Poly(_) => "poly",
            CacheFront::Dict(_) => "dict",
        };
        println!("升级后前端 = {}\n", kind);
    }
}

// ============================================================================
// 策略 4：Generation Guard —— 可失效
// ============================================================================
/// 问题：缓存值会因外部写而过期。
/// 模式：`guard = (key, gen)`；写路径 `gen += 1` 或清槽。
///
/// HFT: hft::risk_limits_ic
/// Web3: web3::storage_layout_ic
pub mod generation_guard {
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Guard<K> {
        pub key: K,
        pub gen: u64,
    }

    pub struct Generational<K, V> {
        slot: Option<(Guard<K>, V)>,
    }

    impl<K: PartialEq + Copy, V: Copy> Generational<K, V> {
        pub fn new() -> Self {
            Self { slot: None }
        }

        pub fn get(&mut self, g: Guard<K>, resolve: impl FnOnce() -> V) -> V {
            if let Some((og, v)) = self.slot {
                if og == g {
                    return v;
                }
            }
            let v = resolve();
            self.slot = Some((g, v));
            v
        }
    }

    pub fn demonstrate() {
        println!("## 策略 4：Generation Guard");
        let mut c = Generational::new();
        let _ = c.get(Guard { key: 1u32, gen: 1 }, || 10u32);
        let v = c.get(Guard { key: 1u32, gen: 2 }, || 20u32);
        println!("gen bump → value={}\n", v);
    }
}

// ============================================================================
// 策略 5：线程局部 IC
// ============================================================================
/// 问题：多线程共享可变槽 → 竞争 / 伪共享。
/// 模式：`thread_local!` 存 MonoCache；只读权威表用 Arc。
///
/// 见 pitfalls::shared_mutable_ic
pub mod thread_local_ic {
    use super::mono_last_key::MonoCache;
    use std::cell::RefCell;

    thread_local! {
        static IC: RefCell<MonoCache<u32, u32>> = RefCell::new(MonoCache::new());
    }

    pub fn lookup(key: u32, resolve: impl FnOnce() -> u32) -> u32 {
        IC.with(|c| c.borrow_mut().get_or_insert(key, resolve))
    }

    pub fn demonstrate() {
        println!("## 策略 5：线程局部 IC");
        for _ in 0..5 {
            let _ = lookup(9, || 99);
        }
        println!("value={}\n", lookup(9, || 99));
    }
}

// ============================================================================
// 策略 6：能静态则静态
// ============================================================================
/// 问题：运行时 IC 有 miss 与失效复杂度。
/// 模式：类型参数 / enum 穷尽 → 编译期单态，零 miss。
pub mod prefer_static {
    pub trait Handler {
        fn name() -> &'static str;
    }

    pub struct VenueA;
    pub struct VenueB;

    impl Handler for VenueA {
        fn name() -> &'static str {
            "A"
        }
    }
    impl Handler for VenueB {
        fn name() -> &'static str {
            "B"
        }
    }

    pub fn run<H: Handler>() {
        let _ = H::name();
    }

    pub fn demonstrate() {
        println!("## 策略 6：编译期已知 → 静态分发");
        run::<VenueA>();
        println!("Handler::name = {}（无 IC，无 miss）\n", VenueA::name());
    }
}

// ============================================================================
// 策略 7：先量 locality 再加 IC
// ============================================================================
/// 问题：不知调用点是否值得特化。
/// 模式：环形计数「与上次 key 相同？」→ 估计粘滞率。
pub mod measure_locality {
    pub struct LocalityProbe<K> {
        last: Option<K>,
        same: u64,
        total: u64,
    }

    impl<K: PartialEq + Copy> LocalityProbe<K> {
        pub fn new() -> Self {
            Self {
                last: None,
                same: 0,
                total: 0,
            }
        }

        pub fn observe(&mut self, key: K) {
            self.total += 1;
            if self.last == Some(key) {
                self.same += 1;
            }
            self.last = Some(key);
        }

        pub fn stickiness(&self) -> f64 {
            if self.total == 0 {
                0.0
            } else {
                self.same as f64 / self.total as f64
            }
        }
    }

    pub fn demonstrate() {
        println!("## 策略 7：先量粘滞率");
        let mut p = LocalityProbe::new();
        for k in [1, 1, 1, 2, 1, 1, 1, 1] {
            p.observe(k);
        }
        println!(
            "stickiness={:.0}% —— >80% 才优先考虑单态 IC\n",
            p.stickiness() * 100.0
        );
    }
}

// ============================================================================
// 策略 8：正确性护栏
// ============================================================================
/// 问题：IC 出错比慢更致命。
/// 模式：清单化——写失效、guard 完备、命中率监控、debug 对照全量查找。
pub mod correctness_rails {
    pub fn demonstrate() {
        println!("## 策略 8：正确性护栏清单");
        println!("  1. 每个写路径有对应 invalidate / gen bump");
        println!("  2. guard 覆盖所有失效维度（身份 + 世代 + 可选 TTL）");
        println!("  3. metrics: hits, misses, hit_rate, invalidate_count");
        println!("  4. debug/test: IC 路径与慢路径结果 assert 相等");
        println!("  5. 资损级路径：考虑 shadow 抽样比对\n");
    }
}

// ============================================================================
// 反向：什么时候 *不要* 用 IC
// ============================================================================
pub mod when_not_to_use {
    pub fn demonstrate() {
        println!("## 反例：什么时候不要用 Inline Cache");
        println!("  - 粘滞率低 / shape 均匀随机");
        println!("  - 查找本身已是 O(1) 且极便宜（直接数组下标）");
        println!("  - 编译期可静态分发");
        println!("  - 无法设计正确失效（宁可不缓存）");
        println!("  - 冷路径、管理 API、一次性脚本\n");
    }
}

pub fn demonstrate() {
    mono_last_key::demonstrate();
    poly_mru::demonstrate();
    mega_fallback::demonstrate();
    generation_guard::demonstrate();
    thread_local_ic::demonstrate();
    prefer_static::demonstrate();
    measure_locality::demonstrate();
    correctness_rails::demonstrate();
    when_not_to_use::demonstrate();
}
