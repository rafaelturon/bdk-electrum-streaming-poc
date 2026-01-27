//! Streaming sync decision engine.
//!
//! This module implements the **Functional Core** of the wallet synchronization logic.
//! It acts as a pure state machine:
//! - **Input**: `EngineEvent` (signals from the outside world).
//! - **Output**: `Vec<EngineCommand>` (side effects to be executed by the driver).
//!
//! # Architecture guarantees
//! * **No Network**: This module never opens sockets or makes HTTP requests.
//! * **No Async**: All functions are blocking and CPU-bound (and fast).
//! * **Deterministic**: Given the same initial state and sequence of events, the output is always identical.

pub mod state;
mod logic;
pub mod types;

#[cfg(test)]
mod tests;

// Re-export core types for easy access
pub use crate::streaming::engine::types::{EngineEvent, EngineCommand};

use std::collections::{BTreeSet, HashMap};
use std::time::Instant;

use bitcoin::{ScriptBuf};
use bitcoin::hashes::sha256;

use crate::streaming::domain::spk_tracker::DerivedSpkTracker;

use state::EngineState;
use logic::*;

/// The synchronization "Brain".
///
/// `SyncEngine` orchestrates the logic of tracking addresses, handling gaps,
/// and deciding when to apply transactions to the wallet.
#[derive(Debug)]
pub struct SyncEngine<K> {
    /// Internal state of the engine (scripts, subscriptions, history).
    state: EngineState<K>,
}

impl<K: Ord + Clone> SyncEngine<K> {
    /// Creates a new engine initialized with a specific SPK tracker.
    ///
    /// # Arguments
    /// * `spk_tracker` - The component responsible for HD wallet derivation and gap limit tracking.
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

    /// The main event handler.
    ///
    /// Consumes an event and returns a list of commands that the driver must execute.
    /// This is the primary entry point for the state machine.
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

    /// Resolves a ScriptHash back to its original ScriptBuf.
    ///
    /// Useful for the driver to reconstruct full objects when only a hash is available.
    pub fn script_for_hash(&self, hash: &sha256::Hash) -> Option<ScriptBuf> {
        self.state.script_by_hash.get(hash).cloned()
    }

    /// Accessor for the internal SPK tracker (Test only).
    ///
    /// Allows tests to inspect or mutate derivation state directly.
    #[cfg(test)]
    pub fn tracker_mut(&mut self) -> &mut crate::streaming::domain::spk_tracker::DerivedSpkTracker<K> {
        &mut self.state.spk_tracker
    }
}
