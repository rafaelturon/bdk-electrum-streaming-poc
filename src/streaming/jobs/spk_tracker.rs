// src/streaming/jobs/spk_tracker.rs
// Gap limit + derivation tracker (adapted from Evan Linjin's experiment)

use std::collections::{btree_map, BTreeMap, HashMap};

use bitcoin::hashes::{sha256, Hash};
use bdk_wallet::miniscript::{Descriptor, DescriptorPublicKey};

/// Tracks derived script hashes for descriptors with gap limit.
///
/// Internally:
/// - Tracks (keychain, index) -> scripthash
/// - Tracks reverse scripthash -> (keychain, index)
///
/// Externally:
/// - Only exposes sha256(script) hashes (Electrum scripthash)
#[derive(Debug, Clone)]
pub struct DerivedSpkTracker<K> {
    lookahead: u32,
    descriptors: BTreeMap<K, Descriptor<DescriptorPublicKey>>,
    derived_spks: BTreeMap<(K, u32), sha256::Hash>,
    derived_spks_rev: HashMap<sha256::Hash, (K, u32)>,
}

impl<K: Ord + Clone> DerivedSpkTracker<K> {
    pub fn new(lookahead: u32) -> Self {
        Self {
            lookahead,
            descriptors: BTreeMap::new(),
            derived_spks: BTreeMap::new(),
            derived_spks_rev: HashMap::new(),
        }
    }

    /// Iterate all currently tracked script hashes.
    pub fn all_spk_hashes(&self) -> impl Iterator<Item = sha256::Hash> + '_ {
        self.derived_spks.values().cloned()
    }

    /// Reverse lookup: given a scripthash, return (keychain, index)
    pub fn index_of_spk_hash(&self, hash: &sha256::Hash) -> Option<(K, u32)> {
        self.derived_spks_rev.get(hash).cloned()
    }

    fn add_derived_spk(&mut self, keychain: K, index: u32) -> Option<sha256::Hash> {
        if let btree_map::Entry::Vacant(entry) =
            self.derived_spks.entry((keychain.clone(), index))
        {
            let descriptor = self
                .descriptors
                .get(&keychain)
                .expect("descriptor must exist");

            let spk = descriptor
                .at_derivation_index(index)
                .expect("descriptor must derive")
                .script_pubkey();

            let hash = sha256::Hash::hash(spk.as_bytes());

            entry.insert(hash);
            self.derived_spks_rev.insert(hash, (keychain, index));

            return Some(hash);
        }
        None
    }

    fn clear_keychain(&mut self, keychain: &K) {
        let removed = self
            .derived_spks
            .extract_if(.., |(kc, _), _| kc == keychain);

        for (_, hash) in removed {
            self.derived_spks_rev.remove(&hash);
        }
    }

    /// Insert or replace a descriptor and derive initial lookahead range.
    ///
    /// Returns newly derived scripthashes.
    pub fn insert_descriptor(
        &mut self,
        keychain: K,
        descriptor: Descriptor<DescriptorPublicKey>,
        next_index: u32,
    ) -> Vec<sha256::Hash> {
        if let Some(old) = self.descriptors.insert(keychain.clone(), descriptor.clone()) {
            if old == descriptor {
                return vec![];
            }
            self.clear_keychain(&keychain);
        }

        (0..=next_index + self.lookahead)
            .filter_map(|i| self.add_derived_spk(keychain.clone(), i))
            .collect()
    }

    /// Mark (keychain, index) as used and derive new lookahead scripts.
    ///
    /// Returns newly derived scripthashes.
    pub fn mark_used_and_derive_new(
        &mut self,
        keychain: &K,
        index: u32,
    ) -> Vec<sha256::Hash> {
        let next_index = index + 1;

        let mut newly_derived = Vec::new();

        for i in next_index..=next_index + self.lookahead {
            if let Some(hash) = self.add_derived_spk(keychain.clone(), i) {
                newly_derived.push(hash);
            } else {
                break;
            }
        }

        newly_derived
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn test_descriptor() -> Descriptor<DescriptorPublicKey> {
        Descriptor::from_str(
            "wpkh([73c5da0a/84h/1h/0h]tpubDC8msFGeGuwnKG9Upg7DM2b4DaRqg3CUZa5g8v2SRQ6K4NSkxUgd7HsL2XVWbVm39yBA4LAxysQAm397zwQSQoQgewGiYZqrA9DsP4zbQ1M/0/*)"
        ).unwrap()
    }

    #[test]
    fn insert_descriptor_derives_initial_range() {
        let mut tracker = DerivedSpkTracker::<String>::new(2);

        let added = tracker.insert_descriptor(
            "external".to_string(),
            test_descriptor(),
            0,
        );

        // next_index=0, lookahead=2 → derive [0..=2] → 3 scripts
        assert_eq!(added.len(), 3);
        assert_eq!(tracker.derived_spks.len(), 3);
    }

    #[test]
    fn reinserting_same_descriptor_is_noop() {
        let mut tracker = DerivedSpkTracker::<String>::new(2);

        let desc = test_descriptor();

        let a1 = tracker.insert_descriptor("kc".to_string(), desc.clone(), 0);
        let a2 = tracker.insert_descriptor("kc".to_string(), desc.clone(), 0);

        assert!(!a1.is_empty());
        assert!(a2.is_empty());
    }

    #[test]
    fn reverse_lookup_works() {
        let mut tracker = DerivedSpkTracker::<String>::new(2);

        let added = tracker.insert_descriptor("kc".to_string(), test_descriptor(), 0);
        let first = &added[0];

        let found = tracker.index_of_spk_hash(first).unwrap();

        assert_eq!(found.0, "kc");
    }

    #[test]
    fn changing_descriptor_clears_old_scripts() {
        let mut tracker = DerivedSpkTracker::<String>::new(1);

        let desc1 = test_descriptor();
        let desc2 = Descriptor::from_str(
            "wpkh([73c5da0a/84h/1h/1h]tpubDC8msFGeGuwnKG9Upg7DM2b4DaRqg3CUZa5g8v2SRQ6K4NSkxUgd7HsL2XVWbVm39yBA4LAxysQAm397zwQSQoQgewGiYZqrA9DsP4zbQ1M/0/*)"
        ).unwrap();

        tracker.insert_descriptor("kc".to_string(), desc1, 0);
        let count1 = tracker.derived_spks.len();

        tracker.insert_descriptor("kc".to_string(), desc2, 0);
        let count2 = tracker.derived_spks.len();

        // Should be re-derived, but same count
        assert_eq!(count1, count2);
    }

    #[test]
    fn lookahead_respected() {
        let mut tracker = DerivedSpkTracker::<String>::new(5);

        tracker.insert_descriptor("kc".to_string(), test_descriptor(), 0);

        // next_index=0, lookahead=5 → derive [0..=5] → 6 scripts
        assert_eq!(tracker.derived_spks.len(), 6);
    }
}
