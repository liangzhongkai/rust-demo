//! # HFT：Arena 要解决的「生产真问题」
//!
//! 约束（和迭代器章节一致）：
//! - **P99 延迟**：热路径禁止频繁 `malloc`/`free`、避免 allocator 全局锁
//! - **抖动**：分配器行为在压力下图钉化（pinning）波动比平均延迟更致命
//! - **吞吐**：连续 arena 内存对 CPU cache / prefetch 更友好
//!
//! 下列场景均为 **单线程 reactor / 单策略线程** 模型——这也是 bump 最常见的部署形态。

#![allow(dead_code)]

use bumpalo::Bump;
use bumpalo::collections::Vec as BumpVec;

pub type Px = i64;
pub type Qty = i64;

#[derive(Debug, Clone, Copy)]
pub struct RawOrder {
    pub px: Px,
    pub qty: Qty,
    pub side: u8, // 0=bid 1=ask —— 简化
}

// =============================================================================
// 场景 1：UDP / channel 单包解码 —— 「一帧一事」arena
// =============================================================================
/// **生产问题**：每个市场快照包解码出 0..N 笔插入；用 `Vec::new` 在热路径上
/// 会触发 allocator 级联，P99 被长尾拖死。
///
/// **Arena 解**：为本包准备一个 `Bump`（或线程 TLS 里复用一个 bump 并 `reset`），
/// 所有短命 `OrderView` 都落在连续内存，处理结束整个作用域释放。
pub mod packet_scratch {
    use super::*;

    #[derive(Debug)]
    pub struct OrderView {
        pub px: Px,
        pub qty: Qty,
    }

    pub fn decode_packet<'a>(buf: &[u8], bump: &'a Bump) -> &'a [OrderView] {
        // 极简协议：u16 长度 + 重复 (px:i64 qty:i64)
        if buf.len() < 2 {
            return &[];
        }
        let n = u16::from_le_bytes(buf[0..2].try_into().unwrap()) as usize;
        let mut tmp = BumpVec::new_in(bump);
        let mut i = 2;
        for _ in 0..n {
            if i + 16 > buf.len() {
                break;
            }
            let px = i64::from_le_bytes(buf[i..i + 8].try_into().unwrap());
            i += 8;
            let qty = i64::from_le_bytes(buf[i..i + 8].try_into().unwrap());
            i += 8;
            tmp.push(OrderView { px, qty });
        }
        let out = tmp.into_bump_slice();
        // `into_bump_slice` 给出 `&mut`，此处只读观察即可
        &*out
    }

    pub fn demonstrate() {
        println!("## 场景 1：单包解码 scratch arena");

        let mut payload = vec![0_u8; 2 + 16 * 2];
        payload[0..2].copy_from_slice(&2u16.to_le_bytes());
        payload[2..10].copy_from_slice(&100_00i64.to_le_bytes());
        payload[10..18].copy_from_slice(&10i64.to_le_bytes());
        payload[18..26].copy_from_slice(&101_00i64.to_le_bytes());
        payload[26..34].copy_from_slice(&5i64.to_le_bytes());

        let bump = Bump::new();
        let orders = decode_packet(&payload, &bump);
        println!("解码得到 {} 笔（均在 bump 内连续存放）\n", orders.len());
    }
}

// =============================================================================
// 场景 2：L2 深度快照 —— 已知上界的批量 `alloc_slice`
// =============================================================================
/// **生产问题**：策略要读取前 K 档；若 K 固定或可配置上限，可直接 **一次性**
/// 在 arena 分配 `[PriceLevel; K]`，省略 `Vec` 容量翻倍拷贝。
pub mod depth_snapshot {
    use super::*;

    #[derive(Debug, Clone, Copy)]
    pub struct PriceLevel {
        pub px: Px,
        pub qty: Qty,
    }

    pub fn copy_top_levels<'a>(
        src: &[PriceLevel],
        k: usize,
        bump: &'a Bump,
    ) -> &'a [PriceLevel] {
        let n = k.min(src.len());
        bump.alloc_slice_copy(&src[..n])
    }

    pub fn demonstrate() {
        println!("## 场景 2：L2 top-K 一次性切片分配");

        let book = [
            PriceLevel { px: 100, qty: 12 },
            PriceLevel { px: 99, qty: 4 },
            PriceLevel { px: 98, qty: 99 },
        ];
        let bump = Bump::new();
        let top = copy_top_levels(&book, 2, &bump);
        println!("top-2 = {:?}\n", top);
    }
}

// =============================================================================
// 场景 3：风控预检 —— 同一批订单的「影子副本」
// =============================================================================
/// **生产问题**：下单前要在 **不改共享 book 快照** 的前提下做试算；
/// 需要临时挂一批虚拟成交/占用，生命周期仅限本次检查。
///
/// **Arena 解**：把试算用的 `FillPreview` 全放进 bump，检查函数返回后整块丢弃，
/// 避免误把临时对象泄漏到全局 `Vec`。
pub mod risk_preview {
    use super::*;

    #[derive(Debug, Clone, Copy)]
    pub struct FillPreview {
        pub px: Px,
        pub qty: Qty,
    }

    pub fn simulate_fills<'a>(orders: &[RawOrder], bump: &'a Bump) -> &'a [FillPreview] {
        let mut out = BumpVec::new_in(bump);
        for o in orders {
            // 极简：市价单按对手价全部成交（演示用）
            out.push(FillPreview { px: o.px, qty: o.qty });
        }
        let s = out.into_bump_slice();
        &*s
    }

    pub fn check_exposure(previews: &[FillPreview], limit: Qty) -> bool {
        let sum: Qty = previews.iter().map(|p| p.qty).sum();
        sum <= limit
    }

    pub fn demonstrate() {
        println!("## 场景 3：风控试算 scratch（试算对象不入全局堆）");

        let live = [
            RawOrder {
                px: 100,
                qty: 1,
                side: 0,
            },
            RawOrder {
                px: 101,
                qty: 2,
                side: 1,
            },
        ];
        let bump = Bump::new();
        let prev = simulate_fills(&live, &bump);
        println!(
            "通过风控? {}（preview 仅指向 arena）\n",
            check_exposure(prev, 5)
        );
    }
}

// =============================================================================
// 场景 4：自定义二进制协议 —— 树形 / 链表中间表示
// =============================================================================
/// **生产问题**：网关解析内部协议时需要 **递归下降** 建树；若每个节点 `Box`
/// 进全局堆，碎片 + cache miss + allocator lock 三高。
///
/// **Arena 解**：节点一律 `bump.alloc(Node { ... })`，子指针用 `&Node`。
pub mod parse_tree {
    use super::*;

    pub enum Node<'a> {
        Leaf(Px),
        Split {
            left: &'a Node<'a>,
            right: &'a Node<'a>,
        },
    }

    pub fn build_balanced<'a>(prices: &[Px], bump: &'a Bump) -> &'a Node<'a> {
        fn rec<'b>(b: &'b Bump, s: &[Px]) -> &'b Node<'b> {
            match s.len() {
                0 => b.alloc(Node::Leaf(0)),
                1 => b.alloc(Node::Leaf(s[0])),
                _ => {
                    let m = s.len() / 2;
                    let left = rec(b, &s[..m]);
                    let right = rec(b, &s[m..]);
                    b.alloc(Node::Split { left, right })
                }
            }
        }
        rec(bump, prices)
    }

    pub fn demonstrate() {
        println!("## 场景 4：解析树 / IR —— 全 arena 结点");

        let px = [1_i64, 2, 3, 4, 5, 6, 7, 8];
        let bump = Bump::new();
        let root = build_balanced(&px, &bump);
        println!("root tag = {:?}\n", std::mem::discriminant(root));
    }
}

// =============================================================================
// 场景 5：微批报价 —— 合并多次 tick 再推送
// =============================================================================
/// **生产问题**：下游只需要「每 50µs 最多一批」的 delta；中间缓存若用
/// `Vec::with_capacity` 反复生长，会引入分配尖刺。
///
/// **Arena 解**：本批开始时 `Bump::new()`，把本批所有 `Delta` 堆在 arena，flush 后丢弃。
pub mod microbatch {
    use super::*;

    #[derive(Debug, Clone, Copy)]
    pub struct Delta {
        pub px: Px,
        pub dq: Qty,
    }

    pub fn flush_batch<'a>(updates: &[Delta], bump: &'a Bump) -> &'a [Delta] {
        bump.alloc_slice_copy(updates)
    }

    pub fn demonstrate() {
        println!("## 场景 5：微批合并 —— arena 承担短期缓冲");

        let bump = Bump::new();
        let slice = flush_batch(
            &[
                Delta { px: 100, dq: 1 },
                Delta { px: 99, dq: -2 },
            ],
            &bump,
        );
        println!("本批 {} 条，连续内存推送\n", slice.len());
    }
}

pub fn demonstrate() {
    packet_scratch::demonstrate();
    depth_snapshot::demonstrate();
    risk_preview::demonstrate();
    parse_tree::demonstrate();
    microbatch::demonstrate();
}
