//! Typed response payloads for daemon IPC methods.
//!
//! One struct per `Request` variant's success `data`. Daemon handlers
//! construct these and `serde_json::to_value` them into the response
//! envelope; clients reach them via `ApiResponse::parse_data::<T>()`.
//! Field renames anywhere here surface as compile errors at every
//! call site instead of silent `unwrap_or("unknown")` fallbacks.

use serde::{Deserialize, Serialize};

/// Response payload for `Request::Status`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StatusResponse {
    /// Daemon binary version (cargo `CARGO_PKG_VERSION` or hand-set).
    pub version: String,
    /// Top-level state string. Currently always `"running"` once the
    /// handler is reachable; reserved for future degraded states.
    pub status: String,
    /// Whether the daemon is in guest mode (read-only, no committed
    /// identity in `me.json`).
    pub guest: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    /// Locks the wire shape — these are the field names other tools
    /// (CLI `gitim status` JSON output, future WebUI `/runtime/status`)
    /// rely on. Renames need to be intentional and update consumers.
    #[test]
    fn status_response_wire_shape() {
        let r = StatusResponse {
            version: "0.1.0".to_string(),
            status: "running".to_string(),
            guest: false,
        };
        let v = serde_json::to_value(&r).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 3);
        assert_eq!(obj.get("version").and_then(|v| v.as_str()), Some("0.1.0"));
        assert_eq!(obj.get("status").and_then(|v| v.as_str()), Some("running"));
        assert_eq!(obj.get("guest").and_then(|v| v.as_bool()), Some(false));
    }

    #[test]
    fn status_response_round_trip() {
        let r = StatusResponse {
            version: "9.9.9".to_string(),
            status: "running".to_string(),
            guest: true,
        };
        let s = serde_json::to_string(&r).unwrap();
        let back: StatusResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(r, back);
    }
}
