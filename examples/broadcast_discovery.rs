//! # Broadcast Discovery Example
//!
//! Demonstrates how to use the `BroadcastPlugin` to automatically discover
//! other PMD instances on the local network via UDP broadcast beacons.
//!
//! ## How to run
//!
//! Open two terminals and run the example in each with different ports:
//!
//! ```sh
//! # Terminal 1
//! cargo run --example broadcast_discovery -- --port 4369
//!
//! # Terminal 2
//! cargo run --example broadcast_discovery -- --port 4370
//! ```
//!
//! Each instance will:
//! 1. Start a UDP broadcast plugin on port 4380
//! 2. Send periodic beacons announcing its node ID and listen address
//! 3. Listen for beacons from other instances
//! 4. Print discovered peers to stdout
//!
//! Both instances should discover each other within ~10 seconds.

use std::net::SocketAddr;
use std::time::Duration;

use portmapd_broadcast::BroadcastPlugin;
use portmapd_discovery::{DiscoveryContext, DiscoveryPlugin, NodeInfo};
use tokio::sync::mpsc;
use tracing::{info, warn};

/// Parse `--port <N>` from command-line arguments (default: 4369).
fn parse_port() -> u16 {
    let args: Vec<String> = std::env::args().collect();
    for i in 0..args.len() {
        if args[i] == "--port" {
            if let Some(port_str) = args.get(i + 1) {
                return port_str.parse().expect("invalid port number");
            }
        }
    }
    4369
}

#[tokio::main]
async fn main() {
    // Initialize structured logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let port = parse_port();
    let node_id = uuid::Uuid::new_v4().to_string();
    let listen_addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();

    info!(
        node_id = %node_id,
        listen_addr = %listen_addr,
        "starting broadcast discovery example"
    );

    // Create a channel for discovered peers
    let (discovered_tx, mut discovered_rx) = mpsc::channel::<SocketAddr>(32);

    // Create the broadcast plugin with a short interval for the demo
    let plugin = BroadcastPlugin::new()
        .with_port(4380) // All instances share the same broadcast port
        .with_interval(Duration::from_secs(3));

    // Build the discovery context
    let ctx = DiscoveryContext {
        local_node: NodeInfo {
            node_id: node_id.clone(),
            addr: listen_addr,
        },
        discovered_tx,
    };

    // Spawn the discovery plugin
    let plugin_name = plugin.name().to_string();
    info!(plugin = %plugin_name, "starting discovery plugin");

    tokio::spawn(async move {
        if let Err(e) = plugin.start(ctx).await {
            warn!(error = %e, "discovery plugin error");
        }
    });

    // Listen for discovered peers and print them
    info!("waiting for peers... (press Ctrl+C to stop)");
    println!();
    println!("  Node ID: {node_id}");
    println!("  Listen:  {listen_addr}");
    println!("  Beacon:  UDP broadcast on port 4380 every 3s");
    println!();

    loop {
        tokio::select! {
            Some(peer_addr) = discovered_rx.recv() => {
                println!("  >>> Discovered peer at {peer_addr}");
                info!(peer = %peer_addr, "discovered new peer");
            }
            _ = tokio::signal::ctrl_c() => {
                println!();
                info!("shutting down");
                break;
            }
        }
    }
}
