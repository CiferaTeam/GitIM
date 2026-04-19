//! Test helper binary that masquerades as a `gitim-runtime`.
//!
//! Supports two flags the self-update path cares about:
//!
//! - `--version`: print `gitim-runtime <version>` and exit 0. Version comes
//!   from the `FAKE_VERSION` env var; default `0.0.0` so a missing env var is
//!   visible in test failures.
//! - `--port <N>`: bind `127.0.0.1:N` and serve `GET /health` with a JSON
//!   body containing `"version"`. Any other request path gets `404`.
//!
//! Intentionally minimal — no async runtime, no HTTP crate. A hand-rolled
//! request/response loop on `std::net::TcpListener` keeps the dependency
//! graph tight (this bin is compiled by the runtime crate's test harness)
//! and avoids link-time conflicts with the crate's own `tokio`/`axum`.
//!
//! Not a production asset. Never referenced outside `tests/update_e2e.rs`.

use std::io::{Read, Write};
use std::net::TcpListener;

fn fake_version() -> String {
    std::env::var("FAKE_VERSION").unwrap_or_else(|_| "0.0.0".to_string())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--version") {
        println!("gitim-runtime {}", fake_version());
        return;
    }

    let port: u16 = args
        .windows(2)
        .find(|w| w[0] == "--port")
        .and_then(|w| w[1].parse().ok())
        .expect("fake-gitim-runtime: --port <N> required when not --version");

    let addr = format!("127.0.0.1:{port}");
    let listener = TcpListener::bind(&addr)
        .unwrap_or_else(|e| panic!("fake-gitim-runtime: bind {addr}: {e}"));

    let version = fake_version();
    let body = format!("{{\"service\":\"gitim-runtime\",\"version\":\"{version}\"}}");

    // Single-threaded serve loop. Test driver polls one request at a time.
    for stream in listener.incoming() {
        let mut stream = match stream {
            Ok(s) => s,
            Err(_) => continue,
        };

        // Read just enough of the request line to dispatch on path. HTTP/1.1
        // parsing is intentionally naive — we don't need to be correct on
        // pipelined requests or bodies in a test helper.
        let mut buf = [0u8; 1024];
        let n = match stream.read(&mut buf) {
            Ok(n) => n,
            Err(_) => continue,
        };
        let text = String::from_utf8_lossy(&buf[..n]);
        let path = text
            .split_whitespace()
            .nth(1)
            .unwrap_or("/")
            .to_string();

        let (status_line, response_body) = if path == "/health" {
            ("HTTP/1.1 200 OK", body.as_str())
        } else {
            ("HTTP/1.1 404 Not Found", "not found")
        };

        let response = format!(
            "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n{response_body}",
            len = response_body.len(),
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
    }
}
