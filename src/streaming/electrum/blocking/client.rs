use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use bitcoin::hashes::sha256;
use bitcoin::{ScriptBuf, Txid, Transaction};

use bdk_electrum::electrum_client::{
    Client,
    ElectrumApi as _,
    ScriptStatus,
};

use serde::{Serialize, Deserialize};
use serde_json;

use crate::streaming::electrum::api::ElectrumApi;

/// Only persist HISTORY.
/// ScriptStatus is opaque and not serializable.
#[derive(Serialize, Deserialize, Default)]
struct CacheFile {
    history_cache: HashMap<String, Vec<Txid>>,
}

/// Hybrid cache-backed polling Electrum client.
///
/// - Uses `script_subscribe` for cheap status polling
/// - Only fetches full history when status changes
/// - Persists history to disk
/// - Warm startup is instant
pub struct CachedPollingElectrumClient {
    inner: Client,

    /// hash -> script
    watched: HashMap<sha256::Hash, ScriptBuf>,

    /// hash -> last known status (memory only)
    last_status: HashMap<sha256::Hash, Option<ScriptStatus>>,

    /// hash -> cached history
    history_cache: HashMap<sha256::Hash, Vec<Txid>>,

    /// queue of detected changes
    pending: VecDeque<sha256::Hash>,

    poll_interval: Duration,

    cache_path: PathBuf,
}

impl CachedPollingElectrumClient {
    pub fn new(inner: Client, cache_path: PathBuf) -> Self {
        let mut this = Self {
            inner,
            watched: HashMap::new(),
            last_status: HashMap::new(),
            history_cache: HashMap::new(),
            pending: VecDeque::new(),
            poll_interval: Duration::from_secs(5),
            cache_path,
        };

        this.load_cache();
        this
    }

    fn hash_key(hash: &sha256::Hash) -> String {
        hash.to_string()
    }

    fn load_cache(&mut self) {
        if let Ok(data) = fs::read(&self.cache_path) {
            if let Ok(file) = serde_json::from_slice::<CacheFile>(&data) {
                for (k, v) in file.history_cache {
                    if let Ok(hash) = k.parse::<sha256::Hash>() {
                        self.history_cache.insert(hash, v);
                    }
                }
                log::info!("Loaded Electrum cache from {:?}", self.cache_path);
            }
        }
    }

    fn save_cache(&self) {
        let mut file = CacheFile::default();

        for (k, v) in &self.history_cache {
            file.history_cache.insert(Self::hash_key(k), v.clone());
        }

        if let Some(parent) = self.cache_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        if let Ok(data) = serde_json::to_vec_pretty(&file) {
            let _ = fs::write(&self.cache_path, data);
        }
    }

    fn poll_once(&mut self) {
        for (hash, script) in self.watched.clone() {
            let status: Option<ScriptStatus> = match self.inner.script_subscribe(&script) {
                Ok(s) => s,
                Err(_) => continue,
            };

            let old_status = self.last_status.get(&hash);

            if old_status != Some(&status) {
                // Status changed → fetch history
                let history = match self.inner.script_get_history(&script) {
                    Ok(entries) => entries.into_iter().map(|e| e.tx_hash).collect(),
                    Err(_) => vec![],
                };

                self.last_status.insert(hash, status.clone());
                self.history_cache.insert(hash, history);
                self.pending.push_back(hash);

                self.save_cache();
            }
        }
    }
}

impl ElectrumApi for CachedPollingElectrumClient {
    fn register_script(&mut self, script: ScriptBuf, hash: sha256::Hash) {
        self.watched.insert(hash, script);

        // If we already have cached history, immediately trigger
        if self.history_cache.contains_key(&hash) {
            self.pending.push_back(hash);
        }

        self.last_status.entry(hash).or_insert(None);
    }

    fn poll_scripthash_changed(&mut self) -> Option<sha256::Hash> {
        if let Some(h) = self.pending.pop_front() {
            return Some(h);
        }

        loop {
            self.poll_once();

            if let Some(h) = self.pending.pop_front() {
                return Some(h);
            }

            thread::sleep(self.poll_interval);
        }
    }
    fn fetch_history_txs(&mut self, _hash: sha256::Hash) -> Vec<Transaction> {
        // TEMP: return empty until you wire real tx fetch
        vec![]
    }
    fn request_history(&mut self, hash: sha256::Hash) {
        // Mock is synchronous: history is always ready
        // So we do nothing here.
        let _ = hash;
    }
}
