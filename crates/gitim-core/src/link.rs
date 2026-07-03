use crate::types::{validate_card_id, validate_quick_session_id, Handler, Link, LinkKind};
use crate::validator::validate_channel_name;
use regex::Regex;
use std::sync::LazyLock;

static LINK_RE: LazyLock<Regex> =
    LazyLock::new(|| crate::preconditions::regex_literal(r"<([#~!])([^>\n]+)>"));

/// Matches bare `session:qs-<ulid>` refs with optional line number.
/// Syntax: `session:qs-<26-char-Crockford-base32>(:L<6+digits>)?`
static SESSION_REF_RE: LazyLock<Regex> = LazyLock::new(|| {
    crate::preconditions::regex_literal(r"\bsession:(qs-[0-9A-HJKMNP-TV-Z]{26})(:L(\d{6,}))?\b")
});

static MSG_LINK_RE: LazyLock<Regex> =
    LazyLock::new(|| crate::preconditions::regex_literal(r"^(.+):L(\d{6,})$"));

static CARD_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| crate::preconditions::regex_literal(r"^([^:]+):L(\d{6,})$"));

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
    // Parse bare session:<id> refs (no <> markers)
    for caps in SESSION_REF_RE.captures_iter(body) {
        // Only match if preceded by start-of-string or non-alphanumeric char
        let Some(matched) = caps.get(0) else {
            continue;
        };
        let start = matched.start();
        if start > 0 {
            let preceding = body.as_bytes()[start - 1];
            if preceding.is_ascii_alphanumeric() || preceding == b'-' {
                continue;
            }
        }
        let session_id = caps[1].to_string();
        if validate_quick_session_id(&session_id).is_err() {
            continue;
        }
        let line_number = caps.get(3).and_then(|m| m.as_str().parse::<u64>().ok());
        let raw = matched.as_str().to_string();
        result.push(Link {
            kind: LinkKind::QuickSession {
                session_id,
                line_number,
            },
            raw,
        });
    }
    result
}

fn parse_channel_or_message(content: &str) -> Option<LinkKind> {
    let (target, label) = split_label(content);
    if target.contains('/') {
        return parse_card_link(target, label);
    }
    if label.is_some() {
        return None;
    }
    if let Some(caps) = MSG_LINK_RE.captures(target) {
        let channel = &caps[1];
        let line_number: u64 = caps[2].parse().ok()?;
        validate_channel_name(channel).ok()?;
        Some(LinkKind::Message {
            channel: channel.to_string(),
            line_number,
        })
    } else {
        validate_channel_name(target).ok()?;
        Some(LinkKind::Channel {
            name: target.to_string(),
        })
    }
}

fn split_label(content: &str) -> (&str, Option<String>) {
    if let Some(pos) = content.find('|') {
        (&content[..pos], Some(content[pos + 1..].to_string()))
    } else {
        (content, None)
    }
}

fn parse_card_link(target: &str, label: Option<String>) -> Option<LinkKind> {
    let (channel, card_target) = target.split_once('/')?;
    if card_target.contains('/') {
        return None;
    }
    validate_channel_name(channel).ok()?;

    let (card_id, line_number) = if let Some(caps) = CARD_LINE_RE.captures(card_target) {
        let card_id = caps[1].to_string();
        let line_number: u64 = caps[2].parse().ok()?;
        (card_id, Some(line_number))
    } else {
        (card_target.to_string(), None)
    };
    validate_card_id(&card_id).ok()?;

    Some(LinkKind::Card {
        channel: channel.to_string(),
        card_id,
        line_number,
        label,
    })
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
    fn test_card_link() {
        let links = extract_links("open <#general/20260520-035646-7cf>");
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].kind,
            LinkKind::Card {
                channel: "general".into(),
                card_id: "20260520-035646-7cf".into(),
                line_number: None,
                label: None,
            }
        );
        assert_eq!(links[0].raw, "<#general/20260520-035646-7cf>");
    }

    #[test]
    fn test_card_discussion_line_link() {
        let links = extract_links("see <#general/20260520-035646-7cf:L000004>");
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].kind,
            LinkKind::Card {
                channel: "general".into(),
                card_id: "20260520-035646-7cf".into(),
                line_number: Some(4),
                label: None,
            }
        );
    }

    #[test]
    fn test_card_link_with_label() {
        let links = extract_links("see <#general/20260520-035646-7cf:L000004|Token rotation>");
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].kind,
            LinkKind::Card {
                channel: "general".into(),
                card_id: "20260520-035646-7cf".into(),
                line_number: Some(4),
                label: Some("Token rotation".into()),
            }
        );
    }

    #[test]
    fn test_invalid_card_id_ignored() {
        let links = extract_links("<#general/not_a_card>");
        assert!(links.is_empty());
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

    #[test]
    fn test_quick_session_bare_ref() {
        let links = extract_links("check session:qs-01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert_eq!(links.len(), 1);
        match &links[0].kind {
            LinkKind::QuickSession {
                session_id,
                line_number,
            } => {
                assert_eq!(session_id, "qs-01ARZ3NDEKTSV4RRFFQ69G5FAV");
                assert_eq!(*line_number, None);
            }
            _ => panic!("expected QuickSession"),
        }
        assert_eq!(links[0].raw, "session:qs-01ARZ3NDEKTSV4RRFFQ69G5FAV");
    }

    #[test]
    fn test_quick_session_with_line_ref() {
        let links = extract_links("see session:qs-01ARZ3NDEKTSV4RRFFQ69G5FAV:L000042");
        assert_eq!(links.len(), 1);
        match &links[0].kind {
            LinkKind::QuickSession {
                session_id,
                line_number,
            } => {
                assert_eq!(session_id, "qs-01ARZ3NDEKTSV4RRFFQ69G5FAV");
                assert_eq!(*line_number, Some(42));
            }
            _ => panic!("expected QuickSession"),
        }
        assert_eq!(
            links[0].raw,
            "session:qs-01ARZ3NDEKTSV4RRFFQ69G5FAV:L000042"
        );
    }

    #[test]
    fn test_quick_session_mixed_with_channel_link() {
        let links = extract_links("<#general> and session:qs-01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert_eq!(links.len(), 2);
        match &links[0].kind {
            LinkKind::Channel { name } => assert_eq!(name, "general"),
            _ => panic!("expected Channel"),
        }
        match &links[1].kind {
            LinkKind::QuickSession { session_id, .. } => {
                assert_eq!(session_id, "qs-01ARZ3NDEKTSV4RRFFQ69G5FAV");
            }
            _ => panic!("expected QuickSession"),
        }
    }

    #[test]
    fn test_quick_session_invalid_prefix_ignored() {
        // "x-session:" with word boundary should not match
        let links = extract_links("x-session:qs-01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert!(links.is_empty());
    }

    #[test]
    fn test_quick_session_invalid_id_ignored() {
        // Not a valid ULID (too short, contains I)
        let links = extract_links("session:qs-abc");
        assert!(links.is_empty());
    }

    #[test]
    fn test_multiple_quick_sessions() {
        let links = extract_links(
            "session:qs-01ARZ3NDEKTSV4RRFFQ69G5FAV and session:qs-01ARZ3NDEKTSV4RRFFQ69G5FBW",
        );
        assert_eq!(links.len(), 2);
    }
}
