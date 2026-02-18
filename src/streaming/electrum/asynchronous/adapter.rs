//! Electrum Protocol Adapter (Async implementation).
//!
//! This module implements the `ElectrumApi` trait using a non-blocking Tokio background task.
//! It acts as a **Facade**, bridging the synchronous/blocking world of the `SyncOrchestrator`
//! (Driver) with the asynchronous world of network I/O.
//!
//! # Architecture
//! * **Shared State**: Uses `Arc<Mutex<SharedState>>` to communicate between the blocking driver thread
//!   and the async background task.
//! * **Command Queue**: The driver pushes commands (Subscribe, Fetch) to a queue. The background task
//!   drains this queue and sends JSON-RPC requests to the socket.
//! * **Event Loop**: The background task runs an infinite loop handling socket reads/writes.

use anyhow::Result;
use serde_json::{json, Value};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, WriteHalf};
use tokio::net::TcpStream;
use tokio_native_tls::{TlsConnector, TlsStream};

use bitcoin::{block, ScriptBuf, Transaction, Txid};
use bitcoin::hashes::{sha256, Hash};
use bitcoin::consensus::Decodable;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};
use std::net::{SocketAddr, ToSocketAddrs};

use crate::streaming::electrum::api::ElectrumApi;
use crate::streaming::engine::types::HistoryTx;

// =====================================================================
// Utils
// =====================================================================

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Generates a unique, monotonically increasing ID for JSON-RPC requests.
pub fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Convert script bytes to electrum scripthash hex (little endian).
///
/// Electrum uses the sha256 hash of the script, reversed, represented as hex.
pub fn electrum_scripthash(script: &[u8]) -> String {
    let hash = sha256::Hash::hash(script);
    let mut bytes = hash.to_byte_array();
    bytes.reverse();
    hex::encode(bytes)
}

// =====================================================================
// Types
// =====================================================================

/// Internal commands sent from the synchronous Driver to the Async Task.
#[derive(Debug)]
pub enum InternalCommand {
    /// Subscribe to status updates for a script hash.
    Subscribe {
        hash: sha256::Hash,
        script: ScriptBuf,
    },
    /// Request the transaction history for a script hash.
    FetchHistory {
        hash: sha256::Hash,
    },
    /// Request a specific transaction by ID (part of resolving history).
    FetchTransaction {
        txid: Txid,
        related_hash: sha256::Hash,
        height: i32,                      // NEW: confirmation height from get_history
    },
    /// Request a block header by height (for building anchors).
    FetchBlockHeader {                    // NEW
        height: u32,
        related_hash: sha256::Hash,
    },
}

/// Tracks the type of an in-flight JSON-RPC request to handle the response correctly.
#[derive(Debug)]
pub enum RequestType {
    History(sha256::Hash),
    Transaction {
        related_hash: sha256::Hash,
        height: i32,                      // NEW: carried from history response
    },
    BlockHeader {                         // NEW
        height: u32,
        related_hash: sha256::Hash,
    },
}

// =====================================================================
// Shared State
// =====================================================================

/// State shared between the blocking Driver thread and the async Tokio task.
struct SharedState {
    // --- Output (Network -> Driver) ---
    /// Queue of script hashes that have received updates or finished syncing.
    /// The driver polls this via `poll_scripthash_changed`.
    ready: VecDeque<sha256::Hash>,

    /// Temporary storage for downloaded transaction histories.
    history_cache: HashMap<sha256::Hash, Vec<HistoryTx>>,   // CHANGED: was Vec<Transaction>

    /// Cache of block headers by height (used by orchestrator for anchors).
    block_header_cache: HashMap<u32, block::Header>,        // NEW

    // --- Input (Driver -> Network) ---
    /// Queue of commands waiting to be sent to the Electrum server.
    command_queue: VecDeque<InternalCommand>,

    // --- Tracking ---
    /// Map of Request ID -> Request Type (to correlate responses).
    inflight_requests: HashMap<u64, RequestType>,
    
    /// Counter for transactions remaining to be downloaded for a specific history request.
    /// Key: ScriptHash, Value: Count of txs still pending.
    remaining_txs: HashMap<sha256::Hash, usize>,

    /// Counter for block headers remaining to be downloaded for a specific history request.
    /// Key: ScriptHash, Value: Count of unique heights still pending.
    remaining_headers: HashMap<sha256::Hash, usize>,        // NEW

    /// Tracks which block heights have already been requested (to avoid duplicates).
    headers_in_flight: HashSet<u32>,                        // NEW
    
    /// Flag indicating if the TLS connection handshake is complete.
    connected: bool,
}

impl SharedState {
    /// Checks if all data (txs + headers) is ready for a given scripthash.
    /// If so, signals the driver via the `ready` queue.
    fn check_history_complete(&mut self, hash: sha256::Hash) {
        let txs_done = self.remaining_txs.get(&hash).copied().unwrap_or(0) == 0;
        let hdrs_done = self.remaining_headers.get(&hash).copied().unwrap_or(0) == 0;

        if txs_done && hdrs_done {
            self.remaining_txs.remove(&hash);
            self.remaining_headers.remove(&hash);
            self.ready.push_back(hash);
            log::info!(
                "[ADAPTER] history complete for {} ({} txs)",
                hash,
                self.history_cache.get(&hash).map(|v| v.len()).unwrap_or(0)
            );
        }
    }
}

// =====================================================================
// Public Client (blocking facade)
// =====================================================================

/// The main adapter struct used by the `SyncOrchestrator`.
///
/// It exposes a synchronous API (`ElectrumApi`) but performs all work
/// asynchronously in a background thread.
pub struct ElectrumAdapter {
    state: Arc<Mutex<SharedState>>,
}

impl ElectrumAdapter {
    /// Connects to the specified Electrum server (ssl/tcp).
    ///
    /// This function blocks the current thread until the background connection
    /// is fully established and the SSL handshake is complete.
    pub fn new(server: String) -> Self {
        let state = Arc::new(Mutex::new(SharedState {
            ready: VecDeque::new(),
            history_cache: HashMap::new(),
            block_header_cache: HashMap::new(),     // NEW
            command_queue: VecDeque::new(),
            inflight_requests: HashMap::new(),
            remaining_txs: HashMap::new(),
            remaining_headers: HashMap::new(),      // NEW
            headers_in_flight: HashSet::new(),      // NEW
            connected: false,
        }));

        let bg_state = state.clone();
        let cv = Arc::new(std::sync::Condvar::new());
        let bg_cv = cv.clone();

        // Spawn the background Tokio runtime and task
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

        // Block until the background task signals connection success
        let mut guard = state.lock().unwrap();
        while !guard.connected {
            guard = cv.wait(guard).unwrap();
        }

        log::info!("[ADAPTER] client fully connected");
        drop(guard);

        Self { state }
    }
}

// =====================================================================
// ElectrumApi
// =====================================================================

impl ElectrumApi for ElectrumAdapter {
    /// Queues a subscription request for a script.
    fn register_script(&mut self, script: ScriptBuf, hash: sha256::Hash) {
        log::trace!("[ADAPTER] register_script({})", hash);
        let mut s = self.state.lock().unwrap();
        s.command_queue.push_back(InternalCommand::Subscribe { hash, script });
        log::trace!(
            "[ADAPTER] queued subscribe for {} (queue len={})",
            hash,
            s.command_queue.len(),
        );
    }

    /// Queues a request to fetch transaction history for a script hash.
    fn request_history(&mut self, hash: sha256::Hash) {
        log::trace!("[ADAPTER] request_history({})", hash);
        let mut s = self.state.lock().unwrap();
        s.command_queue.push_back(InternalCommand::FetchHistory { hash });
    }

    /// Retrieves the downloaded transaction history for a hash.
    ///
    /// # Return Behavior (Crucial for Option B Logic)
    /// * Returns `Some(Vec<HistoryTx>)` if the history AND its block headers are **ready**.
    /// * Returns `None` if still **pending**.
    ///
    /// **Note:** This operation is destructive (it removes the item from the cache).
    fn fetch_history_txs(&mut self, hash: sha256::Hash) -> Option<Vec<HistoryTx>> {
        let mut s = self.state.lock().unwrap();
        let txs = s.history_cache.remove(&hash);
        
        if let Some(ref t) = txs {
            log::trace!("[ADAPTER] fetch_history_txs({}) -> found {} txs", hash, t.len());
        } else {
            log::trace!("[ADAPTER] fetch_history_txs({}) -> cache miss", hash);
        }
        
        txs
    }

    /// Checks if any script hash has new activity or completed syncing.
    fn poll_scripthash_changed(&mut self) -> Option<sha256::Hash> {
        let mut s = self.state.lock().unwrap();
        let item = s.ready.pop_front();
        if let Some(h) = item {
             log::trace!("[ENGINE] poll_scripthash_changed -> {:?}", h);
        }
        item
    }

    /// NEW: Retrieves a cached block header by height.
    fn get_cached_header(&self, height: u32) -> Option<block::Header> {
        let s = self.state.lock().unwrap();
        s.block_header_cache.get(&height).copied()
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
    /// Establishes the TCP/TLS connection and performs the version handshake.
    pub async fn connect(
        server: String,
        state: Arc<Mutex<SharedState>>,
        cv: Arc<std::sync::Condvar>,
    ) -> Result<Self> {
        let (host, port) = parse_server(&server)?;
        log::debug!("[ADAPTER] Connecting to {}:{} ...", host, port);      
        
        let addr: SocketAddr = (host.as_str(), port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| anyhow::anyhow!("no address resolved"))?;
        
        let std_tcp = std::net::TcpStream::connect(addr)?;
        std_tcp.set_nonblocking(true)?;
        
        let tcp = TcpStream::from_std(std_tcp)?;
        let connector = TlsConnector::from(native_tls::TlsConnector::new()?);
        let tls = connector.connect(&host, tcp).await?;

        log::info!("[ADAPTER] TLS connected");

        let (r, w) = tokio::io::split(tls);
        let reader_state = state.clone();

        // Dedicated reader task
        tokio::spawn(async move {
            let mut reader = BufReader::new(r);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line).await {
                    Ok(0) => {
                        log::error!("[ADAPTER] socket closed");
                        break;
                    }
                    Ok(_) => {
                        if let Err(e) = process_message(&line, &reader_state).await {
                            log::error!("[ADAPTER] process_message error: {:?}", e);
                        }
                    }
                    Err(e) => {
                        log::error!("[ADAPTER] read error: {:?}", e);
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
        log::info!("[ADAPTER] electrum connection ready");

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
        Ok(())
    }

    /// The main write loop.
    pub async fn run_forever(&mut self) -> Result<()> {
        log::info!("[ADAPTER] Running forever...");
        loop {
            self.flush_outgoing().await?;
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    async fn flush_outgoing(&mut self) -> Result<()> {
        let commands: Vec<InternalCommand> = {
            let mut s = self.state.lock().unwrap();
            s.command_queue.drain(..).collect()
        };

        for cmd in commands {
            match cmd {
                InternalCommand::Subscribe { hash: _, script } => {
                    let sh = electrum_scripthash(script.as_bytes());
                    self.send(&json!({
                        "jsonrpc": "2.0",
                        "id": next_id(),
                        "method": "blockchain.scripthash.subscribe",
                        "params": [sh]
                    })).await?;
                }
                InternalCommand::FetchHistory { hash } => {
                    let mut bytes = hash.to_byte_array();
                    bytes.reverse();
                    let sh = hex::encode(bytes);
                    let id = next_id();

                    {
                        let mut s = self.state.lock().unwrap();
                        s.inflight_requests.insert(id, RequestType::History(hash));
                    }

                    self.send(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "method": "blockchain.scripthash.get_history",
                        "params": [sh]
                    })).await?;
                }
                InternalCommand::FetchTransaction { txid, related_hash, height } => {
                    let id = next_id();
                    {
                        let mut s = self.state.lock().unwrap();
                        s.inflight_requests.insert(id, RequestType::Transaction {
                            related_hash,
                            height,             // CHANGED: carry height
                        });
                    }

                    self.send(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "method": "blockchain.transaction.get",
                        "params": [txid.to_string(), false]
                    })).await?;
                }
                // NEW: Fetch block header for a confirmed transaction's height
                InternalCommand::FetchBlockHeader { height, related_hash } => {
                    let id = next_id();
                    {
                        let mut s = self.state.lock().unwrap();
                        s.inflight_requests.insert(id, RequestType::BlockHeader {
                            height,
                            related_hash,
                        });
                    }

                    self.send(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "method": "blockchain.block.header",
                        "params": [height]
                    })).await?;
                }
            }
        }
        
        Ok(())
    }

    async fn send(&mut self, v: &Value) -> Result<()> {
        let s = v.to_string();
        log::trace!("[ADAPTER] Send payload:{}", s);
        self.writer.write_all(s.as_bytes()).await?;
        self.writer.write_all(b"\n").await?;
        self.writer.flush().await?;
        Ok(())
    }
}

// =====================================================================
// Message Processing
// =====================================================================

async fn process_message(line: &str, state: &Arc<Mutex<SharedState>>) -> Result<()> {
    let msg: Value = serde_json::from_str(line)?;
    log::trace!("[ADAPTER] process_message line:{}", line.trim());
    
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

                log::debug!("[ADAPTER] scripthash notification for {}", hash);

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

    let request_type = {
        let mut s = state.lock().unwrap();
        s.inflight_requests.remove(&id)
    };

    if let Some(req) = request_type {
        match req {
            RequestType::History(hash) => {
                if let Some(result) = msg.get("result") {
                    let arr = result.as_array().ok_or_else(|| anyhow::anyhow!("history not array"))?;
                    
                    let mut s = state.lock().unwrap();
                    s.remaining_txs.insert(hash, arr.len());

                    if arr.is_empty() {
                        // Empty history, ready immediately
                        s.history_cache.insert(hash, vec![]);
                        s.remaining_headers.insert(hash, 0);
                        s.check_history_complete(hash);
                    } else {
                        // CHANGED: Collect unique confirmed heights that need headers
                        let mut needed_heights: HashSet<u32> = HashSet::new();

                        for item in arr {
                            let txid_str = item["tx_hash"]
                                .as_str()
                                .ok_or_else(|| anyhow::anyhow!("missing tx_hash"))?;
                            let txid: Txid = txid_str.parse()?;

                            // NEW: Extract height from history entry
                            let height = item["height"]
                                .as_i64()
                                .unwrap_or(0) as i32;

                            // Queue tx fetch with its height
                            s.command_queue.push_back(InternalCommand::FetchTransaction { 
                                txid, 
                                related_hash: hash,
                                height,             // NEW: carry height
                            });

                            // Track unique confirmed heights that need headers
                            if height > 0 {
                                let h = height as u32;
                                if !s.block_header_cache.contains_key(&h)
                                    && !s.headers_in_flight.contains(&h)
                                {
                                    needed_heights.insert(h);
                                }
                            }
                        }

                        // Queue header fetches for unique new heights
                        s.remaining_headers.insert(hash, needed_heights.len());
                        for h in needed_heights {
                            s.headers_in_flight.insert(h);
                            s.command_queue.push_back(InternalCommand::FetchBlockHeader {
                                height: h,
                                related_hash: hash,
                            });
                        }
                    }
                }
            }

            // CHANGED: Now carries height alongside the transaction
            RequestType::Transaction { related_hash, height } => {
                if let Some(result) = msg.get("result") {
                    let hex_str = result.as_str().ok_or_else(|| anyhow::anyhow!("tx result is not a string"))?;
                    let tx_bytes = hex::decode(hex_str)?;
                    let tx = Transaction::consensus_decode(&mut &tx_bytes[..])?;

                    let mut s = state.lock().unwrap();
                    
                    // Store as HistoryTx with the height from the original get_history
                    s.history_cache.entry(related_hash).or_default().push(HistoryTx {
                        tx,
                        height,
                    });
                    
                    let rem = s.remaining_txs.get_mut(&related_hash).unwrap();
                    *rem -= 1;

                    // Check if BOTH txs and headers are done
                    if *rem == 0 {
                        s.check_history_complete(related_hash);
                    }
                }
            }

            // NEW: Block header response
            RequestType::BlockHeader { height, related_hash } => {
                if let Some(result) = msg.get("result") {
                    let hex_str = result.as_str().ok_or_else(|| anyhow::anyhow!("header result is not a string"))?;
                    let header_bytes = hex::decode(hex_str)?;
                    let header = block::Header::consensus_decode(&mut &header_bytes[..])?;

                    log::debug!(
                        "[ADAPTER] block header for height {} -> hash={}",
                        height,
                        header.block_hash()
                    );

                    let mut s = state.lock().unwrap();
                    s.block_header_cache.insert(height, header);
                    s.headers_in_flight.remove(&height);

                    // Decrement pending header count for this scripthash
                    if let Some(rem) = s.remaining_headers.get_mut(&related_hash) {
                        *rem = rem.saturating_sub(1);
                    }

                    // Check if BOTH txs and headers are done
                    let txs_done = s.remaining_txs.get(&related_hash).copied().unwrap_or(0) == 0;
                    let hdrs_done = s.remaining_headers.get(&related_hash).copied().unwrap_or(0) == 0;
                    if txs_done && hdrs_done {
                        s.check_history_complete(related_hash);
                    }
                }
            }
        }
    } else {
        log::debug!("[ADAPTER] response with unknown id {} (might be a subscribe response)", id);
    }

    Ok(())
}

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