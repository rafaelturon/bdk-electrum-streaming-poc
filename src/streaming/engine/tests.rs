use std::str::FromStr;
use bitcoin::hashes::Hash;
use bitcoin::Txid;

use bdk_wallet::miniscript::{Descriptor, DescriptorPublicKey};

use crate::streaming::engine::StreamingEngine;
use crate::streaming::jobs::spk_tracker::DerivedSpkTracker;
use crate::streaming::types::{EngineCommand, EngineEvent};

fn test_descriptor() -> Descriptor<DescriptorPublicKey> {
    Descriptor::from_str(
        "wpkh([73c5da0a/84h/1h/0h]tpubDC8msFGeGuwnKG9Upg7DM2b4DaRqg3CUZa5g8v2SRQ6K4NSkxUgd7HsL2XVWbVm39yBA4LAxysQAm397zwQSQoQgewGiYZqrA9DsP4zbQ1M/0/*)"
    ).unwrap()
}


fn setup_engine(lookahead: u32, next_index: u32) -> StreamingEngine<String> {
    let mut tracker = DerivedSpkTracker::new(lookahead);

    tracker.insert_descriptor("kc".to_string(), test_descriptor(), next_index);

    StreamingEngine::new(tracker)
}

fn fake_txid() -> Txid {
    use bitcoin::hashes::sha256d;
    Txid::from_raw_hash(sha256d::Hash::hash(b"fake-tx"))
}

#[test]
fn connected_subscribes_all_spks() {
    let mut engine = setup_engine(2, 0);

    let cmds = engine.handle_event(EngineEvent::Connected);

    // next_index = 0, lookahead = 2
    // Derived range = 0..=2 => 3 spks
    assert_eq!(cmds.len(), 3);

    for cmd in cmds {
        match cmd {
            EngineCommand::Subscribe(_) => {}
            other => panic!("unexpected command: {:?}", other),
        }
    }
}

#[test]
fn history_transition_derives_more() {
    let mut engine = setup_engine(2, 0);

    // First connect
    let cmds = engine.handle_event(EngineEvent::Connected);
    assert!(!cmds.is_empty());

    // Take first subscription hash (CLONE it, don't move)
    let first_hash = match &cmds.last().unwrap() {
        EngineCommand::Subscribe(h) => h.clone(),
        _ => panic!("expected Subscribe"),
    };

    // First: empty history (no-op)
    let cmds = engine.handle_event(EngineEvent::ScriptHashHistory {
        hash: first_hash.clone(),
        txs: vec![],
    });
    assert!(cmds.is_empty());

    // Then: non-empty history → should derive more
    let cmds = engine.handle_event(EngineEvent::ScriptHashHistory {
        hash: first_hash.clone(),
        txs: vec![fake_txid()],
    });

    // Should subscribe at least 1 new spk
    assert!(!cmds.is_empty());

    let mut saw_subscribe = false;
    for cmd in cmds {
        match cmd {
            EngineCommand::Subscribe(_) => saw_subscribe = true,
            other => panic!("unexpected command: {:?}", other),
        }
    }

    assert!(saw_subscribe, "should subscribe to newly derived spk");
}

#[test]
fn no_duplicate_subscriptions() {
    let mut engine = setup_engine(2, 0);

    // First connect
    let cmds1 = engine.handle_event(EngineEvent::Connected);
    let count1 = cmds1.len();
    assert!(count1 > 0);

    // Second connect should not resubscribe
    let cmds2 = engine.handle_event(EngineEvent::Connected);
    assert_eq!(cmds2.len(), 0);
}

#[test]
fn engine_never_duplicates_subscriptions() {
    let mut engine = setup_engine(2, 0);

    let cmds = engine.handle_event(EngineEvent::Connected);

    let first = match &cmds[0] {
        EngineCommand::Subscribe(h) => h.clone(),
        _ => unreachable!(),
    };

    // Trigger derivation
    engine.handle_event(EngineEvent::ScriptHashHistory {
        hash: first.clone(),
        txs: vec![fake_txid()],
    });

    // Trigger again with same history
    let cmds = engine.handle_event(EngineEvent::ScriptHashHistory {
        hash: first,
        txs: vec![fake_txid()],
    });

    // Should NOT emit new subscriptions
    assert!(cmds.is_empty());
}
