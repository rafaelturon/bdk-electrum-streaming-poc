use crate::streaming::engine::StreamingEngine;
use crate::streaming::engine::types::{EngineCommand, EngineEvent};
use crate::streaming::electrum::api::ElectrumApi;


/// Drives the StreamingEngine using an Electrum client.
pub struct ElectrumDriver<K, C> {
    engine: StreamingEngine<K>,
    client: C,
}

impl<K, C> ElectrumDriver<K, C>
where
    K: Ord + Clone,
    C: ElectrumApi,
{
    pub fn new(engine: StreamingEngine<K>, client: C) -> Self {
        Self { engine, client }
    }

    /// For tests: simulate initial connection
    pub fn connect(&mut self) {
        self.process_engine(EngineEvent::Connected);
    }

    /// For tests: process exactly one poll tick
    pub fn tick(&mut self) {
        while let Some(hash) = self.client.poll_scripthash_changed() {
            self.process_engine(EngineEvent::ScriptHashChanged(hash));
        }
    }

    /// Run forever (blocking)
    pub fn run_forever(mut self) -> ! {
        log::info!("[DRIVER] Connected");

        self.process_engine(EngineEvent::Connected);

        loop {
            log::info!("[DRIVER] Waiting for change...");
            if let Some(hash) = self.client.poll_scripthash_changed() {
                log::info!("[DRIVER] Change detected: {}", hash);
                self.process_engine(EngineEvent::ScriptHashChanged(hash));
            }
        }
    }

    fn process_engine(&mut self, event: EngineEvent) {
        let mut queue = vec![event];

        while let Some(ev) = queue.pop() {
            let cmds = self.engine.handle_event(ev);

            for cmd in cmds {
                self.execute_command(cmd, &mut queue);
            }
        }
    }

    fn execute_command(&mut self, cmd: EngineCommand, queue: &mut Vec<EngineEvent>) {
        match cmd {
             EngineCommand::Subscribe(hash) => {
                let script = self.engine.script_for_hash(&hash).expect("engine invariant");
                self.client.register_script(script, hash);
            }

            EngineCommand::FetchHistory(hash) => {
                let txs = self.client.fetch_history(hash);
                queue.push(EngineEvent::ScriptHashHistory { hash, txs });
            }
        }
    }

    /// For tests / inspection
    pub fn into_engine(self) -> StreamingEngine<K> {
        self.engine
    }

    /// For tests
    pub fn client_mut(&mut self) -> &mut C {
        &mut self.client
    }
}
