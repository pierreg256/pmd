use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tracing::{debug, error, info};

use crate::daemon::DaemonState;

/// Run a minimal HTTP server that serves Prometheus metrics on `/metrics`.
pub async fn run_metrics_server(listener: TcpListener, state: Arc<Mutex<DaemonState>>) {
    let addr = listener.local_addr().ok();
    info!(addr = ?addr, "metrics server listening");

    loop {
        let (mut stream, _) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                error!(error = %e, "metrics accept error");
                continue;
            }
        };

        let state = Arc::clone(&state);
        tokio::spawn(async move {
            let mut buf = vec![0u8; 1024];
            // Read just enough to know it's an HTTP request
            let _ = stream.read(&mut buf).await;

            let body = render_metrics(&state).await;
            let response = format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Type: text/plain; version=0.0.4; charset=utf-8\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\
                 \r\n\
                 {}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes()).await;
            debug!("served metrics request");
        });
    }
}

/// Render Prometheus text-format metrics from the daemon state.
async fn render_metrics(state: &Arc<Mutex<DaemonState>>) -> String {
    let st = state.lock().await;
    let node_id = st.membership.node_id().to_string();
    let members = st.membership.members();
    let cluster_size = members.len();
    let peer_count = st.peers.len();
    let listen_addr = st.listen_addr.to_string();

    // Compute average/max phi across connected peers
    let mut phi_values: Vec<f64> = Vec::new();
    for peer in st.peers.values() {
        let phi = peer.phi_detector.phi();
        if phi.is_finite() {
            phi_values.push(phi);
        }
    }
    let phi_max = phi_values.iter().cloned().fold(0.0_f64, f64::max);
    let phi_avg = if phi_values.is_empty() {
        0.0
    } else {
        phi_values.iter().sum::<f64>() / phi_values.len() as f64
    };

    // Count services
    let service_count: usize = members.iter().map(|n| n.services.len()).sum();

    drop(st);

    let mut out = String::with_capacity(1024);

    out.push_str("# HELP pmd_cluster_nodes_total Total number of nodes known in the cluster.\n");
    out.push_str("# TYPE pmd_cluster_nodes_total gauge\n");
    out.push_str(&format!("pmd_cluster_nodes_total {cluster_size}\n"));

    out.push_str("\n# HELP pmd_peers_connected Number of directly connected peers.\n");
    out.push_str("# TYPE pmd_peers_connected gauge\n");
    out.push_str(&format!("pmd_peers_connected {peer_count}\n"));

    out.push_str("\n# HELP pmd_phi_max Maximum phi value across all connected peers.\n");
    out.push_str("# TYPE pmd_phi_max gauge\n");
    out.push_str(&format!("pmd_phi_max {phi_max:.4}\n"));

    out.push_str("\n# HELP pmd_phi_avg Average phi value across all connected peers.\n");
    out.push_str("# TYPE pmd_phi_avg gauge\n");
    out.push_str(&format!("pmd_phi_avg {phi_avg:.4}\n"));

    out.push_str(
        "\n# HELP pmd_services_total Total number of services registered across the cluster.\n",
    );
    out.push_str("# TYPE pmd_services_total gauge\n");
    out.push_str(&format!("pmd_services_total {service_count}\n"));

    out.push_str("\n# HELP pmd_info PMD daemon information.\n");
    out.push_str("# TYPE pmd_info gauge\n");
    out.push_str(&format!(
        "pmd_info{{node_id=\"{node_id}\",listen_addr=\"{listen_addr}\",version=\"{}\"}} 1\n",
        env!("CARGO_PKG_VERSION")
    ));

    out
}
