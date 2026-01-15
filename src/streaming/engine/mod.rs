//! Streaming sync decision engine.
//!
//! This is a PURE state machine.
//! - No network
//! - No async
//! - No IO
//! - Fully deterministic
//!
//! It consumes EngineEvent and emits EngineCommand.

mod state;
mod logic;
pub mod types;

#[cfg(test)]
mod tests;

pub use crate::streaming::engine::types::{EngineEvent, EngineCommand};

use std::collections::{BTreeSet, HashMap};

use bitcoin::{ScriptBuf};
use bitcoin::hashes::sha256;

use crate::streaming::domain::spk_tracker::DerivedSpkTracker;

use state::EngineState;
use logic::*;

#[derive(Debug)]
pub struct StreamingEngine<K> {
    state: EngineState<K>,
}

impl<K: Ord + Clone> StreamingEngine<K> {
    pub fn new(spk_tracker: DerivedSpkTracker<K>) -> Self {
        Self {
            state: EngineState {
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
            EngineEvent::Connected => on_connected(&mut self.state),
            EngineEvent::ScriptHashChanged(h) => on_scripthash_changed(&mut self.state, h),
            EngineEvent::ScriptHashHistory { hash, txs } => {
                on_scripthash_history(&mut self.state, hash, txs)
            }
        }
    }

    pub fn script_for_hash(&self, hash: &sha256::Hash) -> Option<ScriptBuf> {
        self.state.script_by_hash.get(hash).cloned()
    }
}
