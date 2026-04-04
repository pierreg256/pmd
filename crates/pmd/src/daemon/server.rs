use std::sync::Arc;

use anyhow::Result;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio_rustls::TlsAcceptor;
use tracing::{debug, info};

use crate::config::Config;
use crate::daemon::DaemonState;
use crate::daemon::peer;

/// Run the TLS TCP server accept loop.
pub async fn run_server(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    config: Arc<Config>,
    cookie: Arc<Vec<u8>>,
    state: Arc<Mutex<DaemonState>>,
) -> Result<()> {
    let local_addr = listener.local_addr()?;
    info!(addr = %local_addr, "TLS server listening");

    let shutdown = state.lock().await.shutdown.clone();

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                info!("server shutting down");
                return Ok(());
            }
            result = listener.accept() => {
                let (tcp_stream, peer_addr) = result?;
                debug!(peer = %peer_addr, "new TCP connection");

                let acceptor = acceptor.clone();
                let config = Arc::clone(&config);
                let cookie = Arc::clone(&cookie);
                let state = Arc::clone(&state);

                tokio::spawn(async move {
                    match acceptor.accept(tcp_stream).await {
                        Ok(tls_stream) => {
                            peer::handle_inbound_peer(tls_stream, config, cookie, state).await;
                        }
                        Err(e) => {
                            debug!(peer = %peer_addr, error = %e, "TLS handshake failed");
                        }
                    }
                });
            }
        }
    }
}
