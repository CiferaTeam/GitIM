use std::str::FromStr;

use gitim_core::types::{
    AssetRef, AssetRefError, ASSET_REF_VERSION, MAX_ASSETS_PER_MESSAGE, MAX_ASSET_BYTES,
    MAX_ASSET_FILENAME_BYTES, MAX_ASSET_MEDIA_TYPE_BYTES, MAX_ASSET_REF_BYTES,
    MAX_ASSET_REQUEST_BYTES,
};
use serde::Deserialize;

const ORIGIN: &str = "3c6a295e-744a-41dc-ba60-5c21bb94e5a2";
const SHA256: &str = "8f2c4d7d7e931a62c18f6f24c8e388d72524d4c4cd6f88e9538f7d4a66c72a88";

#[derive(Debug, Deserialize)]
struct AssetRefFixture {
    valid: Vec<ValidAssetRefFixture>,
    invalid: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ValidAssetRefFixture {
    raw: String,
    name: String,
    media_type: String,
    size: u64,
    width: Option<u32>,
    height: Option<u32>,
}

fn fixture() -> Result<AssetRefFixture, serde_json::Error> {
    serde_json::from_str(include_str!(
        "../../../testdata/protocol/asset_refs_v1.json"
    ))
}

fn valid_asset() -> AssetRef {
    AssetRef {
        version: ASSET_REF_VERSION,
        origin_runtime_id: ORIGIN.to_string(),
        sha256: SHA256.to_string(),
        name: "asset.txt".to_string(),
        media_type: "text/plain".to_string(),
        size: 42,
        width: None,
        height: None,
    }
}

fn valid_asset_with_rendered_len(target: usize) -> Option<AssetRef> {
    let mut base = valid_asset();
    base.name.clear();
    base.media_type = "a/".to_string();
    let fixed_len = base.to_string().len();
    let max_subtype_bytes = MAX_ASSET_MEDIA_TYPE_BYTES - 2;

    for escaped_filename_bytes in 0..=MAX_ASSET_FILENAME_BYTES {
        let plain_filename_bytes = MAX_ASSET_FILENAME_BYTES - escaped_filename_bytes;
        let rendered_filename_len = plain_filename_bytes + escaped_filename_bytes * 3;

        for escaped_subtype_bytes in 0..=max_subtype_bytes {
            let occupied = fixed_len + rendered_filename_len + escaped_subtype_bytes * 3;
            let Some(plain_subtype_bytes) = target.checked_sub(occupied) else {
                continue;
            };
            let subtype_bytes = escaped_subtype_bytes + plain_subtype_bytes;
            if subtype_bytes == 0 || subtype_bytes > max_subtype_bytes {
                continue;
            }

            let mut asset = valid_asset();
            asset.name = format!(
                "{}{}",
                " ".repeat(escaped_filename_bytes),
                "a".repeat(plain_filename_bytes)
            );
            asset.media_type = format!(
                "a/{}{}",
                "!".repeat(escaped_subtype_bytes),
                "a".repeat(plain_subtype_bytes)
            );
            return Some(asset);
        }
    }
    None
}

#[test]
fn shared_fixture_round_trips_canonical_refs() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = fixture()?;

    for expected in fixture.valid {
        let parsed = AssetRef::from_str(&expected.raw)?;
        assert_eq!(parsed.to_string(), expected.raw);
        assert_eq!(parsed.name, expected.name);
        assert_eq!(parsed.media_type, expected.media_type);
        assert_eq!(parsed.size, expected.size);
        assert_eq!(parsed.width, expected.width);
        assert_eq!(parsed.height, expected.height);
    }

    for raw in fixture.invalid {
        assert!(
            AssetRef::from_str(&raw).is_err(),
            "invalid ref was accepted"
        );
    }
    Ok(())
}

#[test]
fn encoded_reference_limit_is_authoritative() {
    let at_limit = valid_asset_with_rendered_len(MAX_ASSET_REF_BYTES);
    assert!(
        at_limit.is_some(),
        "1024-byte reference must be representable"
    );
    let at_limit = at_limit.unwrap_or_else(valid_asset);
    assert_eq!(at_limit.name.len(), MAX_ASSET_FILENAME_BYTES);
    assert!(at_limit.media_type.len() <= MAX_ASSET_MEDIA_TYPE_BYTES);
    assert_eq!(at_limit.to_string().len(), MAX_ASSET_REF_BYTES);
    assert_eq!(at_limit.validate(), Ok(()));

    let over_limit = valid_asset_with_rendered_len(MAX_ASSET_REF_BYTES + 1);
    assert!(
        over_limit.is_some(),
        "1025-byte reference must be representable"
    );
    let over_limit = over_limit.unwrap_or_else(valid_asset);
    assert_eq!(over_limit.name.len(), MAX_ASSET_FILENAME_BYTES);
    assert!(over_limit.media_type.len() <= MAX_ASSET_MEDIA_TYPE_BYTES);
    assert_eq!(over_limit.to_string().len(), MAX_ASSET_REF_BYTES + 1);
    assert_eq!(over_limit.validate(), Err(AssetRefError::ReferenceTooLong));
}

#[test]
fn parser_rejects_oversized_input_before_parsing() {
    let raw = format!(
        "<^v1/{ORIGIN}/sha256:{SHA256}?name={}&type=text%2Fplain&size=not-a-number>",
        "a".repeat(MAX_ASSET_REF_BYTES)
    );
    assert!(raw.len() > MAX_ASSET_REF_BYTES);
    assert_eq!(
        AssetRef::from_str(&raw),
        Err(AssetRefError::ReferenceTooLong)
    );
}

#[test]
fn protocol_limits_are_stable() {
    assert_eq!(ASSET_REF_VERSION, 1);
    assert_eq!(MAX_ASSET_BYTES, 50 * 1024 * 1024);
    assert_eq!(MAX_ASSETS_PER_MESSAGE, 10);
    assert_eq!(MAX_ASSET_REQUEST_BYTES, 200 * 1024 * 1024);
    assert_eq!(MAX_ASSET_FILENAME_BYTES, 255);
    assert_eq!(MAX_ASSET_MEDIA_TYPE_BYTES, 127);
    assert_eq!(MAX_ASSET_REF_BYTES, 1024);
}

#[test]
fn validation_accepts_field_limits() -> Result<(), AssetRefError> {
    let mut asset = valid_asset();
    asset.name = "a".repeat(MAX_ASSET_FILENAME_BYTES);
    asset.media_type = format!("a/{}", "b".repeat(MAX_ASSET_MEDIA_TYPE_BYTES - 2));
    asset.size = MAX_ASSET_BYTES;
    asset.width = Some(u32::MAX);
    asset.height = Some(u32::MAX);

    asset.validate()
}

#[test]
fn validation_rejects_unsupported_version() {
    let mut asset = valid_asset();
    asset.version = ASSET_REF_VERSION + 1;
    assert_eq!(asset.validate(), Err(AssetRefError::UnsupportedVersion));
}

#[test]
fn validation_rejects_noncanonical_origin_runtime_id() {
    for origin in [
        "3C6A295E-744A-41DC-BA60-5C21BB94E5A2",
        "3c6a295e744a41dcba605c21bb94e5a2",
        "3c6a295e-744a-41dc-ba60-5c21bb94e5az",
    ] {
        let mut asset = valid_asset();
        asset.origin_runtime_id = origin.to_string();
        assert_eq!(asset.validate(), Err(AssetRefError::InvalidOriginRuntimeId));
    }
}

#[test]
fn validation_rejects_noncanonical_sha256() {
    for sha256 in [
        "8F2C4D7D7E931A62C18F6F24C8E388D72524D4C4CD6F88E9538F7D4A66C72A88",
        "8f2c4d7d7e931a62c18f6f24c8e388d72524d4c4cd6f88e9538f7d4a66c72a8",
        "8f2c4d7d7e931a62c18f6f24c8e388d72524d4c4cd6f88e9538f7d4a66c72a8z",
    ] {
        let mut asset = valid_asset();
        asset.sha256 = sha256.to_string();
        assert_eq!(asset.validate(), Err(AssetRefError::InvalidSha256));
    }
}

#[test]
fn validation_rejects_invalid_filenames() {
    let cases = [
        ("", AssetRefError::EmptyFilename),
        ("a/b", AssetRefError::UnsafeFilename),
        (r"a\b", AssetRefError::UnsafeFilename),
        ("a\nb", AssetRefError::UnsafeFilename),
    ];
    for (name, expected) in cases {
        let mut asset = valid_asset();
        asset.name = name.to_string();
        assert_eq!(asset.validate(), Err(expected));
    }

    let mut asset = valid_asset();
    asset.name = "a".repeat(MAX_ASSET_FILENAME_BYTES + 1);
    assert_eq!(asset.validate(), Err(AssetRefError::FilenameTooLong));
}

#[test]
fn validation_rejects_invalid_media_types() {
    let cases = [
        ("", AssetRefError::EmptyMediaType),
        ("Image/png", AssetRefError::InvalidMediaType),
        ("image", AssetRefError::InvalidMediaType),
        ("image/", AssetRefError::InvalidMediaType),
        ("image/png; charset=utf-8", AssetRefError::InvalidMediaType),
        ("image/p ng", AssetRefError::InvalidMediaType),
    ];
    for (media_type, expected) in cases {
        let mut asset = valid_asset();
        asset.media_type = media_type.to_string();
        assert_eq!(asset.validate(), Err(expected));
    }

    let mut asset = valid_asset();
    asset.media_type = format!("a/{}", "b".repeat(MAX_ASSET_MEDIA_TYPE_BYTES - 1));
    assert_eq!(asset.validate(), Err(AssetRefError::MediaTypeTooLong));
}

#[test]
fn validation_rejects_oversized_assets() {
    let mut asset = valid_asset();
    asset.size = MAX_ASSET_BYTES + 1;
    assert_eq!(asset.validate(), Err(AssetRefError::AssetTooLarge));
}

#[test]
fn validation_requires_positive_paired_dimensions() {
    for (width, height, expected) in [
        (Some(1), None, AssetRefError::IncompleteDimensions),
        (None, Some(1), AssetRefError::IncompleteDimensions),
        (Some(0), Some(1), AssetRefError::InvalidDimensions),
        (Some(1), Some(0), AssetRefError::InvalidDimensions),
    ] {
        let mut asset = valid_asset();
        asset.width = width;
        asset.height = height;
        assert_eq!(asset.validate(), Err(expected));
    }
}

#[test]
fn display_percent_encodes_every_non_unreserved_byte() {
    let mut asset = valid_asset();
    asset.name = "报告 #1~.txt".to_string();
    asset.media_type = "application/octet-stream".to_string();

    let rendered = asset.to_string();
    assert!(rendered.contains("name=%E6%8A%A5%E5%91%8A%20%231~.txt"));
    assert!(rendered.contains("type=application%2Foctet-stream"));
}

#[test]
fn parser_rejects_malformed_and_noncanonical_inputs() {
    let prefix = format!("<^v1/{ORIGIN}/sha256:{SHA256}?");
    let cases = [
        format!("{prefix}name=a&type=image%2Fpng&size=18446744073709551616>"),
        format!("{prefix}name=a&type=image%2Fpng&size=1&unknown=x>"),
        format!("{prefix}name=a&name=b&type=image%2Fpng&size=1>"),
        format!("{prefix}name=%ZZ&type=image%2Fpng&size=1>"),
        format!("{prefix}name=%FF&type=image%2Fpng&size=1>"),
        format!("{prefix}name=%61&type=image%2Fpng&size=1>"),
        format!("{prefix}name=a&type=image%2fpng&size=1>"),
        format!("{prefix}name=a&type=image%2Fpng&size=1&width=0&height=1>"),
        format!("{prefix}name=a&type=image%2Fpng&size=1&height=1&width=1>"),
        format!("{prefix}name=a&type=image%2Fpng&size=1&width=1>"),
    ];

    for raw in cases {
        assert!(AssetRef::from_str(&raw).is_err(), "input was accepted");
    }
}

#[test]
fn parser_reports_noncanonical_equivalent_encoding() {
    let raw = format!("<^v1/{ORIGIN}/sha256:{SHA256}?name=%61&type=text%2Fplain&size=42>");
    assert_eq!(AssetRef::from_str(&raw), Err(AssetRefError::NonCanonical));
}

#[test]
fn serialization_omits_absent_dimensions() -> Result<(), serde_json::Error> {
    let value = serde_json::to_value(valid_asset())?;
    assert_eq!(value["version"], ASSET_REF_VERSION);
    assert!(value.get("width").is_none());
    assert!(value.get("height").is_none());
    Ok(())
}

#[test]
fn asset_link_serializes_additively() -> Result<(), serde_json::Error> {
    let raw = valid_asset().to_string();
    let links = gitim_core::link::extract_links(&raw);

    assert_eq!(links.len(), 1);
    let value = serde_json::to_value(&links[0])?;
    assert_eq!(value["kind"]["kind"], "asset");
    assert_eq!(value["kind"]["asset"]["sha256"], SHA256);
    assert_eq!(value["raw"], raw);
    Ok(())
}

#[test]
fn invalid_asset_syntax_is_not_a_typed_link() {
    assert!(gitim_core::link::extract_links("<^v1/bad>").is_empty());
}

#[test]
fn asset_links_coexist_with_existing_links_in_occurrence_order() -> Result<(), serde_json::Error> {
    let raw = valid_asset().to_string();
    let body = format!("<#general> {raw} <~alice> <!https://example.com|Example>");
    let links = gitim_core::link::extract_links(&body);

    assert_eq!(links.len(), 4);
    let value = serde_json::to_value(links)?;
    assert_eq!(
        value,
        serde_json::json!([
            {
                "kind": { "kind": "channel", "name": "general" },
                "raw": "<#general>"
            },
            {
                "kind": {
                    "kind": "asset",
                    "asset": {
                        "version": ASSET_REF_VERSION,
                        "origin_runtime_id": ORIGIN,
                        "sha256": SHA256,
                        "name": "asset.txt",
                        "media_type": "text/plain",
                        "size": 42
                    }
                },
                "raw": raw
            },
            {
                "kind": { "kind": "user_profile", "handler": "alice" },
                "raw": "<~alice>"
            },
            {
                "kind": {
                    "kind": "softlink",
                    "url": "https://example.com",
                    "title": "Example"
                },
                "raw": "<!https://example.com|Example>"
            }
        ])
    );
    Ok(())
}

#[test]
fn malformed_asset_opener_does_not_swallow_channel_link() -> Result<(), serde_json::Error> {
    let links = gitim_core::link::extract_links("<^not-an-asset <#general>>");

    assert_eq!(links.len(), 1);
    assert_eq!(
        serde_json::to_value(&links[0])?,
        serde_json::json!({
            "kind": { "kind": "channel", "name": "general" },
            "raw": "<#general>"
        })
    );
    Ok(())
}

#[test]
fn malformed_asset_opener_does_not_swallow_user_link() -> Result<(), serde_json::Error> {
    let links = gitim_core::link::extract_links("<^not-an-asset <~alice>>");

    assert_eq!(links.len(), 1);
    assert_eq!(
        serde_json::to_value(&links[0])?,
        serde_json::json!({
            "kind": { "kind": "user_profile", "handler": "alice" },
            "raw": "<~alice>"
        })
    );
    Ok(())
}

#[test]
fn malformed_asset_opener_does_not_swallow_softlink() -> Result<(), serde_json::Error> {
    let links = gitim_core::link::extract_links("<^not-an-asset <!https://example.com>>");

    assert_eq!(links.len(), 1);
    assert_eq!(
        serde_json::to_value(&links[0])?,
        serde_json::json!({
            "kind": {
                "kind": "softlink",
                "url": "https://example.com",
                "title": null
            },
            "raw": "<!https://example.com>"
        })
    );
    Ok(())
}

#[test]
fn oversized_asset_candidate_is_ignored_without_swallowing_a_legacy_link(
) -> Result<(), serde_json::Error> {
    let body = format!("<^{} <#general>>", "a".repeat(MAX_ASSET_REF_BYTES + 1));
    let links = gitim_core::link::extract_links(&body);

    assert_eq!(links.len(), 1);
    assert_eq!(
        serde_json::to_value(&links[0])?,
        serde_json::json!({
            "kind": { "kind": "channel", "name": "general" },
            "raw": "<#general>"
        })
    );
    Ok(())
}
