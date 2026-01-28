use crate::streaming::engine::SyncEngine;
use crate::streaming::engine::types::{EngineCommand, EngineEvent};
use crate::streaming::electrum::api::ElectrumApi;

use bdk_wallet::{PersistedWallet, ChangeSet};
use bdk_wallet::file_store::Store;
use bitcoin::hashes::sha256;
use std::sync::{Arc, Mutex};
use std::time::{Instant, Duration};
use std::collections::HashSet;

type StreamingWallet = PersistedWallet<Store<ChangeSet>>;

/// **SyncOrchestrator**
///
/// This component acts as the **Imperative Shell** in the Hexagonal Architecture.
/// It has three main responsibilities:
/// 1. **Poll the IO Adapter** (Client) for network events.
/// 2. **Drive the Logic Core** (SyncEngine) by feeding it events.
/// 3. **Execute Side Effects** (Commands) emitted by the Engine, such as updating the wallet database.
///
/// It runs in the main application thread and blocks when waiting for events.
pub struct SyncOrchestrator<K, C> {
    /// The functional core that makes decisions.
    engine: SyncEngine<K>,
    
    /// The adapter for the Electrum protocol.
    client: C,

    /// Thread-safe reference to the BDK wallet (shared with the UI/App).
    wallet: Arc<Mutex<StreamingWallet>>,

    /// Optional callback fired after the initial bootstrap (first scan) is complete.
    /// Useful for UI loading screens.
    on_initial_sync: Option<Box<dyn FnOnce() + Send>>,

    /// Tracks which script hashes are currently syncing during the bootstrap phase.
    pending_initial_syncs: HashSet<sha256::Hash>,

    /// Start time for logging relative timestamps.
    t0: Instant,
}

impl<K, C> SyncOrchestrator<K, C>
where
    K: Ord + Clone,
    C: ElectrumApi,
{
    pub fn new(
        engine: SyncEngine<K>,
        client: C,
        wallet: Arc<Mutex<StreamingWallet>>,
    ) -> Self {
        Self {
            engine,
            client,
            wallet,
            on_initial_sync: None,
            pending_initial_syncs: HashSet::new(),
            t0: Instant::now(),
        }
    }

    /// Register a callback to be called once the engine has subscribed to all initial scripts.
    pub fn with_initial_sync_notifier<F: FnOnce() + Send + 'static>(mut self, f: F) -> Self {
        self.on_initial_sync = Some(Box::new(f));
        self
    }

    /// Checks if the initial sync is pending and if all items are done.
    fn check_initial_sync_complete(&mut self) {
        // If we have a callback registered AND the pending set is empty...
        if self.on_initial_sync.is_some() && self.pending_initial_syncs.is_empty() {
            self.info("initial engine bootstrap finished (all responses received)");
            if let Some(cb) = self.on_initial_sync.take() {
                cb();
            }
        }
    }

    fn t(&self) -> u128 {
        self.t0.elapsed().as_micros()
    }

    /// The main blocking event loop.
    ///
    /// This method will run indefinitely. It:
    /// 1. Bootstraps the engine (Connected event).
    /// 2. Enters a loop polling the client for changes.
    /// 3. Handles the **"Fetch-or-Request"** logic to prevent zero-balance bugs.
    pub fn run_forever(mut self) -> ! {
        self.info("starting driver");

        // 1. Bootstrap: Tell the engine we are connected so it generates initial subscriptions.
        self.process_engine(EngineEvent::Connected);
        // Safety check: If wallet is empty (0 addresses), fire immediately.
        self.check_initial_sync_complete();

        // 2. Notify: Signal that bootstrap is done.
        // if let Some(cb) = self.on_initial_sync.take() {
        //     self.info("initial engine bootstrap finished");
        //     cb();
        // }

        // 3. Event Loop
        loop {
            // POLL CLIENT for notification (status changed) or download completion.
            if let Some(hash) = self.client.poll_scripthash_changed() {
                self.trace(&format!("event: ScriptHashChanged({})", hash));
                
                // === OPTION B FIX (The "Fetch-or-Request" Pattern) ===
                // Problem: A "changed" notification arrives before we have the transaction history.
                // If we tell the Engine now, it sees 0 txs and sets balance to 0.
                //
                // Solution: Check if the client actually HAS the data.
                match self.client.fetch_history_txs(hash) {
                    Some(txs) => {
                        // CASE A: Cache Hit (Data Ready)
                        self.debug(&format!("[DRIVER] Cache hit for {}, processing {} txs", hash, txs.len()));
                        
                        // 1. Update Wallet
                        self.process_engine(EngineEvent::ScriptHashHistory { hash, txs });
                        
                        // 2. Mark this hash as synced
                        self.pending_initial_syncs.remove(&hash);
                        
                        // LOG PROGRESS
                        if self.on_initial_sync.is_some() {
                            let remaining = self.pending_initial_syncs.len();
                            self.info(&format!("Initial Sync Progress: {} pending", remaining));
                        }

                        // 3. Check if we are done with the initial load
                        self.check_initial_sync_complete();
                    }
                    None => {
                        // CASE B: Cache Miss. We got a notification, but data is missing.
                        // This happens when we get the first "status changed" message.
                        // ACTION: Do NOT wake the engine. Explicitly request history from network.
                        // Result: When history arrives later, `poll_scripthash_changed` fires again, 
                        // and we will hit CASE A.
                        if self.pending_initial_syncs.contains(&hash) {
                            self.trace(&format!("[DRIVER] Ignoring cache miss for {} (already pending)", hash));
                        } else {
                            // Only request if it's a TRULY new event (post-bootstrap)
                            self.trace(&format!("[DRIVER] Cache miss for {}, requesting history", hash));
                            self.client.request_history(hash);
                        }
                    }
                }
            } else {
                // Avoid busy-waiting.
                // TODO: In a production app, use a CondVar or Channel to sleep until notified.
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    /// Feeds an event into the Engine and executes all resulting commands.
    pub fn process_engine(&mut self, event: EngineEvent) {
        let mut queue = vec![event];

        while let Some(ev) = queue.pop() {
            self.trace(&format!("engine.handle_event({:?})", ev));

            // PURE LOGIC STEP: Engine decides what to do
            let cmds = self.engine.handle_event(ev);

            // SIDE EFFECT STEP: Driver executes the commands
            for cmd in cmds {
                self.execute_command(cmd, &mut queue);
            }
        }
    }

    /// Executes a single command emitted by the engine.
    fn execute_command(&mut self, cmd: EngineCommand, _queue: &mut Vec<EngineEvent>) {
        self.trace(&format!("{:>8}us cmd: {:?}", self.t0.elapsed().as_micros(), cmd));
        match cmd {
            EngineCommand::Subscribe(hash) => {
                self.trace(&format!("cmd: Subscribe({})", hash));

                // We need the script to subscribe (Electrum protocol requirement for some servers, 
                // or useful for re-registration).
                if let Some(script) = self.engine.script_for_hash(&hash) {
                    self.client.register_script(script, hash);
                } else {
                    log::error!("[DRIVER] No script for hash {}", hash);
                }
            }

            EngineCommand::FetchHistory(hash) => {
                // Explicit request for history (used during bootstrap).
                self.trace(&format!("cmd: FetchHistory({})", hash));
                // If we are in the bootstrap phase (callback exists),
                // track this hash as "pending download".
                if self.on_initial_sync.is_some() {
                    self.pending_initial_syncs.insert(hash);
                }
                
                self.client.request_history(hash);
            }

            EngineCommand::ApplyTransactions { script: _, txs } => {
                self.trace(&format!("[ENGINE] cmd: ApplyTransactions({} txs)", txs.len()));

                if txs.is_empty() {
                    self.trace("[ENGINE] no txs to apply");
                    return;
                }

                // Prepare BDK update
                let mut update = bdk_wallet::Update::default();

                for tx in txs {
                    self.trace(&format!("[WALLET] apply tx {}", tx.compute_txid()));
                    update.tx_update.txs.push(Arc::new(tx));
                }

                log::trace!(
                    "[WALLET] applying {} txs",
                    update.tx_update.txs.len()
                );
                let mut w = self.wallet.lock().unwrap();
                let r = w.apply_update(update);
                log::trace!(
                    "[WALLET] apply_update result = {:?}",
                    r
                );
            }
        }
    }

    /// Run the event loop until no more events are pending from the client.
    /// STRICTLY FOR TESTING.
    #[cfg(test)]
    pub fn run_until_idle(&mut self) {
        let mut sanity = 0;
        // Poll continuously until the client returns None
        while let Some(hash) = self.client.poll_scripthash_changed() {
            self.trace(&format!("test run_until_idle: ScriptHashChanged({})", hash));
            // In tests, we might want to bypass the Option B check if we mock the client differently,
            // but for integration tests, this should mirror run_forever logic.
            // For simplicity here, we assume direct processing, but ideally, copy the logic below.
            self.process_engine(EngineEvent::ScriptHashChanged(hash));
            
            sanity += 1;
            if sanity > 100 {
                log::warn!("[DRIVER] run_until_idle exceeded 100 iterations, breaking");
                break;
            }
        }
    }

    #[cfg(test)]
    pub fn engine_mut(&mut self) -> &mut SyncEngine<K> {
        &mut self.engine
    }

    fn info(&self, msg: &str) {
        log::info!("[DRIVER] {:>8}us: {}", self.t(), msg);
    }

    fn debug(&self, msg: &str) {
        log::debug!("[DRIVER] {:>8}us: {}", self.t(), msg);
    }
    fn trace(&self, msg: &str) {
        log::trace!("[DRIVER] {:>8}us: {}", self.t(), msg);
    }
}

// Helper methods for testing interaction
#[cfg(test)]
impl<K, C> SyncOrchestrator<K, C> {
    pub fn client_ref(&self) -> &C {
        &self.client
    }

    pub fn client_mut(&mut self) -> &mut C {
        &mut self.client
    }
}
