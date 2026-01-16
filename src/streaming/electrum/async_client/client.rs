use anyhow::Result;
use serde_json::{json, Value};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio_native_tls::{TlsConnector, TlsStream};

use bitcoin::{Txid, ScriptBuf};
use bitcoin::hashes::{sha256, Hash};

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::streaming::electrum::api::ElectrumApi;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

enum PendingRequest {
    Subscribe { scripthash: sha256::Hash },
    GetHistory { scripthash: sha256::Hash },
}


/// Convert script bytes to electrum scripthash hex (little endian)
pub fn electrum_scripthash(script: &[u8]) -> String {
    let hash = sha256::Hash::hash(script);
    let mut bytes = hash.to_byte_array();
    bytes.reverse();
    hex::encode(bytes)
}

// =====================================================================
// Shared State (Between Blocking Driver and Async Task)
// =====================================================================

struct SharedState {
    pending: VecDeque<sha256::Hash>,
    history_cache: HashMap<sha256::Hash, Vec<Txid>>,
    watched: HashMap<sha256::Hash, ScriptBuf>,

    subscribe_queue: VecDeque<(sha256::Hash, ScriptBuf)>,
    fetch_queue: VecDeque<sha256::Hash>,
}

// =====================================================================
// Public Adapter Object (Implements ElectrumApi)
// =====================================================================

pub struct AsyncElectrumClient {
    state: Arc<Mutex<SharedState>>,
}

impl AsyncElectrumClient {
    pub fn new(server: String) -> Self {
        let state = Arc::new(Mutex::new(SharedState {
            pending: VecDeque::new(),
            history_cache: HashMap::new(),
            watched: HashMap::new(),
            subscribe_queue: VecDeque::new(),
            fetch_queue: VecDeque::new(),
        }));

        let bg_state = state.clone();

        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let mut task = AsyncElectrumTask::connect(server, bg_state)
                    .await
                    .expect("connect electrum");

                task.run_forever().await.expect("electrum loop");
            });
        });

        Self { state }
    }
}

// =====================================================================
// ElectrumApi implementation (Blocking Interface)
// =====================================================================

impl ElectrumApi for AsyncElectrumClient {
    fn register_script(&mut self, script: ScriptBuf, hash: sha256::Hash) {
        log::info!("[ASYNC-ADAPTER] register_script({})", hash);
        let mut s = self.state.lock().unwrap();
        s.watched.insert(hash, script.clone());
        s.subscribe_queue.push_back((hash, script));
        log::info!(
            "[ASYNC-ADAPTER] subscribe_queue size = {}",
            s.subscribe_queue.len()
        );
    }

    fn fetch_history(&mut self, hash: sha256::Hash) -> Vec<Txid> {
        let mut s = self.state.lock().unwrap();
        s.fetch_queue.push_back(hash);
        s.history_cache.get(&hash).cloned().unwrap_or_default()
    }

    fn poll_scripthash_changed(&mut self) -> Option<sha256::Hash> {
        let mut s = self.state.lock().unwrap();
        s.pending.pop_front()
    }
}

// =====================================================================
// The Actual Async Task (Owns Socket)
// =====================================================================

struct AsyncElectrumTask {
    reader: BufReader<ReadHalf<TlsStream<TcpStream>>>,
    writer: WriteHalf<TlsStream<TcpStream>>,
    state: Arc<Mutex<SharedState>>,
    pending: HashMap<u64, PendingRequest>,
}

impl AsyncElectrumTask {
    pub async fn connect(server: String, state: Arc<Mutex<SharedState>>) -> Result<Self> {
        let (host, port) = parse_server(&server)?;

        println!("[ASYNC] Connecting to {}:{} ...", host, port);

        let tcp = TcpStream::connect((host.as_str(), port)).await?;

        let connector = native_tls::TlsConnector::new()?;
        let connector = TlsConnector::from(connector);
        let tls = connector.connect(&host, tcp).await?;

        println!("[ASYNC] TLS connected");

        let (r, w) = tokio::io::split(tls);

        let mut this = Self {
            reader: BufReader::new(r),
            writer: w,
            state,
            pending: HashMap::new(),
        };

        this.handshake().await?;

        Ok(this)
    }

    async fn handshake(&mut self) -> Result<()> {
        let req = json!({
            "jsonrpc": "2.0",
            "id": next_id(),
            "method": "server.version",
            "params": ["bdk-streaming-poc", "1.4"]
        });

        self.send(&req).await?;

        let mut line = String::new();
        self.reader.read_line(&mut line).await?;

        println!("[ASYNC] Handshake: {}", line.trim());

        Ok(())
    }

    pub async fn run_forever(&mut self) -> Result<()> {
        loop {
            log::debug!("[ASYNC] loop tick");
            self.flush_outgoing().await?;
            tokio::task::yield_now().await;
            self.read_one_message().await?;
        }
    }

    async fn flush_outgoing(&mut self) -> Result<()> {
        log::debug!("[ASYNC] flush_outgoing()");
        let mut subs = Vec::new();
        let mut fetches = Vec::new();

        {
            let mut s = self.state.lock().unwrap();

            while let Some(x) = s.subscribe_queue.pop_front() {
                subs.push(x);
            }

            while let Some(h) = s.fetch_queue.pop_front() {
                fetches.push(h);
            }
        }
        log::debug!(
            "[ASYNC] drained: subs={}, fetches={}",
            subs.len(),
            fetches.len()
        );


        for (_hash, script) in subs {
            let sh = electrum_scripthash(script.as_bytes());
            log::info!("[ASYNC] -> SEND subscribe {}", sh);

            let id = next_id();
            self.pending.insert(id, PendingRequest::Subscribe { scripthash: _hash });
            self.send(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "blockchain.scripthash.subscribe",
                "params": [sh]
            })).await?;
        }

        for hash in fetches {
            let mut bytes = hash.to_byte_array();
            bytes.reverse();
            let sh = hex::encode(bytes);
            log::info!("[ASYNC] -> SEND get_history {}", sh);

            let id = next_id();
            self.pending.insert(id, PendingRequest::GetHistory { scripthash: hash });
            self.send(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "blockchain.scripthash.get_history",
                "params": [sh]
            })).await?;
        }

        Ok(())
    }

    async fn read_one_message(&mut self) -> Result<()> {
        log::info!("[ASYNC] read_one_message()");

        let mut line = String::new();
        self.reader.read_line(&mut line).await?;
        if line.is_empty() {
            return Ok(());
        }

        log::debug!("[ASYNC] <<< {}", line.trim());

        let msg: Value = serde_json::from_str(&line)?;

        // -------------------------------
        // Case 1: Notification (no id)
        // -------------------------------
        if msg.get("method").is_some() {
            let method = msg["method"].as_str().unwrap();

            if method == "blockchain.scripthash.subscribe" {
                let params = msg["params"].as_array().unwrap();
                let sh = params[0].as_str().unwrap();

                let mut bytes = hex::decode(sh)?;
                bytes.reverse();
                let hash = sha256::Hash::from_slice(&bytes)?;

                log::info!("[ASYNC] NOTIFY status changed for {}", hash);

                // TODO: enqueue history refresh for this script
                self.queue_get_history(hash);
            }

            return Ok(());
        }

        // -------------------------------
        // Case 2: Response (has id)
        // -------------------------------
        let id = msg["id"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("response without id"))?;

        let pending = self.pending.remove(&id)
            .ok_or_else(|| anyhow::anyhow!("unknown response id {}", id))?;

        match pending {
            PendingRequest::Subscribe { scripthash } => {
                log::info!(
                    "[ASYNC] SUB ACK {} status={}",
                    scripthash,
                    msg["result"]
                );

                // IMPORTANT: immediately fetch history
                self.queue_get_history(scripthash);
            }

            PendingRequest::GetHistory { scripthash } => {
                let mut txs = Vec::new();

                if let Some(arr) = msg["result"].as_array() {
                    for entry in arr {
                        if let Some(txid_hex) = entry.get("tx_hash").and_then(|v| v.as_str()) {
                            if let Ok(txid) = txid_hex.parse::<Txid>() {
                                txs.push(txid);
                            }
                        }
                    }
                }

                log::info!(
                    "[ASYNC] HISTORY {} txs={}",
                    scripthash,
                    txs.len()
                );

                // TODO: notify engine
                self.notify_engine_history(scripthash, txs);
            }
        }

        Ok(())
    }


    async fn send(&mut self, v: &Value) -> Result<()> {
        let mut s = serde_json::to_string(v)?;
        s.push('\n');
        self.writer.write_all(s.as_bytes()).await?;
        Ok(())
    }

    fn queue_get_history(&mut self, hash: sha256::Hash) {
        let mut s = self.state.lock().unwrap();
        s.fetch_queue.push_back(hash);
    }

    fn notify_engine_history(&mut self, hash: sha256::Hash, txs: Vec<Txid>) {
        let mut s = self.state.lock().unwrap();

        log::info!(
            "[ASYNC] cache update {} txs={}",
            hash,
            txs.len()
        );

        s.history_cache.insert(hash, txs);
        s.pending.push_back(hash);
    }
}

// =====================================================================

fn parse_server(s: &str) -> Result<(String, u16)> {
    let s = s.trim();

    let s = s.strip_prefix("ssl://")
        .or_else(|| s.strip_prefix("tcp://"))
        .unwrap_or(s);

    let mut parts = s.split(':');

    let host = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("missing host"))?
        .to_string();

    let port = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("missing port"))?
        .parse::<u16>()?;

    Ok((host, port))
}