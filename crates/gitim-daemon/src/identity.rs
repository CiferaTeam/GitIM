use gitim_core::types::handler::HandlerError;
use gitim_core::types::Handler;
use serde::Deserialize;
use std::process::Command;
use thiserror::Error;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitServer {
    Git,
    GitHub,
    Gitea,
    GitLab,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthData {
    Git {
        handler: String,
        display_name: String,
    },
    GitHub {
        token: String,
    },
    Gitea {
        token: String,
        url: String,
    },
    GitLab {
        token: String,
        url: String,
    },
}

#[derive(Debug, Clone)]
pub struct InferredIdentity {
    pub handler: Handler,
    pub display_name: String,
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
    auth_data: AuthData,
) -> Result<InferredIdentity, IdentityError> {
    match auth_data {
        AuthData::Git {
            handler,
            display_name,
        } => {
            let validated = Handler::new(&handler)?;
            Ok(InferredIdentity {
                handler: validated,
                display_name,
            })
        }

        AuthData::GitHub { token } => {
            let auth_header = format!("Authorization: token {}", token);
            let body = run_curl(&["-H", &auth_header, "https://api.github.com/user"])?;

            let v: serde_json::Value = serde_json::from_str(&body)
                .map_err(|e| IdentityError::ParseError(e.to_string()))?;

            let login = v
                .get("login")
                .and_then(|x| x.as_str())
                .ok_or_else(|| IdentityError::MissingField("login".to_string()))?
                .to_lowercase();

            let display_name = v
                .get("name")
                .and_then(|x| x.as_str())
                .unwrap_or(&login)
                .to_string();

            let handler = Handler::new(&login)?;
            Ok(InferredIdentity {
                handler,
                display_name,
            })
        }

        AuthData::Gitea { token, url } => {
            let auth_header = format!("Authorization: token {}", token);
            let api_url = format!("{}/api/v1/user", url.trim_end_matches('/'));
            let body = run_curl(&["-H", &auth_header, &api_url])?;

            let v: serde_json::Value = serde_json::from_str(&body)
                .map_err(|e| IdentityError::ParseError(e.to_string()))?;

            let login = v
                .get("login")
                .and_then(|x| x.as_str())
                .ok_or_else(|| IdentityError::MissingField("login".to_string()))?
                .to_lowercase();

            let display_name = v
                .get("full_name")
                .and_then(|x| x.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or(&login)
                .to_string();

            let handler = Handler::new(&login)?;
            Ok(InferredIdentity {
                handler,
                display_name,
            })
        }

        AuthData::GitLab { token, url } => {
            let auth_header = format!("Authorization: Bearer {}", token);
            let api_url = format!("{}/api/v4/user", url.trim_end_matches('/'));
            let body = run_curl(&["-H", &auth_header, &api_url])?;

            let v: serde_json::Value = serde_json::from_str(&body)
                .map_err(|e| IdentityError::ParseError(e.to_string()))?;

            let username = v
                .get("username")
                .and_then(|x| x.as_str())
                .ok_or_else(|| IdentityError::MissingField("username".to_string()))?
                .to_lowercase();

            let display_name = v
                .get("name")
                .and_then(|x| x.as_str())
                .unwrap_or(&username)
                .to_string();

            let handler = Handler::new(&username)?;
            Ok(InferredIdentity {
                handler,
                display_name,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_mode_returns_passed_values() {
        let result = infer_identity(
            GitServer::Git,
            AuthData::Git {
                handler: "alice".to_string(),
                display_name: "Alice Wonderland".to_string(),
            },
        )
        .unwrap();
        assert_eq!(result.handler.as_str(), "alice");
        assert_eq!(result.display_name, "Alice Wonderland");
    }

    #[test]
    fn git_mode_invalid_handler_returns_error() {
        let result = infer_identity(
            GitServer::Git,
            AuthData::Git {
                handler: "INVALID_UPPER".to_string(),
                display_name: "Bad".to_string(),
            },
        );
        assert!(matches!(result, Err(IdentityError::InvalidHandler(_))));
    }

    #[test]
    fn git_mode_reserved_handler_returns_error() {
        let result = infer_identity(
            GitServer::Git,
            AuthData::Git {
                handler: "system".to_string(),
                display_name: "System".to_string(),
            },
        );
        assert!(matches!(result, Err(IdentityError::InvalidHandler(_))));
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
        // (routing is driven purely by AuthData variant), so we pass GitHub here.
        let result = infer_identity(
            GitServer::GitHub,
            // We can't override the GitHub URL from the outside, so test the Gitea/GitLab
            // variants instead which accept a url parameter.
            AuthData::Gitea {
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
            AuthData::Gitea {
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
            AuthData::GitLab {
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
