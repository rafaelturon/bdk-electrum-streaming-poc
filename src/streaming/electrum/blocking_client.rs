use bitcoin::hashes::sha256;
use bitcoin::Txid;

use electrum_client::Client;

use super::driver::ElectrumApi;

/// Blocking adapter over electrum-client
///
/// This is currently a STUB.
/// electrum-client requires the full Script, but our engine is hash-based.
/// A proper adapter will require storing hash->script mapping.
pub struct BlockingElectrumClient {
    pub inner: Client,
}

impl ElectrumApi for BlockingElectrumClient {
    fn subscribe_scripthash(&mut self, _hash: sha256::Hash) {
        // TODO: needs script mapping
        todo!("BlockingElectrumClient needs hash->script mapping");
    }

    fn fetch_history(&mut self, _hash: sha256::Hash) -> Vec<Txid> {
        // TODO: needs script mapping
        todo!("BlockingElectrumClient needs hash->script mapping");
    }

    fn poll_scripthash_changed(&mut self) -> Option<sha256::Hash> {
        // TODO: needs async or polling loop
        None
    }
}