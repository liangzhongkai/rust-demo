//! # Web3 / 区块链生产场景下的模式匹配
//!
//! Web3 的工作负载是 *大量异构消息 + 严格状态转移*：
//! - 解码 RLP / ABI / 事件 topic
//! - EVM 解释器 opcode 分发
//! - 链重组、MEV bundle 分类
//!
//! 模式匹配在这里的核心价值：**穷尽 variant + 嵌套解构 = 协议正确性**。
//! 下面 6 个场景对应 reth、revm、indexer、searcher 里的常见写法。

#![allow(dead_code)]

pub type Address = [u8; 20];
pub type B256 = [u8; 32];

fn topic(sig: &str) -> B256 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    sig.hash(&mut h);
    let mut out = [0u8; 32];
    out[..8].copy_from_slice(&h.finish().to_le_bytes());
    out
}

// ============================================================================
// 场景 1：交易类型分发（Legacy / EIP-1559 / Blob）
// ============================================================================
/// **生产问题**：以太坊同时存在 Legacy、Type-2 (1559)、Type-3 (4844 blob)
/// 交易，签名/hash/fee 计算路径完全不同。
///
/// **模式匹配套路**：顶层 match `TxEnvelope`，每个 variant 解构 gas 字段。
pub mod tx_type_dispatch {

    #[derive(Debug, Clone, Copy)]
    pub enum TxEnvelope {
        Legacy {
            nonce: u64,
            gas_price: u128,
        },
        Eip1559 {
            nonce: u64,
            max_fee: u128,
            priority_fee: u128,
        },
        Eip4844 {
            nonce: u64,
            max_fee: u128,
            blob_versioned_hashes: u8,
        },
    }

    pub fn effective_gas_price(tx: &TxEnvelope, base_fee: u128) -> u128 {
        match tx {
            TxEnvelope::Legacy { gas_price, .. } => *gas_price,
            TxEnvelope::Eip1559 {
                max_fee,
                priority_fee,
                ..
            } => (*max_fee).min(base_fee + priority_fee),
            TxEnvelope::Eip4844 { max_fee, .. } => *max_fee,
        }
    }

    pub fn tx_label(tx: &TxEnvelope) -> &'static str {
        match tx {
            TxEnvelope::Legacy { .. } => "legacy",
            TxEnvelope::Eip1559 { .. } => "eip1559",
            TxEnvelope::Eip4844 { .. } => "eip4844",
        }
    }

    pub fn demonstrate() {
        println!("## 场景 1：交易类型分发（TxEnvelope match）");

        let txs = [
            TxEnvelope::Legacy {
                nonce: 1,
                gas_price: 30,
            },
            TxEnvelope::Eip1559 {
                nonce: 2,
                max_fee: 100,
                priority_fee: 2,
            },
            TxEnvelope::Eip4844 {
                nonce: 3,
                max_fee: 50,
                blob_versioned_hashes: 2,
            },
        ];
        let base = 20u128;
        for tx in &txs {
            println!(
                "  {} effective_gas = {}",
                tx_label(tx),
                effective_gas_price(tx, base)
            );
        }
        println!("关键：新增 tx type = 新增 enum variant，编译器强制补 arm\n");
    }
}

// ============================================================================
// 场景 2：EVM Opcode 解释器 dispatch
// ============================================================================
/// **生产问题**：revm / reth 的 interpreter 对每个 opcode 做不同栈操作。
/// match 比 function pointer 表更易优化（devirtualization）。
///
/// **模式匹配套路**：`match opcode` + 守卫区分 PUSH1..PUSH32。
pub mod evm_opcode {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Op {
        Stop,
        Add,
        Mul,
        Push(u8), // 实际 opcode 0x60..0x7f
        Unknown(u8),
    }

    pub fn decode(byte: u8) -> Op {
        match byte {
            0x00 => Op::Stop,
            0x01 => Op::Add,
            0x02 => Op::Mul,
            b @ 0x60..=0x7f => Op::Push(b - 0x60 + 1),
            other => Op::Unknown(other),
        }
    }

    #[derive(Debug, Default)]
    pub struct StackEffect {
        pub pop: u8,
        pub push: u8,
    }

    pub fn stack_effect(op: Op) -> StackEffect {
        match op {
            Op::Stop => StackEffect { pop: 0, push: 0 },
            Op::Add | Op::Mul => StackEffect { pop: 2, push: 1 },
            Op::Push(n) => StackEffect {
                pop: 0,
                push: n,
            },
            Op::Unknown(_) => StackEffect { pop: 0, push: 0 },
        }
    }

    pub fn demonstrate() {
        println!("## 场景 2：EVM Opcode dispatch（range 模式 0x60..=0x7f）");

        for byte in [0x00, 0x01, 0x60, 0x65, 0xff] {
            let op = decode(byte);
            println!("  0x{:02x} → {:?}, stack {:?}", byte, op, stack_effect(op));
        }
        println!("关键：range pattern 一次覆盖 32 个 PUSH opcode\n");
    }
}

// ============================================================================
// 场景 3：事件日志解码（topic0 匹配）
// ============================================================================
/// **生产问题**：indexer 从 receipt logs 里筛 ERC20 Transfer / Approval /
/// 自定义事件，topic0 是事件签名哈希。
///
/// **模式匹配套路**：match `log.topics[0]`，解构 indexed 参数。
pub mod event_log_decode {
    use super::*;

    pub fn transfer_topic() -> B256 {
        topic("Transfer(address,address,uint256)")
    }

    pub fn approval_topic() -> B256 {
        topic("Approval(address,address,uint256)")
    }

    #[derive(Debug, Clone, Copy)]
    pub struct Log {
        pub topics: [B256; 4],
        pub topic_count: u8,
        pub data: [u8; 32],
    }

    #[derive(Debug)]
    pub enum DecodedEvent {
        Transfer { from: Address, to: Address, value: u128 },
        Approval { owner: Address, spender: Address, value: u128 },
        Unknown,
    }

    fn addr_from_topic(t: &B256) -> Address {
        let mut a = [0u8; 20];
        a.copy_from_slice(&t[12..32]);
        a
    }

    pub fn decode(log: &Log) -> DecodedEvent {
        let transfer = transfer_topic();
        let approval = approval_topic();
        let t0 = log.topics[0];
        match (t0, log.topic_count) {
            (t, 3) if t == transfer => DecodedEvent::Transfer {
                from: addr_from_topic(&log.topics[1]),
                to: addr_from_topic(&log.topics[2]),
                value: u128::from_be_bytes(log.data[16..32].try_into().unwrap()),
            },
            (t, 3) if t == approval => DecodedEvent::Approval {
                owner: addr_from_topic(&log.topics[1]),
                spender: addr_from_topic(&log.topics[2]),
                value: u128::from_be_bytes(log.data[16..32].try_into().unwrap()),
            },
            _ => DecodedEvent::Unknown,
        }
    }

    pub fn demonstrate() {
        println!("## 场景 3：事件日志解码（topic guard match）");

        let mut from = [0u8; 32];
        from[31] = 0x01;
        let mut to = [0u8; 32];
        to[31] = 0x02;
        let mut data = [0u8; 32];
        data[31] = 100;

        let log = Log {
            topics: [transfer_topic(), from, to, [0u8; 32]],
            topic_count: 3,
            data,
        };
        println!("  decoded = {:?}", decode(&log));
        println!("关键：`(topic, count)` 二元 guard 防止误解析\n");
    }
}

// ============================================================================
// 场景 4：账户状态转移（Option / Result 组合 match）
// ============================================================================
/// **生产问题**：执行 tx 时要处理 nonce 不匹配、余额不足、合约 selfdestruct 等。
///
/// **模式匹配套路**：嵌套 match `(account, tx)` + `Option` 解构。
pub mod account_transition {
    use super::*;

    #[derive(Debug, Clone, Copy)]
    pub struct Account {
        pub nonce: u64,
        pub balance: u128,
    }

    #[derive(Debug, Clone, Copy)]
    pub struct Tx {
        pub from: Address,
        pub nonce: u64,
        pub value: u128,
        pub gas_limit: u64,
    }

    #[derive(Debug, PartialEq, Eq)]
    pub enum ApplyError {
        NonceTooLow { expected: u64, got: u64 },
        InsufficientBalance { need: u128, have: u128 },
        OutOfGas,
    }

    pub fn apply_tx(
        acct: Option<Account>,
        tx: Tx,
    ) -> Result<Account, ApplyError> {
        match (acct, tx) {
            (None, Tx { nonce: 0, value, gas_limit, .. }) => {
                if gas_limit == 0 {
                    return Err(ApplyError::OutOfGas);
                }
                Ok(Account {
                    nonce: 1,
                    balance: 0u128.saturating_sub(value),
                })
            }
            (Some(Account { nonce, balance }), Tx { nonce: n, value, gas_limit, .. })
                if n < nonce =>
            {
                Err(ApplyError::NonceTooLow {
                    expected: nonce,
                    got: n,
                })
            }
            (Some(Account { nonce, balance }), Tx { nonce: n, value, gas_limit, .. })
                if n == nonce =>
            {
                if gas_limit == 0 {
                    return Err(ApplyError::OutOfGas);
                }
                if balance < value {
                    return Err(ApplyError::InsufficientBalance {
                        need: value,
                        have: balance,
                    });
                }
                Ok(Account {
                    nonce: nonce + 1,
                    balance: balance - value,
                })
            }
            (Some(Account { nonce, .. }), Tx { nonce: n, .. }) if n > nonce => {
                // 未来 nonce：进 mempool 队列，这里简化为 no-op 错误
                Err(ApplyError::NonceTooLow {
                    expected: nonce,
                    got: n,
                })
            }
            _ => unreachable!("exhaustive"),
        }
    }

    pub fn demonstrate() {
        println!("## 场景 4：账户状态转移（(Option<Account>, Tx) 嵌套 match）");

        let acct = Account {
            nonce: 5,
            balance: 1000,
        };
        let ok = apply_tx(
            Some(acct),
            Tx {
                from: [0u8; 20],
                nonce: 5,
                value: 100,
                gas_limit: 21_000,
            },
        );
        let bad = apply_tx(
            Some(acct),
            Tx {
                from: [0u8; 20],
                nonce: 4,
                value: 10,
                gas_limit: 21_000,
            },
        );
        println!("  valid tx → {:?}", ok);
        println!("  stale nonce → {:?}", bad);
        println!("关键：Option + 守卫合并「账户不存在 / 存在」两条路径\n");
    }
}

// ============================================================================
// 场景 5：MEV Bundle 分类
// ============================================================================
/// **生产问题**：Flashbots bundle 里有多笔 tx，searcher 要识别 arbitrage /
/// liquidation / sandwich 以决定 priority fee 和 simulation 策略。
///
/// **模式匹配套路**：match slice 模式 `[..]` 长度 + 内部 tx 特征。
pub mod mev_bundle_classify {

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum TxKind {
        Swap,
        Liquidation,
        Transfer,
        Other,
    }

    #[derive(Debug, Clone, Copy)]
    pub struct BundleTx {
        pub kind: TxKind,
        pub profit_wei: u128,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum BundleKind {
        Arbitrage,
        Liquidation,
        Sandwich,
        Unknown,
    }

    pub fn classify(txs: &[BundleTx]) -> BundleKind {
        match txs {
            [a, b, c] if a.kind == TxKind::Swap && c.kind == TxKind::Swap && b.profit_wei > 0 => {
                BundleKind::Sandwich
            }
            [single] if single.kind == TxKind::Liquidation => BundleKind::Liquidation,
            [] => BundleKind::Unknown,
            bundle if bundle.iter().all(|t| t.kind == TxKind::Swap) => BundleKind::Arbitrage,
            _ => BundleKind::Unknown,
        }
    }

    pub fn demonstrate() {
        println!("## 场景 5：MEV Bundle 分类（slice 长度 + 守卫）");

        let sandwich = [
            BundleTx {
                kind: TxKind::Swap,
                profit_wei: 0,
            },
            BundleTx {
                kind: TxKind::Swap,
                profit_wei: 50,
            },
            BundleTx {
                kind: TxKind::Swap,
                profit_wei: 0,
            },
        ];
        let arb = [
            BundleTx {
                kind: TxKind::Swap,
                profit_wei: 10,
            },
            BundleTx {
                kind: TxKind::Swap,
                profit_wei: 20,
            },
        ];
        println!("  3-tx → {:?}", classify(&sandwich));
        println!("  2-swap → {:?}", classify(&arb));
        println!("关键：slice pattern `[a,b,c]` 精确匹配 bundle 拓扑\n");
    }
}

// ============================================================================
// 场景 6：链重组处理（BlockStatus 状态机）
// ============================================================================
/// **生产问题**：indexer 在 `newHeads` 订阅里收到 canonical / fork / reorg 通知，
/// 必须决定 rewind 多少块、哪些 tx 回滚。
///
/// **模式匹配套路**：match `ChainEvent`，守卫区分深度。
pub mod reorg_handler {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum BlockStatus {
        Canonical,
        Fork,
    }

    #[derive(Debug, Clone, Copy)]
    pub enum ChainEvent {
        NewCanonical { number: u64 },
        Reorg { from: u64, to: u64, depth: u64 },
        Finalized { number: u64 },
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum IndexerAction {
        Append,
        Rewind(u64),
        NoOp,
        AlertOps,
    }

    pub fn on_event(ev: ChainEvent, last_indexed: u64) -> IndexerAction {
        match ev {
            ChainEvent::NewCanonical { number } if number == last_indexed + 1 => {
                IndexerAction::Append
            }
            ChainEvent::NewCanonical { .. } => IndexerAction::Rewind(1),
            ChainEvent::Reorg { depth, .. } if depth > 10 => IndexerAction::AlertOps,
            ChainEvent::Reorg { from, .. } => IndexerAction::Rewind(from),
            ChainEvent::Finalized { number } if number <= last_indexed => IndexerAction::NoOp,
            ChainEvent::Finalized { .. } => IndexerAction::NoOp,
        }
    }

    pub fn demonstrate() {
        println!("## 场景 6：链重组处理（ChainEvent + 深度守卫）");

        let events = [
            ChainEvent::NewCanonical { number: 101 },
            ChainEvent::Reorg {
                from: 98,
                to: 101,
                depth: 3,
            },
            ChainEvent::Reorg {
                from: 50,
                to: 101,
                depth: 51,
            },
        ];
        let last = 100u64;
        for ev in events {
            println!("  {:?} → {:?}", ev, on_event(ev, last));
        }
        println!("关键：守卫 `depth > 10` 把运维告警和普通 reorg 分开\n");
    }
}

pub fn demonstrate() {
    tx_type_dispatch::demonstrate();
    evm_opcode::demonstrate();
    event_log_decode::demonstrate();
    account_transition::demonstrate();
    mev_bundle_classify::demonstrate();
    reorg_handler::demonstrate();
}
