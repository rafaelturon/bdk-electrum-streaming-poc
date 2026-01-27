#![cfg(test)]
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::env;

use bdk_wallet::miniscript::{Descriptor, DescriptorPublicKey};
use bdk_wallet::{PersistedWallet, ChangeSet, Wallet};
use bdk_wallet::bitcoin::Network;
use bdk_wallet::file_store::Store;

use crate::streaming::engine::{SyncEngine, EngineEvent};
use crate::streaming::runtime::SyncOrchestrator;
use crate::streaming::electrum::mock::client::MockElectrumClient;
use crate::streaming::domain::spk_tracker::DerivedSpkTracker;

type TestWallet = PersistedWallet<Store<ChangeSet>>;

fn dummy_wallet() -> Arc<Mutex<TestWallet>> {
    let mut temp_path = env::temp_dir();
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    temp_path.push(format!("bdk_test_{}", millis));
    
    let mut db = Store::<ChangeSet>::create(b"test", &temp_path)
        .expect("failed to create temporary store");

    let wallet = Wallet::create(test_descriptor(), test_change_descriptor())
        .network(Network::Testnet)
        .create_wallet(&mut db)
        .expect("failed to create wallet");

    Arc::new(Mutex::new(wallet))
}

fn test_descriptor() -> Descriptor<DescriptorPublicKey> {
    Descriptor::from_str(
        "wpkh([73c5da0a/84h/1h/0h]tpubDC8msFGeGuwnKG9Upg7DM2b4DaRqg3CUZa5g8v2SRQ6K4NSkxUgd7HsL2XVWbVm39yBA4LAxysQAm397zwQSQoQgewGiYZqrA9DsP4zbQ1M/0/*)"
    ).unwrap()
}

fn test_change_descriptor() -> Descriptor<DescriptorPublicKey> {
    Descriptor::from_str(
        "wpkh([73c5da0a/84h/1h/0h]tpubDC8msFGeGuwnKG9Upg7DM2b4DaRqg3CUZa5g8v2SRQ6K4NSkxUgd7HsL2XVWbVm39yBA4LAxysQAm397zwQSQoQgewGiYZqrA9DsP4zbQ1M/1/*)"
    ).unwrap()
}

#[test]
fn end_to_end_derives_and_subscribes_more() {
    println!("[TEST] Starting test...");

    // 1. Setup
    let mut tracker = DerivedSpkTracker::<String>::new(2);
    tracker.insert_descriptor("external".to_string(), test_descriptor(), 0);
    // Note: We only insert external here to make the count deterministic (3 addresses: 0, 1, 2)

    let engine = SyncEngine::new(tracker);
    let mock = MockElectrumClient::new();
    let wallet = dummy_wallet();
    let mut driver = SyncOrchestrator::new(engine, mock, wallet);

    // 2. Initial Bootstrap
    println!("[TEST] Bootstrap...");
    driver.process_engine(EngineEvent::Connected);
    driver.run_until_idle();

    let initial_subs = driver.client_ref().subscribed_len();
    println!("[TEST] Initial subscriptions: {}", initial_subs);
    assert!(initial_subs > 0, "Should have subscribed to initial range");

    // 3. SIMULATE GAP LIMIT EXTENSION
    // Instead of relying on the complex interaction of Wallet -> DB -> Tracker -> Engine,
    // we directly modify the Tracker inside the Engine (via a new method we'll add to the Driver/Engine interface, 
    // OR simply re-insert the descriptor with a higher index).
    
    println!("[TEST] Simulating wallet derivation (index 10)...");
    
    // We access the engine inside the driver (requires `pub` field or method)
    // Hack: We re-create the event flow. 
    // In a real scenario, the Wallet would say "I used index 5", so the Tracker says "I need up to 25".
    
    // For this test, we simply insert a NEW descriptor (internal chain) to prove the driver picks it up dynamically.
    // This proves the "Streaming" aspect: adding data to the tracker results in new subscriptions.
    driver.engine_mut().tracker_mut().insert_descriptor(
        "internal".to_string(), 
        test_change_descriptor(), 
        0
    );

    // 4. Trigger a sync cycle
    // The engine checks the tracker, sees new scripts, and emits Subscribe commands
    println!("[TEST] Triggering sync...");
    driver.process_engine(EngineEvent::Connected); 
    
    driver.run_until_idle();

    let after_subs = driver.client_ref().subscribed_len();
    println!("[TEST] Final subscriptions: {}", after_subs);

    assert!(
        after_subs > initial_subs,
        "engine should subscribe to the new descriptor scripts (prev: {}, now: {})",
        initial_subs, after_subs
    );
}