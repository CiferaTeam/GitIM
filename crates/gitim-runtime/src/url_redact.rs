//! Scrub `user:pat@` credentials from HTTPS URLs in arbitrary text for log and
//! response-body safety. Mirror of `gitim_sync::url_redact::redacted_url`.
//!
//! We duplicate the helper here because pulling `gitim-sync` as a dependency
//! just for six lines of code is architectural overkill and would break this
//! crate's build whenever sync is mid-refactor. Task 9 (redaction audit)
//! consolidates both copies into a neutral helper crate.
//!
//! Assumes RFC 3986 percent-encoded credentials (git's default); a literal `@`
//! in the password would leak the suffix.

use regex::Regex;
use std::sync::LazyLock;

static CREDENTIAL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(https?://)([^:/@\s]+):[^@\s]+@").unwrap());

pub fn redacted_url(text: &str) -> String {
    CREDENTIAL_RE
        .replace_all(text, "$1$2:<REDACTED>@")
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_token_in_github_clone_url() {
        let input = "remote: failed https://x-access-token:ghp_secret_xyz@github.com/owner/repo.git";
        let redacted = redacted_url(input);
        assert!(!redacted.contains("ghp_secret_xyz"), "got {redacted}");
        assert!(redacted.contains("<REDACTED>"));
    }

    #[test]
    fn passthrough_when_no_credentials() {
        let input = "plain error with no url";
        assert_eq!(redacted_url(input), input);
    }
}
