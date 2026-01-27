#[cfg(test)]
mod tests {
    // FIX 1: Import the functions from the 'client' module
    // Adjust the path 'super::client' if your file structure is different.
    // If 'client.rs' is inside 'async_client' folder, this is likely correct:
    use crate::streaming::electrum::asynchronous::adapter::{electrum_scripthash, next_id};
    
    // FIX 2: Correctly import Bitcoin hash types
    use bitcoin::hashes::{sha256, Hash};
    use hex::FromHex;

    #[test]
    fn test_electrum_scripthash_conversion() {
        // Test Vector:
        // P2WPKH script for a known address or arbitrary bytes.
        // Let's use arbitrary bytes to verify the algo: Sha256 -> Reverse -> Hex
        
        let script_hex = "001479b7e77b4e941e12760630737402660126581831";
        let script_bytes = Vec::from_hex(script_hex).unwrap();
        
        let result = electrum_scripthash(&script_bytes);
        
        // Manual verification
        let hash = sha256::Hash::hash(&script_bytes);
        let mut bytes = hash.to_byte_array();
        bytes.reverse();
        let expected = hex::encode(bytes);

        assert_eq!(result, expected);
    }

    #[test]
    fn test_next_id_increments() {
        let id1 = next_id();
        let id2 = next_id();
        assert_eq!(id2, id1 + 1);
    }
}