use std::path::Path;
use tokio::net::UnixListener;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{info, error};
use crate::api::{Request, Response};

pub async fn start_unix_socket(socket_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let _ = std::fs::remove_file(socket_path);

    let listener = UnixListener::bind(socket_path)?;
    info!("listening on {:?}", socket_path);

    loop {
        let (stream, _) = listener.accept().await?;
        tokio::spawn(async move {
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            let mut line = String::new();

            while reader.read_line(&mut line).await.unwrap_or(0) > 0 {
                let response = match serde_json::from_str::<Request>(&line) {
                    Ok(req) => handle_request(req).await,
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

async fn handle_request(req: Request) -> Response {
    match req {
        Request::Status => Response::success(serde_json::json!({
            "version": "0.1.0",
            "status": "running",
        })),
        _ => Response::error("not implemented yet"),
    }
}
