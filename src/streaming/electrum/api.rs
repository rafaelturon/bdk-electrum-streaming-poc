use bitcoin::hashes::sha256;
use bitcoin::Txid;
use bitcoin::ScriptBuf;

/// Minimal Electrum interface used by the driver.
/// Everything is scripthash-based.
pub trait ElectrumApi {
    /// Called once when engine discovers a new script
    fn register_script(&mut self, script: ScriptBuf, hash: sha256::Hash);

    /// Fetch full history for a script hash
    fn fetch_history(&mut self, hash: sha256::Hash) -> Vec<Txid>;

    /// Blocking poll: returns next script hash that changed (if any)
    fn poll_scripthash_changed(&mut self) -> Option<sha256::Hash>;
}