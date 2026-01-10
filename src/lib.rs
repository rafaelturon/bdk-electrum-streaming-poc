use anyhow::Result;
use bdk_wallet::{
    bitcoin::Network,
    ChangeSet,
    PersistedWallet,
    Wallet,
};
use bdk_wallet::file_store::Store;

pub const DB_PATH: &str = "wallet_db.dat";
pub const DB_MAGIC: &[u8] = b"bdk_wallet_magic_bytes";

pub fn setup_wallet(
    descriptor: String,
    change_descriptor: Option<String>,
    network: Network,
) -> Result<PersistedWallet<Store<ChangeSet>>> {
    // Open or create the file store
    let (mut db, _) = Store::<ChangeSet>::load_or_create(DB_MAGIC, DB_PATH)?;

    // Try to load existing wallet
    let wallet_opt = Wallet::load()
        .descriptor(bdk_wallet::KeychainKind::External, Some(descriptor.clone()))
        .descriptor(bdk_wallet::KeychainKind::Internal, change_descriptor.clone())
        .check_network(network)
        .load_wallet(&mut db)?;

    let wallet = match wallet_opt {
        Some(wallet) => {
            println!("Wallet loaded from persistence.");
            wallet
        }
        None => {
            println!("Creating new wallet...");
            let change_desc = change_descriptor
                .clone()
                .expect("Change descriptor required for new wallet");

            Wallet::create(descriptor, change_desc)
                .network(network)
                .create_wallet(&mut db)?
        }
    };

    Ok(wallet)
}
