use bitcoin::hashes::sha256;
use bitcoin::{Transaction, ScriptBuf};

/// A transaction paired with its confirmation height from Electrum's `get_history`.
///
/// The Electrum protocol returns a `height` field for each history entry:
/// - `height > 0`  → confirmed at that block height
/// - `height == 0`  → unconfirmed (in mempool)
/// - `height < 0`  → unconfirmed with unconfirmed parents
#[derive(Debug, Clone)]
pub struct HistoryTx {
    pub tx: Transaction,
    pub height: i32,
}

#[derive(Debug, Clone)]
pub enum EngineEvent {
    Connected,
    ScriptHashChanged(sha256::Hash),
    ScriptHashHistory {
        hash: sha256::Hash,
        txs: Vec<HistoryTx>,             // CHANGED: was Vec<Transaction>
    },
}

#[derive(Debug, Clone)]
pub enum EngineCommand {
    Subscribe(sha256::Hash),
    FetchHistory(sha256::Hash),
    ApplyTransactions {
        script: ScriptBuf,
        txs: Vec<HistoryTx>,             // CHANGED: was Vec<Transaction>
    },
}