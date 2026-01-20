use anyhow::Result;
use serde_json::{json, Value};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio_native_tls::{TlsConnector, TlsStream};

use bitcoin::{ScriptBuf, Transaction, Txid};
use bitcoin::hashes::{sha256, Hash};

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};
use std::str::FromStr;

use crate::streaming::electrum::api::ElectrumApi;

// =====================================================================
// Utils
// =====================================================================

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Convert script bytes to electrum scripthash hex (little endian)
pub fn electrum_scripthash(script: &[u8]) -> String {
    let hash = sha256::Hash::hash(script);
    let mut bytes = hash.to_byte_array();
    bytes.reverse();
    hex::encode(bytes)
}

// =====================================================================
// Shared State
// =====================================================================

struct SharedState {
    /// Hashes whose full history is ready to be consumed by the driver
    ready: VecDeque<sha256::Hash>,

    /// Cached tx history per scripthash
    history_cache: HashMap<sha256::Hash, Vec<Transaction>>,

    /// Watched scripts
    watched: HashMap<sha256::Hash, ScriptBuf>,

    /// Outgoing queues
    subscribe_queue: VecDeque<sha256::Hash>,
    history_request_queue: VecDeque<sha256::Hash>,

    /// Inflight tracking
    inflight_history: HashMap<u64, sha256::Hash>,
    inflight_tx: HashMap<u64, sha256::Hash>,
    remaining_txs: HashMap<sha256::Hash, usize>,
}

// =====================================================================
// Public Client (blocking facade)
// =====================================================================

pub struct AsyncElectrumClient {
    state: Arc<Mutex<SharedState>>,
}

impl AsyncElectrumClient {
    pub fn new(server: String) -> Self {
        let state = Arc::new(Mutex::new(SharedState {
            ready: VecDeque::new(),
            history_cache: HashMap::new(),
            watched: HashMap::new(),
            subscribe_queue: VecDeque::new(),
            history_request_queue: VecDeque::new(),
            inflight_history: HashMap::new(),
            inflight_tx: HashMap::new(),
            remaining_txs: HashMap::new(),
        }));

        let bg_state = state.clone();

        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let mut task =
                    AsyncElectrumTask::connect(server, bg_state)
                        .await
                        .expect("connect electrum");

                task.run_forever().await.expect("electrum loop");
            });
        });

        Self { state }
    }
}

// =====================================================================
// ElectrumApi
// =====================================================================

impl ElectrumApi for AsyncElectrumClient {
    fn register_script(&mut self, script: ScriptBuf, hash: sha256::Hash) {
        log::info!("[ASYNC] register_script({})", hash);
        let mut s = self.state.lock().unwrap();
        s.watched.insert(hash, script);
        s.subscribe_queue.push_back(hash);
    }

    fn request_history(&mut self, hash: sha256::Hash) {
        log::debug!("[ASYNC] request_history({})", hash);
        let mut s = self.state.lock().unwrap();
        s.history_request_queue.push_back(hash);
    }

    fn fetch_history_txs(&mut self, hash: sha256::Hash) -> Vec<Transaction> {
        let mut s = self.state.lock().unwrap();
        let txs = s.history_cache.remove(&hash).unwrap_or_default();
        log::debug!(
            "[ASYNC] fetch_history_txs({}) -> {} txs",
            hash,
            txs.len()
        );
        txs
    }

    fn poll_scripthash_changed(&mut self) -> Option<sha256::Hash> {
        let mut s = self.state.lock().unwrap();
        s.ready.pop_front()
    }
}

// =====================================================================
// Async Task
// =====================================================================

struct AsyncElectrumTask {
    reader: BufReader<ReadHalf<TlsStream<TcpStream>>>,
    writer: WriteHalf<TlsStream<TcpStream>>,
    state: Arc<Mutex<SharedState>>,
}

impl AsyncElectrumTask {
    pub async fn connect(server: String, state: Arc<Mutex<SharedState>>) -> Result<Self> {
        let (host, port) = parse_server(&server)?;
        log::info!("[ASYNC] Connecting to {}:{} ...", host, port);

        let tcp = TcpStream::connect((host.as_str(), port)).await?;
        let connector = TlsConnector::from(native_tls::TlsConnector::new()?);
        let tls = connector.connect(&host, tcp).await?;

        let (r, w) = tokio::io::split(tls);

        let mut this = Self {
            reader: BufReader::new(r),
            writer: w,
            state,
        };

        this.handshake().await?;
        Ok(this)
    }

    async fn handshake(&mut self) -> Result<()> {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": next_id(),
            "method": "server.version",
            "params": ["bdk-streaming-poc", "1.4"]
        }))
        .await?;

        let mut line = String::new();
        self.reader.read_line(&mut line).await?;
        log::info!("[ASYNC] Handshake OK: {}", line.trim());
        Ok(())
    }

    pub async fn run_forever(&mut self) -> Result<()> {
        loop {
            self.flush_outgoing().await?;
            self.read_one_message().await?;
        }
    }

    async fn flush_outgoing(&mut self) -> Result<()> {
        let (subs, history_reqs) = {
            let mut s = self.state.lock().unwrap();
            (
                s.subscribe_queue.drain(..).collect::<Vec<_>>(),
                s.history_request_queue.drain(..).collect::<Vec<_>>(),
            )
        };

        for hash in subs {
            let script = {
                let s = self.state.lock().unwrap();
                s.watched.get(&hash).cloned()
            };

            if let Some(script) = script {
                let sh = electrum_scripthash(script.as_bytes());
                self.send(&json!({
                    "jsonrpc": "2.0",
                    "id": next_id(),
                    "method": "blockchain.scripthash.subscribe",
                    "params": [sh]
                }))
                .await?;
            }
        }

        for hash in history_reqs {
            let mut bytes = hash.to_byte_array();
            bytes.reverse();
            let sh = hex::encode(bytes);
            let id = next_id();

            {
                let mut s = self.state.lock().unwrap();
                s.inflight_history.insert(id, hash);
            }

            self.send(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "blockchain.scripthash.get_history",
                "params": [sh]
            }))
            .await?;
        }

        Ok(())
    }

    async fn read_one_message(&mut self) -> Result<()> {
        let mut line = String::new();
        self.reader.read_line(&mut line).await?;
        if line.is_empty() {
            return Ok(());
        }

        let msg: Value = serde_json::from_str(&line)?;
        log::trace!("[ASYNC] <<< {}", line.trim());

        let mut tx_requests = Vec::new();
        let mut ready = Vec::new();

        {
            let mut s = self.state.lock().unwrap();

            if let Some(id) = msg.get("id").and_then(|v| v.as_u64()) {
                if let Some(hash) = s.inflight_history.remove(&id) {
                    let arr_opt = msg["result"].as_array();
                    if arr_opt.map(|a| a.is_empty()).unwrap_or(true) {
                        s.history_cache.insert(hash, vec![]);
                        ready.push(hash);
                    } else {
                        let arr = arr_opt.unwrap();

                        s.remaining_txs.insert(hash, arr.len());

                        for e in arr {
                            let txid = Txid::from_str(e["tx_hash"].as_str().unwrap())?;
                            let tx_req = next_id();
                            s.inflight_tx.insert(tx_req, hash);
                            tx_requests.push((tx_req, txid));
                        }
                    }
                } else if let Some(hash) = s.inflight_tx.remove(&id) {
                    let tx: Transaction =
                        serde_json::from_value(msg["result"].clone())?;
                    s.history_cache.entry(hash).or_default().push(tx);

                    let left = s.remaining_txs.get_mut(&hash).unwrap();
                    *left -= 1;
                    if *left == 0 {
                        s.remaining_txs.remove(&hash);
                        ready.push(hash);
                    }
                }
            }
        }

        for (id, txid) in tx_requests {
            self.send(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "blockchain.transaction.get",
                "params": [txid.to_string(), true]
            }))
            .await?;
        }

        let mut s = self.state.lock().unwrap();
        for h in ready {
            log::debug!("[ASYNC] history complete for {}", h);
            s.ready.push_back(h);
        }

        Ok(())
    }

    async fn send(&mut self, v: &Value) -> Result<()> {
        let mut s = serde_json::to_string(v)?;
        s.push('\n');
        self.writer.write_all(s.as_bytes()).await?;
        Ok(())
    }
}

// =====================================================================

fn parse_server(s: &str) -> Result<(String, u16)> {
    let s = s.trim();
    let s = s.strip_prefix("ssl://")
        .or_else(|| s.strip_prefix("tcp://"))
        .unwrap_or(s);

    let mut parts = s.split(':');
    let host = parts.next().unwrap().to_string();
    let port = parts.next().unwrap().parse::<u16>()?;
    Ok((host, port))
}