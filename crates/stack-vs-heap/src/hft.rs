//! # HFT 生产场景下的栈 vs 堆
//!
//! 高频交易的硬约束：
//! - **延迟**：热路径禁止 malloc；P99 抖动常来自堆分配器锁竞争
//! - **吞吐**：栈上 Copy 类型 cache-friendly；堆指针追逐导致 cache miss
//! - **可预测**：固定上界的数据结构放栈/预分配，避免 realloc 尖刺
//!
//! 下面 7 个场景是真实交易系统里的内存布局决策。

#![allow(dead_code)]

use crate::util::{bench_ns, AllocCounter, InlineBuffer, RingBuffer};

pub type Px = i64;
pub type Qty = i64;
pub type TsNs = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Level {
    pub px: Px,
    pub qty: Qty,
}

#[derive(Debug, Clone, Copy)]
pub struct Tick {
    pub ts_ns: TsNs,
    pub bid: Px,
    pub ask: Px,
}

// ============================================================================
// 场景 1：L2 订单簿 Top-N —— 栈上固定数组
// ============================================================================
/// **生产问题**：每 tick 重建 Top-20 档位，用 `Vec::new()` 导致
/// 500k tick/s 下每秒数十万次 malloc，P99 延迟尖刺 10×。
///
/// **栈/堆套路**：`[Level; 20]` 栈数组 + `len` 计数，零堆分配。
pub mod l2_topn_stack {
    use super::*;

    pub const TOP_N: usize = 20;

    #[derive(Debug, Clone, Copy)]
    pub struct BookSnapshot {
        pub bids: [Level; TOP_N],
        pub bid_len: u8,
        pub asks: [Level; TOP_N],
        pub ask_len: u8,
    }

    impl BookSnapshot {
        pub fn from_raw(bids: &[Level], asks: &[Level]) -> Self {
            let mut snap = Self {
                bids: [Level { px: 0, qty: 0 }; TOP_N],
                bid_len: 0,
                asks: [Level { px: 0, qty: 0 }; TOP_N],
                ask_len: 0,
            };
            let bn = bids.len().min(TOP_N);
            snap.bids[..bn].copy_from_slice(&bids[..bn]);
            snap.bid_len = bn as u8;
            let an = asks.len().min(TOP_N);
            snap.asks[..an].copy_from_slice(&asks[..an]);
            snap.ask_len = an as u8;
            snap
        }

        pub fn best_bid(&self) -> Option<Level> {
            (self.bid_len > 0).then_some(self.bids[0])
        }
    }

    pub fn build_heap(bids: &[Level], asks: &[Level]) -> (Vec<Level>, Vec<Level>) {
        (
            bids.iter().take(TOP_N).copied().collect(),
            asks.iter().take(TOP_N).copied().collect(),
        )
    }

    pub fn demonstrate() {
        println!("## 场景 1：L2 Top-N 快照（栈数组 vs Vec）");

        let bids: Vec<Level> = (0..20)
            .map(|i| Level {
                px: 100_00 - i,
                qty: 10,
            })
            .collect();
        let asks: Vec<Level> = (0..20)
            .map(|i| Level {
                px: 100_01 + i,
                qty: 8,
            })
            .collect();

        let snap = BookSnapshot::from_raw(&bids, &asks);
        println!("  栈快照 best_bid = {:?}", snap.best_bid());

        let mut counter = AllocCounter::default();
        for _ in 0..1000 {
            let mut bv = Vec::new();
            let mut av = Vec::new();
            for l in &bids {
                counter.track_vec_push(&mut bv, *l);
            }
            for l in &asks {
                counter.track_vec_push(&mut av, *l);
            }
            let _ = (bv, av);
        }
        println!("  堆 Vec 构建 1000 次: {} 次 realloc", counter.allocs);
        println!("  关键：Copy + 固定上界 → [T; N] 栈数组\n");
    }
}

// ============================================================================
// 场景 2：ITCH/FIX 字段解析 —— 栈上 byte slice，拒绝 String
// ============================================================================
/// **生产问题**：解析 symbol/venue 字段时 `String::from_utf8_lossy` 或
/// `to_string()`，每笔 tick 2-3 次堆分配，回测与实盘 alloc profile 对不上。
///
/// **栈/堆套路**：解析结果用 `&str` 或 `&[u8]` 借用输入 buffer；只在落库边界转 String。
pub mod zero_alloc_parse {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct ParsedTick {
        pub symbol_id: u16,
        pub px: Px,
        pub qty: Qty,
    }

    /// 定长 binary tick：symbol_id(2) + px(8) + qty(8) = 18 bytes
    pub fn parse_tick(buf: &[u8]) -> Option<ParsedTick> {
        if buf.len() < 18 {
            return None;
        }
        let symbol_id = u16::from_le_bytes(buf[0..2].try_into().ok()?);
        let px = i64::from_le_bytes(buf[2..10].try_into().ok()?);
        let qty = i64::from_le_bytes(buf[10..18].try_into().ok()?);
        Some(ParsedTick { symbol_id, px, qty })
    }

    pub fn parse_tick_slow(buf: &[u8]) -> Option<(String, ParsedTick)> {
        let tick = parse_tick(buf)?;
        let sym = format!("SYM_{}", tick.symbol_id);
        Some((sym, tick))
    }

    pub fn demonstrate() {
        println!("## 场景 2：Binary tick 解析（零堆 vs format!）");

        let raw: [u8; 18] = {
            let mut b = [0u8; 18];
            b[0..2].copy_from_slice(&1u16.to_le_bytes());
            b[2..10].copy_from_slice(&100_50i64.to_le_bytes());
            b[10..18].copy_from_slice(&5i64.to_le_bytes());
            b
        };

        let tick = parse_tick(&raw).unwrap();
        println!("  零堆解析: {:?}", tick);

        let mut counter = AllocCounter::default();
        for _ in 0..1000 {
            let mut s = String::new();
            counter.track_string_push(&mut s, "SYM_");
            counter.track_string_push(&mut s, "1");
            let _ = s;
        }
        println!("  format!/String 路径 1000 次: ~{} realloc 事件", counter.allocs);
        println!("  关键：解析层只产出 Copy / 借用；String 推到 IO 边界\n");
    }
}

// ============================================================================
// 场景 3：行情事件环形缓冲 —— 栈 backing 数组
// ============================================================================
/// **生产问题**：策略需要最近 1024 笔 trade 做 microstructure 特征，
/// `VecDeque` 或无限 `push` 导致堆增长和 cache 不友好。
///
/// **栈/堆套路**：`RingBuffer<T, 1024>` 固定容量，满则覆盖最旧。
pub mod tick_ring {
    use super::*;

    pub type TradeRing = RingBuffer<Tick, 1024>;

    pub fn vwap_recent(ring: &TradeRing) -> Option<Px> {
        let mut sum_pq: i128 = 0;
        let mut sum_q: i128 = 0;
        for t in ring.iter() {
            let mid = (t.bid + t.ask) / 2;
            let q = 1i128;
            sum_pq += mid as i128 * q;
            sum_q += q;
        }
        (sum_q > 0).then(|| (sum_pq / sum_q) as Px)
    }

    pub fn demonstrate() {
        println!("## 场景 3：Tick 环形缓冲（固定栈数组 backing）");

        let mut ring: TradeRing = RingBuffer::default();
        for i in 0..1100 {
            ring.push(Tick {
                ts_ns: i,
                bid: 100_00 + (i % 5) as i64,
                ask: 100_01 + (i % 5) as i64,
            });
        }
        println!("  ring len = {} (cap=1024)", ring.len());
        println!("  recent VWAP = {:?}", vwap_recent(&ring));
        println!("  关键：满容量 overwrite → 零 realloc，内存占用恒定\n");
    }
}

// ============================================================================
// 场景 4：InlineBuffer —— 小 N 栈，大 N 溢出堆
// ============================================================================
/// **生产问题**：撮合回报通常 1-3 笔 fill，极端 burst 可能 50+。
/// 纯 `Vec` 每次 1 fill 也可能触发 growth；纯 `[T; 50]` 浪费栈空间。
///
/// **栈/堆套路**：SmallVec 模式 —— 前 N 个放栈，超出才 spill 到 Vec。
pub mod fill_inline {
    use super::*;

    #[derive(Debug, Clone, Copy)]
    pub struct Fill {
        pub order_id: u64,
        pub px: Px,
        pub qty: Qty,
    }

    pub type FillBuffer = InlineBuffer<Fill, 8>;

    pub fn match_fills(count: usize) -> FillBuffer {
        let mut buf = FillBuffer::default();
        for i in 0..count {
            buf.push(Fill {
                order_id: i as u64,
                px: 100_00,
                qty: 1,
            });
        }
        buf
    }

    pub fn demonstrate() {
        println!("## 场景 4：撮合 Fill 缓冲（InlineBuffer N=8）");

        let small = match_fills(3);
        let large = match_fills(12);
        println!("  3 fills: len={}, heap_spill={}", small.len(), small.heap_spill_count());
        println!(
            "  12 fills: len={}, heap_spill={}",
            large.len(),
            large.heap_spill_count()
        );
        println!("  关键：常见 case 零堆；极端 case 仍正确\n");
    }
}

// ============================================================================
// 场景 5：批量 tick 聚合 —— 栈上 batch 再一次性发送
// ============================================================================
/// **生产问题**：每个 tick 单独 `send()` 系统调用 + 可能的小包堆缓冲；
/// 攒批后吞吐提升但 batch buffer 放哪？
///
/// **栈/堆套路**：`[Tick; BATCH]` 栈数组攒满再 flush 到预分配 send buffer。
pub mod tick_batch {
    use super::*;

    pub const BATCH: usize = 64;

    pub struct TickBatcher {
        buf: [Tick; BATCH],
        len: usize,
        sent_batches: u64,
    }

    impl TickBatcher {
        pub fn new() -> Self {
            Self {
                buf: [Tick {
                    ts_ns: 0,
                    bid: 0,
                    ask: 0,
                }; BATCH],
                len: 0,
                sent_batches: 0,
            }
        }

        pub fn push(&mut self, t: Tick) -> Option<&[Tick]> {
            self.buf[self.len] = t;
            self.len += 1;
            if self.len == BATCH {
                self.len = 0;
                self.sent_batches += 1;
                Some(&self.buf)
            } else {
                None
            }
        }

        pub fn flush(&mut self) -> Option<&[Tick]> {
            if self.len == 0 {
                return None;
            }
            let slice = &self.buf[..self.len];
            self.len = 0;
            self.sent_batches += 1;
            Some(slice)
        }
    }

    pub fn demonstrate() {
        println!("## 场景 5：Tick 批量攒批（栈 batch buffer）");

        let mut batcher = TickBatcher::new();
        for i in 0..130 {
            if let Some(batch) = batcher.push(Tick {
                ts_ns: i,
                bid: 100,
                ask: 101,
            }) {
                println!("  flush batch of {} ticks", batch.len());
            }
        }
        if let Some(tail) = batcher.flush() {
            println!("  final flush {} ticks", tail.len());
        }
        println!("  sent_batches = {}", batcher.sent_batches);
        println!("  关键：batch 在栈；flush 时才触达网络/共享内存\n");
    }
}

// ============================================================================
// 场景 6：策略分派 —— 栈 enum vs Box<dyn>
// ============================================================================
/// **生产问题**：`Box<dyn Strategy>` 每 tick 一次 vtable + 堆 indirection；
/// 500k/s 下 measurable overhead，且阻碍 inline。
///
/// **栈/堆套路**：`enum Strategy { MidSpread(...), Momentum(...) }` 栈上 tagged union。
pub mod strategy_enum {
    use super::*;

    pub enum Strategy {
        MidSpread { threshold: i64 },
        Momentum { lookback: u8 },
    }

    impl Strategy {
        pub fn on_tick(&self, t: Tick) -> i64 {
            match self {
                Strategy::MidSpread { threshold } => {
                    let spread = t.ask - t.bid;
                    if spread <= *threshold {
                        (t.bid + t.ask) / 2
                    } else {
                        0
                    }
                }
                Strategy::Momentum { lookback: _ } => t.ask - t.bid,
            }
        }
    }

    pub fn run_dyn(s: &dyn Fn(Tick) -> i64, ticks: &[Tick]) -> i64 {
        ticks.iter().map(|&t| s(t)).sum()
    }

    pub fn run_enum(s: &Strategy, ticks: &[Tick]) -> i64 {
        ticks.iter().map(|&t| s.on_tick(t)).sum()
    }

    pub fn demonstrate() {
        println!("## 场景 6：策略分派（enum 栈 vs dyn 堆）");

        let ticks: Vec<Tick> = (0..1000)
            .map(|i| Tick {
                ts_ns: i,
                bid: 100,
                ask: 101,
            })
            .collect();

        let strat = Strategy::MidSpread { threshold: 2 };
        let dyn_fn = |t: Tick| -> i64 { strat.on_tick(t) };

        let r1 = run_enum(&strat, &ticks);
        let r2 = run_dyn(&dyn_fn, &ticks);
        assert_eq!(r1, r2);

        let (min_e, mean_e) = bench_ns(50, 2000, || {
            let _ = run_enum(&strat, &ticks);
        });
        let (min_d, mean_d) = bench_ns(50, 2000, || {
            let _ = run_dyn(&dyn_fn, &ticks);
        });
        println!("  enum: min={}ns mean={}ns", min_e, mean_e);
        println!("  dyn:  min={}ns mean={}ns", min_d, mean_d);
        println!("  关键：enum 单态 match 可 inline；dyn 间接调用 + 可能堆分配\n");
    }
}

// ============================================================================
// 场景 7：预分配 delta 缓冲 —— reserve 消除热路径 realloc
// ============================================================================
/// **生产问题**：L2 diff 输出长度波动大，默认 Vec 从 0 增长，
/// burst 时连续 realloc 导致 GC 式延迟尖刺。
///
/// **栈/堆套路**：启动时 `reserve(expected)`；diff 层复用 `clear()` 不清 capacity。
pub mod delta_buffer {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Delta {
        Insert(Level),
        Update(Level),
        Remove(Px),
    }

    pub struct DeltaWriter {
        buf: Vec<Delta>,
    }

    impl DeltaWriter {
        pub fn new(expected: usize) -> Self {
            Self {
                buf: Vec::with_capacity(expected),
            }
        }

        pub fn clear_reuse(&mut self) {
            self.buf.clear(); // 保留 capacity
        }

        pub fn push_delta(&mut self, d: Delta) {
            self.buf.push(d);
        }

        pub fn as_slice(&self) -> &[Delta] {
            &self.buf
        }
    }

    pub fn write_deltas_naive(n: usize) -> (Vec<Delta>, AllocCounter) {
        let mut w = DeltaWriter::new(0);
        let mut c = AllocCounter::default();
        for i in 0..n {
            c.track_vec_push(
                &mut w.buf,
                Delta::Insert(Level {
                    px: 100 + i as i64,
                    qty: 1,
                }),
            );
        }
        (w.buf, c)
    }

    pub fn write_deltas_pooled(n: usize) -> (Vec<Delta>, AllocCounter) {
        let mut w = DeltaWriter::new(n);
        let mut c = AllocCounter::default();
        for i in 0..n {
            c.track_vec_push(
                &mut w.buf,
                Delta::Insert(Level {
                    px: 100 + i as i64,
                    qty: 1,
                }),
            );
        }
        (w.buf, c)
    }

    pub fn demonstrate() {
        println!("## 场景 7：Delta 缓冲预分配（reserve vs 默认增长）");

        let n = 500;
        let (v1, c1) = write_deltas_naive(n);
        let (v2, c2) = write_deltas_pooled(n);
        assert_eq!(v1.len(), v2.len());
        println!("  naive: {} realloc", c1.allocs);
        println!("  pooled(reserve={}): {} realloc", n, c2.allocs);

        let mut w = DeltaWriter::new(128);
        for _ in 0..10 {
            for i in 0..50 {
                w.push_delta(Delta::Update(Level { px: i, qty: 1 }));
            }
            let _ = w.as_slice();
            w.clear_reuse();
        }
        println!("  10 轮 reuse: capacity 保持 = {}", w.buf.capacity());
        println!("  关键：堆可用，但热路径要 predictable —— reserve + reuse\n");
    }
}

pub fn demonstrate() {
    l2_topn_stack::demonstrate();
    zero_alloc_parse::demonstrate();
    tick_ring::demonstrate();
    fill_inline::demonstrate();
    tick_batch::demonstrate();
    strategy_enum::demonstrate();
    delta_buffer::demonstrate();
}
