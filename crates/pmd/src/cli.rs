use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "pmd",
    about = "Port Mapper Daemon — distributed node membership",
    version = env!("CARGO_PKG_VERSION"),
)]
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

        /// Bind address (default: auto-detect first non-loopback IPv4)
        #[arg(short, long)]
        bind: Option<String>,

        /// Run in foreground (don't daemonize)
        #[arg(short, long)]
        foreground: bool,

        /// Path to config file
        #[arg(short, long)]
        config: Option<String>,

        /// Discovery plugins to enable (e.g. "broadcast")
        #[arg(short, long)]
        discovery: Vec<String>,
    },

    /// Stop the running PMD daemon
    Stop {
        #[arg(short, long, default_value_t = 4369)]
        port: u16,
    },

    /// Show daemon status and connected nodes
    Status {
        #[arg(short, long, default_value_t = 4369)]
        port: u16,
    },

    /// Manually connect to a peer
    Join {
        /// Peer address (host:port)
        addr: String,
        #[arg(short, long, default_value_t = 4369)]
        port: u16,
    },

    /// Disconnect from a peer
    Leave {
        /// Peer address (host:port)
        addr: String,
        #[arg(short, long, default_value_t = 4369)]
        port: u16,
    },

    /// List all known nodes in the cluster
    Nodes {
        #[arg(short, long, default_value_t = 4369)]
        port: u16,
    },

    /// Register a named service on this node
    Register {
        /// Service name
        name: String,
        /// Service port
        #[arg(short = 'P', long)]
        service_port: u16,
        /// Port of the local daemon instance
        #[arg(short, long, default_value_t = 4369)]
        port: u16,
    },

    /// Unregister a named service
    Unregister {
        /// Service name
        name: String,
        #[arg(short, long, default_value_t = 4369)]
        port: u16,
    },

    /// Look up a service by name across the cluster
    Lookup {
        /// Service name
        name: String,
        #[arg(short, long, default_value_t = 4369)]
        port: u16,
    },

    /// Subscribe to membership events (streams events until interrupted)
    Subscribe {
        #[arg(short, long, default_value_t = 4369)]
        port: u16,
    },
}
