use anyhow::Result;
use clap::{Parser, ValueEnum};
use bdk_wallet::bitcoin::Network;
use bdk_electrum::electrum_client;

use bdk_electrum_streaming_poc::setup_wallet;
use bdk_electrum_streaming_poc::polling::auto_sync;
use bdk_electrum_streaming_poc::streaming::runtime::ElectrumDriver;

#[derive(ValueEnum, Clone, Debug)]
enum SyncMode {
    Polling,
    Streaming,
    Both,
}

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

    #[arg(long, value_enum, default_value_t = SyncMode::Polling)]
    sync_mode: SyncMode,
}

fn main() -> Result<()> {
    env_logger::init();
    let args = Args::parse();

    println!("[MAIN] Sync mode: {:?}", args.sync_mode);

    match args.sync_mode {
        SyncMode::Polling => {
            run_polling(&args)?;
        }
        SyncMode::Streaming => {
            run_streaming(&args)?;
        }
        SyncMode::Both => {
            println!("[MAIN] Running POLLING first...");
            run_polling(&args)?;

            println!("\n\n[MAIN] Running STREAMING next...");
            run_streaming(&args)?;

            println!("\n[MAIN] Both modes finished. (Comparison hook goes here later)");
        }
    }

    Ok(())
}

fn run_polling(args: &Args) -> Result<()> {
    println!("[POLLING] Setting up wallet...");

    let mut wallet = setup_wallet(
        args.descriptor.clone(),
        args.change_descriptor.clone(),
        args.network,
    )?;

    println!("[POLLING] Connecting to Electrum: {}", args.electrum_url);
    let electrum_client = electrum_client::Client::new(&args.electrum_url)?;
    let client = bdk_electrum::BdkElectrumClient::new(electrum_client);

    println!("[POLLING] Starting Auto Sync...");
    let stats = auto_sync(&mut wallet, &client, 10)?;

    let balance = wallet.balance();

    println!("[POLLING] Sync Finished");
    println!("-----------------------------------");
    println!("Total Time:       {:?}", stats.total_time);
    println!("Total Rounds:     {}", stats.rounds);
    println!("Total Balance:    {} sats", balance.total());
    println!("-----------------------------------");

    Ok(())
}

fn run_streaming(args: &Args) -> Result<()> {
    use std::str::FromStr;
    use std::time::Instant;

    use bdk_wallet::miniscript::{Descriptor, DescriptorPublicKey};
    use bdk_electrum_streaming_poc::streaming::domain::spk_tracker::DerivedSpkTracker;
    use bdk_electrum_streaming_poc::streaming::engine::StreamingEngine;

    use bdk_electrum_streaming_poc::streaming::electrum::async_client::client::AsyncElectrumClient;

    println!("[STREAMING] Setting up descriptors...");

    let external: Descriptor<DescriptorPublicKey> = Descriptor::from_str(&args.descriptor)?;
    let change: Option<Descriptor<DescriptorPublicKey>> =
        match &args.change_descriptor {
            Some(d) => Some(Descriptor::from_str(d)?),
            None => None,
        };

    println!("[STREAMING] Building script tracker...");

    let mut tracker = DerivedSpkTracker::<String>::new(20);
    tracker.insert_descriptor("external".to_string(), external, 0);

    if let Some(change_desc) = change {
        tracker.insert_descriptor("internal".to_string(), change_desc, 0);
    }

    println!("[STREAMING] Building streaming engine...");
    let engine = StreamingEngine::new(tracker);

    println!("[STREAMING] Creating async electrum client...");
    let client = AsyncElectrumClient::new(args.electrum_url.clone());

    let driver = ElectrumDriver::new(engine, client);

    println!("[STREAMING] Starting streaming loop...");

    let t0 = Instant::now();
    println!("[STREAMING] t0 = {:?}", t0);

    driver.run_forever();
}

