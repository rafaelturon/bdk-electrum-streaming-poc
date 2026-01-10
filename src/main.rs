use anyhow::Result;
use bdk_electrum::electrum_client;
use bdk_electrum::BdkElectrumClient;
use bdk_wallet::bitcoin::Network;
use clap::Parser;
use std::time::Instant;

use bdk_electrum_streaming_poc::setup_wallet;

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

    println!("Setting up wallet...");
    let mut wallet = setup_wallet(
        args.descriptor,
        args.change_descriptor,
        args.network,
    )?;

    println!("Connecting to Electrum: {}", args.electrum_url);
    let electrum_client = electrum_client::Client::new(&args.electrum_url)?;
    let client = BdkElectrumClient::new(electrum_client);

    println!("Starting Progressive Sync Loop...");
    let start_time = Instant::now();
    let mut round = 0usize;
    loop {
        round += 1;
        println!("Sync round #{round} ...");
        let round_start = Instant::now();
        // Build a new sync request every round
        let request = wallet.start_full_scan().build();
        // Small batch scan = fast return
        let update = client.full_scan(request, 5, 5, false)?;
        wallet.apply_update(update)?;
        let round_time = round_start.elapsed();
        println!("Round #{round} done in {:?}", round_time);

        // Stop condition: after a few rounds, or when no more activity
        // For now: just run 10 rounds max
        if round >= 10 {
            break;
        }

        // Small pause so we don't hammer the server
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    let total_time = start_time.elapsed();
    let balance = wallet.balance();

    println!("\nSync Loop Finished");
    println!("-----------------------------------");
    println!("Total Time:      {:?}", total_time);
    println!("Total Rounds:     {}", round);
    println!("Total Balance:    {} sats", balance.total());
    println!("-----------------------------------");


    Ok(())
}
