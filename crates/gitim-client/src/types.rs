use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;

use crate::error::ClientError;

#[derive(Debug, Deserialize, PartialEq)]
pub struct ApiResponse {
    pub ok: bool,
    pub data: Option<Value>,
    pub error: Option<String>,
}

impl ApiResponse {
    /// Decode `data` into a typed payload `T`. Use this once the daemon
    /// handler returns a typed response shape — the call site stops touching
    /// raw JSON fields and gets compile-time checks against the schema.
    ///
    /// Errors when `data` is absent (callers should branch on `ok`/`error`
    /// first if they need a clearer message) or when the JSON shape doesn't
    /// match `T`.
    pub fn parse_data<T: DeserializeOwned>(&self) -> Result<T, ClientError> {
        let data = self.data.as_ref().ok_or_else(|| {
            ClientError::ProtocolError("response is missing `data`".to_string())
        })?;
        serde_json::from_value(data.clone())
            .map_err(|e| ClientError::ProtocolError(format!("data shape mismatch: {e}")))
    }
}

/// Build a JSON request object with a "method" field merged with params.
pub fn build_request(method: &str, params: Value) -> Value {
    let mut obj = match params {
        Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    obj.insert("method".to_string(), Value::String(method.to_string()));
    Value::Object(obj)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde::Deserialize;
    use serde_json::json;

    #[derive(Debug, Deserialize, PartialEq)]
    struct StatusShape {
        status: String,
        version: String,
    }

    #[test]
    fn parse_data_extracts_typed_payload() {
        let resp = ApiResponse {
            ok: true,
            data: Some(json!({"status": "running", "version": "0.1.0"})),
            error: None,
        };
        let parsed = resp.parse_data::<StatusShape>().unwrap();
        assert_eq!(
            parsed,
            StatusShape {
                status: "running".to_string(),
                version: "0.1.0".to_string(),
            }
        );
    }

    #[test]
    fn parse_data_errors_when_data_absent() {
        let resp = ApiResponse {
            ok: false,
            data: None,
            error: Some("boom".to_string()),
        };
        let result = resp.parse_data::<StatusShape>();
        assert!(result.is_err(), "missing data should error");
    }

    /// End-to-end protocol contract: a daemon constructs the typed
    /// payload, wraps it in the response envelope, sends it on the wire,
    /// the client receives bytes, deserializes the envelope, and pulls
    /// the typed payload out via parse_data. Renaming any field of
    /// StatusResponse breaks this test deterministically.
    #[test]
    fn round_trip_status_response_via_envelope() {
        use gitim_core::responses::StatusResponse;

        let payload = StatusResponse {
            version: "1.2.3".to_string(),
            status: "running".to_string(),
            guest: true,
        };
        let envelope = json!({
            "ok": true,
            "data": serde_json::to_value(&payload).unwrap(),
        });
        let wire = envelope.to_string();
        let resp: ApiResponse = serde_json::from_str(&wire).unwrap();
        let decoded: StatusResponse = resp.parse_data().unwrap();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn parse_data_errors_when_shape_mismatches() {
        let resp = ApiResponse {
            ok: true,
            data: Some(json!({"unexpected": 42})),
            error: None,
        };
        let result = resp.parse_data::<StatusShape>();
        assert!(result.is_err(), "wrong shape should error");
    }

    #[test]
    fn deserialize_success_response() {
        let raw = r#"{"ok":true,"data":{"status":"running"}}"#;
        let resp: ApiResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(
            resp,
            ApiResponse {
                ok: true,
                data: Some(json!({"status": "running"})),
                error: None,
            }
        );
    }

    #[test]
    fn deserialize_error_response() {
        let raw = r#"{"ok":false,"error":"not found"}"#;
        let resp: ApiResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(
            resp,
            ApiResponse {
                ok: false,
                data: None,
                error: Some("not found".to_string()),
            }
        );
    }

    #[test]
    fn build_request_merges_method_with_params() {
        let req = build_request("send", json!({"channel": "general", "body": "hi"}));
        assert_eq!(
            req,
            json!({"method": "send", "channel": "general", "body": "hi"})
        );
    }
}
