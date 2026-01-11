use bitcoin::hashes::sha256;
use bitcoin::Txid;

#[derive(Debug, Clone)]
pub enum EngineEvent {
    Connected,
    ScriptHashChanged(sha256::Hash),
    ScriptHashHistory {
        hash: sha256::Hash,
        txs: Vec<Txid>,
    },
}

#[derive(Debug, Clone)]

pub enum EngineCommand {
    Subscribe(sha256::Hash),
    FetchHistory(sha256::Hash),
}

