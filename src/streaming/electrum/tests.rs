use std::str::FromStr;

use bitcoin::Txid;
use bitcoin::hashes::{sha256d, Hash};

use bdk_wallet::miniscript::{Descriptor, DescriptorPublicKey};

use crate::streaming::engine::StreamingEngine;
use crate::streaming::runtime::ElectrumDriver;
use crate::streaming::electrum::mock_client::MockElectrumClient;
use crate::streaming::domain::spk_tracker::DerivedSpkTracker;

fn test_descriptor() -> Descriptor<DescriptorPublicKey> {
    Descriptor::from_str(
        "wpkh([73c5da0a/84h/1h/0h]tpubDC8msFGeGuwnKG9Upg7DM2b4DaRqg3CUZa5g8v2SRQ6K4NSkxUgd7HsL2XVWbVm39yBA4LAxysQAm397zwQSQoQgewGiYZqrA9DsP4zbQ1M/0/*)",
    )
    .unwrap()
}

fn fake_txid() -> Txid {
    Txid::from_raw_hash(sha256d::Hash::hash(b"e2e-test"))
}

#[test]
fn end_to_end_derives_and_subscribes_more() {
    // --- Build tracker
    let mut tracker = DerivedSpkTracker::<String>::new(2);
    let initial = tracker.insert_descriptor("kc".to_string(), test_descriptor(), 0);
    assert!(!initial.is_empty());

    // --- Build engine
    let engine = StreamingEngine::new(tracker);

    // --- Build mock electrum
    let mock = MockElectrumClient::new();

    // --- Build driver
    let mut driver = ElectrumDriver::new(engine, mock);

    // --- Connect
    driver.connect();

    // Capture initial subscriptions
    let initial_hashes: Vec<_> = {
        let client = driver.client_mut();
        client.subscribed.iter().cloned().collect()
    };

    assert!(!initial_hashes.is_empty());

    // --- Simulate usage of one script
    let used_hash = *initial_hashes.last().unwrap();
    let txid = fake_txid();

    {
        let client = driver.client_mut();
        client.push_tx(used_hash, txid);
    }

    // --- One tick is enough: it does
    // ScriptHashChanged -> FetchHistory -> ScriptHashHistory -> Subscribe(new)
    driver.tick();

    // --- Now engine must have derived more scripts
    let after_hashes: Vec<_> = {
        let client = driver.client_mut();
        client.subscribed.iter().cloned().collect()
    };

    assert!(
        after_hashes.len() > initial_hashes.len(),
        "engine should subscribe newly derived scripts"
    );
}