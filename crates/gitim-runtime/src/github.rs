use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;
use reqwest::StatusCode;

#[derive(Debug, thiserror::Error)]
pub enum GithubError {
    #[error("token rejected by github")]
    InvalidToken,
    #[error("token lacks required scopes")]
    InsufficientScope,
    #[error("token valid but no access to repository")]
    RepoNotFoundOrNoAccess,
    #[error("rate limited by github")]
    RateLimited,
    #[error("network error")]
    NetworkError(#[from] reqwest::Error),
    #[error("unexpected github response status {0}")]
    UnexpectedStatus(u16),
    #[error("parse error: {0}")]
    ParseError(String),
}

pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const USER_AGENT: &str = concat!("gitim-runtime/", env!("CARGO_PKG_VERSION"));

async fn send_github_get(url: &str, token: &str) -> Result<StatusCode, GithubError> {
    let response = reqwest::Client::new()
        .get(url)
        .bearer_auth(token)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await?;

    let status = response.status();
    // Drain the body so a hang mid-stream surfaces as NetworkError — some
    // servers send headers before stalling the body (see the timeout test).
    response.bytes().await?;
    Ok(status)
}

pub async fn verify_token(token: &str, api_base: &str) -> Result<(), GithubError> {
    let url = format!("{}/user", api_base.trim_end_matches('/'));
    let status = send_github_get(&url, token).await?;
    match status.as_u16() {
        401 => Err(GithubError::InvalidToken),
        403 => Err(GithubError::InsufficientScope),
        429 => Err(GithubError::RateLimited),
        s if (200..300).contains(&s) => Ok(()),
        s => Err(GithubError::UnexpectedStatus(s)),
    }
}

/// Fetch an email for the authenticated user, good enough to attribute
/// commits to their GitHub contribution graph.
///
/// Prefers the public email on /user. Falls back to the GitHub-hosted
/// noreply address `{id}+{login}@users.noreply.github.com` when public
/// email is null. Both common causes land there:
///   - fine-grained PAT without the Account-level "Email addresses" read
///     permission (note: it lives under Account permissions, not
///     Repository permissions — easy to miss)
///   - accounts with "Keep my email addresses private" set (the default
///     for new accounts), which keeps /user.email null regardless of
///     what the PAT is allowed to read
///
/// The noreply form is always verified on the account so the contribution
/// graph still credits the user, and it requires no PAT scope at all.
///
/// Returns `Ok(None)` only when the response is so degenerate we can't
/// derive either (no `id` + no `login`) — in practice, only a mocked or
/// malformed /user response. Auth / network failures surface as `Err`.
pub async fn fetch_user_email(token: &str, api_base: &str) -> Result<Option<String>, GithubError> {
    let url = format!("{}/user", api_base.trim_end_matches('/'));
    let response = reqwest::Client::new()
        .get(&url)
        .bearer_auth(token)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await?;

    let status = response.status();
    match status.as_u16() {
        401 => return Err(GithubError::InvalidToken),
        403 => return Err(GithubError::InsufficientScope),
        429 => return Err(GithubError::RateLimited),
        s if (200..300).contains(&s) => {}
        s => return Err(GithubError::UnexpectedStatus(s)),
    }

    let body: serde_json::Value = response.json().await.map_err(GithubError::from)?;
    if let Some(email) = body
        .get("email")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        return Ok(Some(email.to_string()));
    }
    let id = body.get("id").and_then(|v| v.as_u64());
    let login = body
        .get("login")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    if let (Some(id), Some(login)) = (id, login) {
        return Ok(Some(format!("{id}+{login}@users.noreply.github.com")));
    }
    Ok(None)
}

pub async fn check_repo_access(
    owner: &str,
    repo: &str,
    token: &str,
    api_base: &str,
) -> Result<(), GithubError> {
    let url = format!("{}/repos/{owner}/{repo}", api_base.trim_end_matches('/'));
    let status = send_github_get(&url, token).await?;
    match status.as_u16() {
        404 => Err(GithubError::RepoNotFoundOrNoAccess),
        403 => Err(GithubError::InsufficientScope),
        401 => Err(GithubError::InvalidToken),
        429 => Err(GithubError::RateLimited),
        s if (200..300).contains(&s) => Ok(()),
        s => Err(GithubError::UnexpectedStatus(s)),
    }
}

pub fn parse_github_url(url: &str) -> Result<(String, String), GithubError> {
    // Only github.com — gitea/gitlab/self-hosted github enterprise are rejected
    // here so callers don't silently hit the wrong API.
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        crate::preconditions::regex_compile(r"^https://github\.com/([^/]+)/([^/]+?)(?:\.git)?/?$")
    });
    let caps = re
        .captures(url)
        .ok_or_else(|| GithubError::ParseError(format!("not a github.com repo url: {url}")))?;
    let owner = caps
        .get(1)
        .map(|m| m.as_str().to_string())
        .ok_or_else(|| GithubError::ParseError(format!("regex capture failed for owner: {url}")))?;
    let repo = caps
        .get(2)
        .map(|m| m.as_str().to_string())
        .ok_or_else(|| GithubError::ParseError(format!("regex capture failed for repo: {url}")))?;
    Ok((owner, repo))
}
