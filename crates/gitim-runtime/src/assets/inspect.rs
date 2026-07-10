use super::AssetError;
use serde::{Deserialize, Serialize};

const MAX_INSPECTION_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetInspection {
    pub media_type: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub inline_safe: bool,
}

pub fn checked_dimensions(width: usize, height: usize) -> Option<(u32, u32)> {
    let width = u32::try_from(width).ok()?;
    let height = u32::try_from(height).ok()?;
    (width > 0 && height > 0).then_some((width, height))
}

pub fn inspect_bytes(bytes: &[u8], _filename: &str) -> Result<AssetInspection, AssetError> {
    let header = &bytes[..bytes.len().min(MAX_INSPECTION_BYTES)];
    let media_type = detect_media_type(header);
    let supported_inline = matches!(
        media_type,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/avif"
    );
    let dimensions = if supported_inline {
        dimensions(header, media_type)
    } else {
        None
    };
    Ok(AssetInspection {
        media_type: media_type.to_string(),
        width: dimensions.map(|value| value.0),
        height: dimensions.map(|value| value.1),
        inline_safe: supported_inline && dimensions.is_some(),
    })
}

fn detect_media_type(bytes: &[u8]) -> &'static str {
    if bytes.is_empty() {
        return mime::APPLICATION_OCTET_STREAM.as_ref();
    }
    if let Some(kind) = infer::get(bytes) {
        return kind.mime_type();
    }

    let text_prefix = &bytes[..bytes.len().min(512)];
    if let Ok(text) = std::str::from_utf8(text_prefix) {
        let normalized = text
            .trim_start_matches(|character: char| character.is_ascii_whitespace())
            .to_ascii_lowercase();
        if normalized.starts_with("<svg")
            || normalized.starts_with("<?xml") && normalized.contains("<svg")
        {
            return "image/svg+xml";
        }
        if normalized.starts_with("<!doctype html") || normalized.starts_with("<html") {
            return "text/html";
        }
    }

    mime::APPLICATION_OCTET_STREAM.as_ref()
}

fn dimensions(bytes: &[u8], media_type: &str) -> Option<(u32, u32)> {
    imagesize::blob_size(bytes)
        .ok()
        .and_then(|size| checked_dimensions(size.width, size.height))
        .or_else(|| {
            (media_type == "image/avif")
                .then(|| avif_dimensions(bytes))
                .flatten()
        })
}

fn avif_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 12 || &bytes[4..8] != b"ftyp" {
        return None;
    }
    let brand = &bytes[8..12];
    if !matches!(brand, b"avif" | b"avis" | b"MA1A" | b"MA1B") {
        return None;
    }

    for tag_offset in 4..bytes.len().saturating_sub(15) {
        if &bytes[tag_offset..tag_offset + 4] != b"ispe" {
            continue;
        }
        let size_offset = tag_offset - 4;
        let box_size = u32::from_be_bytes(bytes[size_offset..tag_offset].try_into().ok()?) as usize;
        let box_end = size_offset.checked_add(box_size)?;
        if box_size < 20 || box_end > bytes.len() {
            continue;
        }
        let width = u32::from_be_bytes(bytes[tag_offset + 8..tag_offset + 12].try_into().ok()?);
        let height = u32::from_be_bytes(bytes[tag_offset + 12..tag_offset + 16].try_into().ok()?);
        if width > 0 && height > 0 {
            return Some((width, height));
        }
    }
    None
}
