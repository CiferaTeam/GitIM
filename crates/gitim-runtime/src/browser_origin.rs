use axum::http::{header, HeaderMap};
use std::collections::HashSet;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct BrowserOriginPolicy {
    allowed_web_origins: Arc<HashSet<String>>,
}

impl BrowserOriginPolicy {
    pub(crate) fn from_environment() -> Self {
        let configured = std::env::var("GITIM_WEB_ORIGINS").ok();
        Self::new(
            configured
                .as_deref()
                .into_iter()
                .flat_map(|origins| origins.split(','))
                .map(str::trim),
        )
    }

    pub(crate) fn new(configured: impl IntoIterator<Item = impl AsRef<str>>) -> Self {
        let mut allowed_web_origins = [
            "https://gitim.io",
            "https://www.gitim.io",
            "http://localhost:5173",
            "http://127.0.0.1:5173",
            "http://[::1]:5173",
            "http://localhost:4173",
            "http://127.0.0.1:4173",
            "http://[::1]:4173",
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<HashSet<_>>();
        allowed_web_origins.extend(configured.into_iter().filter_map(|origin| {
            let origin = origin.as_ref();
            is_canonical_origin(origin).then(|| origin.to_string())
        }));
        Self {
            allowed_web_origins: Arc::new(allowed_web_origins),
        }
    }

    pub(crate) fn allows_request(&self, headers: &HeaderMap) -> bool {
        match singleton_origin(headers) {
            Ok(None) => true,
            Ok(Some(origin)) => self.allows_origin(origin),
            Err(()) => false,
        }
    }

    pub(crate) fn allows_origin(&self, raw: &str) -> bool {
        is_canonical_origin(raw) && self.allowed_web_origins.contains(raw)
    }
}

fn singleton_origin(headers: &HeaderMap) -> Result<Option<&str>, ()> {
    let values = headers.get_all(header::ORIGIN);
    let mut values = values.iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(());
    }
    let value = value.to_str().map_err(|_| ())?;
    if value.is_empty() || value.trim() != value || value.contains(',') {
        return Err(());
    }
    Ok(Some(value))
}

pub(crate) fn is_canonical_origin(raw: &str) -> bool {
    if raw == "null" || raw == "*" || raw.contains('@') {
        return false;
    }
    let Ok(url) = reqwest::Url::parse(raw) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || url.host_str().is_none()
    {
        return false;
    }
    url.origin().ascii_serialization() == raw
}

#[cfg(test)]
mod tests {
    use super::BrowserOriginPolicy;
    use axum::http::{header, HeaderMap, HeaderValue};

    fn headers(origin: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(origin) = origin {
            headers.insert(header::ORIGIN, HeaderValue::from_str(origin).unwrap());
        }
        headers
    }

    #[test]
    fn default_web_origins_and_non_browser_callers_are_allowed() {
        let policy = BrowserOriginPolicy::new(std::iter::empty::<&str>());

        assert!(policy.allows_request(&headers(None)));
        assert!(policy.allows_request(&headers(Some("https://gitim.io"))));
        assert!(policy.allows_request(&headers(Some("http://localhost:5173"))));
    }

    #[test]
    fn arbitrary_and_malformed_browser_origins_are_rejected() {
        let policy = BrowserOriginPolicy::new(std::iter::empty::<&str>());

        for origin in [
            "https://evil.example",
            "https://gitim.io/",
            "https://user@gitim.io",
            "null",
            "*",
        ] {
            assert!(!policy.allows_request(&headers(Some(origin))), "{origin}");
        }

        let mut duplicated = HeaderMap::new();
        duplicated.append(header::ORIGIN, HeaderValue::from_static("https://gitim.io"));
        duplicated.append(
            header::ORIGIN,
            HeaderValue::from_static("https://evil.example"),
        );
        assert!(!policy.allows_request(&duplicated));
    }

    #[test]
    fn configured_origins_are_additive_and_exact() {
        let policy = BrowserOriginPolicy::new(["https://preview.gitim.example"]);

        assert!(policy.allows_request(&headers(Some("https://gitim.io"))));
        assert!(policy.allows_request(&headers(Some("https://preview.gitim.example"))));
        assert!(!policy.allows_request(&headers(Some("https://preview.gitim.example/"))));
    }
}
