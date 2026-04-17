use gitim_runtime::github::{check_repo_access, parse_github_url, verify_token, GithubError};
use mockito::Server;
use std::time::Duration;

#[tokio::test]
async fn verify_token_401_returns_invalid_token() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("GET", "/user")
        .with_status(401)
        .with_body(r#"{"message":"Bad credentials"}"#)
        .create_async()
        .await;

    let err = verify_token("bad-token", &server.url()).await.unwrap_err();
    assert!(matches!(err, GithubError::InvalidToken), "got {err:?}");
    mock.assert_async().await;
}

#[tokio::test]
async fn verify_token_200_returns_ok_unit() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("GET", "/user")
        .with_status(200)
        .with_body(r#"{"login":"octocat","name":"The Octocat"}"#)
        .create_async()
        .await;

    let result = verify_token("good-token", &server.url()).await;
    assert!(matches!(result, Ok(())), "got {result:?}");
    mock.assert_async().await;
}

#[tokio::test]
async fn verify_token_403_returns_insufficient_scope() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("GET", "/user")
        .with_status(403)
        .with_body(r#"{"message":"Resource not accessible by personal access token"}"#)
        .create_async()
        .await;

    let err = verify_token("limited-token", &server.url())
        .await
        .unwrap_err();
    assert!(matches!(err, GithubError::InsufficientScope), "got {err:?}");
    mock.assert_async().await;
}

#[tokio::test]
async fn verify_token_timeout_returns_network_error() {
    let mut server = Server::new_async().await;
    // Response headers flush at 200 OK, then the body-writer callback blocks
    // for 11s before writing — the per-request 10s timeout fires mid-body.
    let _mock = server
        .mock("GET", "/user")
        .with_status(200)
        .with_chunked_body(|w| {
            std::thread::sleep(Duration::from_secs(11));
            w.write_all(b"{}")
        })
        .create_async()
        .await;

    let start = std::time::Instant::now();
    let err = verify_token("any", &server.url()).await.unwrap_err();
    let elapsed = start.elapsed();

    assert!(
        matches!(err, GithubError::NetworkError(_)),
        "expected NetworkError, got {err:?}"
    );
    assert!(
        elapsed >= Duration::from_secs(9) && elapsed < Duration::from_secs(20),
        "timeout fired too early or too late: {elapsed:?}"
    );
}

#[tokio::test]
async fn verify_token_200_succeeds_regardless_of_body() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("GET", "/user")
        .with_status(200)
        .with_body("not a json response")
        .create_async()
        .await;

    let result = verify_token("good-token", &server.url()).await;
    assert!(matches!(result, Ok(())), "got {result:?}");
    mock.assert_async().await;
}

#[tokio::test]
async fn verify_token_429_returns_rate_limited() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("GET", "/user")
        .with_status(429)
        .with_body(r#"{"message":"API rate limit exceeded"}"#)
        .create_async()
        .await;

    let err = verify_token("t", &server.url()).await.unwrap_err();
    assert!(matches!(err, GithubError::RateLimited), "got {err:?}");
    mock.assert_async().await;
}

#[tokio::test]
async fn check_repo_access_200_returns_ok() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("GET", "/repos/octocat/hello-world")
        .with_status(200)
        .with_body(r#"{"name":"hello-world","private":false}"#)
        .create_async()
        .await;

    let result = check_repo_access("octocat", "hello-world", "t", &server.url()).await;
    assert!(matches!(result, Ok(())), "got {result:?}");
    mock.assert_async().await;
}

#[tokio::test]
async fn check_repo_access_404_returns_repo_not_found_or_no_access() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("GET", "/repos/octocat/secret")
        .with_status(404)
        .with_body(r#"{"message":"Not Found"}"#)
        .create_async()
        .await;

    let err = check_repo_access("octocat", "secret", "t", &server.url())
        .await
        .unwrap_err();
    assert!(
        matches!(err, GithubError::RepoNotFoundOrNoAccess),
        "got {err:?}"
    );
    mock.assert_async().await;
}

#[tokio::test]
async fn check_repo_access_403_returns_insufficient_scope() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("GET", "/repos/octocat/forbidden")
        .with_status(403)
        .with_body(r#"{"message":"Resource not accessible"}"#)
        .create_async()
        .await;

    let err = check_repo_access("octocat", "forbidden", "t", &server.url())
        .await
        .unwrap_err();
    assert!(matches!(err, GithubError::InsufficientScope), "got {err:?}");
    mock.assert_async().await;
}

#[test]
fn check_repo_access_parses_owner_repo_from_https_url() {
    let (owner, repo) = parse_github_url("https://github.com/owner/repo").unwrap();
    assert_eq!(owner, "owner");
    assert_eq!(repo, "repo");
}

#[test]
fn check_repo_access_parses_owner_repo_from_dot_git_url() {
    let (owner, repo) = parse_github_url("https://github.com/owner/repo.git").unwrap();
    assert_eq!(owner, "owner");
    assert_eq!(repo, "repo");
}

#[test]
fn check_repo_access_parses_trailing_slash() {
    let (owner, repo) = parse_github_url("https://github.com/owner/repo/").unwrap();
    assert_eq!(owner, "owner");
    assert_eq!(repo, "repo");
}

#[test]
fn check_repo_access_rejects_non_github_host() {
    let err = parse_github_url("https://gitlab.com/owner/repo").unwrap_err();
    assert!(matches!(err, GithubError::ParseError(_)), "got {err:?}");
}
