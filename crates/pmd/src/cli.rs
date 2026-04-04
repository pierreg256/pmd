use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "pmd", about = "Port Mapper Daemon — distributed node membership")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Start the PMD daemon
    Start {
        /// TCP listen port for inter-PMD connections
        #[arg(short, long, default_value_t = 4369)]
        port: u16,

        /// Bind address
        #[arg(short, long, default_value = "0.0.0.0")]
        bind: String,

        /// Run in foreground (don't daemonize)
        #[arg(short, long)]
        foreground: bool,

        /// Path to config file
        #[arg(short, long)]
        config: Option<String>,
    },

    /// Stop the running PMD daemon
    Stop,

    /// Show daemon status and connected nodes
    Status,

    /// Manually connect to a peer
    Join {
        /// Peer address (host:port)
        addr: String,
    },

    /// Disconnect from a peer
    Leave {
        /// Peer address (host:port)
        addr: String,
    },

    /// List all known nodes in the cluster
    Nodes,
}
