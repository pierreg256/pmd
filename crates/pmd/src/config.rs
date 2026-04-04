use std::path::PathBuf;

use anyhow::{Context, Result};

/// PMD daemon configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// TCP listen port for inter-PMD connections.
    pub port: u16,
    /// Bind address.
    pub bind: String,
    /// Path to the PMD home directory (~/.pmd).
    pub home_dir: PathBuf,
    /// Path to the Unix control socket.
    pub socket_path: PathBuf,
    /// Path to the PID file.
    pub pid_path: PathBuf,
    /// Path to the TLS certificate.
    pub cert_path: PathBuf,
    /// Path to the TLS private key.
    pub key_path: PathBuf,
    /// Path to the shared cookie file.
    pub cookie_path: PathBuf,
    /// Anti-entropy sync interval in seconds.
    pub sync_interval_secs: u64,
    /// Heartbeat interval in seconds.
    pub heartbeat_interval_secs: u64,
    /// Heartbeat timeout in seconds.
    pub heartbeat_timeout_secs: u64,
}

impl Config {
    pub fn new(port: u16, bind: String) -> Result<Self> {
        let home_dir = dirs::home_dir()
            .context("cannot determine home directory")?
            .join(".pmd");

        let tls_dir = home_dir.join("tls");

        Ok(Self {
            port,
            bind,
            socket_path: home_dir.join("pmd.sock"),
            pid_path: home_dir.join("pmd.pid"),
            cert_path: tls_dir.join("cert.pem"),
            key_path: tls_dir.join("key.pem"),
            cookie_path: home_dir.join("cookie"),
            sync_interval_secs: 5,
            heartbeat_interval_secs: 10,
            heartbeat_timeout_secs: 30,
            home_dir,
        })
    }

    /// Ensure the home and TLS directories exist.
    pub fn ensure_dirs(&self) -> Result<()> {
        std::fs::create_dir_all(&self.home_dir)
            .context("failed to create ~/.pmd")?;
        std::fs::create_dir_all(self.home_dir.join("tls"))
            .context("failed to create ~/.pmd/tls")?;
        Ok(())
    }
}
