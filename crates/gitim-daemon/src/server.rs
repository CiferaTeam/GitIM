use std::path::Path;
use tokio::net::UnixListener;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{info, error};
use crate::api::{Request, Response};
use crate::state::SharedState;

pub async fn start_unix_socket(
    socket_path: &Path,
    state: SharedState,
) -> Result<(), Box<dyn std::error::Error>> {
    let _ = std::fs::remove_file(socket_path);

    let listener = UnixListener::bind(socket_path)?;
    info!("listening on {:?}", socket_path);

    loop {
        let (stream, _) = listener.accept().await?;
        let state = state.clone();
        tokio::spawn(async move {
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            let mut line = String::new();

            while reader.read_line(&mut line).await.unwrap_or(0) > 0 {
                let response = match serde_json::from_str::<Request>(&line) {
                    Ok(req) => crate::handlers::handle_request(req, state.clone()).await,
                    Err(e) => Response::error(format!("invalid request: {}", e)),
                };

                let mut resp_json = serde_json::to_string(&response).unwrap();
                resp_json.push('\n');
                if let Err(e) = writer.write_all(resp_json.as_bytes()).await {
                    error!("write error: {}", e);
                    break;
                }
                line.clear();
            }
        });
    }
}
