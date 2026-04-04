use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Detect the first non-loopback IPv4 address, falling back to 127.0.0.1.
pub fn default_bind_address() -> String {
    local_ip_address::local_ip()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|_| "127.0.0.1".to_string())
}

/// PMD daemon configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub bind: String,
    pub home_dir: PathBuf,
    pub socket_path: PathBuf,
    pub pid_path: PathBuf,
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
    pub cookie_path: PathBuf,
    #[allow(dead_code)] // used by TLS shared CA mode (Phase 6)
    pub ca_cert_path: Option<PathBuf>,
    pub sync_interval_secs: u64,
    pub heartbeat_interval_secs: u64,
    #[allow(dead_code)] // used for peer timeout detection
    pub heartbeat_timeout_secs: u64,
}

/// TOML-serializable config file (`~/.pmd/config.toml`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfigFile {
    pub port: Option<u16>,
    pub bind: Option<String>,
    pub sync_interval_secs: Option<u64>,
    pub heartbeat_interval_secs: Option<u64>,
    pub heartbeat_timeout_secs: Option<u64>,
    pub ca_cert_path: Option<String>,
    /// Discovery plugins to enable (e.g. ["broadcast"]).
    #[serde(default)]
    pub discovery: Vec<String>,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
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
            socket_path: home_dir.join(format!("pmd-{port}.sock")),
            pid_path: home_dir.join(format!("pmd-{port}.pid")),
            cert_path: tls_dir.join("cert.pem"),
            key_path: tls_dir.join("key.pem"),
            cookie_path: home_dir.join("cookie"),
            ca_cert_path: None,
            sync_interval_secs: 5,
            heartbeat_interval_secs: 10,
            heartbeat_timeout_secs: 30,
            home_dir,
        })
    }

    /// Load config from TOML file, then override with CLI arguments.
    pub fn from_file_and_args(
        config_path: Option<&str>,
        cli_port: u16,
        cli_bind: Option<&str>,
        cli_discovery: &[String],
    ) -> Result<(Self, HashMap<String, String>, Vec<String>)> {
        let home_dir = dirs::home_dir()
            .context("cannot determine home directory")?
            .join(".pmd");

        let file_path = config_path
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir.join("config.toml"));

        let cf = if file_path.exists() {
            let contents = std::fs::read_to_string(&file_path)
                .with_context(|| format!("failed to read config: {}", file_path.display()))?;
            toml::from_str::<ConfigFile>(&contents)
                .with_context(|| format!("failed to parse config: {}", file_path.display()))?
        } else {
            ConfigFile::default()
        };

        let port = if cli_port != 4369 {
            cli_port
        } else {
            cf.port.unwrap_or(cli_port)
        };
        let bind = if let Some(b) = cli_bind {
            b.to_string()
        } else {
            cf.bind.unwrap_or_else(default_bind_address)
        };

        let tls_dir = home_dir.join("tls");

        let config = Self {
            port,
            bind,
            socket_path: home_dir.join(format!("pmd-{port}.sock")),
            pid_path: home_dir.join(format!("pmd-{port}.pid")),
            cert_path: tls_dir.join("cert.pem"),
            key_path: tls_dir.join("key.pem"),
            cookie_path: home_dir.join("cookie"),
            ca_cert_path: cf.ca_cert_path.map(PathBuf::from),
            sync_interval_secs: cf.sync_interval_secs.unwrap_or(5),
            heartbeat_interval_secs: cf.heartbeat_interval_secs.unwrap_or(10),
            heartbeat_timeout_secs: cf.heartbeat_timeout_secs.unwrap_or(30),
            home_dir,
        };

        // CLI discovery flags override config file
        let discovery = if cli_discovery.is_empty() {
            cf.discovery
        } else {
            cli_discovery.to_vec()
        };

        Ok((config, cf.metadata, discovery))
    }
    /// Ensure the home and TLS directories exist.
    pub fn ensure_dirs(&self) -> Result<()> {
        std::fs::create_dir_all(&self.home_dir).context("failed to create ~/.pmd")?;
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
        assert!(config.ca_cert_path.is_none());
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
        assert!(config.socket_path.ends_with("pmd-4369.sock"));
        assert!(config.pid_path.ends_with("pmd-4369.pid"));
        assert!(config.cert_path.ends_with("cert.pem"));
        assert!(config.key_path.ends_with("key.pem"));
        assert!(config.cookie_path.ends_with("cookie"));
    }

    #[test]
    fn test_config_different_ports_different_sockets() {
        let c1 = Config::new(4369, "0.0.0.0".into()).unwrap();
        let c2 = Config::new(4370, "0.0.0.0".into()).unwrap();
        assert_ne!(c1.socket_path, c2.socket_path);
        assert_ne!(c1.pid_path, c2.pid_path);
        assert_eq!(c1.cert_path, c2.cert_path);
        assert_eq!(c1.cookie_path, c2.cookie_path);
    }

    #[test]
    fn test_config_ensure_dirs() {
        let tmpdir = std::env::temp_dir().join(format!(
            "pmd-cfg-{}-{}",
            std::process::id(),
            rand::random::<u32>()
        ));
        let config = Config {
            port: 4369,
            bind: "0.0.0.0".into(),
            home_dir: tmpdir.clone(),
            socket_path: tmpdir.join("pmd-4369.sock"),
            pid_path: tmpdir.join("pmd-4369.pid"),
            cert_path: tmpdir.join("tls/cert.pem"),
            key_path: tmpdir.join("tls/key.pem"),
            cookie_path: tmpdir.join("cookie"),
            ca_cert_path: None,
            sync_interval_secs: 5,
            heartbeat_interval_secs: 10,
            heartbeat_timeout_secs: 30,
        };
        config.ensure_dirs().unwrap();
        assert!(tmpdir.exists());
        assert!(tmpdir.join("tls").exists());
        let _ = std::fs::remove_dir_all(&tmpdir);
    }

    #[test]
    fn test_config_file_parse() {
        let toml_str = r#"
port = 5000
bind = "10.0.0.1"
sync_interval_secs = 10
ca_cert_path = "/etc/pmd/ca.pem"
discovery = ["broadcast"]

[metadata]
role = "worker"
region = "us-east-1"
"#;
        let cf: ConfigFile = toml::from_str(toml_str).unwrap();
        assert_eq!(cf.port, Some(5000));
        assert_eq!(cf.bind.as_deref(), Some("10.0.0.1"));
        assert_eq!(cf.sync_interval_secs, Some(10));
        assert_eq!(cf.ca_cert_path.as_deref(), Some("/etc/pmd/ca.pem"));
        assert_eq!(cf.discovery, vec!["broadcast"]);
        assert_eq!(cf.metadata.get("role").unwrap(), "worker");
        assert_eq!(cf.metadata.get("region").unwrap(), "us-east-1");
    }

    #[test]
    fn test_config_file_defaults() {
        let cf: ConfigFile = toml::from_str("").unwrap();
        assert!(cf.port.is_none());
        assert!(cf.discovery.is_empty());
        assert!(cf.bind.is_none());
        assert!(cf.metadata.is_empty());
    }
}
