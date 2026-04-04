mod cli;
mod config;
mod daemon;
mod protocol;
mod tls;

use anyhow::{Context, Result};
use clap::Parser;
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
            config: _config_path,
        } => {
            // Initialize logging
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
                )
                .init();

            let config = Config::new(port, bind)?;
            config.ensure_dirs()?;

            let node_id = uuid::Uuid::new_v4().to_string();

            if foreground {
                daemon::run(config, node_id).await?;
            } else {
                // Daemonize
                let daemonize = daemonize::Daemonize::new()
                    .pid_file(&config.pid_path)
                    .working_directory(".");

                match daemonize.start() {
                    Ok(_) => {
                        // Re-init logging after fork
                        tracing_subscriber::fmt()
                            .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
                            .init();
                        daemon::run(config, node_id).await?;
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
            let resp =
                send_control_request(&config.socket_path, &ControlRequest::Join { addr })
                    .await
                    .context("failed to connect to daemon — is it running?")?;
            print_response(&resp);
        }

        Commands::Leave { addr, port } => {
            let config = Config::new(port, "0.0.0.0".into())?;
            let resp =
                send_control_request(&config.socket_path, &ControlRequest::Leave { addr })
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
                }
            }
        }
    }
}
