use gitim_core::types::{Link, LinkKind, ThreadEntry};

pub(crate) fn link_to_json(link: &Link) -> serde_json::Value {
    match &link.kind {
        LinkKind::Channel { name } => serde_json::json!({
            "kind": "channel",
            "name": name,
            "raw": link.raw,
        }),
        LinkKind::Message {
            channel,
            line_number,
        } => serde_json::json!({
            "kind": "message",
            "channel": channel,
            "line_number": line_number,
            "raw": link.raw,
        }),
        LinkKind::Card {
            channel,
            card_id,
            line_number,
            label,
        } => serde_json::json!({
            "kind": "card",
            "channel": channel,
            "card_id": card_id,
            "line_number": line_number,
            "label": label,
            "raw": link.raw,
        }),
        LinkKind::UserProfile { handler } => serde_json::json!({
            "kind": "user_profile",
            "handler": handler.as_str(),
            "raw": link.raw,
        }),
        LinkKind::Softlink { url, title } => {
            let mut v = serde_json::json!({
                "kind": "softlink",
                "url": url,
                "raw": link.raw,
            });
            if let Some(t) = title {
                v["title"] = serde_json::json!(t);
            }
            v
        }
        LinkKind::Asset { asset } => serde_json::json!({
            "kind": "asset",
            "asset": asset,
            "raw": link.raw,
        }),
    }
}

pub(crate) fn entry_to_json(entry: &ThreadEntry) -> serde_json::Value {
    match entry {
        ThreadEntry::Message(m) => serde_json::json!({
            "type": "message",
            "line_number": m.line_number,
            "point_to": m.point_to,
            "author": m.author.as_str(),
            "timestamp": m.timestamp,
            "body": m.body,
            "mentions": m.mentions.iter().map(|h| h.as_str()).collect::<Vec<_>>(),
            "links": m.links.iter().map(link_to_json).collect::<Vec<_>>(),
        }),
        ThreadEntry::Event(ev) => serde_json::json!({
            "type": "event",
            "event_type": ev.event_type,
            "line_number": ev.line_number,
            "author": ev.author.as_str(),
            "timestamp": ev.timestamp,
            "meta": ev.meta,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_card_link() {
        let link = Link {
            raw: "<#general/20260520-035646-7cf:L000004>".to_string(),
            kind: LinkKind::Card {
                channel: "general".to_string(),
                card_id: "20260520-035646-7cf".to_string(),
                line_number: Some(4),
                label: None,
            },
        };

        let json = link_to_json(&link);

        assert_eq!(json["kind"], "card");
        assert_eq!(json["channel"], "general");
        assert_eq!(json["card_id"], "20260520-035646-7cf");
        assert_eq!(json["line_number"], 4);
        assert_eq!(json["label"], serde_json::Value::Null);
        assert_eq!(json["raw"], "<#general/20260520-035646-7cf:L000004>");
    }

    #[test]
    fn serializes_asset_link() -> Result<(), gitim_core::types::AssetRefError> {
        let raw = "<^v1/3c6a295e-744a-41dc-ba60-5c21bb94e5a2/sha256:8f2c4d7d7e931a62c18f6f24c8e388d72524d4c4cd6f88e9538f7d4a66c72a88?name=asset.txt&type=text%2Fplain&size=42&width=640&height=480>";
        let link = Link {
            raw: raw.to_string(),
            kind: LinkKind::Asset {
                asset: raw.parse()?,
            },
        };

        let json = link_to_json(&link);

        assert_eq!(
            json,
            serde_json::json!({
                "kind": "asset",
                "asset": {
                    "version": 1,
                    "origin_runtime_id": "3c6a295e-744a-41dc-ba60-5c21bb94e5a2",
                    "sha256": "8f2c4d7d7e931a62c18f6f24c8e388d72524d4c4cd6f88e9538f7d4a66c72a88",
                    "name": "asset.txt",
                    "media_type": "text/plain",
                    "size": 42,
                    "width": 640,
                    "height": 480
                },
                "raw": raw
            })
        );
        Ok(())
    }
}
