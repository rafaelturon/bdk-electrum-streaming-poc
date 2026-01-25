#![cfg(test)]
use crate::streaming::engine::{StreamingEngine, EngineEvent, EngineCommand};
use crate::streaming::domain::spk_tracker::DerivedSpkTracker;
use bdk_wallet::miniscript::Descriptor;
use std::str::FromStr;
use bitcoin::{Transaction, TxIn, TxOut, ScriptBuf, Amount, Txid};
use bitcoin::transaction::Version;
use bitcoin::absolute::LockTime;
use bitcoin::hashes::Hash;

// =========================================================================
// Helpers
// =========================================================================

fn fake_descriptor(idx: u32) -> Descriptor<bdk_wallet::miniscript::DescriptorPublicKey> {
    let s = format!("wpkh([73c5da0a/84h/1h/0h]tpubDC8msFGeGuwnKG9Upg7DM2b4DaRqg3CUZa5g8v2SRQ6K4NSkxUgd7HsL2XVWbVm39yBA4LAxysQAm397zwQSQoQgewGiYZqrA9DsP4zbQ1M/{}/0/*)", idx);
    Descriptor::from_str(&s).unwrap()
}

// Suppress warning if not used in the modified test flow
#[allow(dead_code)]
fn fake_tx() -> Transaction {
    Transaction {
        version: Version(2),
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: bitcoin::OutPoint {
                txid: Txid::all_zeros(),
                vout: 0,
            },
            script_sig: ScriptBuf::new(),
            sequence: bitcoin::Sequence::MAX,
            witness: bitcoin::Witness::default(),
        }],
        output: vec![TxOut {
            value: Amount::from_sat(1000),
            script_pubkey: ScriptBuf::new(),
        }],
    }
}

fn setup_engine(lookahead: u32, _start_index: u32) -> StreamingEngine<String> {
    let mut tracker = DerivedSpkTracker::new(lookahead);
    tracker.insert_descriptor("external".to_string(), fake_descriptor(0), 0);
    tracker.insert_descriptor("internal".to_string(), fake_descriptor(1), 0);
    StreamingEngine::new(tracker)
}

// =========================================================================
// Tests
// =========================================================================

#[test]
fn no_duplicate_subscriptions() {
    let mut engine = setup_engine(2, 0);

    // 1. First connect
    let cmds1 = engine.handle_event(EngineEvent::Connected);
    assert!(!cmds1.is_empty(), "Should generate commands on first connect");

    // 2. Second connect
    let cmds2 = engine.handle_event(EngineEvent::Connected);
    assert_eq!(cmds2.len(), 0, "Engine should not resubscribe to active scripts");
}

#[test]
fn connected_subscribes_all_spks() {
    let mut engine = setup_engine(2, 0);
    let cmds = engine.handle_event(EngineEvent::Connected);

    let sub_count = cmds.iter().filter(|c| matches!(c, EngineCommand::Subscribe(_))).count();

    // 2 descriptors * 3 addresses (0,1,2) = 6 subscriptions
    assert_eq!(sub_count, 6, "Should generate exactly 6 subscriptions");
}

#[test]
fn history_transition_derives_more() {
    let mut engine = setup_engine(2, 0);

    // 1. Initial Connect
    let cmds = engine.handle_event(EngineEvent::Connected);
    
    // Ensure we have initial subscriptions
    let initial_count = cmds.iter().filter(|c| matches!(c, EngineCommand::Subscribe(_))).count();
    assert_eq!(initial_count, 6);

    // 2. Simulate Tracker Update (Derivation)
    // We insert a NEW descriptor ID to force the tracker to track new scripts.
    // We use index 10 to ensure the generated scripts (indices 10, 11, 12) 
    // are distinct from the initial range (0, 1, 2).
    engine.tracker_mut().insert_descriptor(
        "external_extension".to_string(), 
        fake_descriptor(0), // Same xpub, but new tracker entry
        10
    );

    // 3. Trigger Sync
    // Sending 'Connected' forces the engine to diff the tracker vs active subscriptions.
    let cmds_derive = engine.handle_event(EngineEvent::Connected);

    // 4. Verify New Subscriptions
    let new_subs = cmds_derive.iter()
        .filter(|c| matches!(c, EngineCommand::Subscribe(_)))
        .count();

    assert!(new_subs > 0, "Should derive and subscribe to new scripts after tracker update");
}