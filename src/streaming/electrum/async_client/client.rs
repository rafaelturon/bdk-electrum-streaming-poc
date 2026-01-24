use anyhow::Result;
use serde_json::{json, Value};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, WriteHalf};
use tokio::net::TcpStream;
use tokio_native_tls::{TlsConnector, TlsStream};

use bitcoin::{ScriptBuf, Transaction, Txid};
use bitcoin::hashes::{sha256, Hash};

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};
use std::net::{SocketAddr, ToSocketAddrs};

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
    ready: VecDeque<sha256::Hash>,
    history_cache: HashMap<sha256::Hash, Vec<Transaction>>,
    watched: HashMap<sha256::Hash, ScriptBuf>,
    subscribe_queue: VecDeque<sha256::Hash>,
    history_request_queue: VecDeque<sha256::Hash>,
    inflight_history: HashMap<u64, sha256::Hash>,
    tx_request_queue: VecDeque<(u64, Txid)>,
    connected: bool,
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
            tx_request_queue: VecDeque::new(),
            connected: false,
            inflight_tx: HashMap::new(),
            remaining_txs: HashMap::new(),  
        }));

        let bg_state = state.clone();
        let cv = Arc::new(std::sync::Condvar::new());
        let bg_cv = cv.clone();

        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let mut task =
                    AsyncElectrumTask::connect(server, bg_state, bg_cv)
                        .await
                        .expect("connect electrum");

                task.run_forever().await.expect("electrum loop");
            });
        });

        // Wait until async client is connected
        let mut guard = state.lock().unwrap();
        while !guard.connected {
            guard = cv.wait(guard).unwrap();
        }

        log::info!("[ASYNC] client fully connected");
        drop(guard);

        Self { state }
    }
}

// =====================================================================
// ElectrumApi
// =====================================================================

impl ElectrumApi for AsyncElectrumClient {
    fn register_script(&mut self, script: ScriptBuf, hash: sha256::Hash) {
        log::debug!("[ADAPTER] register_script({})", hash);
        let mut s = self.state.lock().unwrap();

        s.watched.insert(hash, script);
        s.subscribe_queue.push_back(hash);
        s.history_request_queue.push_back(hash);

        log::debug!(
            "[ADAPTER] queued subscribe + history for {} (subs={}, hist={})",
            hash,
            s.subscribe_queue.len(),
            s.history_request_queue.len()
        );
    }

    fn request_history(&mut self, hash: sha256::Hash) {
        log::debug!("[HISTORY] request_history({})", hash);
        let mut s = self.state.lock().unwrap();
        s.history_request_queue.push_back(hash);
    }

    fn fetch_history_txs(&mut self, hash: sha256::Hash) -> Vec<Transaction> {
        let mut s = self.state.lock().unwrap();
        log::trace!("[HISTORY] cache keys at fetch = {:?}", s.history_cache.keys().collect::<Vec<_>>());
        let txs = s.history_cache.remove(&hash).unwrap_or_default();
        log::trace!(
            "[HISTORY] fetch_history_txs({}) -> {} txs",
            hash,
            txs.len()
        );
        txs
    }

    fn poll_scripthash_changed(&mut self) -> Option<sha256::Hash> {
        let mut s = self.state.lock().unwrap();
        let item = s.ready.pop_front();
        log::trace!("[ENGINE] poll_scripthash_changed -> {:?}", item);
        item
    }
}

// =====================================================================
// Async Task
// =====================================================================

struct AsyncElectrumTask {
    writer: WriteHalf<TlsStream<TcpStream>>,
    state: Arc<Mutex<SharedState>>,
    cv: Arc<std::sync::Condvar>,
}

impl AsyncElectrumTask {
    pub async fn connect(
        server: String,
        state: Arc<Mutex<SharedState>>,
        cv: Arc<std::sync::Condvar>,
    ) -> Result<Self> {
        let (host, port) = parse_server(&server)?;
        log::debug!("[CONNECT] Connecting to {}:{} ...", host, port);      
        
        log::info!("[CONNECT] resolving host...");
        let addr: SocketAddr = (host.as_str(), port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| anyhow::anyhow!("no address resolved"))?;
        log::debug!("[CONNECT] resolved to {}", addr);
        
        log::info!("[CONNECT] connecting (blocking std::net::TcpStream)...");
        let std_tcp = std::net::TcpStream::connect(addr)?;
        std_tcp.set_nonblocking(true)?;
        log::info!("[CONNECT] std TCP CONNECTED");
        
        let tcp = TcpStream::from_std(std_tcp)?;
        log::info!("[CONNECT] converted to tokio TcpStream");

        log::info!("[CONNECT] building TLS connector");
        let connector = TlsConnector::from(native_tls::TlsConnector::new()?);

        log::info!("[CONNECT] starting TLS handshake");
        let tls = connector.connect(&host, tcp).await?;

        log::info!("[CONNECT] TLS CONNECTED");

        let (r, w) = tokio::io::split(tls);
        let reader_state = state.clone();

        // ===============================
        // Dedicated reader task
        // ===============================
        tokio::spawn(async move {
            let mut reader = BufReader::new(r);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line).await {
                    Ok(0) => {
                        log::error!("[ASYNC] socket closed");
                        break;
                    }
                    Ok(_) => {
                        if let Err(e) = process_message(&line, &reader_state).await {
                            log::error!("[ASYNC] process_message error: {:?}", e);
                        }
                    }
                    Err(e) => {
                        log::error!("[ASYNC] read error: {:?}", e);
                        break;
                    }
                }
            }
        });

        let mut this = Self {
            writer: w,
            state: state.clone(),
            cv,
        };

        this.handshake().await?;
        {
            let mut s = this.state.lock().unwrap();
            s.connected = true;
        }

        this.cv.notify_all();
        log::info!("[ASYNC] electrum connection ready");

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
        log::info!("[SEND] server.version done");

        Ok(())
    }

    pub async fn run_forever(&mut self) -> Result<()> {
        log::info!("[ASYNC] Running forever...");
        loop {
            self.flush_outgoing().await?;
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    async fn flush_outgoing(&mut self) -> Result<()> {
        let (subs, history_reqs, tx_reqs) = {
            let mut s = self.state.lock().unwrap();
            (
                s.subscribe_queue.drain(..).collect::<Vec<_>>(),
                s.history_request_queue.drain(..).collect::<Vec<_>>(),
                s.tx_request_queue.drain(..).collect::<Vec<_>>(),
            )
        };

        // Subscriptions
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
                log::info!("[SEND] blockchain.scripthash.subscribe done");
            }
        }

        // History requests
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
            log::info!("[SEND] blockchain.scripthash.get_history done");
        }

        // Tx requests
        for (id, txid) in tx_reqs {
            self.send(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "blockchain.transaction.get",
                "params": [txid.to_string(), true]
            }))
            .await?;
            log::info!("[SEND] blockchain.transaction.get done");
        }

        Ok(())
    }

    async fn send(&mut self, v: &Value) -> Result<()> {
        let s = v.to_string();
        log::trace!("[SEND] payload:{}", s);
        self.writer.write_all(s.as_bytes()).await?;
        self.writer.write_all(b"\n").await?;
        self.writer.flush().await?;
        Ok(())
    }
}

// =====================================================================
async fn process_message(line: &str, state: &Arc<Mutex<SharedState>>) -> Result<()> {
    let msg: Value = serde_json::from_str(line)?;
    log::trace!("[HISTORY] process_message line:{}", line.trim());

    // ============================================================
    // Notifications (no id)
    // ============================================================
    if msg.get("id").is_none() {
        if let Some(method) = msg.get("method").and_then(|m| m.as_str()) {
            if method == "blockchain.scripthash.subscribe" {
                let params = msg["params"].as_array().ok_or_else(|| {
                    anyhow::anyhow!("invalid subscribe notification params")
                })?;

                let sh_hex = params[0].as_str().ok_or_else(|| {
                    anyhow::anyhow!("invalid scripthash in notification")
                })?;

                let mut bytes = hex::decode(sh_hex)?;
                bytes.reverse();
                let hash = sha256::Hash::from_slice(&bytes)?;

                log::debug!("[HISTORY] scripthash notification for {}", hash);

                let mut s = state.lock().unwrap();
                s.ready.push_back(hash);
            }
        }
        return Ok(());
    }

    // ============================================================
    // Responses (have id)
    // ============================================================
    let id = msg["id"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("response without numeric id"))?;

    // ------------------------------------------------------------
    // Handle responses with id
    // ------------------------------------------------------------
    if let Some(id) = msg.get("id").and_then(|v| v.as_u64()) {
        // -------------------------------
        // History response
        // -------------------------------
        if let Some(result) = msg.get("result") {
            let mut s = state.lock().unwrap();

            if let Some(hash) = s.inflight_history.remove(&id) {
                let arr = result.as_array().ok_or_else(|| anyhow::anyhow!("history not array"))?;

                log::error!(
                    "[HISTORY] response for {} -> {} entries",
                    hash,
                    arr.len()
                );

                s.remaining_txs.insert(hash, arr.len());

                if arr.is_empty() {
                    // No txs: history is empty, finalize immediately
                    s.history_cache.insert(hash, vec![]);
                    s.ready.push_back(hash);
                } else {
                    for item in arr {
                        let txid_str = item["tx_hash"]
                            .as_str()
                            .ok_or_else(|| anyhow::anyhow!("missing tx_hash"))?;

                        let txid: Txid = txid_str.parse()?;
                        let tx_req_id = next_id();

                        s.inflight_tx.insert(tx_req_id, hash);
                        s.tx_request_queue.push_back((tx_req_id, txid));
                    }
                }

                return Ok(());
            }
        }
    }

    // -------------------------------
    // Transaction response
    // -------------------------------
    if let Some(id) = msg.get("id").and_then(|v| v.as_u64()) {
        if let Some(result) = msg.get("result") {
            let tx: Transaction = serde_json::from_value(result.clone())?;

            let mut s = state.lock().unwrap();

            if let Some(hash) = s.inflight_tx.remove(&id) {
                log::error!(
                    "[HISTORY] received tx {} for {}",
                    tx.compute_txid(),
                    hash
                );

                s.history_cache.entry(hash).or_default().push(tx);

                let rem = s.remaining_txs.get_mut(&hash).unwrap();
                *rem -= 1;

                if *rem == 0 {
                    s.remaining_txs.remove(&hash);
                    s.ready.push_back(hash);

                    log::error!(
                        "[HISTORY] history complete for {} ({} txs)",
                        hash,
                        s.history_cache.get(&hash).map(|v| v.len()).unwrap_or(0)
                    );
                }

                return Ok(());
            }
        }
    }


    // ============================================================
    // Unknown response
    // ============================================================
    log::warn!("[ASYNC] response with unknown id {}", id);

    Ok(())
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
