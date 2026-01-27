//! Streaming sync decision engine.
//!
//! This is a PURE state machine.
//! - No network
//! - No async
//! - No IO
//! - Fully deterministic
//!
//! It consumes EngineEvent and emits EngineCommand.

pub mod state;
mod logic;
pub mod types;

#[cfg(test)]
mod tests;

pub use crate::streaming::engine::types::{EngineEvent, EngineCommand};

use std::collections::{BTreeSet, HashMap};
use std::time::Instant;

use bitcoin::{ScriptBuf};
use bitcoin::hashes::sha256;

use crate::streaming::domain::spk_tracker::DerivedSpkTracker;

use state::EngineState;
use logic::*;

#[derive(Debug)]
pub struct SyncEngine<K> {
    state: EngineState<K>,
}

impl<K: Ord + Clone> SyncEngine<K> {
    pub fn new(spk_tracker: DerivedSpkTracker<K>) -> Self {
        Self {
            state: EngineState {
                start_time: Instant::now(),
                first_history_seen_at: None,
                first_tx_seen_at: None,
                spk_tracker,
                spk_index_by_hash: HashMap::new(),
                script_by_hash: HashMap::new(),
                subscribed: BTreeSet::new(),
                histories: HashMap::new(),
                connected: false,
            },
        }
    }

    pub fn handle_event(&mut self, event: EngineEvent) -> Vec<EngineCommand> {
        match event {
            EngineEvent::Connected => {
                on_connected(&mut self.state)
            },
            EngineEvent::ScriptHashChanged(hash) => {
                logic::on_scripthash_changed(&mut self.state, hash)
            },
            EngineEvent::ScriptHashHistory { hash, txs } => {
                logic::on_scripthash_history(&mut self.state, hash, txs)
            },
        }
    }

    pub fn script_for_hash(&self, hash: &sha256::Hash) -> Option<ScriptBuf> {
        self.state.script_by_hash.get(hash).cloned()
    }

    #[cfg(test)]
    pub fn tracker_mut(&mut self) -> &mut crate::streaming::domain::spk_tracker::DerivedSpkTracker<K> {
        // FIX: Use 'spk_tracker' instead of 'tracker'
        &mut self.state.spk_tracker
    }
}
