//! Demonstrates the two mempool acquisition paths from the Medium article:
//!
//! 1. `NewPooledTransactionHashes` → `GetPooledTransactions` → `PooledTransactions`
//! 2. Direct `Transactions` broadcast

use alloy_primitives::{Address, B256, U256};
use eth_mempool_crawler::{MockTx, MockTxType, TxSource, analyze_mock_tx};
use std::sync::Arc;
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== Mempool Flow Demo ===\n");

    let (tx_tx, mut tx_rx) = mpsc::unbounded_channel::<(TxSource, Arc<MockTx>)>();

    // Simulate Path 1: hash announcement → request → response
    let hash = B256::from(U256::from(0xdeadbeefu64));
    let path1_tx = Arc::new(MockTx {
        hash,
        tx_type: MockTxType::Eip1559,
        sender: Address::from([0x01; 20]),
        receiver: Some(Address::from([0x02; 20])),
        value: U256::from(1_000_000_000_000_000_000u64),
        gas_limit: 21_000,
        max_fee_per_gas: 30_000_000_000,
        max_priority_fee_per_gas: Some(2_000_000_000),
        input_len: 4,
    });

    println!("Path 1: peer announces hash {hash}");
    println!("        crawler sends GetPooledTransactions");
    println!("        peer responds with PooledTransactions\n");
    tx_tx.send((TxSource::HashRequestResponse, path1_tx))?;

    // Simulate Path 2: direct full transaction broadcast
    let path2_tx = Arc::new(MockTx {
        hash: B256::from(U256::from(0xcafebabeu64)),
        tx_type: MockTxType::Legacy,
        sender: Address::from([0x03; 20]),
        receiver: Some(Address::from([0x04; 20])),
        value: U256::from(500_000_000_000_000_000u64),
        gas_limit: 65_000,
        max_fee_per_gas: 25_000_000_000,
        max_priority_fee_per_gas: None,
        input_len: 128,
    });

    println!("Path 2: peer broadcasts full TransactionSigned directly\n");
    tx_tx.send((TxSource::DirectBroadcast, path2_tx))?;

    drop(tx_tx);

    while let Some((source, tx)) = tx_rx.recv().await {
        let result = analyze_mock_tx(&tx);
        println!(
            "[{:?}] hash={} sender={:?} value={} wei gas={:?}",
            source, result.hash, result.sender, result.value, result.gas_price_or_max_fee
        );
    }

    Ok(())
}
