use bitcoin::hashes::sha256;
use bitcoin::{block, ScriptBuf};

use crate::streaming::engine::types::HistoryTx;

/// Minimal Electrum interface used by the driver.
/// Everything is scripthash-based.
pub trait ElectrumApi {
    /// Called once when engine discovers a new script
    fn register_script(&mut self, script: ScriptBuf, hash: sha256::Hash);

    /// Blocking poll: returns next script hash that changed (if any)
    fn poll_scripthash_changed(&mut self) -> Option<sha256::Hash>;

    /// Retrieves the downloaded transaction history for a hash.
    ///
    /// Returns `Some(Vec<HistoryTx>)` if history is ready and in the cache.
    /// Returns `None` if the history is still pending.
    ///
    /// CHANGED: now returns `HistoryTx` which pairs each tx with its
    ///          confirmation height from the Electrum server.
    fn fetch_history_txs(&mut self, hash: sha256::Hash) -> Option<Vec<HistoryTx>>;

    fn request_history(&mut self, hash: sha256::Hash);

    /// Retrieves a cached block header by height.
    ///
    /// Returns `Some(Header)` if the header has been fetched and cached.
    /// Returns `None` if the header is not yet available.
    ///
    /// NEW: The adapter fetches block headers alongside transaction history.
    ///      The orchestrator uses these to build `ConfirmationBlockTime` anchors.
    fn get_cached_header(&self, height: u32) -> Option<block::Header>;
}