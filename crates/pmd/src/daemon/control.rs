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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use tokio_util::sync::CancellationToken;

    use crate::daemon::DaemonState;
    use crate::daemon::membership::Membership;
    use crate::protocol::{ControlRequest, ControlResponse};

    fn make_test_state() -> Arc<Mutex<DaemonState>> {
        let mut membership = Membership::new("test-node");
        membership.add_node(
            "test-node",
            "127.0.0.1:4369".parse().unwrap(),
            HashMap::new(),
        );
        Arc::new(Mutex::new(DaemonState {
            membership,
            listen_addr: "127.0.0.1:4369".parse().unwrap(),
            peers: HashMap::new(),
            pending_joins: Vec::new(),
            pending_leaves: Vec::new(),
            shutdown: CancellationToken::new(),
        }))
    }

    fn temp_socket_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("pmd-test-{}-{}.sock", std::process::id(), rand::random::<u32>()))
    }

    #[tokio::test]
    async fn test_control_status() {
        let socket_path = temp_socket_path();
        let _ = std::fs::remove_file(&socket_path);
        let listener = UnixListener::bind(&socket_path).unwrap();
        let state = make_test_state();

        let server_state = Arc::clone(&state);
        tokio::spawn(async move {
            let _ = run_control_socket(listener, server_state).await;
        });

        // Give server a moment to start
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let resp = send_control_request(&socket_path, &ControlRequest::Status)
            .await
            .unwrap();
        match resp {
            ControlResponse::Status { node_id, peer_count, node_count, .. } => {
                assert_eq!(node_id, "test-node");
                assert_eq!(peer_count, 0);
                assert_eq!(node_count, 1);
            }
            other => panic!("expected Status, got {other:?}"),
        }

        let _ = std::fs::remove_file(&socket_path);
    }

    #[tokio::test]
    async fn test_control_nodes() {
        let socket_path = temp_socket_path();
        let _ = std::fs::remove_file(&socket_path);
        let listener = UnixListener::bind(&socket_path).unwrap();
        let state = make_test_state();

        let server_state = Arc::clone(&state);
        tokio::spawn(async move {
            let _ = run_control_socket(listener, server_state).await;
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let resp = send_control_request(&socket_path, &ControlRequest::Nodes)
            .await
            .unwrap();
        match resp {
            ControlResponse::Nodes { nodes } => {
                assert_eq!(nodes.len(), 1);
                assert_eq!(nodes[0].node_id, "test-node");
            }
            other => panic!("expected Nodes, got {other:?}"),
        }

        let _ = std::fs::remove_file(&socket_path);
    }

    #[tokio::test]
    async fn test_control_join_queues_address() {
        let socket_path = temp_socket_path();
        let _ = std::fs::remove_file(&socket_path);
        let listener = UnixListener::bind(&socket_path).unwrap();
        let state = make_test_state();

        let server_state = Arc::clone(&state);
        tokio::spawn(async move {
            let _ = run_control_socket(listener, server_state).await;
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let resp = send_control_request(
            &socket_path,
            &ControlRequest::Join { addr: "10.0.0.1:4369".into() },
        )
        .await
        .unwrap();
        assert!(matches!(resp, ControlResponse::Ok));

        let st = state.lock().await;
        assert_eq!(st.pending_joins, vec!["10.0.0.1:4369".to_string()]);

        let _ = std::fs::remove_file(&socket_path);
    }

    #[tokio::test]
    async fn test_control_leave_queues_address() {
        let socket_path = temp_socket_path();
        let _ = std::fs::remove_file(&socket_path);
        let listener = UnixListener::bind(&socket_path).unwrap();
        let state = make_test_state();

        let server_state = Arc::clone(&state);
        tokio::spawn(async move {
            let _ = run_control_socket(listener, server_state).await;
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let resp = send_control_request(
            &socket_path,
            &ControlRequest::Leave { addr: "10.0.0.1:4369".into() },
        )
        .await
        .unwrap();
        assert!(matches!(resp, ControlResponse::Ok));

        let st = state.lock().await;
        assert_eq!(st.pending_leaves, vec!["10.0.0.1:4369".to_string()]);

        let _ = std::fs::remove_file(&socket_path);
    }

    #[tokio::test]
    async fn test_control_shutdown_cancels_token() {
        let socket_path = temp_socket_path();
        let _ = std::fs::remove_file(&socket_path);
        let listener = UnixListener::bind(&socket_path).unwrap();
        let state = make_test_state();

        let shutdown = state.lock().await.shutdown.clone();
        assert!(!shutdown.is_cancelled());

        let server_state = Arc::clone(&state);
        tokio::spawn(async move {
            let _ = run_control_socket(listener, server_state).await;
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let resp = send_control_request(&socket_path, &ControlRequest::Shutdown)
            .await
            .unwrap();
        assert!(matches!(resp, ControlResponse::Ok));
        assert!(shutdown.is_cancelled());

        let _ = std::fs::remove_file(&socket_path);
    }
}
