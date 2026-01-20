#![cfg(FALSE)]
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use bitcoin::Transaction;
use bitcoin::transaction::Version;
use bitcoin::absolute::LockTime;

use bdk_wallet::miniscript::{Descriptor, DescriptorPublicKey};
use bdk_wallet::{PersistedWallet, ChangeSet};
use bdk_wallet::file_store::Store;

use crate::streaming::engine::StreamingEngine;
use crate::streaming::runtime::ElectrumDriver;
use crate::streaming::electrum::mock::client::MockElectrumClient;
use crate::streaming::domain::spk_tracker::DerivedSpkTracker;

type TestWallet = PersistedWallet<Store<ChangeSet>>;

fn dummy_wallet() -> Arc<Mutex<TestWallet>> {
    let store = Store::<ChangeSet>::memory();
    Arc::new(Mutex::new(PersistedWallet::new(store)))
}

fn test_descriptor() -> Descriptor<DescriptorPublicKey> {
    Descriptor::from_str(
        "wpkh([73c5da0a/84h/1h/0h]\
         tpubDC8msFGeGuwnKG9Upg7DM2b4DaRqg3CUZa5g8v2SRQ6K4NSkxUgd7HsL2XVWbVm39yBA4LAxysQAm397zwQSQoQgewGiYZqrA9DsP4zbQ1M/0/*)"
    ).unwrap()
}

fn fake_tx() -> Transaction {
    Transaction {
        version: Version(2),
        lock_time: LockTime::ZERO,
        input: vec![],
        output: vec![],
    }
}

#[test]
fn end_to_end_derives_and_subscribes_more() {
    // --- Tracker
    let mut tracker = DerivedSpkTracker::<String>::new(2);
    tracker.insert_descriptor("kc".to_string(), test_descriptor(), 0);

    // --- Engine
    let engine = StreamingEngine::new(tracker);

    // --- Mock electrum
    let mock = MockElectrumClient::new();

    // --- Wallet
    let wallet = dummy_wallet();

    // --- Driver
    let mut driver = ElectrumDriver::new(engine, mock, wallet);

    // --- Initial bootstrap
    driver.run_until_idle();

    let initial_subs = driver.client_ref().subscribed_len();
    assert!(initial_subs > 0);

    let used_hash = driver
        .client_ref()
        .last_subscribed()
        .expect("should have subscriptions");

    // --- Simulate history with tx
    {
        let client = driver.client_mut();
        client.push_history(used_hash, vec![fake_tx()]);
    }

    driver.run_until_idle();

    let after_subs = driver.client_ref().subscribed_len();

    assert!(
        after_subs > initial_subs,
        "engine should derive and subscribe more scripts"
    );
}