mod cli;
mod config;
mod daemon;
mod protocol;
mod tls;

use std::collections::HashMap;

use anyhow::{Context, Result};
use clap::Parser;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use cli::{Cli, Commands};
use config::Config;
use daemon::control::send_control_request;
use protocol::{ControlRequest, ControlResponse};

/// Entry point — synchronous so that `daemonize` can fork *before* the
/// tokio runtime is created.  Forking after `#[tokio::main]` leaves the
/// child with a broken runtime (stale epoll fd, dead threads), which is
/// why the daemon silently failed to bind its control socket.
fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Start {
            port,
            bind,
            foreground,
            config: config_path,
            discovery,
        } => {
            let (config, metadata, discovery_plugins) = Config::from_file_and_args(
                config_path.as_deref(),
                port,
                bind.as_deref(),
                &discovery,
            )?;
            config.ensure_dirs()?;

            // Check if a daemon is already running on this port by probing the
            // control socket.  If we can connect, it's alive — exit cleanly.
            if std::os::unix::net::UnixStream::connect(&config.socket_path).is_ok() {
                println!("PMD daemon is already running on port {port}.");
                return Ok(());
            }

            let node_id = uuid::Uuid::new_v4().to_string();

            if !foreground {
                let daemonize = daemonize::Daemonize::new()
                    .pid_file(&config.pid_path)
                    .working_directory(".");

                daemonize
                    .start()
                    .map_err(|e| anyhow::anyhow!("daemonize failed: {e}"))?;
            }

            // Build the tokio runtime *after* daemonize so the child gets a
            // clean runtime with its own threads and epoll fd.
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async {
                tracing_subscriber::fmt()
                    .with_env_filter(
                        tracing_subscriber::EnvFilter::try_from_default_env()
                            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
                    )
                    .init();

                daemon::run(config, node_id, metadata, discovery_plugins).await
            })?;
        }

        // All CLI commands use a lightweight runtime.
        other => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(run_cli_command(other))?;
        }
    }

    Ok(())
}

async fn run_cli_command(command: Commands) -> Result<()> {
    match command {
        Commands::Start { .. } => unreachable!("handled in main"),

        Commands::Stop { port } => {
            let config = Config::new(port, "0.0.0.0".into())?;
            let resp = send_control_request(&config.socket_path, &ControlRequest::Shutdown)
                .await
                .context("failed to connect to daemon — is it running?")?;
            print_response(&resp);
        }

        Commands::Status { port } => {
            let config = Config::new(port, "0.0.0.0".into())?;
            let resp = send_control_request(&config.socket_path, &ControlRequest::Status)
                .await
                .context("failed to connect to daemon — is it running?")?;
            print_response(&resp);
        }

        Commands::Join { addr, port } => {
            let config = Config::new(port, "0.0.0.0".into())?;
            let resp = send_control_request(&config.socket_path, &ControlRequest::Join { addr })
                .await
                .context("failed to connect to daemon — is it running?")?;
            print_response(&resp);
        }

        Commands::Leave { addr, port } => {
            let config = Config::new(port, "0.0.0.0".into())?;
            let resp = send_control_request(&config.socket_path, &ControlRequest::Leave { addr })
                .await
                .context("failed to connect to daemon — is it running?")?;
            print_response(&resp);
        }

        Commands::Nodes { port } => {
            let config = Config::new(port, "0.0.0.0".into())?;
            let resp = send_control_request(&config.socket_path, &ControlRequest::Nodes)
                .await
                .context("failed to connect to daemon — is it running?")?;
            print_response(&resp);
        }

        Commands::Register {
            name,
            service_port,
            port,
        } => {
            let config = Config::new(port, "0.0.0.0".into())?;
            let resp = send_control_request(
                &config.socket_path,
                &ControlRequest::Register {
                    name,
                    port: service_port,
                    metadata: HashMap::new(),
                },
            )
            .await
            .context("failed to connect to daemon — is it running?")?;
            print_response(&resp);
        }

        Commands::Unregister { name, port } => {
            let config = Config::new(port, "0.0.0.0".into())?;
            let resp =
                send_control_request(&config.socket_path, &ControlRequest::Unregister { name })
                    .await
                    .context("failed to connect to daemon — is it running?")?;
            print_response(&resp);
        }

        Commands::Lookup { name, port } => {
            let config = Config::new(port, "0.0.0.0".into())?;
            let resp = send_control_request(&config.socket_path, &ControlRequest::Lookup { name })
                .await
                .context("failed to connect to daemon — is it running?")?;
            print_response(&resp);
        }

        Commands::Subscribe { port } => {
            let config = Config::new(port, "0.0.0.0".into())?;
            let stream = tokio::net::UnixStream::connect(&config.socket_path)
                .await
                .context("failed to connect to daemon — is it running?")?;
            let (reader, mut writer) = stream.into_split();

            let req = serde_json::to_string(&ControlRequest::Subscribe)?;
            writer.write_all(req.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await?;

            let mut reader = BufReader::new(reader);
            let mut line = String::new();

            // Read initial OK
            reader.read_line(&mut line).await?;
            line.clear();

            // Stream events
            loop {
                line.clear();
                let n = reader.read_line(&mut line).await?;
                if n == 0 {
                    break;
                }
                if let Ok(ControlResponse::Event {
                    event,
                    node_id,
                    addr,
                }) = serde_json::from_str::<ControlResponse>(line.trim())
                {
                    let ev = match event {
                        protocol::MembershipEvent::NodeJoined => "JOINED",
                        protocol::MembershipEvent::NodeLeft => "LEFT",
                    };
                    println!("{ev} {node_id} {addr}");
                }
            }
        }
    }

    Ok(())
}

fn print_response(resp: &ControlResponse) {
    match resp {
        ControlResponse::Ok => println!("OK"),
        ControlResponse::Error { message } => {
            eprintln!("Error: {message}");
            std::process::exit(1);
        }
        ControlResponse::Status {
            node_id,
            listen_addr,
            peer_count,
            node_count,
        } => {
            println!("Node ID:     {node_id}");
            println!("Listen:      {listen_addr}");
            println!("Peers:       {peer_count}");
            println!("Cluster:     {node_count} node(s)");
        }
        ControlResponse::Nodes { nodes } => {
            if nodes.is_empty() {
                println!("No nodes in cluster.");
            } else {
                println!(
                    "{:<40} {:<22} {:<12} {:<22} PHI",
                    "NODE ID", "ADDRESS", "JOINED AT", "LAST SEEN AT"
                );
                for n in nodes {
                    let last_seen = if n.is_local {
                        "(local)".to_string()
                    } else {
                        match n.last_seen_at {
                            Some(ts) => format_epoch(ts),
                            None => "(indirect)".to_string(),
                        }
                    };
                    let phi_str = match n.phi {
                        Some(phi) => format!("{phi:.2}"),
                        None => "-".to_string(),
                    };
                    println!(
                        "{:<40} {:<22} {:<12} {:<22} {}",
                        n.node_id, n.addr, n.joined_at, last_seen, phi_str
                    );
                    for s in &n.services {
                        println!("  └─ {} → {}:{}", s.name, s.host, s.port);
                    }
                }
            }
        }
        ControlResponse::Services { entries } => {
            if entries.is_empty() {
                println!("No services found.");
            } else {
                println!("{:<20} {:<40} ENDPOINT", "SERVICE", "NODE");
                for e in entries {
                    println!("{:<20} {:<40} {}:{}", e.name, e.node_id, e.host, e.port);
                }
            }
        }
        ControlResponse::Event { .. } => {
            // Handled in Subscribe branch
        }
    }
}

/// Format a Unix epoch timestamp as a human-readable UTC string.
fn format_epoch(epoch: u64) -> String {
    let secs = epoch;
    let days_since_epoch = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Civil date from days since 1970-01-01 (algorithm from Howard Hinnant)
    let z = days_since_epoch as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!("{y:04}-{m:02}-{d:02} {hours:02}:{minutes:02}:{seconds:02}Z")
}
