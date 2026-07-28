use super::{AssetError, MAX_INLINE_IMAGE_AXIS, MAX_INLINE_IMAGE_PIXELS};
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
        inline_safe: supported_inline && dimensions.is_some_and(inline_dimensions_safe),
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

fn dimensions(bytes: &[u8], _media_type: &str) -> Option<(u32, u32)> {
    imagesize::blob_size(bytes)
        .ok()
        .and_then(|size| checked_dimensions(size.width, size.height))
}

fn inline_dimensions_safe((width, height): (u32, u32)) -> bool {
    width <= MAX_INLINE_IMAGE_AXIS
        && height <= MAX_INLINE_IMAGE_AXIS
        && u64::from(width)
            .checked_mul(u64::from(height))
            .is_some_and(|pixels| pixels <= MAX_INLINE_IMAGE_PIXELS)
}
