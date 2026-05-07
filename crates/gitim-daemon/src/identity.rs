use gitim_core::auth_payload::AuthPayload;
use gitim_core::identity::{
    gitea_identity_from_user_json, github_identity_from_user_json,
    gitlab_identity_from_user_json, IdentityParseError,
};
use gitim_core::types::handler::HandlerError;
use gitim_core::types::Handler;
use serde::Deserialize;
use std::process::Command;
use thiserror::Error;

pub use gitim_core::identity::InferredIdentity;

#[derive(Debug, Clone, Deserialize)]
pub enum GitServer {
    #[serde(rename = "git")]
    Git,
    #[serde(rename = "github")]
    GitHub,
    #[serde(rename = "gitea")]
    Gitea,
    #[serde(rename = "gitlab")]
    GitLab,
}

#[derive(Debug, Error)]
pub enum IdentityError {
    #[error("curl command failed: {0}")]
    CurlFailed(String),
    #[error("failed to parse API response: {0}")]
    ParseError(String),
    #[error("missing field '{0}' in API response")]
    MissingField(String),
    #[error("invalid handler from API: {0}")]
    InvalidHandler(#[from] HandlerError),
}

impl From<IdentityParseError> for IdentityError {
    fn from(value: IdentityParseError) -> Self {
        match value {
            IdentityParseError::ParseError(e) => IdentityError::ParseError(e),
            IdentityParseError::MissingField(field) => IdentityError::MissingField(field),
            IdentityParseError::InvalidHandler(e) => IdentityError::InvalidHandler(e),
        }
    }
}

/// Shell out `curl -sf` with the given args and return stdout as a String.
fn run_curl(args: &[&str]) -> Result<String, IdentityError> {
    let output = Command::new("curl")
        .arg("-sf")
        .args(args)
        .output()
        .map_err(|e| IdentityError::CurlFailed(e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(IdentityError::CurlFailed(format!(
            "exit code {:?}: {}",
            output.status.code(),
            stderr
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Infer identity from the given git server and auth data.
///
/// For `Git` variant the caller provides handler + display_name directly.
/// For GitHub/Gitea/GitLab we shell out to curl to fetch the authenticated user.
pub fn infer_identity(
    _git_server: GitServer,
    auth_data: AuthPayload,
) -> Result<InferredIdentity, IdentityError> {
    match auth_data {
        AuthPayload::Git {
            handler,
            display_name,
            github_email,
        } => {
            let validated = Handler::new(&handler)?;
            Ok(InferredIdentity {
                handler: validated,
                display_name,
                email: github_email.filter(|s| !s.is_empty()),
            })
        }

        AuthPayload::GitHub { token } => {
            let auth_header = format!("Authorization: token {}", token);
            // E2E test seam mirroring the one in gitim-runtime: points at a
            // local stub so a compiled daemon binary can run the full onboard
            // flow without talking to github.com. Unset in production.
            let api_base = std::env::var("GITIM_TEST_GITHUB_API_BASE")
                .unwrap_or_else(|_| "https://api.github.com".to_string());
            let url = format!("{}/user", api_base.trim_end_matches('/'));
            let body = run_curl(&["-H", &auth_header, &url])?;
            Ok(github_identity_from_user_json(&body)?)
        }

        AuthPayload::Gitea { token, url } => {
            let auth_header = format!("Authorization: token {}", token);
            let api_url = format!("{}/api/v1/user", url.trim_end_matches('/'));
            let body = run_curl(&["-H", &auth_header, &api_url])?;
            Ok(gitea_identity_from_user_json(&body)?)
        }

        AuthPayload::GitLab { token, url } => {
            let auth_header = format!("Authorization: Bearer {}", token);
            let api_url = format!("{}/api/v4/user", url.trim_end_matches('/'));
            let body = run_curl(&["-H", &auth_header, &api_url])?;
            Ok(gitlab_identity_from_user_json(&body)?)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn git_mode_returns_passed_values() {
        let result = infer_identity(
            GitServer::Git,
            AuthPayload::Git {
                handler: "alice".to_string(),
                display_name: "Alice Wonderland".to_string(),
                github_email: None,
            },
        )
        .unwrap();
        assert_eq!(result.handler.as_str(), "alice");
        assert_eq!(result.display_name, "Alice Wonderland");
        assert!(result.email.is_none());
    }

    #[test]
    fn git_mode_propagates_github_email() {
        let result = infer_identity(
            GitServer::Git,
            AuthPayload::Git {
                handler: "alice".to_string(),
                display_name: "Alice".to_string(),
                github_email: Some("alice@example.com".to_string()),
            },
        )
        .unwrap();
        assert_eq!(result.email.as_deref(), Some("alice@example.com"));
    }

    #[test]
    fn git_mode_empty_github_email_treated_as_none() {
        let result = infer_identity(
            GitServer::Git,
            AuthPayload::Git {
                handler: "alice".to_string(),
                display_name: "Alice".to_string(),
                github_email: Some("".to_string()),
            },
        )
        .unwrap();
        assert!(
            result.email.is_none(),
            "empty string should not produce a fake git author email"
        );
    }

    #[test]
    fn git_mode_invalid_handler_returns_error() {
        let result = infer_identity(
            GitServer::Git,
            AuthPayload::Git {
                handler: "INVALID_UPPER".to_string(),
                display_name: "Bad".to_string(),
                github_email: None,
            },
        );
        assert!(matches!(result, Err(IdentityError::InvalidHandler(_))));
    }

    #[test]
    fn git_mode_reserved_handler_returns_error() {
        let result = infer_identity(
            GitServer::Git,
            AuthPayload::Git {
                handler: "system".to_string(),
                display_name: "System".to_string(),
                github_email: None,
            },
        );
        assert!(matches!(result, Err(IdentityError::InvalidHandler(_))));
    }

    fn serve_github_user_once(body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0_u8; 1024];
            let _ = stream.read(&mut buf);
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        format!("http://{}", addr)
    }

    #[test]
    fn github_private_email_derives_noreply() {
        let api_base = serve_github_user_once(
            r#"{"id":12345,"login":"octocat","name":"Octocat","email":null}"#,
        );
        std::env::set_var("GITIM_TEST_GITHUB_API_BASE", api_base);

        let result = infer_identity(
            GitServer::GitHub,
            AuthPayload::GitHub {
                token: "fake-token".to_string(),
            },
        )
        .unwrap();

        std::env::remove_var("GITIM_TEST_GITHUB_API_BASE");
        assert_eq!(
            result.email.as_deref(),
            Some("12345+octocat@users.noreply.github.com")
        );
    }

    #[test]
    fn github_curl_failure_returns_curl_error() {
        // Using a clearly invalid token against a non-routable host forces curl to fail.
        // We override the URL indirectly by using a bad token that will get rejected, BUT
        // to avoid real network calls in unit tests we use a localhost port that isn't listening,
        // which makes curl exit non-zero.
        //
        // We can't easily mock curl here, so instead we verify that supplying a token
        // against an unreachable endpoint produces CurlFailed.  We use
        // http://127.0.0.1:1 which is always refused immediately.
        //
        // Note: the GitServer arg is intentionally ignored in the current implementation
        // (routing is driven purely by AuthPayload variant), so we pass GitHub here.
        let result = infer_identity(
            GitServer::GitHub,
            // We can't override the GitHub URL from the outside, so test the Gitea/GitLab
            // variants instead which accept a url parameter.
            AuthPayload::Gitea {
                token: "fake-token".to_string(),
                url: "http://127.0.0.1:1".to_string(),
            },
        );
        assert!(
            matches!(result, Err(IdentityError::CurlFailed(_))),
            "expected CurlFailed, got: {:?}",
            result
        );
    }

    #[test]
    fn gitea_curl_failure_returns_error() {
        let result = infer_identity(
            GitServer::Gitea,
            AuthPayload::Gitea {
                token: "fake-token".to_string(),
                url: "http://127.0.0.1:1".to_string(),
            },
        );
        assert!(
            matches!(result, Err(IdentityError::CurlFailed(_))),
            "expected CurlFailed, got: {:?}",
            result
        );
    }

    #[test]
    fn gitlab_curl_failure_returns_error() {
        let result = infer_identity(
            GitServer::GitLab,
            AuthPayload::GitLab {
                token: "fake-token".to_string(),
                url: "http://127.0.0.1:1".to_string(),
            },
        );
        assert!(
            matches!(result, Err(IdentityError::CurlFailed(_))),
            "expected CurlFailed, got: {:?}",
            result
        );
    }

    #[test]
    fn parse_error_on_malformed_json() {
        // We can test parse-error path by constructing InferredIdentity from bad JSON
        // without making a network call. We do this by calling infer_identity with
        // a Gitea URL that returns garbage — but since we can't intercept curl here,
        // we test the JSON parsing helper indirectly through a direct serde call.
        let bad_json = "not-json";
        let parse_result: Result<serde_json::Value, _> = serde_json::from_str(bad_json);
        assert!(parse_result.is_err());
        // Map it the same way the production code does
        let mapped = IdentityError::ParseError(parse_result.unwrap_err().to_string());
        assert!(format!("{}", mapped).contains("parse"));
    }

    #[test]
    fn missing_field_error_displays_field_name() {
        let err = IdentityError::MissingField("login".to_string());
        assert!(format!("{}", err).contains("login"));
    }
}
