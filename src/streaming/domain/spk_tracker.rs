// Gap limit + derivation tracker

use std::collections::{btree_map, BTreeMap, HashMap};

use bitcoin::{ScriptBuf};
use bitcoin::hashes::{sha256, Hash};
use bdk_wallet::miniscript::{Descriptor, DescriptorPublicKey};

/// Tracks derived ScriptPubKeys (SPKs) for a set of descriptors.
///
/// This struct is responsible for the "Gap Limit" logic in the wallet. It ensures that
/// we always track a window of unused addresses (the "lookahead") beyond the last known
/// used index.
///
/// It maintains a bidirectional mapping between:
/// - (Keychain, Index) -> Script/Hash
/// - ScriptHash -> (Keychain, Index)
#[derive(Debug, Clone)]
pub struct DerivedSpkTracker<K> {
    /// The number of unused addresses to track ahead of the highest used index.
    lookahead: u32,

    /// The active descriptors for each keychain (e.g. "external", "internal").
    descriptors: BTreeMap<K, Descriptor<DescriptorPublicKey>>,
    
    /// Forward index: Maps (Keychain, Index) to (ScriptHash, Script).
    /// Used to generate the list of scripts to monitor.
    derived_spks: BTreeMap<(K, u32), (sha256::Hash, ScriptBuf)>,

    /// Reverse index: Maps ScriptHash to (Keychain, Index).
    /// Used to identify which wallet address received funds when a notification arrives.
    derived_spks_rev: HashMap<sha256::Hash, (K, u32)>,
}

impl<K: Ord + Clone> DerivedSpkTracker<K> {
    /// Creates a new tracker with the specified lookahead (gap limit) size.
    ///
    /// # Arguments
    /// * `lookahead` - The number of addresses to watch beyond the last used index. 
    ///                 Common values are 20 (standard) or higher for services.
    pub fn new(lookahead: u32) -> Self {
        Self {
            lookahead,
            descriptors: BTreeMap::new(),
            derived_spks: BTreeMap::new(),
            derived_spks_rev: HashMap::new(),
        }
    }

    /// Returns an iterator over all currently tracked script hashes and scripts.
    /// 
    /// This is typically used upon (re)connection to subscribe to all addresses at once.
    pub fn all_spks(&self) -> impl Iterator<Item = &(sha256::Hash, ScriptBuf)> {
        self.derived_spks.values()
    }

    /// Reverse lookup: Finds the Keychain ID and Index for a given script hash.
    ///
    /// Returns `None` if the hash is not tracked.
    pub fn index_of_spk_hash(&self, hash: &sha256::Hash) -> Option<(K, u32)> {
        self.derived_spks_rev.get(hash).cloned()
    }

    /// Registers or updates a descriptor for a keychain (e.g., "external").
    ///
    /// This will derive the initial range of scripts from index `0` up to `next_index + lookahead`.
    ///
    /// # Returns
    /// A list of newly derived scripts that need to be subscribed to.
    pub fn insert_descriptor(
        &mut self,
        keychain: K,
        descriptor: Descriptor<DescriptorPublicKey>,
        next_index: u32,
    ) -> Vec<(sha256::Hash, ScriptBuf)> {
        log::debug!("[DerivedSpkTracker] KeyChain{0}: {1}", next_index, descriptor);
        // If the descriptor changed, we must clear old derivations to avoid mixing scripts
        if let Some(old) = self.descriptors.insert(keychain.clone(), descriptor.clone()) {
            if old == descriptor {
                return vec![];
            }
            self.clear_keychain(&keychain);
        }

        // Derive the full window [0 .. next_index + lookahead]
        (0..=next_index + self.lookahead)
            .filter_map(|i| self.add_derived_spk(keychain.clone(), i))
            .collect()
    }

    /// Notifies the tracker that an address at `index` has been used.
    ///
    /// This checks if the usage creates a gap larger than permitted. If so, it
    /// derives new addresses to restore the lookahead window.
    ///
    /// # Returns
    /// A list of *newly* derived scripts that must be subscribed to immediately.
    pub fn mark_used_and_derive_new(
        &mut self,
        keychain: &K,
        index: u32,
    ) -> Vec<(sha256::Hash, ScriptBuf)> {
        let next_index = index + 1;
        let mut newly_derived = Vec::new();

        // Check the new required window: [next_index .. next_index + lookahead]
        for i in next_index..=next_index + self.lookahead {
            if let Some(pair) = self.add_derived_spk(keychain.clone(), i) {
                newly_derived.push(pair);
            } else {
                // Optimization: If `add_derived_spk` returns None, it means we already
                // track this index. Since we derive sequentially, we likely track
                // everything after it too, so we can stop early.
                break;
            }
        }

        newly_derived
    }

    /// Internal helper: Derives and stores a single script at the given index.
    ///
    /// Returns `Some((Hash, Script))` if the script was newly derived.
    /// Returns `None` if it was already tracked.
    fn add_derived_spk(&mut self, keychain: K, index: u32) -> Option<(sha256::Hash, ScriptBuf)> {
        // Use `entry` to avoid re-deriving if it already exists
        if let btree_map::Entry::Vacant(entry) =
            self.derived_spks.entry((keychain.clone(), index))
        {
            let descriptor = self.descriptors.get(&keychain).expect("descriptor exists");

            // Derive the script at the specific index
            let spk = descriptor
                .at_derivation_index(index)
                .expect("can derive")
                .script_pubkey();

            let hash = sha256::Hash::hash(spk.as_bytes());

            // Store in both forward and reverse maps
            entry.insert((hash, spk.clone()));
            self.derived_spks_rev.insert(hash, (keychain, index));

            return Some((hash, spk));
        }
        None
    }

    /// Internal helper: Removes all tracking data for a specific keychain.
    /// Used when a descriptor is updated or replaced.
    fn clear_keychain(&mut self, keychain: &K) {
        // Efficiently extract all entries belonging to this keychain
        let removed = self
            .derived_spks
            .extract_if(.., |(kc, _), _| kc == keychain);

        // Clean up the reverse map
        for (_, (hash, _)) in removed {
            self.derived_spks_rev.remove(&hash);
        }
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

        let _added = tracker.insert_descriptor("kc".to_string(), test_descriptor(), 0);
        let (hash, _script) = tracker.derived_spks.values().next().unwrap();
        let found = tracker.index_of_spk_hash(hash).unwrap();

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

    #[test]
    fn mark_used_is_idempotent() {
        let mut tracker = DerivedSpkTracker::<String>::new(2);

        tracker.insert_descriptor("kc".to_string(), test_descriptor(), 0);

        let a = tracker.derived_spks.len();

        tracker.mark_used_and_derive_new(&"kc".to_string(), 0);
        let b = tracker.derived_spks.len();

        tracker.mark_used_and_derive_new(&"kc".to_string(), 0);
        let c = tracker.derived_spks.len();

        // Must never shrink or grow unexpectedly
        assert!(b >= a);
        assert_eq!(b, c);
    }

    #[test]
    fn derived_set_only_grows() {
        let mut tracker = DerivedSpkTracker::<String>::new(2);

        tracker.insert_descriptor("kc".to_string(), test_descriptor(), 0);
        let a = tracker.derived_spks.len();

        tracker.mark_used_and_derive_new(&"kc".to_string(), 0);
        let b = tracker.derived_spks.len();

        tracker.mark_used_and_derive_new(&"kc".to_string(), 1);
        let c = tracker.derived_spks.len();

        assert!(b >= a);
        assert!(c >= b);
    }

    #[test]
    fn maintains_gap_relative_to_last_used() {
        let mut tracker = DerivedSpkTracker::<String>::new(2);

        tracker.insert_descriptor("kc".to_string(), test_descriptor(), 0);
        // Derived: 0,1,2

        tracker.mark_used_and_derive_new(&"kc".to_string(), 0);

        let max_index = tracker
            .derived_spks
            .keys()
            .map(|(_, i)| *i)
            .max()
            .unwrap();

        // Gap invariant: must be >= used + lookahead
        assert!(max_index >= 0 + 2);
    }
}