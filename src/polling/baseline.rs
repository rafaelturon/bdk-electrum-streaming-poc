// BDK 2.3 compatible polling baseline

use anyhow::Result;
use bdk_electrum::BdkElectrumClient;
use bdk_wallet::{PersistedWallet, ChangeSet};
use bdk_wallet::file_store::Store;
use std::time::{Duration, Instant};

const MARKER_FILE: &str = "initial_scan_done.marker";
pub struct SyncStats {
    pub total_time: Duration,
    pub rounds: usize,
}

fn has_done_initial_scan() -> bool {
    std::path::Path::new(MARKER_FILE).exists()
}

fn mark_initial_scan_done() -> std::io::Result<()> {
    std::fs::write(MARKER_FILE, b"ok")
}

pub fn auto_sync(
    wallet: &mut PersistedWallet<Store<ChangeSet>>,
    client: &BdkElectrumClient<bdk_electrum::electrum_client::Client>,
    rounds: usize,
) -> Result<SyncStats> {
    if !has_done_initial_scan() {
        log::info!("[SYNC] No scan marker: running COLD START scan");
        let stats = cold_start_sync(wallet, client, rounds)?;
        mark_initial_scan_done()?;
        Ok(stats)
    } else {
        log::info!("[SYNC] First WARM run after restart will still be slow (no streaming cache yet)");
        warm_sync(wallet, client)
    }
}

pub fn cold_start_sync(
    wallet: &mut PersistedWallet<Store<ChangeSet>>,
    client: &BdkElectrumClient<bdk_electrum::electrum_client::Client>,
    rounds: usize,
) -> Result<SyncStats> {
    log::info!("[COLD] Starting progressive sync...");
    let global_start = Instant::now();

    for round in 1..=rounds {
        log::info!("[COLD] Sync round #{} ...", round);
        let round_start = Instant::now();
        let request = wallet.start_full_scan().build();
        // stop_gap = 20 → discovery mode
        let update = client.full_scan(request, 20, 5, false)?;
        wallet.apply_update(update)?;

        let round_time = round_start.elapsed();
        log::info!("[COLD] Round #{} done in {:?}", round, round_time);
    }

    let total_time = global_start.elapsed();

    Ok(SyncStats {
        total_time,
        rounds,
    })
}

pub fn warm_sync(
    wallet: &mut PersistedWallet<Store<ChangeSet>>,
    client: &BdkElectrumClient<bdk_electrum::electrum_client::Client>,
) -> Result<SyncStats> {
    log::info!("[WARM] Starting incremental sync loop...");
    let rounds = 1;
    let global_start = Instant::now();

    for round in 1..=rounds {
        log::info!("[WARM] Sync round #{} ...", round);
        let round_start = Instant::now();
        // stop_gap = 0  → disables discovery
        let request = wallet.start_full_scan().build();
        let update = client.full_scan(request, 0, 5, false)?;
        wallet.apply_update(update)?;

        let round_time = round_start.elapsed();
        log::info!("[WARM] Round #{} done in {:?}", round, round_time);
    }

    let total_time = global_start.elapsed();

    Ok(SyncStats {
        total_time,
        rounds,
    })
}





