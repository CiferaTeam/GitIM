//! Wire-level auth payload for the daemon `onboard` request.
//!
//! Tagged on `type`. This is the source of truth for what callers
//! (CLI, runtime, future clients) must serialize, and what the daemon
//! deserializes before identity inference.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum AuthPayload {
    /// Caller-supplied identity. No remote API call. Used for `git`
    /// onboards and as the agent-provisioning auth inside github-backed
    /// workspaces (so agent commits attribute via `github_email`).
    #[serde(rename = "git")]
    Git {
        handler: String,
        display_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        github_email: Option<String>,
    },
    /// PAT auth — daemon shells out to `https://api.github.com/user`.
    #[serde(rename = "github")]
    GitHub { token: String },
    /// Self-hosted Gitea — `url` is the instance base, daemon hits
    /// `<url>/api/v1/user`.
    #[serde(rename = "gitea")]
    Gitea { token: String, url: String },
    /// Self-hosted GitLab — `url` is the instance base, daemon hits
    /// `<url>/api/v4/user`.
    #[serde(rename = "gitlab")]
    GitLab { token: String, url: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn deserialize_git_variant() {
        let raw = r#"{"type":"git","handler":"alice","display_name":"Alice W"}"#;
        let p: AuthPayload = serde_json::from_str(raw).unwrap();
        assert_eq!(
            p,
            AuthPayload::Git {
                handler: "alice".into(),
                display_name: "Alice W".into(),
                github_email: None,
            }
        );
    }

    #[test]
    fn deserialize_git_with_email() {
        let raw =
            r#"{"type":"git","handler":"alice","display_name":"Alice","github_email":"a@b.com"}"#;
        let p: AuthPayload = serde_json::from_str(raw).unwrap();
        match p {
            AuthPayload::Git { github_email, .. } => {
                assert_eq!(github_email.as_deref(), Some("a@b.com"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn deserialize_github_variant() {
        let raw = r#"{"type":"github","token":"ghp_xyz"}"#;
        let p: AuthPayload = serde_json::from_str(raw).unwrap();
        assert_eq!(p, AuthPayload::GitHub { token: "ghp_xyz".into() });
    }

    #[test]
    fn deserialize_gitea_variant() {
        let raw = r#"{"type":"gitea","token":"t","url":"https://gitea.example"}"#;
        let p: AuthPayload = serde_json::from_str(raw).unwrap();
        assert_eq!(
            p,
            AuthPayload::Gitea {
                token: "t".into(),
                url: "https://gitea.example".into(),
            }
        );
    }

    #[test]
    fn deserialize_gitlab_variant() {
        let raw = r#"{"type":"gitlab","token":"t","url":"https://gitlab.example"}"#;
        let p: AuthPayload = serde_json::from_str(raw).unwrap();
        assert_eq!(
            p,
            AuthPayload::GitLab {
                token: "t".into(),
                url: "https://gitlab.example".into(),
            }
        );
    }

    #[test]
    fn missing_type_field_fails() {
        let raw = r#"{"handler":"alice","display_name":"Alice"}"#;
        let r: Result<AuthPayload, _> = serde_json::from_str(raw);
        assert!(r.is_err(), "deserialization without `type` must fail");
    }

    #[test]
    fn serialize_git_omits_none_email() {
        let p = AuthPayload::Git {
            handler: "alice".into(),
            display_name: "Alice".into(),
            github_email: None,
        };
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.contains(r#""type":"git""#));
        assert!(!s.contains("github_email"));
    }

    #[test]
    fn serialize_git_includes_email() {
        let p = AuthPayload::Git {
            handler: "alice".into(),
            display_name: "Alice".into(),
            github_email: Some("a@b.com".into()),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.contains(r#""github_email":"a@b.com""#));
    }
}
