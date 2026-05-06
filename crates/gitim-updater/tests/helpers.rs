//! Unit tests for gitim-updater pure helpers.

use gitim_updater::{
    detect_platform, download_url, is_newer, latest_release_api_url, parse_version, BINARIES,
};

#[test]
fn parse_version_empty_string_is_none() {
    assert_eq!(parse_version(""), None);
}

#[test]
fn parse_version_plain_triple() {
    assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
}

#[test]
fn parse_version_with_v_prefix() {
    assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
}

#[test]
fn parse_version_multi_digit_minor() {
    assert_eq!(parse_version("v0.10.0"), Some((0, 10, 0)));
}

#[test]
fn parse_version_garbage_is_none() {
    assert_eq!(parse_version("bad"), None);
}

#[test]
fn parse_version_two_component_is_none() {
    assert_eq!(parse_version("1.2"), None);
}

#[test]
fn parse_version_four_component_is_none() {
    // Stricter than the CLI original: trailing segments → None, not silently discarded.
    assert_eq!(parse_version("1.2.3.4"), None);
}

#[test]
fn is_newer_older_remote_returns_false() {
    assert!(!is_newer("0.4.0", "0.3.9"));
}

#[test]
fn is_newer_same_version_returns_false() {
    assert!(!is_newer("0.3.1", "0.3.1"));
}

#[test]
fn is_newer_newer_remote_returns_true() {
    assert!(is_newer("0.3.1", "0.4.0"));
    // v-prefix on the remote must be tolerated, since GitHub tags are `vX.Y.Z`.
    assert!(is_newer("0.3.1", "v0.4.0"));
}

#[test]
fn is_newer_malformed_fails_closed() {
    // Fail-closed: we never want to "offer to update" based on unparseable input.
    assert!(!is_newer("bad", "0.4.0"));
    assert!(!is_newer("0.3.1", "bad"));
}

#[test]
fn download_url_contract() {
    assert_eq!(
        download_url("v0.3.1", "darwin-arm64"),
        "https://github.com/CiferaTeam/gitim-releases/releases/download/v0.3.1/gitim-v0.3.1-darwin-arm64.tar.gz"
    );
}

#[test]
fn latest_release_api_url_contract() {
    assert_eq!(
        latest_release_api_url(),
        "https://api.github.com/repos/CiferaTeam/gitim-releases/releases/latest"
    );
}

#[test]
fn detect_platform_on_host_returns_known_slug() {
    // We can't hard-code a specific value (test runs on mac, linux, CI…),
    // but any supported host MUST return one of the four canonical slugs.
    let p = detect_platform().expect("test host should be a supported platform");
    let known = [
        "darwin-arm64",
        "darwin-x86_64",
        "linux-arm64",
        "linux-x86_64",
    ];
    assert!(known.contains(&p.as_str()), "unexpected platform slug: {p}");
}

#[test]
fn binaries_constant_contract() {
    // The installer relies on this order and set; lock it in so a drive-by edit
    // surfaces as a test diff, not a silent release bug.
    assert_eq!(BINARIES, &["gitim", "gitim-daemon", "gitim-runtime"]);
}
