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

#[cfg(test)]
mod tests;

use std::collections::{HashMap, BTreeSet};

use crate::streaming::jobs::spk_tracker::DerivedSpkTracker;

// Re-export canonical types from streaming/types.rs
pub use crate::streaming::types::{EngineEvent, EngineCommand};

use state::EngineState;

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
                subscribed: BTreeSet::new(),
                histories: HashMap::new(),
                connected: false,
            },
        }
    }

    /// Main entrypoint: feed event, get commands
    pub fn handle_event(&mut self, event: EngineEvent) -> Vec<EngineCommand> {
        match event {
            EngineEvent::Connected => logic::on_connected(&mut self.state),

            EngineEvent::ScriptHashChanged(hash) => {
                logic::on_scripthash_changed(&mut self.state, hash)
            }

            EngineEvent::ScriptHashHistory { hash, txs } => {
                logic::on_scripthash_history(&mut self.state, hash, txs)
            }
        }
    }

    // ================================
    // Introspection helpers (for tests)
    // ================================

    pub fn is_subscribed(&self, hash: &bitcoin::hashes::sha256::Hash) -> bool {
        self.state.subscribed.contains(hash)
    }

    pub fn subscription_count(&self) -> usize {
        self.state.subscribed.len()
    }
}
