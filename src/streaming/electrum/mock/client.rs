use std::collections::{BTreeSet, HashMap, VecDeque};

use bitcoin::hashes::sha256;
use bitcoin::{block, ScriptBuf, Transaction};

use crate::streaming::electrum::ElectrumApi;
use crate::streaming::engine::types::HistoryTx;

/// Pure in-memory mock Electrum client for tests
pub struct MockElectrumClient {
    pub subscribed: BTreeSet<sha256::Hash>,
    pub scripts: HashMap<sha256::Hash, ScriptBuf>,
    pub histories: HashMap<sha256::Hash, Vec<Transaction>>,
    pub notifications: VecDeque<sha256::Hash>,
}

impl MockElectrumClient {
    pub fn new() -> Self {
        Self {
            subscribed: BTreeSet::new(),
            scripts: HashMap::new(),
            histories: HashMap::new(),
            notifications: VecDeque::new(),
        }
    }

    pub fn push_history(&mut self, hash: sha256::Hash, txs: Vec<Transaction>) {
        println!("[MOCK] push_history called for {}", hash); // DEBUG LOG
        self.histories.insert(hash, txs);
        
        // CRITICAL: Push to queue
        self.notifications.push_back(hash);
        println!("[MOCK] Queue size is now: {}", self.notifications.len()); // DEBUG LOG
    }

    pub fn subscribed_len(&self) -> usize {
        self.subscribed.len()
    }

    pub fn last_subscribed(&self) -> Option<sha256::Hash> {
        self.subscribed.iter().next().cloned()
    }
}

impl ElectrumApi for MockElectrumClient {
    fn register_script(&mut self, script: ScriptBuf, hash: sha256::Hash) {
        self.subscribed.insert(hash);
        self.scripts.insert(hash, script);
    }

    fn poll_scripthash_changed(&mut self) -> Option<sha256::Hash> {
        let item = self.notifications.pop_front();
        if let Some(h) = item {
            println!("[MOCK] Popped notification for {}", h); // DEBUG LOG
        }
        item
    }

    fn fetch_history_txs(&mut self, hash: sha256::Hash) -> Option<Vec<HistoryTx>> {
        self.histories.get(&hash).cloned().map(|txs| {
            txs.into_iter()
                .map(|tx| HistoryTx { tx, height: 0 }) // Mock: treat all as unconfirmed
                .collect()
        })
    }

    fn request_history(&mut self, hash: sha256::Hash) {
        println!("[MOCK] request_history called for {}", hash); // DEBUG LOG
        // Simulate async completion
        self.notifications.push_back(hash);
    }

    fn get_cached_header(&self, _height: u32) -> Option<block::Header> {
        None // Mock doesn't need real block headers
    }
}