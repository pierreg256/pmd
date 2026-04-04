mod cli;
mod config;
mod daemon;
mod protocol;
mod tls;

use std::collections::HashMap;

use anyhow::{Context, Result};
use clap::Parser;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::error;

use cli::{Cli, Commands};
use config::Config;
use daemon::control::send_control_request;
use protocol::{ControlRequest, ControlResponse};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Start {
            port,
            bind,
            foreground,
            config: config_path,
        } => {
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
                )
                .init();

            let (config, metadata) =
                Config::from_file_and_args(config_path.as_deref(), port, &bind)?;
            config.ensure_dirs()?;

            let node_id = uuid::Uuid::new_v4().to_string();

            if foreground {
                daemon::run(config, node_id, metadata).await?;
            } else {
                let daemonize = daemonize::Daemonize::new()
                    .pid_file(&config.pid_path)
                    .working_directory(".");

                match daemonize.start() {
                    Ok(_) => {
                        tracing_subscriber::fmt()
                            .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
                            .init();
                        daemon::run(config, node_id, metadata).await?;
                    }
                    Err(e) => {
                        error!(error = %e, "failed to daemonize");
                        anyhow::bail!("daemonize failed: {e}");
                    }
                }
            }
        }

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
                println!("{:<40} {:<22} JOINED AT", "NODE ID", "ADDRESS");
                for n in nodes {
                    println!("{:<40} {:<22} {}", n.node_id, n.addr, n.joined_at);
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
