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
    #[error("network error: {0}")]
    NetworkError(String),
    #[error("parse error: {0}")]
    ParseError(String),
}

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const USER_AGENT: &str = "gitim-runtime";

fn map_transport_error(err: reqwest::Error) -> GithubError {
    GithubError::NetworkError(err.to_string())
}

async fn send_github_get(url: &str, token: &str) -> Result<StatusCode, GithubError> {
    let response = reqwest::Client::new()
        .get(url)
        .bearer_auth(token)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(map_transport_error)?;

    let status = response.status();
    // Drain the body so a hang mid-stream surfaces as NetworkError — some
    // servers send headers before stalling the body (see the timeout test).
    response.bytes().await.map_err(map_transport_error)?;
    Ok(status)
}

pub async fn verify_token(token: &str, api_base: &str) -> Result<(), GithubError> {
    let url = format!("{}/user", api_base.trim_end_matches('/'));
    let status = send_github_get(&url, token).await?;
    match status.as_u16() {
        401 => Err(GithubError::InvalidToken),
        403 => Err(GithubError::InsufficientScope),
        429 => Err(GithubError::RateLimited),
        _ => Ok(()),
    }
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
        _ => Ok(()),
    }
}

pub fn parse_github_url(url: &str) -> Result<(String, String), GithubError> {
    // Only github.com — gitea/gitlab/self-hosted github enterprise are rejected
    // here so callers don't silently hit the wrong API.
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"^https://github\.com/([^/]+)/([^/]+?)(?:\.git)?/?$")
            .expect("static regex compiles")
    });
    let caps = re
        .captures(url)
        .ok_or_else(|| GithubError::ParseError(format!("not a github.com repo url: {url}")))?;
    let owner = caps.get(1).unwrap().as_str().to_string();
    let repo = caps.get(2).unwrap().as_str().to_string();
    Ok((owner, repo))
}
