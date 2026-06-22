use crate::analysis::TxAnalysisResult;
use std::time::Instant;

/// Lightweight peer identity used by the mock pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PeerInfo {
    pub id: String,
    pub client_version: String,
}

#[derive(Debug, Clone)]
pub struct PeerUpdateData {
    pub connected_peers: Vec<PeerInfo>,
    pub timestamp: Instant,
}

/// How a transaction entered the pipeline — mirrors the two P2P paths from the article.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxSource {
    /// `NewPooledTransactionHashes` → `GetPooledTransactions` → `PooledTransactions`
    HashRequestResponse,
    /// Direct `Transactions` broadcast
    DirectBroadcast,
}

#[derive(Debug)]
pub enum UiUpdate {
    PeerUpdate(PeerUpdateData),
    NewTx(Box<TxAnalysisResult>),
    Shutdown,
}
