use anyhow::Result;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio_native_tls::TlsConnector;

use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Convert a script hash hex (big endian) into Electrum's expected little-endian hex
fn electrum_scripthash_from_hex(hex_be: &str) -> String {
    let mut bytes = hex::decode(hex_be).expect("hex decode");
    bytes.reverse();
    hex::encode(bytes)
}

#[tokio::main]
async fn main() -> Result<()> {
    // --- CHANGE THIS IF YOU WANT ---
    let host = "electrum.blockstream.info";
    let port = 60002;

    println!("Connecting to {}:{} ...", host, port);

    let tcp = TcpStream::connect((host, port)).await?;

    let connector = native_tls::TlsConnector::new()?;
    let connector = TlsConnector::from(connector);

    let tls = connector.connect(host, tcp).await?;
    println!("TLS connected");

    let (read_half, mut write_half) = tokio::io::split(tls);
    let mut reader = BufReader::new(read_half);

    // --- Handshake: server.version ---
    let version_req = json!({
        "jsonrpc": "2.0",
        "id": next_id(),
        "method": "server.version",
        "params": ["bdk-streaming-poc", "1.4"]
    });

    send(&mut write_half, &version_req).await?;

    // Read handshake response
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    println!("Handshake response: {}", line.trim());

    // --- Subscribe to ONE script hash ---

    // This is a REAL testnet scripthash (you can replace later)
    // This corresponds to some random testnet address.
    // It does not matter if it has coins, you will still get status notifications.

    let scripthash_be = "0000000000000000000000000000000000000000000000000000000000000000";
    let scripthash = electrum_scripthash_from_hex(scripthash_be);

    println!("Subscribing to scripthash: {}", scripthash);

    let sub_req = json!({
        "jsonrpc": "2.0",
        "id": next_id(),
        "method": "blockchain.scripthash.subscribe",
        "params": [scripthash]
    });

    send(&mut write_half, &sub_req).await?;

    // --- Read forever ---
    println!("Waiting for notifications...\n");

    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).await?;

        if n == 0 {
            println!("Connection closed by server");
            break;
        }

        let msg: Value = serde_json::from_str(&line)?;
        println!("<<< {}", msg);
    }

    Ok(())
}

async fn send<W: AsyncWriteExt + Unpin>(w: &mut W, v: &Value) -> Result<()> {
    let mut s = serde_json::to_string(v)?;
    s.push('\n');
    w.write_all(s.as_bytes()).await?;
    Ok(())
}