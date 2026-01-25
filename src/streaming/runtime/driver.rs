use crate::streaming::engine::StreamingEngine;
use crate::streaming::engine::types::{EngineCommand, EngineEvent};
use crate::streaming::electrum::api::ElectrumApi;

use bdk_wallet::{PersistedWallet, ChangeSet};
use bdk_wallet::file_store::Store;

use std::sync::{Arc, Mutex};
use std::time::{Instant, Duration};

type StreamingWallet = PersistedWallet<Store<ChangeSet>>;

/// Drives the StreamingEngine using an Electrum client.
pub struct ElectrumDriver<K, C> {
    engine: StreamingEngine<K>,
    client: C,
    wallet: Arc<Mutex<StreamingWallet>>,

    /// Optional callback fired after initial engine bootstrap finishes
    on_initial_sync: Option<Box<dyn FnOnce() + Send>>,

    /// Timeline
    t0: Instant,
}

impl<K, C> ElectrumDriver<K, C>
where
    K: Ord + Clone,
    C: ElectrumApi,
{
    pub fn new(
        engine: StreamingEngine<K>,
        client: C,
        wallet: Arc<Mutex<StreamingWallet>>,
    ) -> Self {
        Self {
            engine,
            client,
            wallet,
            on_initial_sync: None,
            t0: Instant::now(),
        }
    }

    pub fn with_initial_sync_notifier<F: FnOnce() + Send + 'static>(mut self, f: F) -> Self {
        self.on_initial_sync = Some(Box::new(f));
        self
    }

    fn t(&self) -> u128 {
        self.t0.elapsed().as_micros()
    }

    #[cfg(test)]
    pub fn run_until_idle(&mut self) {
        let mut sanity = 0;
        while let Some(hash) = self.client.poll_scripthash_changed() {
            self.trace(&format!("test run_until_idle: ScriptHashChanged({})", hash));
            self.process_engine(EngineEvent::ScriptHashChanged(hash));
            
            sanity += 1;
            if sanity > 100 {
                log::warn!("[DRIVER] run_until_idle exceeded 100 iterations, breaking");
                break;
            }
        }
    }

    #[cfg(test)]
    pub fn engine_mut(&mut self) -> &mut StreamingEngine<K> {
        &mut self.engine
    }

    fn info(&self, msg: &str) {
        log::trace!("[DRIVER] {:>8}us: {}", self.t(), msg);
    }

    fn trace(&self, msg: &str) {
        log::trace!("[DRIVER] {:>8}us: {}", self.t(), msg);
    }

    fn debug(&self, msg: &str) {
        log::debug!("[DRIVER] {:>8}us: {}", self.t(), msg);
    }

    pub fn run_forever(mut self) -> ! {
        self.info("starting driver");

        // ---- Bootstrap engine
        self.process_engine(EngineEvent::Connected);

        // ---- Signal "initial sync done" (engine is fully enumerated & requests issued)
        if let Some(cb) = self.on_initial_sync.take() {
            self.trace("initial engine bootstrap finished");
            cb();
        }

        // ---- Main loop
        loop {
            if let Some(hash) = self.client.poll_scripthash_changed() {
                self.trace(&format!("event: ScriptHashChanged({})", hash));
                self.process_engine(EngineEvent::ScriptHashChanged(hash));
            } else {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    pub fn process_engine(&mut self, event: EngineEvent) {
        let mut queue = vec![event];

        while let Some(ev) = queue.pop() {
            self.trace(&format!("engine.handle_event({:?})", ev));

            let cmds = self.engine.handle_event(ev);

            for cmd in cmds {
                self.execute_command(cmd, &mut queue);
            }
        }
    }

    fn execute_command(&mut self, cmd: EngineCommand, _queue: &mut Vec<EngineEvent>) {
        self.debug(&format!("{:>8}us cmd: {:?}", self.t0.elapsed().as_micros(), cmd));
        match cmd {
            EngineCommand::Subscribe(hash) => {
                self.trace(&format!("cmd: Subscribe({})", hash));

                if let Some(script) = self.engine.script_for_hash(&hash) {
                    self.client.register_script(script, hash);
                } else {
                    log::error!("[DRIVER] No script for hash {}", hash);
                }
            }

            EngineCommand::FetchHistory(hash) => {
                self.trace(&format!("cmd: FetchHistory({})", hash));
                self.client.request_history(hash);
            }

            EngineCommand::ApplyTransactions { script: _, txs } => {
                self.trace(&format!("[ENGINE] cmd: ApplyTransactions({} txs)", txs.len()));

                if txs.is_empty() {
                    self.trace("[ENGINE] no txs to apply");
                    return;
                }

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
}

#[cfg(test)]
impl<K, C> ElectrumDriver<K, C> {
    pub fn client_ref(&self) -> &C {
        &self.client
    }

    pub fn client_mut(&mut self) -> &mut C {
        &mut self.client
    }
}
