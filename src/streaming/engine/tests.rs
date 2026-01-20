use std::str::FromStr;

use bitcoin::Transaction;
use bitcoin::transaction::Version;
use bitcoin::absolute::LockTime;

use bdk_wallet::miniscript::{Descriptor, DescriptorPublicKey};

use crate::streaming::engine::StreamingEngine;
use crate::streaming::domain::spk_tracker::DerivedSpkTracker;
use crate::streaming::engine::types::{EngineCommand, EngineEvent};

fn test_descriptor() -> Descriptor<DescriptorPublicKey> {
    Descriptor::from_str(
        "wpkh([73c5da0a/84h/1h/0h]\
         tpubDC8msFGeGuwnKG9Upg7DM2b4DaRqg3CUZa5g8v2SRQ6K4NSkxUgd7HsL2XVWbVm39yBA4LAxysQAm397zwQSQoQgewGiYZqrA9DsP4zbQ1M/0/*)"
    ).unwrap()
}

fn setup_engine(lookahead: u32, next_index: u32) -> StreamingEngine<String> {
    let mut tracker = DerivedSpkTracker::new(lookahead);
    tracker.insert_descriptor("kc".to_string(), test_descriptor(), next_index);
    StreamingEngine::new(tracker)
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
fn connected_subscribes_all_spks() {
    let mut engine = setup_engine(2, 0);

    let cmds = engine.handle_event(EngineEvent::Connected);

    // lookahead=2, next_index=0 → range 0..=2 → 3 scripts
    assert_eq!(cmds.len(), 3);

    for cmd in cmds {
        matches!(cmd, EngineCommand::Subscribe(_));
    }
}

#[test]
fn history_transition_derives_more() {
    let mut engine = setup_engine(2, 0);

    let cmds = engine.handle_event(EngineEvent::Connected);
    let first_hash = match &cmds[0] {
        EngineCommand::Subscribe(h) => h.clone(),
        _ => unreachable!(),
    };

    // Empty history → no-op
    let cmds = engine.handle_event(EngineEvent::ScriptHashHistory {
        hash: first_hash.clone(),
        txs: vec![],
    });
    assert!(cmds.is_empty());

    // Non-empty history → derive more
    let cmds = engine.handle_event(EngineEvent::ScriptHashHistory {
        hash: first_hash,
        txs: vec![fake_tx()],
    });

    assert!(cmds.iter().any(|c| matches!(c, EngineCommand::Subscribe(_))));
}

#[test]
fn no_duplicate_subscriptions() {
    let mut engine = setup_engine(2, 0);

    let cmds1 = engine.handle_event(EngineEvent::Connected);
    let cmds2 = engine.handle_event(EngineEvent::Connected);

    assert!(!cmds1.is_empty());
    assert!(cmds2.is_empty());
}
