use crate::api::{Request, Response};
use crate::state::SharedState;
use std::path::Path;
use std::sync::atomic::Ordering;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tracing::{debug, error, info};

pub async fn start_unix_socket(
    socket_path: &Path,
    state: SharedState,
) -> Result<(), Box<dyn std::error::Error>> {
    let _ = std::fs::remove_file(socket_path);

    let listener = UnixListener::bind(socket_path)?;
    info!("listening on {:?}", socket_path);

    loop {
        let (stream, _) = listener.accept().await?;
        state.last_client_activity.store(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_else(|e| { tracing::error!("system time before epoch: {e}"); Default::default() })
                .as_secs(),
            Ordering::Relaxed,
        );
        let state = state.clone();
        tokio::spawn(async move {
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            let mut line = String::new();

            while reader.read_line(&mut line).await.unwrap_or(0) > 0 {
                let parsed = serde_json::from_str::<Request>(&line);
                let is_subscribe = matches!(&parsed, Ok(Request::Subscribe));

                let response = match parsed {
                    Ok(req) => crate::handlers::handle_request(req, state.clone()).await,
                    Err(e) => Response::error(format!("invalid request: {}", e)),
                };

                let mut resp_json = serde_json::to_string(&response).unwrap_or_else(|e| { tracing::error!("serializing response: {e}"); String::new() });
                resp_json.push('\n');
                if let Err(e) = writer.write_all(resp_json.as_bytes()).await {
                    error!("write error: {}", e);
                    return;
                }
                line.clear();

                if is_subscribe {
                    debug!("client entered subscribe mode");
                    handle_subscribed(&mut reader, &mut writer, &state).await;
                    return;
                }
            }
        });
    }
}

async fn handle_subscribed(
    reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    state: &SharedState,
) {
    let mut rx = state.event_tx.subscribe();
    let mut line = String::new();

    loop {
        tokio::select! {
            // Client sends a request
            result = reader.read_line(&mut line) => {
                match result {
                    Ok(0) | Err(_) => return, // Client disconnected
                    Ok(_) => {
                        let response = match serde_json::from_str::<Request>(&line) {
                            Ok(req) => crate::handlers::handle_request(req, state.clone()).await,
                            Err(e) => Response::error(format!("invalid request: {}", e)),
                        };
                        let mut resp_json = serde_json::to_string(&response).unwrap_or_else(|e| { tracing::error!("serializing response: {e}"); String::new() });
                        resp_json.push('\n');
                        if let Err(e) = writer.write_all(resp_json.as_bytes()).await {
                            error!("write error: {}", e);
                            return;
                        }
                        line.clear();
                    }
                }
            }
            // Broadcast event received
            result = rx.recv() => {
                match result {
                    Ok(event) => {
                        let mut event_json = serde_json::to_string(&event).unwrap_or_else(|e| { tracing::error!("serializing event: {e}"); String::new() });
                        event_json.push('\n');
                        if let Err(e) = writer.write_all(event_json.as_bytes()).await {
                            error!("push write error: {}", e);
                            return;
                        }
                        if let Err(e) = writer.flush().await {
                            error!("push flush error: {}", e);
                            return;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        debug!("subscriber lagged by {} events", n);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        return;
                    }
                }
            }
        }
    }
}
