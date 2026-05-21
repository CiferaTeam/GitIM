use crate::types::{Handler, Link, LinkKind};
use crate::validator::validate_channel_name;
use regex::Regex;
use std::sync::LazyLock;

// SAFETY: The regex pattern is a statically-verified literal; Regex::new
// can only fail on invalid syntax, which is impossible here.
#[allow(clippy::unwrap_used)]
static LINK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<([#~!])([^>\n]+)>").unwrap());

// SAFETY: The regex pattern is a statically-verified literal; Regex::new
// can only fail on invalid syntax, which is impossible here.
#[allow(clippy::unwrap_used)]
static MSG_LINK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(.+):L(\d{6,})$").unwrap());

/// 从消息 body 中提取所有协议级链接，按出现顺序返回，不去重。
pub fn extract_links(body: &str) -> Vec<Link> {
    let mut result = Vec::new();
    for caps in LINK_RE.captures_iter(body) {
        let prefix = &caps[1];
        let content = &caps[2];
        let raw = caps[0].to_string();
        let kind = match prefix {
            "#" => parse_channel_or_message(content),
            "~" => parse_user_profile(content),
            "!" => parse_softlink(content),
            _ => None,
        };
        if let Some(kind) = kind {
            result.push(Link { kind, raw });
        }
    }
    result
}

fn parse_channel_or_message(content: &str) -> Option<LinkKind> {
    if let Some(caps) = MSG_LINK_RE.captures(content) {
        let channel = &caps[1];
        let line_number: u64 = caps[2].parse().ok()?;
        validate_channel_name(channel).ok()?;
        Some(LinkKind::Message {
            channel: channel.to_string(),
            line_number,
        })
    } else {
        validate_channel_name(content).ok()?;
        Some(LinkKind::Channel {
            name: content.to_string(),
        })
    }
}

fn parse_user_profile(content: &str) -> Option<LinkKind> {
    let handler = Handler::new(content).ok()?;
    Some(LinkKind::UserProfile { handler })
}

fn parse_softlink(content: &str) -> Option<LinkKind> {
    if let Some(pos) = content.find('|') {
        let url = &content[..pos];
        if url.is_empty() {
            return None;
        }
        // Safe: '|' is ASCII (0x7C), so pos + 1 is always a valid UTF-8 boundary
        let title = &content[pos + 1..];
        Some(LinkKind::Softlink {
            url: url.to_string(),
            title: Some(title.to_string()),
        })
    } else {
        Some(LinkKind::Softlink {
            url: content.to_string(),
            title: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_link() {
        let links = extract_links("see <#general>");
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].kind,
            LinkKind::Channel {
                name: "general".into()
            }
        );
        assert_eq!(links[0].raw, "<#general>");
    }

    #[test]
    fn test_message_link() {
        let links = extract_links("refer to <#dev:L000042>");
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].kind,
            LinkKind::Message {
                channel: "dev".into(),
                line_number: 42
            }
        );
        assert_eq!(links[0].raw, "<#dev:L000042>");
    }

    #[test]
    fn test_user_profile_link() {
        let links = extract_links("check <~alice>");
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].kind,
            LinkKind::UserProfile {
                handler: Handler::new("alice").unwrap()
            }
        );
        assert_eq!(links[0].raw, "<~alice>");
    }

    #[test]
    fn test_softlink_bare() {
        let links = extract_links("visit <!https://example.com>");
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].kind,
            LinkKind::Softlink {
                url: "https://example.com".into(),
                title: None
            }
        );
        assert_eq!(links[0].raw, "<!https://example.com>");
    }

    #[test]
    fn test_softlink_with_title() {
        let links = extract_links("see <!https://example.com|Example Site>");
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].kind,
            LinkKind::Softlink {
                url: "https://example.com".into(),
                title: Some("Example Site".into()),
            }
        );
    }

    #[test]
    fn test_softlink_empty_title() {
        let links = extract_links("see <!https://example.com|>");
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].kind,
            LinkKind::Softlink {
                url: "https://example.com".into(),
                title: Some("".into()),
            }
        );
    }

    #[test]
    fn test_multiple_links() {
        let links = extract_links("<#general> and <~bob> and <!https://x.com>");
        assert_eq!(links.len(), 3);
        assert_eq!(
            links[0].kind,
            LinkKind::Channel {
                name: "general".into()
            }
        );
        assert_eq!(
            links[1].kind,
            LinkKind::UserProfile {
                handler: Handler::new("bob").unwrap()
            }
        );
        assert_eq!(
            links[2].kind,
            LinkKind::Softlink {
                url: "https://x.com".into(),
                title: None
            }
        );
    }

    #[test]
    fn test_duplicate_links_not_deduped() {
        let links = extract_links("<#general> <#general>");
        assert_eq!(links.len(), 2);
    }

    #[test]
    fn test_no_links() {
        let links = extract_links("just a plain message");
        assert!(links.is_empty());
    }

    #[test]
    fn test_mention_not_captured() {
        let links = extract_links("<@alice>");
        assert!(links.is_empty());
    }

    #[test]
    fn test_empty_markers_not_matched() {
        let links = extract_links("<#> <~> <!>");
        assert!(links.is_empty());
    }

    #[test]
    fn test_uppercase_channel_ignored() {
        let links = extract_links("<#General>");
        assert!(links.is_empty());
    }

    #[test]
    fn test_consecutive_hyphen_channel_ignored() {
        let links = extract_links("<#bad--name>");
        assert!(links.is_empty());
    }

    #[test]
    fn test_short_line_number_ignored() {
        let links = extract_links("<#dev:L042>");
        // L042 is only 3 digits, less than 6 — should not parse as message link.
        // "dev:L042" also fails validate_channel_name, so no link at all.
        assert!(links.is_empty());
    }

    #[test]
    fn test_unclosed_marker() {
        let links = extract_links("<#general");
        assert!(links.is_empty());
    }

    #[test]
    fn test_system_handler_ignored() {
        let links = extract_links("<~system>");
        assert!(links.is_empty());
    }

    #[test]
    fn test_softlink_url_with_encoded_pipe() {
        // The first | splits url from title
        let links = extract_links("<!https://x.com/a%7Cb|my title>");
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].kind,
            LinkKind::Softlink {
                url: "https://x.com/a%7Cb".into(),
                title: Some("my title".into()),
            }
        );
    }

    #[test]
    fn test_message_link_long_line_number() {
        let links = extract_links("<#logs:L00000000099>");
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].kind,
            LinkKind::Message {
                channel: "logs".into(),
                line_number: 99
            }
        );
    }

    #[test]
    fn test_mention_and_link_coexist() {
        let links = extract_links("<@alice> see <#general>");
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].kind,
            LinkKind::Channel {
                name: "general".into()
            }
        );
    }

    #[test]
    fn test_empty_url_softlink_ignored() {
        // <!|title> has empty URL — should be rejected
        let links = extract_links("<!|some title>");
        assert!(links.is_empty());
    }

    #[test]
    fn test_newline_in_link_not_matched() {
        // Link markers must not span lines
        let links = extract_links("<!https://x.com\n|pwn>");
        assert!(links.is_empty());
    }

    #[test]
    fn test_bare_text_softlink_accepted() {
        // <!not a url> is syntactically valid — no URL validation
        let links = extract_links("<!not a url>");
        assert_eq!(links.len(), 1);
        match &links[0].kind {
            LinkKind::Softlink { url, title } => {
                assert_eq!(url, "not a url");
                assert_eq!(*title, None);
            }
            _ => panic!("expected Softlink"),
        }
    }
}
