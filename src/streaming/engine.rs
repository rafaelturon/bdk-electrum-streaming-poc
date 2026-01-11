//! Streaming sync decision engine.
//!
//! This is a PURE state machine.
//! - No network
//! - No async
//! - No IO
//! - Fully deterministic
//!
//! It consumes EngineEvent and emits EngineCommand.

use std::collections::{BTreeSet, HashMap};

use bitcoin::hashes::sha256;
use bitcoin::Txid;

use crate::streaming::jobs::spk_tracker::DerivedSpkTracker;
use crate::streaming::types::{EngineCommand, EngineEvent};

/// Engine that decides:
/// - What script hashes to subscribe to
/// - When to derive more
/// - When to emit wallet updates
#[derive(Debug)]
pub struct StreamingEngine<K> {
    /// Tracks descriptors, derivation, gap limit, etc
    spk_tracker: DerivedSpkTracker<K>,

    /// Currently subscribed script hashes
    subscribed: BTreeSet<sha256::Hash>,

    /// Cached histories (used to detect transitions empty → non-empty)
    histories: HashMap<sha256::Hash, Vec<Txid>>,

    /// Whether we are connected
    connected: bool,
}

impl<K: Ord + Clone> StreamingEngine<K> {
    /// Create new engine
    pub fn new(spk_tracker: DerivedSpkTracker<K>) -> Self {
        Self {
            spk_tracker,
            subscribed: BTreeSet::new(),
            histories: HashMap::new(),
            connected: false,
        }
    }

    /// Main entrypoint: feed event, get commands
    pub fn handle_event(&mut self, event: EngineEvent) -> Vec<EngineCommand> {
        match event {
            EngineEvent::Connected => self.on_connected(),
            EngineEvent::ScriptHashChanged(hash) => self.on_scripthash_changed(hash),
            EngineEvent::ScriptHashHistory { hash, txs } => {
                self.on_scripthash_history(hash, txs)
            }
        }
    }

    // ================================
    // Event handlers
    // ================================

    fn on_connected(&mut self) -> Vec<EngineCommand> {
        self.connected = true;

        let mut cmds = Vec::new();

        // Subscribe to all known script hashes
        for h in self.spk_tracker.all_spk_hashes() {
            if self.subscribed.insert(h) {
                cmds.push(EngineCommand::Subscribe(h));
            }
        }

        cmds
    }

    fn on_scripthash_changed(&mut self, hash: sha256::Hash) -> Vec<EngineCommand> {
        // Ask for full history for this script hash
        vec![EngineCommand::FetchHistory(hash)]
    }

    fn on_scripthash_history(
        &mut self,
        hash: sha256::Hash,
        txs: Vec<Txid>,
    ) -> Vec<EngineCommand> {
        let mut cmds = Vec::new();

        let prev = self.histories.get(&hash);

        let was_empty = prev.map(|v| v.is_empty()).unwrap_or(true);
        let is_empty = txs.is_empty();

        self.histories.insert(hash, txs);

        // If this script just transitioned empty → non-empty
        if was_empty && !is_empty {
            // Find derivation index
            if let Some((keychain, index)) = self.spk_tracker.index_of_spk_hash(&hash) {
                // Mark used → derive more
                let newly_derived = self
                    .spk_tracker
                    .mark_used_and_derive_new(&keychain, index);

                for new_hash in newly_derived {
                    if self.subscribed.insert(new_hash) {
                        cmds.push(EngineCommand::Subscribe(new_hash));
                    }
                }
            }
        }

        // TODO (later):
        // - Translate history into ChainUpdate
        // - Emit ApplyWalletUpdate

        cmds
    }

    // ================================
    // Introspection helpers (for tests)
    // ================================

    pub fn is_subscribed(&self, hash: &sha256::Hash) -> bool {
        self.subscribed.contains(hash)
    }

    pub fn subscription_count(&self) -> usize {
        self.subscribed.len()
    }
}

#[cfg(test)]
mod tests;
