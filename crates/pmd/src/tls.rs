use std::sync::Arc;

use anyhow::{Context, Result};
use rcgen::{CertificateParams, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio_rustls::{TlsAcceptor, TlsConnector};
use tracing::info;

use crate::config::Config;

/// Generate a self-signed certificate and key, writing PEM files to disk.
pub fn generate_self_signed_cert(config: &Config) -> Result<()> {
    let key_pair = KeyPair::generate()?;

    let mut params = CertificateParams::new(vec!["pmd-node".to_string()])?;
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "pmd-node");

    let cert = params.self_signed(&key_pair)?;

    std::fs::write(&config.cert_path, cert.pem())?;
    std::fs::write(&config.key_path, key_pair.serialize_pem())?;

    // Restrict key file permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&config.key_path, std::fs::Permissions::from_mode(0o600))?;
    }

    info!(cert = %config.cert_path.display(), "generated self-signed TLS certificate");
    Ok(())
}

/// Load certificate and key from PEM files, generating them if they don't exist.
fn load_or_generate_identity(
    config: &Config,
) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
    if !config.cert_path.exists() || !config.key_path.exists() {
        generate_self_signed_cert(config)?;
    }

    let cert_pem = std::fs::read(&config.cert_path).context("failed to read TLS certificate")?;
    let key_pem = std::fs::read(&config.key_path).context("failed to read TLS private key")?;

    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut &cert_pem[..])
        .collect::<Result<Vec<_>, _>>()
        .context("failed to parse certificate PEM")?;

    let key = rustls_pemfile::pkcs8_private_keys(&mut &key_pem[..])
        .next()
        .ok_or_else(|| anyhow::anyhow!("no PKCS8 private key found in PEM"))?
        .context("failed to parse private key PEM")?;

    Ok((certs, PrivateKeyDer::Pkcs8(key)))
}

/// Build a rustls `ServerConfig` for the daemon listener.
///
/// Uses self-signed certs with no client verification (mTLS with self-signed
/// certs requires a shared CA — for V1 we verify identity via cookie HMAC).
pub fn build_server_config(config: &Config) -> Result<Arc<rustls::ServerConfig>> {
    let (certs, key) = load_or_generate_identity(config)?;

    let server_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("failed to build TLS server config")?;

    Ok(Arc::new(server_config))
}

/// Build a `TlsAcceptor` from the server config.
pub fn build_acceptor(config: &Config) -> Result<TlsAcceptor> {
    let server_config = build_server_config(config)?;
    Ok(TlsAcceptor::from(server_config))
}

/// Build a `TlsConnector` for outbound peer connections.
///
/// Accepts any server cert (self-signed) — identity is verified by the
/// cookie HMAC challenge during the application-level handshake.
pub fn build_connector(config: &Config) -> Result<TlsConnector> {
    let (certs, key) = load_or_generate_identity(config)?;

    let client_config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoCertVerifier))
        .with_client_auth_cert(certs, key)
        .context("failed to build TLS client config")?;

    Ok(TlsConnector::from(Arc::new(client_config)))
}

/// Certificate verifier that accepts any server certificate.
/// Identity verification is done at the application level via cookie HMAC.
#[derive(Debug)]
struct NoCertVerifier;

impl rustls::client::danger::ServerCertVerifier for NoCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn test_config() -> Config {
        // Install rustls crypto provider for tests
        let _ = rustls::crypto::ring::default_provider().install_default();

        let tmpdir = std::env::temp_dir().join(format!(
            "pmd-tls-test-{}-{}",
            std::process::id(),
            rand::random::<u32>()
        ));
        std::fs::create_dir_all(tmpdir.join("tls")).unwrap();
        Config {
            port: 4369,
            bind: "127.0.0.1".into(),
            home_dir: tmpdir.clone(),
            socket_path: tmpdir.join("pmd.sock"),
            pid_path: tmpdir.join("pmd.pid"),
            cert_path: tmpdir.join("tls/cert.pem"),
            key_path: tmpdir.join("tls/key.pem"),
            cookie_path: tmpdir.join("cookie"),
            ca_cert_path: None,
            sync_interval_secs: 5,
            heartbeat_interval_secs: 10,
            heartbeat_timeout_secs: 30,
            reconnect_base_secs: 1,
            reconnect_max_secs: 60,
        }
    }

    #[test]
    fn test_generate_self_signed_cert_creates_files() {
        let config = test_config();
        generate_self_signed_cert(&config).unwrap();

        assert!(config.cert_path.exists(), "cert.pem should exist");
        assert!(config.key_path.exists(), "key.pem should exist");

        let cert_pem = std::fs::read_to_string(&config.cert_path).unwrap();
        let key_pem = std::fs::read_to_string(&config.key_path).unwrap();
        assert!(cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(key_pem.contains("BEGIN PRIVATE KEY"));

        // Cleanup
        let _ = std::fs::remove_dir_all(&config.home_dir);
    }

    #[test]
    fn test_build_acceptor_succeeds() {
        let config = test_config();
        let acceptor = build_acceptor(&config);
        assert!(
            acceptor.is_ok(),
            "build_acceptor should succeed: {:?}",
            acceptor.err()
        );

        let _ = std::fs::remove_dir_all(&config.home_dir);
    }

    #[test]
    fn test_build_connector_succeeds() {
        let config = test_config();
        let connector = build_connector(&config);
        assert!(
            connector.is_ok(),
            "build_connector should succeed: {:?}",
            connector.err()
        );

        let _ = std::fs::remove_dir_all(&config.home_dir);
    }

    #[tokio::test]
    async fn test_tls_handshake_succeeds() {
        let config = test_config();
        let acceptor = build_acceptor(&config).unwrap();
        let connector = build_connector(&config).unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let _tls = acceptor.accept(stream).await.unwrap();
        });

        let client = tokio::spawn(async move {
            let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            let server_name = rustls::pki_types::ServerName::try_from("pmd-node").unwrap();
            let _tls = connector.connect(server_name, stream).await.unwrap();
        });

        let timeout = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            server.await.unwrap();
            client.await.unwrap();
        })
        .await;
        assert!(timeout.is_ok(), "TLS handshake timed out");

        let _ = std::fs::remove_dir_all(&config.home_dir);
    }

    #[test]
    fn test_key_file_permissions() {
        let config = test_config();
        generate_self_signed_cert(&config).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::metadata(&config.key_path).unwrap().permissions();
            assert_eq!(perms.mode() & 0o777, 0o600, "key file should be 0600");
        }

        let _ = std::fs::remove_dir_all(&config.home_dir);
    }
}
