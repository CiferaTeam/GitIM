//! CliError → process exit code mapping.
//!
//! Per Architecture §4 (docs/plans/runtime-cli/00-requirements.md), agents
//! distinguish exit code classes:
//! - 0: success (only emitted by handlers, not this module)
//! - 1: CLI / network failure — agent may retry network class, must fix
//!   config class (we collapse both into 1 because the agent's response is
//!   "look at stderr" either way)
//! - 2: permanent business error (structured `error_code` from runtime, or
//!   4xx with no `error_code` — agent should NOT retry, the request is
//!   semantically rejected)
//! - 3: transient runtime failure (HTTP 5xx — agent MAY retry with backoff)
//!
//! The variant fan-in here is the source of truth for the contract; if you
//! add a CliError variant, you must extend this match (it's not `_` to force
//! the decision).

use super::http::CliError;

pub const SUCCESS: i32 = 0;
pub const CLI_OR_NETWORK: i32 = 1;
pub const PERMANENT: i32 = 2;
pub const TRANSIENT: i32 = 3;

/// Map a CliError variant to its documented exit code.
///
/// HTTP status precedence inside `HttpStatus`:
/// - `>= 500` → transient (3)
/// - `400..500` → permanent (2)
/// - anything else (shouldn't happen since success returns Ok, but guard
///   against future runtime quirks) → CLI/network (1)
pub fn from_cli_error(err: &CliError) -> i32 {
    match err {
        CliError::Transport(_) | CliError::Parse(_) | CliError::InvalidConfig(_) => CLI_OR_NETWORK,
        CliError::ResponseErrorCode { .. } => PERMANENT,
        CliError::HttpStatus(status, _) => {
            if *status >= 500 {
                TRANSIENT
            } else if (400..500).contains(status) {
                PERMANENT
            } else {
                // Anomalous status (3xx, etc.) — unreachable in practice but
                // we don't want to silently classify as success.
                CLI_OR_NETWORK
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_is_cli_or_network() {
        let err = CliError::Transport("connection refused".to_string());
        assert_eq!(from_cli_error(&err), 1);
    }

    #[test]
    fn parse_is_cli_or_network() {
        let err = CliError::Parse("malformed json".to_string());
        assert_eq!(from_cli_error(&err), 1);
    }

    #[test]
    fn invalid_config_is_cli_or_network() {
        let err = CliError::InvalidConfig("no workspace".to_string());
        assert_eq!(from_cli_error(&err), 1);
    }

    #[test]
    fn response_error_code_is_permanent() {
        let err = CliError::ResponseErrorCode {
            code: "handler_conflict".to_string(),
            message: "already exists".to_string(),
            http_status: 200,
            preflight_detail: None,
        };
        assert_eq!(from_cli_error(&err), 2);
    }

    #[test]
    fn response_error_code_permanent_even_when_underlying_status_is_500() {
        // Sanity: if the runtime ever returns 5xx + error_code (shouldn't,
        // but...) we still treat error_code as the canonical signal and
        // classify as permanent. Don't retry on a structured rejection.
        let err = CliError::ResponseErrorCode {
            code: "internal_bug".to_string(),
            message: "".to_string(),
            http_status: 500,
            preflight_detail: None,
        };
        assert_eq!(from_cli_error(&err), 2);
    }

    #[test]
    fn http_4xx_is_permanent() {
        for status in [400u16, 404, 422, 499] {
            let err = CliError::HttpStatus(status, "bad".to_string());
            assert_eq!(from_cli_error(&err), 2, "status {status}");
        }
    }

    #[test]
    fn http_5xx_is_transient() {
        for status in [500u16, 502, 503, 504, 599] {
            let err = CliError::HttpStatus(status, "oops".to_string());
            assert_eq!(from_cli_error(&err), 3, "status {status}");
        }
    }

    #[test]
    fn anomalous_3xx_is_cli_or_network() {
        // Belt-and-suspenders: a misbehaving server returning 3xx that
        // somehow reached us shouldn't be classified as success or any of
        // the documented exit classes — surface it as a CLI/network error.
        let err = CliError::HttpStatus(302, "redirected".to_string());
        assert_eq!(from_cli_error(&err), 1);
    }
}
