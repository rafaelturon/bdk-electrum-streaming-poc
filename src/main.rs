use anyhow::Result;
use bdk_electrum::electrum_client;
use bdk_electrum::BdkElectrumClient;
use bdk_wallet::bitcoin::Network;
use clap::Parser;

use bdk_electrum_streaming_poc::setup_wallet;
use bdk_electrum_streaming_poc::polling::auto_sync;

#[derive(Parser)]
#[command(author, version, about)]
struct Args {
    #[arg(long, default_value = "testnet")]
    network: Network,

    #[arg(long)]
    descriptor: String,

    #[arg(long)]
    change_descriptor: Option<String>,

    #[arg(long, default_value = "ssl://electrum.blockstream.info:60002")]
    electrum_url: String,
}

fn main() -> Result<()> {
    env_logger::init();
    let args = Args::parse();

    println!("[MAIN] Setting up wallet...");
    let mut wallet = setup_wallet(
        args.descriptor,
        args.change_descriptor,
        args.network,
    )?;

    println!("[MAIN] Connecting to Electrum: {}", args.electrum_url);
    let electrum_client = electrum_client::Client::new(&args.electrum_url)?;
    let client = BdkElectrumClient::new(electrum_client);

    println!("[MAIN] Starting Auto Sync (Cold/Warm detection)...");
    let stats = auto_sync(&mut wallet, &client, 10)?;
    let balance = wallet.balance();

    println!("[MAIN] Sync Loop Finished");
    println!("-----------------------------------");
    println!("Total Time:       {:?}", stats.total_time);
    println!("Total Rounds:     {}", stats.rounds);
    println!("Total Balance:    {} sats", balance.total());
    println!("-----------------------------------");

    Ok(())
}
