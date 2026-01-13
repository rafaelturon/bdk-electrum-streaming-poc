use bitcoin::{Script, ScriptBuf, Txid};

use electrum_client::{Client, ElectrumApi as _};

use crate::streaming::electrum::driver::ElectrumApi;

pub struct BlockingElectrumClient {
    pub inner: Client,
}

impl ElectrumApi for BlockingElectrumClient {
    fn subscribe_script(&mut self, script: &Script) {
        let _ = self.inner.script_subscribe(script);
    }

    fn fetch_history(&mut self, script: &Script) -> Vec<Txid> {
        match self.inner.script_get_history(script) {
            Ok(entries) => entries.into_iter().map(|e| e.tx_hash).collect(),
            Err(_) => vec![],
        }
    }

    fn poll_script_changed(&mut self) -> Option<ScriptBuf> {
        // ⚠️ electrum-client 0.21 DOES NOT expose notifications publicly.
        // We will implement this properly later using:
        // - raw socket loop
        // - or async client
        // - or custom transport

        // For now: stub = no notifications
        None
    }
}
