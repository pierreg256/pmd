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
    /// Heartbeat timeout in seconds (used in Phase 5: reconnection).
    #[allow(dead_code)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default_values() {
        let config = Config::new(4369, "0.0.0.0".into()).unwrap();
        assert_eq!(config.port, 4369);
        assert_eq!(config.bind, "0.0.0.0");
        assert_eq!(config.sync_interval_secs, 5);
        assert_eq!(config.heartbeat_interval_secs, 10);
        assert_eq!(config.heartbeat_timeout_secs, 30);
    }

    #[test]
    fn test_config_custom_port() {
        let config = Config::new(5555, "127.0.0.1".into()).unwrap();
        assert_eq!(config.port, 5555);
        assert_eq!(config.bind, "127.0.0.1");
    }

    #[test]
    fn test_config_paths_under_home() {
        let config = Config::new(4369, "0.0.0.0".into()).unwrap();
        assert!(config.socket_path.ends_with("pmd.sock"));
        assert!(config.pid_path.ends_with("pmd.pid"));
        assert!(config.cert_path.ends_with("cert.pem"));
        assert!(config.key_path.ends_with("key.pem"));
        assert!(config.cookie_path.ends_with("cookie"));
    }

    #[test]
    fn test_config_ensure_dirs() {
        let tmpdir = std::env::temp_dir().join(format!("pmd-test-{}", std::process::id()));
        let config = Config {
            port: 4369,
            bind: "0.0.0.0".into(),
            home_dir: tmpdir.clone(),
            socket_path: tmpdir.join("pmd.sock"),
            pid_path: tmpdir.join("pmd.pid"),
            cert_path: tmpdir.join("tls/cert.pem"),
            key_path: tmpdir.join("tls/key.pem"),
            cookie_path: tmpdir.join("cookie"),
            sync_interval_secs: 5,
            heartbeat_interval_secs: 10,
            heartbeat_timeout_secs: 30,
        };
        config.ensure_dirs().unwrap();
        assert!(tmpdir.exists());
        assert!(tmpdir.join("tls").exists());

        // Cleanup
        let _ = std::fs::remove_dir_all(&tmpdir);
    }
}
