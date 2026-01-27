use bitcoin::hashes::sha256;
use std::time::Instant;
use bitcoin::Txid;
use crate::streaming::engine::state::EngineState;
use crate::streaming::engine::types::EngineCommand;

pub fn on_connected<K: Ord + Clone>(state: &mut EngineState<K>) -> Vec<EngineCommand> {
    log::info!("[ENGINE] on_connected: enumerating scripts");
    let count = state.spk_tracker.all_spks().count();
    log::debug!("[ENGINE] total scripts = {}", count);

    state.connected = true;

    let mut cmds = Vec::new();

    for (hash, script) in state.spk_tracker.all_spks() {
        log::trace!("[ENGINE] discovered script {}", hash);
        if let Some((kc, idx)) = state.spk_tracker.index_of_spk_hash(hash) {
            state.spk_index_by_hash.insert(*hash, (kc, idx));
            state.script_by_hash.insert(*hash, script.clone());
        }

        if state.subscribed.insert(*hash) {
            // 1) WARM BOOTSTRAP: fetch full history first
            cmds.push(EngineCommand::FetchHistory(*hash));

            // 2) Then subscribe for future updates
            cmds.push(EngineCommand::Subscribe(*hash));
        }
    }

    cmds
}

pub fn on_scripthash_changed<K>(_: &mut EngineState<K>, hash: sha256::Hash) -> Vec<EngineCommand> {
    vec![EngineCommand::FetchHistory(hash)]
}

pub fn on_scripthash_history<K: Ord + Clone>(
    state: &mut EngineState<K>,
    hash: sha256::Hash,
    txs: Vec<bitcoin::Transaction>,
) -> Vec<EngineCommand> {
    // BENCHMARK HOOK — FIRST REAL DATA
    if state.first_history_seen_at.is_none() && !txs.is_empty() {
        state.first_history_seen_at = Some(Instant::now());
        println!(
            "STREAMING WALLET READY — first history received after {:?}",
            state.start_time.elapsed()
        );
    }

    let mut cmds = Vec::new();

    let prev = state.histories.get(&hash);
    let was_empty = prev.map(|v| v.is_empty()).unwrap_or(true);
    let is_empty = txs.is_empty();

    let now = Instant::now();
    // First history response with any content
    if state.first_history_seen_at.is_none() && !txs.is_empty() {
        state.first_history_seen_at = Some(now);
        log::info!(
            "[METRIC] First non-empty history at {:?}",
            now.duration_since(state.start_time)
        );
    }
    // First TX seen globally
    if state.first_tx_seen_at.is_none() && !txs.is_empty() {
        state.first_tx_seen_at = Some(now);
        log::info!(
            "[METRIC] First TX seen at {:?}",
            now.duration_since(state.start_time)
        );
    }    
    let txids: Vec<Txid> = txs.iter().map(|t| t.compute_txid()).collect();
    state.histories.insert(hash, txids.clone());


    if was_empty && !is_empty {
        if let Some((keychain, index)) = state.spk_index_by_hash.get(&hash).cloned() {
            let newly = state
                .spk_tracker
                .mark_used_and_derive_new(&keychain, index);

            for (new_hash, new_script) in newly {
                state.spk_index_by_hash.insert(new_hash, (keychain.clone(), index + 1));
                state.script_by_hash.insert(new_hash, new_script.clone());

                if state.subscribed.insert(new_hash) {
                    // 1) Warm fetch for newly derived script
                    cmds.push(EngineCommand::FetchHistory(new_hash));

                    // 2) Then subscribe for future updates
                    cmds.push(EngineCommand::Subscribe(new_hash));
                }
            }
        }
    }

    let script = state.script_by_hash.get(&hash).cloned().unwrap();
    cmds.push(EngineCommand::ApplyTransactions {
        script,
        txs,
    });

    cmds
}
