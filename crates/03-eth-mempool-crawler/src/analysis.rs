use alloy_primitives::{Address, B256, U256};
use std::time::{SystemTime, UNIX_EPOCH};

/// Simplified transaction type for the mock pipeline (no Reth dependency).
#[derive(Debug, Clone)]
pub struct MockTx {
    pub hash: B256,
    pub tx_type: MockTxType,
    pub sender: Address,
    pub receiver: Option<Address>,
    pub value: U256,
    pub gas_limit: u64,
    pub max_fee_per_gas: u128,
    pub max_priority_fee_per_gas: Option<u128>,
    pub input_len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MockTxType {
    Legacy,
    Eip2930,
    Eip1559,
    Eip4844,
}

#[derive(Debug, Clone)]
pub struct TxAnalysisResult {
    pub hash: B256,
    pub tx_type: MockTxType,
    pub sender: Option<Address>,
    pub receiver: Option<Address>,
    pub value: U256,
    pub gas_limit: u64,
    pub gas_price_or_max_fee: Option<u128>,
    pub max_priority_fee: Option<u128>,
    pub input_len: usize,
    pub first_seen_unix_ms: u64,
    pub is_private: bool,
}

pub fn analyze_mock_tx(tx: &MockTx) -> TxAnalysisResult {
    let (gas_price_or_max_fee, max_priority_fee) = match tx.tx_type {
        MockTxType::Legacy | MockTxType::Eip2930 => (Some(tx.max_fee_per_gas), None),
        MockTxType::Eip1559 | MockTxType::Eip4844 => {
            (Some(tx.max_fee_per_gas), tx.max_priority_fee_per_gas)
        }
    };

    TxAnalysisResult {
        hash: tx.hash,
        tx_type: tx.tx_type,
        sender: Some(tx.sender),
        receiver: tx.receiver,
        value: tx.value,
        gas_limit: tx.gas_limit,
        gas_price_or_max_fee,
        max_priority_fee,
        input_len: tx.input_len,
        first_seen_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        is_private: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run_mock_crawler;

    #[test]
    fn analyze_mock_tx_extracts_fields() {
        let tx = MockTx {
            hash: B256::from(U256::from(1u64)),
            tx_type: MockTxType::Eip1559,
            sender: Address::from([0x01; 20]),
            receiver: Some(Address::from([0x02; 20])),
            value: U256::from(1000u64),
            gas_limit: 21_000,
            max_fee_per_gas: 30_000_000_000,
            max_priority_fee_per_gas: Some(2_000_000_000),
            input_len: 4,
        };

        let result = analyze_mock_tx(&tx);
        assert_eq!(result.hash, tx.hash);
        assert_eq!(result.sender, Some(tx.sender));
        assert_eq!(result.gas_price_or_max_fee, Some(30_000_000_000));
        assert_eq!(result.max_priority_fee, Some(2_000_000_000));
    }

    #[tokio::test]
    async fn mock_pipeline_completes() {
        run_mock_crawler(1).await.expect("mock pipeline should finish");
    }
}
