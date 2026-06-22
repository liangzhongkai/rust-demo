//! Mock async pipeline that mirrors the architecture from the Medium article:
//!
//! ```text
//! NetworkSimulator ──► TxEventHandler ──► Processor ──► Display
//!        │                    │                │
//!        └── PeerEvents ──────┴── UiUpdates ───┘
//! ```
//!
//! Each stage runs as an independent Tokio task connected via MPSC channels.

use crate::analysis::{MockTx, MockTxType, TxAnalysisResult, analyze_mock_tx};
use crate::types::{PeerInfo, TxSource, UiUpdate};
use alloy_primitives::{Address, B256, U256};
use anyhow::Result;
use rand::{Rng, SeedableRng};
use rand::rngs::StdRng;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::info;

const MOCK_PEERS: &[(&str, &str)] = &[
    ("peer-geth-1", "Geth/v1.14.0"),
    ("peer-reth-2", "Reth/v1.1.0"),
    ("peer-besu-3", "Besu/v24.3.0"),
];

/// Simulated network events — analogous to `NetworkTransactionEvent` from reth-network.
#[derive(Debug)]
enum SimulatedNetworkEvent {
    SessionEstablished { peer: PeerInfo },
    IncomingPooledTransactionHashes { peer_id: String, hashes: Vec<B256> },
    IncomingTransactions { peer_id: String, txs: Vec<Arc<MockTx>> },
}

pub async fn run_mock_crawler(duration_secs: u64) -> Result<()> {
    let (net_tx, net_rx) = mpsc::unbounded_channel::<SimulatedNetworkEvent>();
    let (decoded_tx_tx, mut decoded_tx_rx) = mpsc::unbounded_channel::<Arc<MockTx>>();
    let (ui_tx, ui_rx) = mpsc::unbounded_channel::<UiUpdate>();

    let decoded_tx_for_net = decoded_tx_tx.clone();

    // Task 1: Network simulator — produces P2P-like events
    let simulator = tokio::spawn(async move {
        network_simulator(net_tx, duration_secs).await;
    });
    let tx_handler = tokio::spawn(async move {
        tx_event_handler(net_rx, decoded_tx_for_net).await;
    });

    // Task 3: Peer event handler — mirrors EthP2PHandler::handle_network_event_wrapper
    // (merged into simulator for simplicity; peer updates sent directly)

    // Task 4: Processor — mirrors the decoded transaction processor task
    let ui_tx_for_processor = ui_tx.clone();
    let processor = tokio::spawn(async move {
        while let Some(tx) = decoded_tx_rx.recv().await {
            let result = analyze_mock_tx(&tx);
            if ui_tx_for_processor
                .send(UiUpdate::NewTx(Box::new(result)))
                .is_err()
            {
                break;
            }
        }
    });

    // Task 5: Display — simplified stand-in for the ratatui UI task
    let display = tokio::spawn(async move {
        display_loop(ui_rx).await;
    });

    // Shut down display once the simulator finishes
    let shutdown = tokio::spawn(async move {
        simulator.await.ok();
        let _ = ui_tx.send(UiUpdate::Shutdown);
    });

    // Drop our copies so worker tasks exit when their peers finish
    drop(decoded_tx_tx);

    let _ = tokio::join!(tx_handler, processor, display, shutdown);
    Ok(())
}

async fn network_simulator(
    net_tx: mpsc::UnboundedSender<SimulatedNetworkEvent>,
    duration_secs: u64,
) {
    info!("🌐 Mock network starting — simulating Discv4 + RLPx handshake");

    for (id, client) in MOCK_PEERS {
        let peer = PeerInfo {
            id: id.to_string(),
            client_version: client.to_string(),
        };
        let _ = net_tx.send(SimulatedNetworkEvent::SessionEstablished {
            peer: peer.clone(),
        });
        info!("🤝 Session established with {} ({})", peer.id, peer.client_version);
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let deadline = Instant::now() + Duration::from_secs(duration_secs);
    let mut rng = StdRng::from_entropy();
    let mut tick = 0u64;

    while Instant::now() < deadline {
        let peer_idx = rng.gen_range(0..MOCK_PEERS.len());
        let (peer_id, _) = MOCK_PEERS[peer_idx];

        // Alternate between the two mempool acquisition paths from the article
        if tick % 2 == 0 {
            // Path 1: hash broadcast → GetPooledTransactions → PooledTransactions
            let hash = random_hash(&mut rng);
            info!(
                peer = peer_id,
                hash = %hash,
                "📡 NewPooledTransactionHashes (Path 1: hash → request → response)"
            );
            let _ = net_tx.send(SimulatedNetworkEvent::IncomingPooledTransactionHashes {
                peer_id: peer_id.to_string(),
                hashes: vec![hash],
            });
        } else {
            // Path 2: direct full transaction broadcast
            let tx = Arc::new(random_tx(&mut rng, tick));
            info!(
                peer = peer_id,
                hash = %tx.hash,
                "📨 Transactions broadcast (Path 2: direct full tx)"
            );
            let _ = net_tx.send(SimulatedNetworkEvent::IncomingTransactions {
                peer_id: peer_id.to_string(),
                txs: vec![tx],
            });
        }

        tick += 1;
        tokio::time::sleep(Duration::from_millis(800)).await;
    }

    info!("🛑 Mock network shutting down");
}

async fn tx_event_handler(
    mut net_rx: mpsc::UnboundedReceiver<SimulatedNetworkEvent>,
    decoded_tx_tx: mpsc::UnboundedSender<Arc<MockTx>>,
) {
    // In-memory "mempool" for hash → full tx lookup (simulates peer response)
    let pool: dashmap::DashMap<B256, Arc<MockTx>> = dashmap::DashMap::new();

    while let Some(event) = net_rx.recv().await {
        match event {
            SimulatedNetworkEvent::SessionEstablished { peer } => {
                info!("Peer session ready: {} ({})", peer.id, peer.client_version);
            }

            SimulatedNetworkEvent::IncomingPooledTransactionHashes { peer_id, hashes } => {
                for hash in hashes {
                    // Simulate GetPooledTransactions request/response
                    if let Some(tx) = pool.get(&hash) {
                        info!(
                            peer = %peer_id,
                            hash = %hash,
                            source = ?TxSource::HashRequestResponse,
                            "✅ PooledTransactions response received"
                        );
                        let _ = decoded_tx_tx.send(Arc::clone(tx.value()));
                    } else {
                        // Peer would return the tx; we synthesize it here
                        let tx = Arc::new(synthetic_tx(hash));
                        pool.insert(hash, Arc::clone(&tx));
                        info!(
                            peer = %peer_id,
                            hash = %hash,
                            source = ?TxSource::HashRequestResponse,
                            "✅ PooledTransactions response (synthesized)"
                        );
                        let _ = decoded_tx_tx.send(tx);
                    }
                }
            }

            SimulatedNetworkEvent::IncomingTransactions { peer_id, txs } => {
                for tx in txs {
                    pool.insert(tx.hash, Arc::clone(&tx));
                    info!(
                        peer = %peer_id,
                        hash = %tx.hash,
                        source = ?TxSource::DirectBroadcast,
                        "✅ Direct TransactionSigned forwarded to processor"
                    );
                    let _ = decoded_tx_tx.send(tx);
                }
            }
        }
    }
}

async fn display_loop(mut ui_rx: mpsc::UnboundedReceiver<UiUpdate>) {
    let mut total = 0u64;
    let mut hash_path = 0u64;
    let mut direct_path = 0u64;

    println!("\n{}", "=".repeat(80));
    println!("  Ethereum Mempool Crawler — Mock Pipeline");
    println!("  Architecture: NetworkSimulator → TxHandler → Processor → Display");
    println!("{}\n", "=".repeat(80));

    while let Some(update) = ui_rx.recv().await {
        match update {
            UiUpdate::PeerUpdate(data) => {
                println!(
                    "👥 Peers connected: {}",
                    data.connected_peers
                        .iter()
                        .map(|p| format!("{} ({})", p.id, p.client_version))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            UiUpdate::NewTx(tx) => {
                total += 1;
                // Alternate counting based on tick parity (approximation for demo)
                if total % 2 == 1 {
                    hash_path += 1;
                } else {
                    direct_path += 1;
                }
                print_tx(&tx, total);
            }
            UiUpdate::Shutdown => break,
        }
    }

    println!("\n{}", "─".repeat(80));
    println!("  Summary: {total} txs observed");
    println!("    Path 1 (hash→request→response): ~{hash_path}");
    println!("    Path 2 (direct broadcast):      ~{direct_path}");
    println!("{}\n", "─".repeat(80));
}

fn print_tx(tx: &TxAnalysisResult, seq: u64) {
    let tx_type = match tx.tx_type {
        MockTxType::Legacy => "Legacy",
        MockTxType::Eip2930 => "EIP-2930",
        MockTxType::Eip1559 => "EIP-1559",
        MockTxType::Eip4844 => "EIP-4844",
    };
    let receiver = tx
        .receiver
        .map(|a| format!("{a}"))
        .unwrap_or_else(|| "(contract creation)".to_string());
    let gas = tx
        .gas_price_or_max_fee
        .map(|g| format!("{g} wei"))
        .unwrap_or_else(|| "n/a".to_string());

    println!(
        "[{seq:>3}] {tx_type} | hash={} | from={:?} → {receiver} | value={} wei | gas={gas} | input={}B",
        tx.hash,
        tx.sender,
        tx.value,
        tx.input_len,
    );
}

fn random_hash(rng: &mut StdRng) -> B256 {
    let mut bytes = [0u8; 32];
    rng.fill(&mut bytes);
    B256::from(bytes)
}

fn random_tx(rng: &mut StdRng, seed: u64) -> MockTx {
    let hash = B256::from(U256::from(seed + 1));
    MockTx {
        hash,
        tx_type: MockTxType::Eip1559,
        sender: Address::from([0x01; 20]),
        receiver: Some(Address::from([0x02; 20])),
        value: U256::from(rng.gen_range(1_000u64..1_000_000u64)),
        gas_limit: 21_000,
        max_fee_per_gas: rng.gen_range(10u128..100u128) * 1_000_000_000,
        max_priority_fee_per_gas: Some(rng.gen_range(1u128..5u128) * 1_000_000_000),
        input_len: rng.gen_range(0..256),
    }
}

fn synthetic_tx(hash: B256) -> MockTx {
    MockTx {
        hash,
        tx_type: MockTxType::Eip1559,
        sender: Address::from([0xab; 20]),
        receiver: Some(Address::from([0xcd; 20])),
        value: U256::from(42_000),
        gas_limit: 21_000,
        max_fee_per_gas: 30_000_000_000,
        max_priority_fee_per_gas: Some(2_000_000_000),
        input_len: 0,
    }
}
