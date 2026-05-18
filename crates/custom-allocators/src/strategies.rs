//! # 从 HFT / Web3 场景泛化出的 **一般性问题 → 策略**
//!
//! | 症状 | 根因线索 | 首选策略 | 与 `arena-allocators` 关系 |
//! |------|-----------|----------|---------------------------|
//! | P99 尖刺、分配器 CPU 高 | 热路径 `malloc` / 锁 | 线程私池、固定环、`reserve_exact` | Arena 解「一帧一命」图 |
//! | RSS 峰值与 QPS 成正比 | 大对象反复 `with_capacity` | 分桶 buffer 池 + 上限 | Arena 难控长期驻留 |
//! | 碎片、长期运行渐进变慢 | 混用大小类、频繁 `realloc` | 对象池、预分配、换 mimalloc | Arena reset 消除碎片（块内） |
//! | 指标与 OOM 不一致 | 估算不准、COW/栈外不在统计里 | 真 prof + 限流 + 背压 | 同上 |

#![allow(dead_code)]

#[derive(Debug, Clone, Copy)]
pub enum MemoryPressureSymptom {
    TailLatency,
    RssPeak,
    Fragmentation,
    FalsePositiveMetrics,
}

#[derive(Debug, Clone, Copy)]
pub enum ResponseStrategy {
    ThreadLocalPool,
    ByteBufferPoolWithCap,
    ReserveExactOnce,
    ArenaOrBumpForRequest,
    ExternalProfilerAndBackpressure,
}

pub fn recommend(sym: MemoryPressureSymptom) -> &'static [ResponseStrategy] {
    match sym {
        MemoryPressureSymptom::TailLatency => &[
            ResponseStrategy::ThreadLocalPool,
            ResponseStrategy::ArenaOrBumpForRequest,
            ResponseStrategy::ReserveExactOnce,
        ],
        MemoryPressureSymptom::RssPeak => &[
            ResponseStrategy::ByteBufferPoolWithCap,
            ResponseStrategy::ExternalProfilerAndBackpressure,
        ],
        MemoryPressureSymptom::Fragmentation => &[
            ResponseStrategy::ThreadLocalPool,
            ResponseStrategy::ArenaOrBumpForRequest,
        ],
        MemoryPressureSymptom::FalsePositiveMetrics => &[
            ResponseStrategy::ExternalProfilerAndBackpressure,
        ],
    }
}

pub fn demonstrate() {
    println!("### strategies：症状 → 策略映射（示例）");
    for s in [
        MemoryPressureSymptom::TailLatency,
        MemoryPressureSymptom::RssPeak,
    ] {
        println!("  {:?} => {:?}", s, recommend(s));
    }
    println!(
        "\n  总原则：先 **量**（证明热路径在分配），再 **选寿命模型**（请求级 / 批级 / 长期池），最后 **加硬帽**（池大小、RPC 窗口、队列深度）。"
    );
}
