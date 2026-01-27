use bitcoin::{Txid, ScriptBuf, Transaction};
use bitcoin::hashes::sha256;

/// Commands sent FROM Driver TO Async Client
#[derive(Debug)]
pub enum ElectrumCommand {
    Subscribe {
        script: ScriptBuf,
        hash: sha256::Hash,
    },
    FetchHistory {
        hash: sha256::Hash,
    },
    ApplyTransactions { script: ScriptBuf, txs: Vec<Transaction> },
}

/// Events sent FROM Async Client TO Driver
#[derive(Debug)]
pub enum ElectrumEvent {
    Connected,
    Disconnected,

    ScriptHashChanged {
        hash: sha256::Hash,
    },

    ScriptHashHistory {
        hash: sha256::Hash,
        txids: Vec<Txid>,
    },
}