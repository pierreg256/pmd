use std::sync::Arc;

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;
use tracing::{debug, error, info};

use crate::daemon::DaemonState;
use crate::protocol::{ControlRequest, ControlResponse, NodeInfoResponse};

/// Run the control socket server. Listens on the Unix domain socket
/// for CLI commands and dispatches them to the daemon state.
pub async fn run_control_socket(
    listener: UnixListener,
    state: Arc<Mutex<DaemonState>>,
) -> Result<()> {
    info!("control socket listening");

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let state = Arc::clone(&state);
                tokio::spawn(async move {
                    if let Err(e) = handle_control_client(stream, state).await {
                        debug!(error = %e, "control client error");
                    }
                });
            }
            Err(e) => {
                error!(error = %e, "control socket accept error");
            }
        }
    }
}

async fn handle_control_client(
    stream: UnixStream,
    state: Arc<Mutex<DaemonState>>,
) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    reader.read_line(&mut line).await?;

    let request: ControlRequest = serde_json::from_str(line.trim())?;
    debug!(?request, "control request");

    let response = dispatch_request(request, state).await;

    let response_json = serde_json::to_string(&response)?;
    writer.write_all(response_json.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;

    Ok(())
}

async fn dispatch_request(
    request: ControlRequest,
    state: Arc<Mutex<DaemonState>>,
) -> ControlResponse {
    match request {
        ControlRequest::Status => {
            let st = state.lock().await;
            ControlResponse::Status {
                node_id: st.membership.node_id().to_string(),
                listen_addr: st.listen_addr.to_string(),
                peer_count: st.peers.len(),
                node_count: st.membership.members().len(),
            }
        }
        ControlRequest::Nodes => {
            let st = state.lock().await;
            let nodes = st
                .membership
                .members()
                .into_iter()
                .map(|n| NodeInfoResponse {
                    node_id: n.node_id,
                    addr: n.addr.to_string(),
                    joined_at: n.joined_at,
                })
                .collect();
            ControlResponse::Nodes { nodes }
        }
        ControlRequest::Join { addr } => {
            let mut st = state.lock().await;
            st.pending_joins.push(addr);
            ControlResponse::Ok
        }
        ControlRequest::Leave { addr } => {
            let mut st = state.lock().await;
            st.pending_leaves.push(addr);
            ControlResponse::Ok
        }
        ControlRequest::Shutdown => {
            let st = state.lock().await;
            st.shutdown.cancel();
            ControlResponse::Ok
        }
    }
}

/// Send a control request to the daemon and receive the response.
pub async fn send_control_request(
    socket_path: &std::path::Path,
    request: &ControlRequest,
) -> Result<ControlResponse> {
    let stream = UnixStream::connect(socket_path).await?;
    let (reader, mut writer) = stream.into_split();

    let request_json = serde_json::to_string(request)?;
    writer.write_all(request_json.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;

    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    reader.read_line(&mut line).await?;

    let response: ControlResponse = serde_json::from_str(line.trim())?;
    Ok(response)
}
