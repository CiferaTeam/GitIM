use regex::Regex;
use std::sync::LazyLock;

static CREDENTIAL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(https?://)([^:/@\s]+):[^@\s]+@").unwrap());

pub fn redacted_url(text: &str) -> String {
    CREDENTIAL_RE
        .replace_all(text, "$1$2:<REDACTED>@")
        .into_owned()
}
