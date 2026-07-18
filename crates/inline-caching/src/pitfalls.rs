//! # Inline Cache 常见陷阱与诊断
//!
//! 现象 → 根因 → 修法。IC 加速的是「正确且稳定」的路径；失效与污染
//! 会把平均延迟优势变成 **静默资损**。

#![allow(dead_code)]

// ============================================================================
// 陷阱 1：升级 / 热更新后未失效 —— 读脏
// ============================================================================
/// **现象**：合约升级或 tick_size 热更新后，系统仍按旧布局/旧精度下单。
/// **根因**：IC 只比「地址 / symbol」不比 generation。
/// **修法**：guard 含 epoch；写路径强制 `invalidate()`。
pub mod stale_after_upgrade {
    #[derive(Clone, Copy, PartialEq, Eq)]
    struct Guard {
        id: u32,
        gen: u64,
    }

    struct BadIc {
        id: u32,
        cached: u32,
    }

    impl BadIc {
        // ❌ 只比 id
        fn lookup_bad(&mut self, id: u32, value: u32) -> u32 {
            if self.id == id {
                return self.cached;
            }
            self.id = id;
            self.cached = value;
            value
        }
    }

    struct GoodIc {
        guard: Option<Guard>,
        cached: u32,
    }

    impl GoodIc {
        fn lookup(&mut self, g: Guard, resolve: impl FnOnce() -> u32) -> u32 {
            if self.guard == Some(g) {
                return self.cached;
            }
            self.cached = resolve();
            self.guard = Some(g);
            self.cached
        }
    }

    pub fn demonstrate() {
        println!("## 陷阱 1：升级后未失效");

        let mut bad = BadIc { id: 0, cached: 0 };
        let _ = bad.lookup_bad(1, 8); // offset=8
        // 升级后真实 offset=16，但 bad IC 仍返回 8
        let stale = bad.lookup_bad(1, 16);
        println!("坏 IC 仍返回 stale offset={}", stale);

        let mut good = GoodIc {
            guard: None,
            cached: 0,
        };
        let _ = good.lookup(Guard { id: 1, gen: 1 }, || 8);
        let fresh = good.lookup(Guard { id: 1, gen: 2 }, || 16);
        println!("好 IC 在 gen bump 后返回 {}\n", fresh);
    }
}

// ============================================================================
// 陷阱 2：Megamorphic 污染 —— 假 IC
// ============================================================================
/// **现象**：加了「缓存」后延迟反而更差；命中率 < 50%。
/// **根因**：调用点看到几十种 shape，PIC 槽不断淘汰，比直接 HashMap 还慢。
/// **修法**：监控命中率；低于阈值直接走字典，或拆分调用点（按 venue 分函数）。
pub mod megamorphic_pollution {
    pub fn demonstrate() {
        println!("## 陷阱 2：Megamorphic 污染");

        let shapes_per_site = 40u32;
        let pic_slots = 2u32;
        let approx_hit_rate = pic_slots as f64 / shapes_per_site as f64;
        println!(
            "40 shapes / 2-slot PIC ≈ 命中率 {:.0}% —— 不如 HashMap",
            approx_hit_rate * 100.0
        );
        println!("修法：拆调用点（每个 venue 一个函数）或升 megamorphic 字典\n");
    }
}

// ============================================================================
// 陷阱 3：错误的 guard key —— 比了身份却漏了布局
// ============================================================================
/// **现象**：同一 proxy 地址，实现已换，仍用旧 slot。
/// **根因**：guard 用了稳定身份（proxy），没用可变身份（impl codehash / gen）。
/// **修法**：问「什么变了会让缓存失效？」——那一项必须进 guard。
pub mod wrong_guard_key {
    pub fn demonstrate() {
        println!("## 陷阱 3：错误的 guard key");
        println!("  ❌ guard = proxy_address");
        println!("  ✅ guard = (proxy_address, implementation_gen | codehash)");
        println!("  ❌ guard = token_address  alone 当 decimals 可被恶意改（极少但存在）");
        println!("  ✅ 对可变元数据：TTL / 事件订阅失效\n");
    }
}

// ============================================================================
// 陷阱 4：跨线程共享可变 IC —— 数据竞争 / 伪共享
// ============================================================================
/// **现象**：偶发错单；或多核吞吐不升。
/// **根因**：多线程写同一 `last_sym` 槽无同步；或 IC 落在同一 cache line。
/// **修法**：IC **线程局部**；共享只放只读表。
pub mod shared_mutable_ic {
    use std::cell::RefCell;

    thread_local! {
        static LAST_SYM: RefCell<Option<u32>> = const { RefCell::new(None) };
    }

    pub fn demonstrate() {
        println!("## 陷阱 4：跨线程共享可变 IC");

        LAST_SYM.with(|c| *c.borrow_mut() = Some(42));
        let v = LAST_SYM.with(|c| *c.borrow());
        println!("thread_local last_sym={:?}", v);
        println!("规则：可变 IC 槽 TLS；HashMap 本体可 Arc 只读共享\n");
    }
}

// ============================================================================
// 陷阱 5：无 guard 的「信任上次」—— use-after-invalid
// ============================================================================
/// **现象**：空指针式逻辑错误：缓存了函数指针后表项被卸掉仍调用。
/// **根因**：IC 命中路径跳过存在性检查。
/// **修法**：命中仍做廉价 guard；卸载表项时 bump 全局 epoch。
pub mod trust_without_guard {
    pub fn demonstrate() {
        println!("## 陷阱 5：无 guard 信任上次");
        println!("  命中路径最少做：shape/gen 相等比较（几个周期）");
        println!("  禁止：缓存裸指针后在卸载 handler 时不 bump epoch\n");
    }
}

// ============================================================================
// 陷阱 6：冷路径也套 IC —— 复杂度白加
// ============================================================================
/// **现象**：代码难读，bench 无收益。
/// **根因**：调用频率低或 shape 均匀随机，IC 永远 miss。
/// **修法**：先量 locality；无局部性就别加。
pub mod ic_on_cold_path {
    pub fn demonstrate() {
        println!("## 陷阱 6：冷路径硬套 IC");
        println!("  适合：同 key 连打、突发单 shape、线程 pin 单源");
        println!("  不适合：均匀随机 symbol、一次性管理接口、初始化路径\n");
    }
}

// ============================================================================
// 陷阱 7：忽略命中率 —— 优化凭感觉
// ============================================================================
/// **现象**：上线「优化」后 P99 不变或变差。
/// **根因**：没打 hits/misses 指标。
/// **修法**：每个 IC 暴露计数；告警 hit_rate < 阈值。
pub mod ignore_hit_rate {
    pub struct IcStats {
        pub hits: u64,
        pub misses: u64,
    }

    impl IcStats {
        pub fn hit_rate(&self) -> f64 {
            let t = self.hits + self.misses;
            if t == 0 {
                0.0
            } else {
                self.hits as f64 / t as f64
            }
        }
    }

    pub fn demonstrate() {
        println!("## 陷阱 7：忽略命中率");

        let good = IcStats {
            hits: 9_900,
            misses: 100,
        };
        let bad = IcStats {
            hits: 400,
            misses: 600,
        };
        println!(
            "好 {:.1}% / 坏 {:.1}% —— 坏应拆除或改字典",
            good.hit_rate() * 100.0,
            bad.hit_rate() * 100.0
        );
        println!("SLA：热路径 IC hit_rate 目标通常 > 90%\n");
    }
}

// ============================================================================
// 陷阱 8：本该静态分发却用运行时 IC
// ============================================================================
/// **现象**：部署后永远只有一种 venue/策略，却维护 MonoIc。
/// **根因**：用运行时机制解决编译期已知问题。
/// **修法**：泛型 / enum / `#[cfg]` 静态分发；IC 留给运行时才确定的多态。
pub mod should_be_static {
    pub fn demonstrate() {
        println!("## 陷阱 8：该静态分发却用 IC");
        println!("  编译期唯一实现 → 泛型 trait / 单态函数（见 generics / zero-cost）");
        println!("  运行时少量粘滞 shape → Mono/Poly IC");
        println!("  运行时大量 shape → HashMap / perfect hash，别伪装 PIC\n");
    }
}

pub fn demonstrate() {
    stale_after_upgrade::demonstrate();
    megamorphic_pollution::demonstrate();
    wrong_guard_key::demonstrate();
    shared_mutable_ic::demonstrate();
    trust_without_guard::demonstrate();
    ic_on_cold_path::demonstrate();
    ignore_hit_rate::demonstrate();
    should_be_static::demonstrate();
}
