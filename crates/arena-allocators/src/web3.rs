//! # Web3 / 链上基础设施：Arena 的典型落点
//!
//! 特征：
//! - **模拟与回放** 一次要物化大量「短命」中间对象（调用帧、slot 镜像、日志视图）
//! - **解码** ABI / RLP / SSZ 往往形成深而肥的树；若全局堆分配，GC 压力在 Rust 里体现为分配器争用
//! - **Bundle / Block 边界清晰** —— 天然「一界一批」与 arena 对齐
//!
//! 下列示例刻意保持 **无外部_crypto_依赖**，只突出内存模式；真实工程在同样边界上使用 arenas。

#![allow(dead_code)]

use bumpalo::Bump;
use bumpalo::collections::Vec as BumpVec;

/// 简化的「地址」与「wei」
pub type Address = [u8; 20];
pub type Wei = u128;

#[derive(Debug, Clone, Copy)]
pub struct CallFrame {
    pub from: Address,
    pub to: Address,
    pub value: Wei,
    pub selector: [u8; 4],
}

// =============================================================================
// 场景 1：Bundle 内多候选 tx —— 每候选一块 arena
// =============================================================================
/// **生产问题**：搜索多笔 insert / backrun 组合时，每笔都要构造 **临时调用图**
/// 与 scratch 状态；若共用 `Vec`，忘了 `clear` 会串状态；若频繁 `with_capacity`
/// 会产生分配热点。
///
/// **Arena 解**：`for tx in candidates { let bump = Bump::new(); ... }`
/// 保证候选之间 **零泄漏、零误共享**。
pub mod bundle_sim {
    use super::*;

    pub fn materialize_call_graph(depth: usize, bump: &Bump) -> &[CallFrame] {
        let mut v = BumpVec::new_in(bump);
        let mut cur = [1u8; 20];
        for i in 0..depth {
            let next = [(i as u8).wrapping_add(2); 20];
            v.push(CallFrame {
                from: cur,
                to: next,
                value: 0,
                selector: [0xA9, 0x05, 0x9C, 0xBB], // transfer(address,uint256)
            });
            cur = next;
        }
        let s = v.into_bump_slice();
        &*s
    }

    pub fn demonstrate() {
        println!("## 场景 1：MEV bundle —— 每候选独立 bump");

        for cand in 0..3u8 {
            let bump = Bump::new();
            let chain = materialize_call_graph(4 + (cand as usize), &bump);
            println!("候选 {}: {} 帧", cand, chain.len());
        }
        println!();
    }
}

// =============================================================================
// 场景 2：Receipt 日志 fan-out —— 短命 `LogView` 批处理
// =============================================================================
/// **生产问题**：索引器对单笔 receipt 要展开几十上百条 log 到内部格式；
/// 这批对象只服务 **当前区块高度的一次刷盘**。
///
/// **Arena 解**：区块处理函数持有一个 `&Bump`，所有 `LogView` 落在同 arena，
/// flush 后整块释放，避免 `into_iter().map(collect)` 级联分配。
pub mod log_fanout {
    use super::*;

    #[derive(Debug, Clone, Copy)]
    pub struct LogView {
        pub topic0: [u8; 32],
        pub data_word0: u64,
    }

    pub fn expand_receipt<'a>(
        raw_topics: &[[u8; 32]],
        bump: &'a Bump,
    ) -> &'a [LogView] {
        let mut v = BumpVec::new_in(bump);
        for (i, t) in raw_topics.iter().enumerate() {
            v.push(LogView {
                topic0: *t,
                data_word0: i as u64,
            });
        }
        let s = v.into_bump_slice();
        &*s
    }

    pub fn demonstrate() {
        println!("## 场景 2：Receipt 日志展开 —— 单 receipt arena");

        let topics = [[7u8; 32], [8u8; 32], [9u8; 32]];
        let bump = Bump::new();
        let logs = expand_receipt(&topics, &bump);
        println!("物化 {} 条 LogView（连续 bump 内存）\n", logs.len());
    }
}

// =============================================================================
// 场景 3：状态 diff 聚合 —— 试算「若执行该 tx，余额如何变」
// =============================================================================
/// **生产问题**：模拟器在 **沙盒** 里对数千账户打补丁；真实实现常用 `HashMap`，
/// 但在 **单次 tx 内** 往往只有少数几跳；可用 arena `Vec` 存 `(addr, delta)`，
/// 再按需排序/去重，避免一开始就造大哈希表。
pub mod state_diff_scratch {
    use super::*;

    #[derive(Debug, Clone, Copy)]
    pub struct BalanceDelta {
        pub who: Address,
        pub delta: i128,
    }

    pub fn record_deltas<'a>(bump: &'a Bump, ops: &[(Address, i128)]) -> &'a [BalanceDelta] {
        let mut v = BumpVec::new_in(bump);
        for (who, d) in ops {
            v.push(BalanceDelta { who: *who, delta: *d });
        }
        let s = v.into_bump_slice();
        &*s
    }

    pub fn net_for(a: Address, ds: &[BalanceDelta]) -> i128 {
        ds.iter().filter(|x| x.who == a).map(|x| x.delta).sum()
    }

    pub fn demonstrate() {
        println!("## 场景 3：试算余额 diff —— arena Vec 聚合再上屏");

        let bump = Bump::new();
        let alice = [1u8; 20];
        let bob = [2u8; 20];
        let deltas = record_deltas(
            &bump,
            &[(alice, -100), (bob, 100), (alice, -50), (bob, 50)],
        );
        println!(
            "alice net = {}, bob net = {}\n",
            net_for(alice, deltas),
            net_for(bob, deltas)
        );
    }
}

// =============================================================================
// 场景 4：Merkle 证明验证 —— 路径结点缓冲区
// =============================================================================
/// **生产问题**：验证 inclusion 时要逐层 `hash(parent, sibling)`；中间 `Node`
/// 寿命仅为 **当前层** 到 **下一层**。
///
/// **Arena 解**：路径 `siblings` 已驻留在 bump；每步新 `hash` 也可 `bump.alloc`。
pub mod merkle_path {
    use super::*;

    #[derive(Debug, Clone, Copy)]
    pub struct NodeHash(pub [u8; 32]);

    pub fn fake_hash<'a>(left: NodeHash, right: NodeHash, bump: &'a Bump) -> &'a NodeHash {
        bump.alloc(NodeHash(core::array::from_fn(|i| left.0[i] ^ right.0[i])))
    }

    pub fn verify_path(leaf: NodeHash, siblings: &[NodeHash], bump: &Bump) -> NodeHash {
        let mut acc = leaf;
        for s in siblings {
            acc = *fake_hash(acc, *s, bump);
        }
        acc
    }

    pub fn demonstrate() {
        println!("## 场景 4：Merkle 路径 —— 每层哈希 bump 分配");

        let bump = Bump::new();
        let sibs = [
            NodeHash([1u8; 32]),
            NodeHash([2u8; 32]),
            NodeHash([3u8; 32]),
        ];
        let root = verify_path(NodeHash([0xAB; 32]), &sibs, &bump);
        println!("root[0..4] = {:02x?}\n", &root.0[..4]);
    }
}

// =============================================================================
// 场景 5：嵌套 ABI 元组解码 —— IR 子树全进 arena
// =============================================================================
/// **生产问题**：`(address,(uint256,uint256[]),bytes)` 这类嵌套在解码时会生成
/// **树形中间表示**；ERC-4337、多跳 swap 路由中极常见。
///
/// **Arena 解**：`DecodeNode` 递归引用只在 **本次 decode** 存活。
pub mod abi_ir {
    use super::*;

    pub enum DecodeNode<'a> {
        U256(u128),
        Addr(Address),
        Pair {
            head: &'a DecodeNode<'a>,
            tail: &'a DecodeNode<'a>,
        },
    }

    pub fn decode_route<'a>(bump: &'a Bump, hops: &[Address]) -> &'a DecodeNode<'a> {
        match hops.split_first() {
            None => bump.alloc(DecodeNode::U256(0)),
            Some((first, rest)) if rest.is_empty() => bump.alloc(DecodeNode::Addr(*first)),
            Some((first, rest)) => {
                let tail = decode_route(bump, rest);
                let head = bump.alloc(DecodeNode::Addr(*first));
                bump.alloc(DecodeNode::Pair {
                    head: &*head,
                    tail,
                })
            }
        }
    }

    pub fn demonstrate() {
        println!("## 场景 5：路由解码 IR —— 嵌套 tuple 指向 bump");

        let path = [[0x11u8; 20], [0x22u8; 20], [0x33u8; 20]];
        let bump = Bump::new();
        let root = decode_route(&bump, &path);
        println!("根结点判别式 = {:?}\n", std::mem::discriminant(root));
    }
}

pub fn demonstrate() {
    bundle_sim::demonstrate();
    log_fanout::demonstrate();
    state_diff_scratch::demonstrate();
    merkle_path::demonstrate();
    abi_ir::demonstrate();
}
