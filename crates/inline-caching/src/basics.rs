//! # Inline Cache 底层机制
//!
//! 源自虚拟机（Self / HotSpot / V8）的 **调用点特化缓存**：在「查属性 /
//! 选实现 / 解布局」的 *调用点* 旁缓存上次成功路径，下次用廉价的
//! **shape / guard** 校验命中后直达偏移或函数指针，避开完整查表。
//!
//! 状态机（从快到慢）：
//!
//! ```text
//! Uninitialized → Monomorphic → Polymorphic(N) → Megamorphic
//! ```
//!
//! Rust 热路径里没有 JIT 自动插桩，但同一思想处处可见：
//! - 上次 symbol 的 tick size / 风控限额
//! - 上次 ABI selector 的解码器
//! - 上次 venue 的消息 handler
//!
//! 本章用最小可运行模型演示 IC 状态迁移与命中统计。

#![allow(dead_code)]

/// 对象「外形」—— IC 的 guard key。VM 里叫 Map/HiddenClass；系统里常是
/// 协议版本、合约 storage layout 版本、instrument 身份等。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShapeId(pub u32);

/// 调用点旁的缓存槽：命中则跳过完整查找。
#[derive(Debug, Clone, Copy)]
pub struct IcSlot {
    pub shape: ShapeId,
    /// 缓存的「解析结果」：偏移、handler 索引、decimals 等。
    pub cached: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IcState {
    Uninit,
    Mono,
    Poly,
    Mega,
}

/// 单态 IC：只记住一种 shape。HFT/Web3 最常见——热路径几乎不变。
pub struct MonoIc {
    slot: Option<IcSlot>,
    pub hits: u64,
    pub misses: u64,
}

impl MonoIc {
    pub fn new() -> Self {
        Self {
            slot: None,
            hits: 0,
            misses: 0,
        }
    }

    pub fn state(&self) -> IcState {
        if self.slot.is_some() {
            IcState::Mono
        } else {
            IcState::Uninit
        }
    }

    /// 命中：shape 一致 → 直接返回缓存值；否则 miss 并更新槽。
    pub fn lookup(&mut self, shape: ShapeId, resolve: impl FnOnce() -> u32) -> u32 {
        if let Some(slot) = self.slot {
            if slot.shape == shape {
                self.hits += 1;
                return slot.cached;
            }
        }
        self.misses += 1;
        let cached = resolve();
        self.slot = Some(IcSlot { shape, cached });
        cached
    }

    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

/// 多态 IC（PIC）：少量 shape 用线性小表；超出阈值升级 megamorphic。
pub struct PolyIc<const N: usize> {
    slots: [Option<IcSlot>; N],
    len: usize,
    mega: bool,
    /// megamorphic 回退字典（示意：生产里用 HashMap / perfect hash）。
    dict: Vec<(ShapeId, u32)>,
    pub hits: u64,
    pub misses: u64,
    pub mega_lookups: u64,
}

impl<const N: usize> PolyIc<N> {
    pub fn new() -> Self {
        Self {
            slots: [None; N],
            len: 0,
            mega: false,
            dict: Vec::new(),
            hits: 0,
            misses: 0,
            mega_lookups: 0,
        }
    }

    pub fn state(&self) -> IcState {
        if self.mega {
            IcState::Mega
        } else if self.len == 0 {
            IcState::Uninit
        } else if self.len == 1 {
            IcState::Mono
        } else {
            IcState::Poly
        }
    }

    pub fn lookup(&mut self, shape: ShapeId, resolve: impl FnOnce() -> u32) -> u32 {
        if self.mega {
            self.mega_lookups += 1;
            if let Some((_, v)) = self.dict.iter().find(|(s, _)| *s == shape) {
                self.hits += 1;
                return *v;
            }
            self.misses += 1;
            let v = resolve();
            self.dict.push((shape, v));
            return v;
        }

        for i in 0..self.len {
            if let Some(slot) = self.slots[i] {
                if slot.shape == shape {
                    self.hits += 1;
                    // 简单 MRU：命中槽挪到前部，利于分支预测。
                    if i > 0 {
                        self.slots.swap(0, i);
                    }
                    return slot.cached;
                }
            }
        }

        self.misses += 1;
        let cached = resolve();
        if self.len < N {
            self.slots[self.len] = Some(IcSlot { shape, cached });
            self.len += 1;
        } else {
            // 升级 megamorphic：把现有槽倒进字典。
            self.mega = true;
            for i in 0..self.len {
                if let Some(slot) = self.slots[i] {
                    self.dict.push((slot.shape, slot.cached));
                }
            }
            self.dict.push((shape, cached));
        }
        cached
    }
}

// ============================================================================
// 演示
// ============================================================================

pub mod mono_demo {
    use super::*;

    pub fn demonstrate() {
        println!("## Basics-1：单态 Inline Cache");

        let mut ic = MonoIc::new();
        // 模拟属性表：shape → field offset
        let resolve = |shape: ShapeId| match shape.0 {
            1 => 8u32,  // Price 在 offset 8
            2 => 16u32, // Qty 在 offset 16
            _ => 0,
        };

        for _ in 0..100 {
            let _ = ic.lookup(ShapeId(1), || resolve(ShapeId(1)));
        }
        // 偶尔换 shape → miss + 改写槽
        let _ = ic.lookup(ShapeId(2), || resolve(ShapeId(2)));
        let _ = ic.lookup(ShapeId(1), || resolve(ShapeId(1)));

        println!(
            "state={:?} hits={} misses={} hit_rate={:.1}%",
            ic.state(),
            ic.hits,
            ic.misses,
            ic.hit_rate() * 100.0
        );
        println!("规则：热路径 shape 稳定时，IC 把 O(表查找) 压成 1 次比较\n");
    }
}

pub mod poly_demo {
    use super::*;

    pub fn demonstrate() {
        println!("## Basics-2：多态 IC → Megamorphic 升级");

        let mut ic: PolyIc<2> = PolyIc::new();
        let resolve = |s: ShapeId| s.0 * 10;

        for shape in [1u32, 2, 1, 2, 1] {
            let _ = ic.lookup(ShapeId(shape), || resolve(ShapeId(shape)));
        }
        println!("两 shape 内：state={:?} hits={}", ic.state(), ic.hits);

        // 第三个 shape 触发 mega
        let _ = ic.lookup(ShapeId(3), || resolve(ShapeId(3)));
        println!(
            "第三 shape 后：state={:?} mega_lookups={}",
            ic.state(),
            ic.mega_lookups
        );
        println!("规则：PIC 槽数宜小（2~4）；形状过多宁用 HashMap，别假装还是 IC\n");
    }
}

pub mod guard_demo {
    use super::*;

    /// Guard 不只比 shape：还可含 generation / epoch，用于失效。
    #[derive(Clone, Copy, PartialEq, Eq)]
    struct Guard {
        shape: ShapeId,
        gen: u64,
    }

    struct GenerationalIc {
        guard: Option<Guard>,
        cached: u32,
        pub hits: u64,
        pub misses: u64,
    }

    impl GenerationalIc {
        fn new() -> Self {
            Self {
                guard: None,
                cached: 0,
                hits: 0,
                misses: 0,
            }
        }

        fn lookup(&mut self, g: Guard, resolve: impl FnOnce() -> u32) -> u32 {
            if self.guard == Some(g) {
                self.hits += 1;
                return self.cached;
            }
            self.misses += 1;
            self.cached = resolve();
            self.guard = Some(g);
            self.cached
        }
    }

    pub fn demonstrate() {
        println!("## Basics-3：带 generation 的 Guard");

        let mut ic = GenerationalIc::new();
        let shape = ShapeId(7);
        let mut gen = 1u64;

        for _ in 0..5 {
            let _ = ic.lookup(Guard { shape, gen }, || 42);
        }
        // 配置热更新 / 合约升级 → gen bump → 整槽失效
        gen = 2;
        let v = ic.lookup(Guard { shape, gen }, || 99);
        println!(
            "upgrade 后 value={} hits={} misses={}",
            v, ic.hits, ic.misses
        );
        println!("规则：可失效的缓存必须把 epoch 编进 guard，否则静默读脏数据\n");
    }
}

pub fn demonstrate() {
    mono_demo::demonstrate();
    poly_demo::demonstrate();
    guard_demo::demonstrate();
}
