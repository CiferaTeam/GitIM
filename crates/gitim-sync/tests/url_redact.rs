use gitim_sync::url_redact::redacted_url;

#[test]
fn redact_github_x_access_token() {
    let input = "https://x-access-token:ghp_abc123@github.com/o/r.git";
    let expected = "https://x-access-token:<REDACTED>@github.com/o/r.git";
    assert_eq!(redacted_url(input), expected);
}

#[test]
fn redact_classic_username_password() {
    let input = "https://user:pat@host/path";
    let expected = "https://user:<REDACTED>@host/path";
    assert_eq!(redacted_url(input), expected);
}

#[test]
fn redact_gitlab_oauth2() {
    let input = "https://oauth2:token@gitlab.com/o/r.git";
    let expected = "https://oauth2:<REDACTED>@gitlab.com/o/r.git";
    assert_eq!(redacted_url(input), expected);
}

#[test]
fn redact_ssh_form_leaves_untouched() {
    let input = "git@github.com:o/r.git";
    assert_eq!(redacted_url(input), input);
}

#[test]
fn redact_no_credential_untouched() {
    let input = "https://github.com/o/r.git";
    assert_eq!(redacted_url(input), input);
}

#[test]
fn redact_multiline_text_handles_all_urls() {
    let input = "\
fetching from https://user:secret1@github.com/a/b.git
failed to push to https://x-access-token:ghp_deadbeef@github.com/c/d.git
also tried https://oauth2:zzz@gitlab.com/e/f.git
plus clean https://github.com/g/h.git still works
";
    let out = redacted_url(input);
    assert!(
        out.contains("https://user:<REDACTED>@github.com/a/b.git"),
        "user:pat: {out}"
    );
    assert!(
        out.contains("https://x-access-token:<REDACTED>@github.com/c/d.git"),
        "x-access-token: {out}"
    );
    assert!(
        out.contains("https://oauth2:<REDACTED>@gitlab.com/e/f.git"),
        "oauth2: {out}"
    );
    assert!(
        out.contains("https://github.com/g/h.git"),
        "clean url preserved: {out}"
    );
    assert!(!out.contains("secret1"), "leaked secret1: {out}");
    assert!(!out.contains("ghp_deadbeef"), "leaked ghp_deadbeef: {out}");
    assert!(!out.contains("oauth2:zzz"), "leaked oauth2:zzz: {out}");
}

#[test]
fn redact_preserves_sentinel_outside_credential() {
    let input = "the token ghp_TESTSENTINEL was wrong";
    assert_eq!(redacted_url(input), input);
}
