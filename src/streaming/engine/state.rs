use std::collections::{BTreeSet, HashMap};
use std::time::Instant;
use bitcoin::{Txid, ScriptBuf};
use bitcoin::hashes::sha256;

use crate::streaming::domain::spk_tracker::DerivedSpkTracker;

#[derive(Debug)]
pub struct EngineState<K> {
    pub start_time: Instant,
    pub first_history_seen: bool,

    pub spk_tracker: DerivedSpkTracker<K>,

    /// scripthash -> (keychain, index)
    pub spk_index_by_hash: HashMap<sha256::Hash, (K, u32)>,

    /// scripthash -> Script
    pub script_by_hash: HashMap<sha256::Hash, ScriptBuf>,

    pub subscribed: BTreeSet<sha256::Hash>,
    pub histories: HashMap<sha256::Hash, Vec<Txid>>,
    pub connected: bool,
}