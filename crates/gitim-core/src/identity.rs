use crate::types::handler::HandlerError;
use crate::types::Handler;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InferredIdentity {
    pub handler: Handler,
    pub display_name: String,
    pub email: Option<String>,
}

#[derive(Debug, Error)]
pub enum IdentityParseError {
    #[error("failed to parse API response: {0}")]
    ParseError(String),
    #[error("missing field '{0}' in API response")]
    MissingField(String),
    #[error("invalid handler from API: {0}")]
    InvalidHandler(#[from] HandlerError),
}

fn github_email_from_user_json(v: &serde_json::Value) -> Option<String> {
    if let Some(email) = v
        .get("email")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
    {
        return Some(email.to_string());
    }

    let id = v.get("id").and_then(|x| x.as_u64());
    let login = v
        .get("login")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty());
    match (id, login) {
        (Some(id), Some(login)) => Some(format!("{id}+{login}@users.noreply.github.com")),
        _ => None,
    }
}

pub fn github_identity_from_user_json(body: &str) -> Result<InferredIdentity, IdentityParseError> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| IdentityParseError::ParseError(e.to_string()))?;

    let login = v
        .get("login")
        .and_then(|x| x.as_str())
        .ok_or_else(|| IdentityParseError::MissingField("login".to_string()))?
        .to_lowercase();

    let display_name = v
        .get("name")
        .and_then(|x| x.as_str())
        .unwrap_or(&login)
        .to_string();

    let email = github_email_from_user_json(&v);
    let handler = Handler::new(&login)?;
    Ok(InferredIdentity {
        handler,
        display_name,
        email,
    })
}

pub fn gitea_identity_from_user_json(body: &str) -> Result<InferredIdentity, IdentityParseError> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| IdentityParseError::ParseError(e.to_string()))?;

    let login = v
        .get("login")
        .and_then(|x| x.as_str())
        .ok_or_else(|| IdentityParseError::MissingField("login".to_string()))?
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
        email: None,
    })
}

pub fn gitlab_identity_from_user_json(body: &str) -> Result<InferredIdentity, IdentityParseError> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| IdentityParseError::ParseError(e.to_string()))?;

    let username = v
        .get("username")
        .and_then(|x| x.as_str())
        .ok_or_else(|| IdentityParseError::MissingField("username".to_string()))?
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
        email: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_identity_uses_login_and_name() {
        let identity =
            github_identity_from_user_json(r#"{"login":"Flame4","name":"Flame"}"#).unwrap();
        assert_eq!(identity.handler.as_str(), "flame4");
        assert_eq!(identity.display_name, "Flame");
        assert!(identity.email.is_none());
    }

    #[test]
    fn github_identity_derives_noreply_email() {
        let identity =
            github_identity_from_user_json(r#"{"id":12345,"login":"octocat","email":null}"#)
                .unwrap();
        assert_eq!(
            identity.email.as_deref(),
            Some("12345+octocat@users.noreply.github.com")
        );
    }
}
