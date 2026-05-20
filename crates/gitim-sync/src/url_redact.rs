//! Scrub `user:pat@` credentials from HTTPS URLs in arbitrary text for log safety.
//! Assumes RFC 3986 percent-encoded credentials (git's default); a literal `@` in
//! the password would leak the suffix.

use regex::Regex;
use std::sync::LazyLock;

static CREDENTIAL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(https?://)([^:/@\s]+):[^@\s]+@")
        .unwrap_or_else(|_| unreachable!("credential URL regex is valid"))
});

pub fn redacted_url(text: &str) -> String {
    CREDENTIAL_RE
        .replace_all(text, "$1$2:<REDACTED>@")
        .into_owned()
}
