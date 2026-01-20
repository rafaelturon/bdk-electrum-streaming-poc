use anyhow::Result;
use clap::{Parser, ValueEnum};
use bdk_wallet::bitcoin::Network;
use bdk_electrum::electrum_client;

use bdk_electrum_streaming_poc::setup_wallet;
use bdk_electrum_streaming_poc::polling::auto_sync;
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use std::time::{Instant, Duration};

#[derive(Clone)]
struct StreamingStatsHandle {
    t0: Instant,
    done: Arc<AtomicBool>,
    finished_at: Arc<std::sync::Mutex<Option<Duration>>>,
}

impl StreamingStatsHandle {
    fn new() -> Self {
        Self {
            t0: Instant::now(),
            done: Arc::new(AtomicBool::new(false)),
            finished_at: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    fn mark_done(&self) -> bool {
        if !self.done.swap(true, Ordering::SeqCst) {
            let mut g = self.finished_at.lock().unwrap();
            *g = Some(self.t0.elapsed());
            true
        } else {
            false
        }
    }

    fn is_done(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }

    fn elapsed(&self) -> Option<Duration> {
        *self.finished_at.lock().unwrap()
    }
}

#[derive(ValueEnum, Clone, Debug)]
enum SyncMode {
    Polling,
    Streaming,
    Both,
}

#[derive(Debug)]
struct SyncResult {
    mode: &'static str,
    total_time: Duration,
    rounds: Option<u64>,   // polling has rounds, streaming does not (yet)
    balance: Option<u64>,  // polling has balance, streaming may not yet
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
            let _ = run_polling(&args)?;
        }
        SyncMode::Streaming => {
            let _ = run_streaming(&args)?;
        }
        SyncMode::Both => {
            println!("[MAIN] Running POLLING first...");
            let polling = run_polling(&args)?;

            println!("\n\n[MAIN] Running STREAMING next...");
            let streaming = run_streaming(&args)?;

            print_comparison(&polling, &streaming);
        }
    }

    Ok(())
}

fn run_polling(args: &Args) -> Result<SyncResult> {
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

    Ok(SyncResult {
        mode: "Polling",
        total_time: stats.total_time,
        rounds: Some(stats.rounds as u64),
        balance: Some(balance.total().to_sat()),
    })
}

fn run_streaming(args: &Args) -> Result<SyncResult> {
    use std::str::FromStr;

    use bdk_wallet::miniscript::{Descriptor, DescriptorPublicKey};
    use bdk_electrum_streaming_poc::streaming::domain::spk_tracker::DerivedSpkTracker;
    use bdk_electrum_streaming_poc::streaming::engine::StreamingEngine;
    use bdk_electrum_streaming_poc::streaming::electrum::async_client::client::AsyncElectrumClient;
    use bdk_electrum_streaming_poc::streaming::runtime::ElectrumDriver;

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

    // ---- STATS ----
    let stats = StreamingStatsHandle::new();
    
    let wallet = setup_wallet(
        args.descriptor.clone(),
        args.change_descriptor.clone(),
        args.network,
    )?;
    let wallet = Arc::new(Mutex::new(wallet));
       
    let driver = ElectrumDriver::new(engine, client, wallet.clone())
        .with_initial_sync_notifier({
            let stats = stats.clone();
            move || {
                stats.mark_done();
                log::info!(
                    "[STREAMING] Initial sync completed in {:?}",
                    stats.elapsed().unwrap()
                );
            }
        });

    std::thread::spawn(move || {
        driver.run_forever();
    });

    while !stats.is_done() {
        std::thread::sleep(Duration::from_millis(50));
    }

    let dt = stats.elapsed().unwrap();

    let balance = {
        let w = wallet.lock().unwrap();
        w.balance().total().to_sat()
    };

    println!("[STREAMING] Initial Sync Finished");
    println!("-----------------------------------");
    println!("Total Time:       {:?}", dt);
    println!("Total Balance:    {} sats", balance);
    println!("-----------------------------------");

    Ok(SyncResult {
        mode: "Streaming",
        total_time: dt,
        rounds: None,
        balance: Some(balance),
    })
}

fn print_comparison(a: &SyncResult, b: &SyncResult) {
    println!();
    println!("==================================================");
    println!("                 SYNC COMPARISON                  ");
    println!("==================================================");
    println!("{:<15} | {:<15} | {:<15}", "Metric", a.mode, b.mode);
    println!("--------------------------------------------------");

    println!(
        "{:<15} | {:<15?} | {:<15?}",
        "Total Time",
        a.total_time,
        b.total_time
    );

    println!(
        "{:<15} | {:<15} | {:<15}",
        "Rounds",
        a.rounds.map(|v| v.to_string()).unwrap_or("-".into()),
        b.rounds.map(|v| v.to_string()).unwrap_or("-".into()),
    );

    println!(
        "{:<15} | {:<15} | {:<15}",
        "Balance",
        a.balance.map(|v| format!("{} sats", v)).unwrap_or("-".into()),
        b.balance.map(|v| format!("{} sats", v)).unwrap_or("-".into()),
    );

    let speedup = a.total_time.as_secs_f64() / b.total_time.as_secs_f64();

    println!("--------------------------------------------------");
    println!("Speedup: {:.2}x", speedup);
    println!("==================================================");
}
