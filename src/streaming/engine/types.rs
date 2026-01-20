use bitcoin::hashes::sha256;
use bitcoin::{Transaction, ScriptBuf};

#[derive(Debug, Clone)]
pub enum EngineEvent {
    Connected,
    ScriptHashChanged(sha256::Hash),
    ScriptHashHistory {
        hash: sha256::Hash,
        txs: Vec<Transaction>,
    },
}

#[derive(Debug, Clone)]

pub enum EngineCommand {
    Subscribe(sha256::Hash),
    FetchHistory(sha256::Hash),
    ApplyTransactions { script: ScriptBuf, txs: Vec<Transaction>, },
}

