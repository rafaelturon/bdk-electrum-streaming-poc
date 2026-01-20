use bitcoin::hashes::sha256;
use bitcoin::{ScriptBuf, Transaction};

/// Minimal Electrum interface used by the driver.
/// Everything is scripthash-based.
pub trait ElectrumApi {
    /// Called once when engine discovers a new script
    fn register_script(&mut self, script: ScriptBuf, hash: sha256::Hash);

    /// Blocking poll: returns next script hash that changed (if any)
    fn poll_scripthash_changed(&mut self) -> Option<sha256::Hash>;

    fn fetch_history_txs(&mut self, hash: sha256::Hash) -> Vec<Transaction>;

    fn request_history(&mut self, hash: sha256::Hash);
}