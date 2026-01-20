use std::collections::{BTreeSet, HashMap, VecDeque};

use bitcoin::hashes::sha256;
use bitcoin::Txid;
use bitcoin::{ScriptBuf, Transaction};

use crate::streaming::electrum::ElectrumApi;

/// Pure in-memory mock Electrum client for tests
pub struct MockElectrumClient {
    pub subscribed: BTreeSet<sha256::Hash>,
    pub histories: HashMap<sha256::Hash, Vec<Txid>>,
    pub notifications: VecDeque<sha256::Hash>,
}

impl MockElectrumClient {
    pub fn new() -> Self {
        Self {
            subscribed: BTreeSet::new(),
            histories: HashMap::new(),
            notifications: VecDeque::new(),
        }
    }

    pub fn push_notification(&mut self, hash: sha256::Hash) {
        self.notifications.push_back(hash);
    }

    pub fn push_tx(&mut self, hash: sha256::Hash, txid: Txid) {
        self.histories.entry(hash).or_default().push(txid);
        self.notifications.push_back(hash);
    }
}

impl ElectrumApi for MockElectrumClient {
    fn register_script(&mut self, _script: ScriptBuf, hash: sha256::Hash) {
        self.subscribed.insert(hash);
    }

    fn poll_scripthash_changed(&mut self) -> Option<sha256::Hash> {
        self.notifications.pop_front()
    }
    fn fetch_history_txs(&mut self, _hash: sha256::Hash) -> Vec<Transaction> {
        // Mock returns empty tx list
        vec![]
    }
    fn request_history(&mut self, hash: sha256::Hash) {
        // Mock is synchronous: history is always ready
        // So we do nothing here.
        let _ = hash;
    }
}