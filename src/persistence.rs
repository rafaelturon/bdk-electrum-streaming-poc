use anyhow::Result;
use bdk_wallet::{bitcoin::Network, ChangeSet, KeychainKind, PersistedWallet, Wallet};
use bdk_wallet::file_store::Store;

pub const DB_PATH: &str = "wallet_db.dat";
pub const DB_MAGIC: &[u8] = b"bdk_wallet_magic_bytes";

/// Must match the lookahead used by the streaming DerivedSpkTracker.
const LOOKAHEAD: u32 = 50;

pub fn setup_wallet(
    descriptor: String,
    change_descriptor: Option<String>,
    network: Network,
) -> Result<PersistedWallet<Store<ChangeSet>>> {
    // Open or create the file store
    let (mut db, _) = Store::<ChangeSet>::load_or_create(DB_MAGIC, DB_PATH)?;

    // Try to load existing wallet
    let wallet_opt = Wallet::load()
        .descriptor(KeychainKind::External, Some(descriptor.clone()))
        .descriptor(KeychainKind::Internal, change_descriptor.clone())
        .check_network(network)
        .load_wallet(&mut db)?;

    let mut wallet = match wallet_opt {
        Some(wallet) => {
            log::info!("[WALLET] Loaded from persistence.");
            wallet
        }
        None => {
            log::info!("[WALLET] Creating new...");
            let change_desc = change_descriptor
                .clone()
                .expect("[WALLET] Change descriptor required for new wallet");

            Wallet::create(descriptor, change_desc)
                .network(network)
                .lookahead(LOOKAHEAD)
                .create_wallet(&mut db)?
        }
    };

    // FIX: Ensure both keychains have enough addresses revealed.
    // This is critical when loading a wallet that was previously created with a
    // smaller lookahead — the persisted state won't cover higher-index addresses
    // where change outputs may have landed.
    let _ = wallet.reveal_addresses_to(KeychainKind::External, LOOKAHEAD);
    let _ = wallet.reveal_addresses_to(KeychainKind::Internal, LOOKAHEAD);
    log::info!(
        "[WALLET] Revealed addresses to index {} for both keychains",
        LOOKAHEAD
    );

    Ok(wallet)
}