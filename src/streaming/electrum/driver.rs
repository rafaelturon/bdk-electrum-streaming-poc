use crate::streaming::engine::StreamingEngine;
use crate::streaming::types::{EngineCommand, EngineEvent};

use bitcoin::{Script, ScriptBuf};

pub trait ElectrumApi {
    fn subscribe_script(&mut self, script: &Script);
    fn fetch_history(&mut self, script: &Script) -> Vec<bitcoin::Txid>;
    fn poll_script_changed(&mut self) -> Option<ScriptBuf>;
}

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

    pub fn run_forever(mut self) -> ! {
        self.process_engine(EngineEvent::Connected);

        loop {
            if let Some(script) = self.client.poll_script_changed() {
                use bitcoin::hashes::{sha256, Hash};
                let hash = sha256::Hash::hash(script.as_bytes());

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
                self.client.subscribe_script(&script);
            }

            EngineCommand::FetchHistory(hash) => {
                let script = self.engine.script_for_hash(&hash).expect("engine invariant");
                let txs = self.client.fetch_history(&script);

                queue.push(EngineEvent::ScriptHashHistory { hash, txs });
            }
        }
    }

    pub fn into_engine(self) -> StreamingEngine<K> {
        self.engine
    }
}
