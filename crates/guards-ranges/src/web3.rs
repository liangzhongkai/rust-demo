//! # Web3 生产场景：守卫与范围
//!
//! 链上系统的 guard/range 出现在：
//! - **协议常量区间**：opcode、chain id、decimals、confirmations
//! - **费用分层**：priority fee / blob gas / base fee 档位
//! - **安全边界**：零地址、amount 上下界、calldata 长度
//!
//! 下面 6 个场景对应 revm、indexer、searcher、钱包里的常见写法。

#![allow(dead_code)]

pub type Address = [u8; 20];

const ZERO_ADDR: Address = [0u8; 20];

// ============================================================================
// 场景 1：Priority fee 分档（range + tx type guard）
// ============================================================================
/// **生产问题**：mempool 排序靠 effective priority fee；searcher 要按档位
/// 决定 bundle 是否值得提交。
///
/// **范围套路**：wei 分档 + guard 区分 legacy / 1559。
pub mod priority_fee_tier {
    #[derive(Debug, Clone, Copy)]
    pub enum TxKind {
        Legacy { gas_price: u128 },
        Eip1559 { max_priority: u128 },
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum FeeTier {
        Low,
        Market,
        Aggressive,
        Insane,
    }

    pub fn tier(tx: TxKind) -> FeeTier {
        let priority = match tx {
            TxKind::Legacy { gas_price } => gas_price,
            TxKind::Eip1559 { max_priority } => max_priority,
        };
        match priority {
            0..=1_000_000_000 => FeeTier::Low,           // ≤ 1 gwei
            1_000_000_001..=30_000_000_000 => FeeTier::Market,
            30_000_000_001..=100_000_000_000 => FeeTier::Aggressive,
            _ => FeeTier::Insane,
        }
    }

    pub fn demonstrate() {
        println!("## 场景 1：Priority fee 分档（range on wei）");

        let txs = [
            TxKind::Legacy { gas_price: 500_000_000 },
            TxKind::Eip1559 {
                max_priority: 50_000_000_000,
            },
        ];
        for tx in txs {
            println!("  {:?} → {:?}", tx, tier(tx));
        }
        println!("关键：legacy / 1559 先统一成 priority 再 range 分档\n");
    }
}

// ============================================================================
// 场景 2：确认数 finality（range + reorg guard）
// ============================================================================
/// **生产问题**：CEX 充值 / bridge 放行需要 N 个确认；深度 reorg 要告警。
///
/// **范围套路**：confirmations range 映射 risk level。
pub mod finality_depth {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Risk {
        Pending,
        Probabilistic,
        Safe,
        Finalized,
    }

    pub fn assess(confirmations: u64, reorg_depth: u64) -> Risk {
        if reorg_depth > 10 {
            return Risk::Pending; // 深度 reorg，回退到 pending
        }
        match confirmations {
            0 => Risk::Pending,
            1..=5 => Risk::Probabilistic,
            6..=31 => Risk::Safe,
            _ => Risk::Finalized,
        }
    }

    pub fn demonstrate() {
        println!("## 场景 2：确认数 finality（range + reorg guard）");

        for (conf, reorg) in [(12, 0), (3, 0), (20, 15)] {
            println!("  conf={} reorg={} → {:?}", conf, reorg, assess(conf, reorg));
        }
        println!("关键：`reorg_depth > 10` guard 覆盖一切 range 结论\n");
    }
}

// ============================================================================
// 场景 3：Token transfer 边界（amount range + zero address guard）
// ============================================================================
/// **生产问题**：钱包 / indexer 在广播前校验 amount、from/to 合法性。
///
/// **守卫套路**：零地址字面量 arm + amount range。
pub mod transfer_bounds {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ValidateResult {
        Ok,
        ZeroAddress,
        ZeroAmount,
        Overflow,
    }

    pub fn validate(from: Address, to: Address, amount: u128) -> ValidateResult {
        match (from, to, amount) {
            (ZERO_ADDR, _, _) | (_, ZERO_ADDR, _) => ValidateResult::ZeroAddress,
            (_, _, 0) => ValidateResult::ZeroAmount,
            (_, _, a) if a > u128::MAX / 2 => ValidateResult::Overflow,
            _ => ValidateResult::Ok,
        }
    }

    pub fn demonstrate() {
        println!("## 场景 3：Transfer 边界（地址 guard + amount range）");

        let from = [1u8; 20];
        println!("  zero to → {:?}", validate(from, ZERO_ADDR, 100));
        println!("  zero amount → {:?}", validate(from, [2u8; 20], 0));
        println!("关键：特殊地址用常量 arm，不用 guard 比较数组\n");
    }
}

// ============================================================================
// 场景 4：Calldata 长度路由（range dispatch）
// ============================================================================
/// **生产问题**：EOA transfer calldata=0；ERC20 transfer=68 bytes；
/// 更长可能是 swap / multicall，模拟器走不同路径。
///
/// **范围套路**：`len` range 一次分类。
pub mod calldata_router {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum CalldataKind {
        Empty,
        Erc20Transfer,
        Erc20Approve,
        ContractCall,
    }

    pub fn classify(input: &[u8]) -> CalldataKind {
        match input.len() {
            0 => CalldataKind::Empty,
            4..=68 if input.starts_with(&[0xa9, 0x05, 0x9c, 0xbb]) => CalldataKind::Erc20Transfer,
            4..=68 if input.starts_with(&[0x09, 0x5e, 0xa7, 0xb3]) => CalldataKind::Erc20Approve,
            4.. => CalldataKind::ContractCall,
            _ => CalldataKind::ContractCall,
        }
    }

    pub fn demonstrate() {
        println!("## 场景 4：Calldata 长度（len range + selector guard）");

        let empty: &[u8] = &[];
        let mut transfer = [0u8; 68];
        transfer[..4].copy_from_slice(&[0xa9, 0x05, 0x9c, 0xbb]);
        println!("  empty → {:?}", classify(empty));
        println!("  transfer → {:?}", classify(&transfer));
        println!("关键：`4..=68` range + `starts_with` guard 精分类\n");
    }
}

// ============================================================================
// 场景 5：Chain ID 白名单（离散 + range 混合）
// ============================================================================
/// **生产问题**：多链钱包 / bridge 只支持已知 chain id；测试网与主网
/// 范围不同。
///
/// **范围套路**：主网 id 离散 match，测试网用 range。
pub mod chain_id_gate {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ChainSupport {
        Ethereum,
        L2,
        Testnet,
        Unknown,
    }

    pub fn classify(chain_id: u64) -> ChainSupport {
        match chain_id {
            1 => ChainSupport::Ethereum,
            10 | 42161 | 8453 => ChainSupport::L2,
            11_155_111..=11_155_120 => ChainSupport::Testnet, // 常见 testnet 段
            _ => ChainSupport::Unknown,
        }
    }

    pub fn demonstrate() {
        println!("## 场景 5：Chain ID（离散 + range 混合）");

        for id in [1, 42161, 11_155_111, 999] {
            println!("  chain {} → {:?}", id, classify(id));
        }
        println!("关键：知名 id 离散列出；测试网段用 range 覆盖\n");
    }
}

// ============================================================================
// 场景 6：Blob 交易 gas（EIP-4844 count range + guard）
// ============================================================================
/// **生产问题**：Type-3 交易 blob 数量 1..=6，超出无效；blob gas 影响
/// block builder 排序。
///
/// **守卫/范围套路**：blob count range + max_fee guard。
pub mod blob_tx_validate {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum BlobVerdict {
        Valid,
        TooManyBlobs,
        FeeTooLow,
        Invalid,
    }

    pub fn validate(blob_count: u8, max_fee_per_blob: u128, min_blob_fee: u128) -> BlobVerdict {
        match blob_count {
            0 => BlobVerdict::Invalid,
            1..=6 if max_fee_per_blob >= min_blob_fee => BlobVerdict::Valid,
            1..=6 => BlobVerdict::FeeTooLow,
            _ => BlobVerdict::TooManyBlobs,
        }
    }

    pub fn demonstrate() {
        println!("## 场景 6：Blob 交易（count range + fee guard）");

        for (n, fee) in [(2, 1_000), (2, 10), (8, 1_000)] {
            println!(
                "  blobs={} fee={} → {:?}",
                n,
                fee,
                validate(n, fee, 100)
            );
        }
        println!("关键：同 range `1..=6` 内 fee guard 区分 Valid/FeeTooLow\n");
    }
}

pub fn demonstrate() {
    priority_fee_tier::demonstrate();
    finality_depth::demonstrate();
    transfer_bounds::demonstrate();
    calldata_router::demonstrate();
    chain_id_gate::demonstrate();
    blob_tx_validate::demonstrate();
}
