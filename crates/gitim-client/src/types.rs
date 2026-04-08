use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize, PartialEq)]
pub struct ApiResponse {
    pub ok: bool,
    pub data: Option<Value>,
    pub error: Option<String>,
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
    use serde_json::json;

    #[test]
    fn deserialize_success_response() {
        let raw = r#"{"ok":true,"data":{"status":"running"}}"#;
        let resp: ApiResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(resp, ApiResponse {
            ok: true,
            data: Some(json!({"status": "running"})),
            error: None,
        });
    }

    #[test]
    fn deserialize_error_response() {
        let raw = r#"{"ok":false,"error":"not found"}"#;
        let resp: ApiResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(resp, ApiResponse {
            ok: false,
            data: None,
            error: Some("not found".to_string()),
        });
    }

    #[test]
    fn build_request_merges_method_with_params() {
        let req = build_request("send", json!({"channel": "general", "body": "hi"}));
        assert_eq!(req, json!({"method": "send", "channel": "general", "body": "hi"}));
    }
}
