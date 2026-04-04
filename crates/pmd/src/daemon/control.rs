use std::sync::Arc;

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;
use tracing::{debug, error, info};

use crate::daemon::DaemonState;
use crate::protocol::{ControlRequest, ControlResponse, NodeInfoResponse, ServiceEntry};

/// Run the control socket server.
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

async fn handle_control_client(stream: UnixStream, state: Arc<Mutex<DaemonState>>) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    reader.read_line(&mut line).await?;

    let request: ControlRequest = serde_json::from_str(line.trim())?;
    debug!(?request, "control request");

    match request {
        ControlRequest::Subscribe => {
            // Long-lived: stream events until client disconnects
            let mut rx = {
                let st = state.lock().await;
                st.event_tx.subscribe()
            };
            // Send initial OK to confirm subscription
            let ok = serde_json::to_string(&ControlResponse::Ok)?;
            writer.write_all(ok.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await?;

            while let Ok(change) = rx.recv().await {
                let event_resp = ControlResponse::Event {
                    event: change.event,
                    node_id: change.node_id,
                    addr: change.addr,
                };
                let json = serde_json::to_string(&event_resp)?;
                if writer.write_all(json.as_bytes()).await.is_err() {
                    break;
                }
                if writer.write_all(b"\n").await.is_err() {
                    break;
                }
                let _ = writer.flush().await;
            }
        }
        other => {
            let response = dispatch_request(other, state).await;
            let response_json = serde_json::to_string(&response)?;
            writer.write_all(response_json.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await?;
        }
    }

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
                .map(|n| {
                    let node_id_clone = n.node_id.clone();
                    let addr_ip = n.addr.ip().to_string();
                    NodeInfoResponse {
                        node_id: n.node_id,
                        addr: n.addr.to_string(),
                        joined_at: n.joined_at,
                        metadata: n.metadata,
                        services: n
                            .services
                            .into_iter()
                            .map(|s| ServiceEntry {
                                name: s.name,
                                node_id: node_id_clone.clone(),
                                host: addr_ip.clone(),
                                port: s.port,
                                metadata: s.metadata,
                            })
                            .collect(),
                    }
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
        ControlRequest::Register {
            name,
            port,
            metadata,
        } => {
            let mut st = state.lock().await;
            st.membership.register_service(&name, port, metadata);
            ControlResponse::Ok
        }
        ControlRequest::Unregister { name } => {
            let mut st = state.lock().await;
            st.membership.unregister_service(&name);
            ControlResponse::Ok
        }
        ControlRequest::Lookup { name } => {
            let st = state.lock().await;
            let entries = st
                .membership
                .lookup_service(&name)
                .into_iter()
                .map(|(node_id, addr, port, metadata)| ServiceEntry {
                    name: name.clone(),
                    node_id,
                    host: addr.ip().to_string(),
                    port,
                    metadata,
                })
                .collect();
            ControlResponse::Services { entries }
        }
        ControlRequest::Subscribe => {
            // Handled in handle_control_client directly
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
    use crate::protocol::ControlRequest;

    fn make_test_state() -> Arc<Mutex<DaemonState>> {
        let mut membership = Membership::new("test-node");
        membership.add_node(
            "test-node",
            "127.0.0.1:4369".parse().unwrap(),
            HashMap::new(),
        );
        let (event_tx, _) = tokio::sync::broadcast::channel(16);
        Arc::new(Mutex::new(DaemonState {
            membership,
            listen_addr: "127.0.0.1:4369".parse().unwrap(),
            peers: HashMap::new(),
            pending_joins: Vec::new(),
            pending_leaves: Vec::new(),
            shutdown: CancellationToken::new(),
            event_tx,
        }))
    }

    fn temp_socket_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "pmd-test-{}-{}.sock",
            std::process::id(),
            rand::random::<u32>()
        ))
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
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let resp = send_control_request(&socket_path, &ControlRequest::Status)
            .await
            .unwrap();
        match resp {
            ControlResponse::Status {
                node_id,
                peer_count,
                node_count,
                ..
            } => {
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
            &ControlRequest::Join {
                addr: "10.0.0.1:4369".into(),
            },
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
            &ControlRequest::Leave {
                addr: "10.0.0.1:4369".into(),
            },
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

    #[tokio::test]
    async fn test_control_register_and_lookup() {
        let socket_path = temp_socket_path();
        let _ = std::fs::remove_file(&socket_path);
        let listener = UnixListener::bind(&socket_path).unwrap();
        let state = make_test_state();
        let server_state = Arc::clone(&state);
        tokio::spawn(async move {
            let _ = run_control_socket(listener, server_state).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Register
        let resp = send_control_request(
            &socket_path,
            &ControlRequest::Register {
                name: "web".into(),
                port: 8080,
                metadata: HashMap::new(),
            },
        )
        .await
        .unwrap();
        assert!(matches!(resp, ControlResponse::Ok));

        // Lookup
        let resp =
            send_control_request(&socket_path, &ControlRequest::Lookup { name: "web".into() })
                .await
                .unwrap();
        match resp {
            ControlResponse::Services { entries } => {
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].name, "web");
                assert_eq!(entries[0].port, 8080);
            }
            other => panic!("expected Services, got {other:?}"),
        }

        // Unregister
        let resp = send_control_request(
            &socket_path,
            &ControlRequest::Unregister { name: "web".into() },
        )
        .await
        .unwrap();
        assert!(matches!(resp, ControlResponse::Ok));

        // Lookup again — empty
        let resp =
            send_control_request(&socket_path, &ControlRequest::Lookup { name: "web".into() })
                .await
                .unwrap();
        match resp {
            ControlResponse::Services { entries } => assert!(entries.is_empty()),
            other => panic!("expected Services, got {other:?}"),
        }

        let _ = std::fs::remove_file(&socket_path);
    }
}
