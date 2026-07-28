use std::fmt;
use std::str::FromStr;

use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, CONTROLS};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const ASSET_REF_VERSION: u8 = 1;
pub const MAX_ASSET_BYTES: u64 = 50 * 1024 * 1024;
pub const MAX_ASSETS_PER_MESSAGE: usize = 10;
pub const MAX_ASSET_REQUEST_BYTES: u64 = 200 * 1024 * 1024;
pub const MAX_ASSET_FILENAME_BYTES: usize = 255;
pub const MAX_ASSET_MEDIA_TYPE_BYTES: usize = 127;
pub const MAX_ASSET_REF_BYTES: usize = 1024;

/// Percent-encode set for canonical asset reference components (`name` and
/// `type` query values inside the `<^...>` reference). Also used for the
/// `name` query value of `/assets/resolve/...` URLs so the runtime CLI emits
/// byte-identical encodings; single definition lives here in gitim-core.
///
/// Note: the Content-Disposition `filename*` parameter uses a different set
/// (RFC 5987 attr-char), defined where it is served in gitim-runtime.
pub const ASSET_COMPONENT_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'!')
    .add(b'"')
    .add(b'#')
    .add(b'$')
    .add(b'%')
    .add(b'&')
    .add(b'\'')
    .add(b'(')
    .add(b')')
    .add(b'*')
    .add(b'+')
    .add(b',')
    .add(b'/')
    .add(b':')
    .add(b';')
    .add(b'<')
    .add(b'=')
    .add(b'>')
    .add(b'?')
    .add(b'@')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AssetRefError {
    #[error("asset reference has invalid syntax")]
    InvalidFormat,
    #[error("asset reference version is unsupported")]
    UnsupportedVersion,
    #[error("origin runtime id is not a canonical UUID")]
    InvalidOriginRuntimeId,
    #[error("asset SHA-256 is not canonical lowercase hexadecimal")]
    InvalidSha256,
    #[error("asset reference contains invalid percent encoding")]
    InvalidPercentEncoding,
    #[error("asset reference contains invalid UTF-8")]
    InvalidUtf8,
    #[error("asset reference query is invalid")]
    InvalidQuery,
    #[error("asset size is invalid")]
    InvalidSize,
    #[error("asset filename is empty")]
    EmptyFilename,
    #[error("asset filename exceeds its byte limit")]
    FilenameTooLong,
    #[error("asset filename contains an unsafe character")]
    UnsafeFilename,
    #[error("asset media type is empty")]
    EmptyMediaType,
    #[error("asset media type exceeds its byte limit")]
    MediaTypeTooLong,
    #[error("asset media type is invalid")]
    InvalidMediaType,
    #[error("asset exceeds its byte limit")]
    AssetTooLarge,
    #[error("asset dimensions must include both width and height")]
    IncompleteDimensions,
    #[error("asset dimensions must be positive 32-bit integers")]
    InvalidDimensions,
    #[error("encoded asset reference exceeds its byte limit")]
    ReferenceTooLong,
    #[error("asset reference is not canonical")]
    NonCanonical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetRef {
    pub version: u8,
    pub origin_runtime_id: String,
    pub sha256: String,
    pub name: String,
    pub media_type: String,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

impl AssetRef {
    pub fn validate(&self) -> Result<(), AssetRefError> {
        if self.version != ASSET_REF_VERSION {
            return Err(AssetRefError::UnsupportedVersion);
        }
        if !is_canonical_uuid(&self.origin_runtime_id) {
            return Err(AssetRefError::InvalidOriginRuntimeId);
        }
        if self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(AssetRefError::InvalidSha256);
        }
        validate_filename(&self.name)?;
        validate_media_type(&self.media_type)?;
        if self.size > MAX_ASSET_BYTES {
            return Err(AssetRefError::AssetTooLarge);
        }
        match (self.width, self.height) {
            (None, None) => {}
            (Some(width), Some(height)) if width > 0 && height > 0 => {}
            (Some(_), Some(_)) => return Err(AssetRefError::InvalidDimensions),
            _ => return Err(AssetRefError::IncompleteDimensions),
        }
        if self.to_string().len() > MAX_ASSET_REF_BYTES {
            return Err(AssetRefError::ReferenceTooLong);
        }
        Ok(())
    }
}

impl FromStr for AssetRef {
    type Err = AssetRefError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        if raw.len() > MAX_ASSET_REF_BYTES {
            return Err(AssetRefError::ReferenceTooLong);
        }
        let inner = raw
            .strip_prefix("<^v")
            .and_then(|value| value.strip_suffix('>'))
            .ok_or(AssetRefError::InvalidFormat)?;
        let (path, query) = inner.split_once('?').ok_or(AssetRefError::InvalidFormat)?;

        let mut path_parts = path.split('/');
        let version = path_parts
            .next()
            .ok_or(AssetRefError::InvalidFormat)?
            .parse::<u8>()
            .map_err(|_| AssetRefError::InvalidFormat)?;
        let origin_runtime_id = path_parts
            .next()
            .ok_or(AssetRefError::InvalidFormat)?
            .to_string();
        let sha256 = path_parts
            .next()
            .and_then(|value| value.strip_prefix("sha256:"))
            .ok_or(AssetRefError::InvalidFormat)?
            .to_string();
        if path_parts.next().is_some() {
            return Err(AssetRefError::InvalidFormat);
        }

        let fields: Vec<&str> = query.split('&').collect();
        if fields.len() != 3 && fields.len() != 5 {
            return Err(AssetRefError::InvalidQuery);
        }
        let encoded_name = query_value(fields[0], "name=")?;
        let encoded_media_type = query_value(fields[1], "type=")?;
        let size = query_value(fields[2], "size=")?
            .parse::<u64>()
            .map_err(|_| AssetRefError::InvalidSize)?;
        let (width, height) = if fields.len() == 5 {
            let width = query_value(fields[3], "width=")?
                .parse::<u32>()
                .map_err(|_| AssetRefError::InvalidDimensions)?;
            let height = query_value(fields[4], "height=")?
                .parse::<u32>()
                .map_err(|_| AssetRefError::InvalidDimensions)?;
            (Some(width), Some(height))
        } else {
            (None, None)
        };

        let parsed = Self {
            version,
            origin_runtime_id,
            sha256,
            name: decode_component(encoded_name)?,
            media_type: decode_component(encoded_media_type)?,
            size,
            width,
            height,
        };
        parsed.validate()?;
        if parsed.to_string() != raw {
            return Err(AssetRefError::NonCanonical);
        }
        Ok(parsed)
    }
}

impl fmt::Display for AssetRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "<^v{}/{}/sha256:{}?name={}&type={}&size={}",
            self.version,
            self.origin_runtime_id,
            self.sha256,
            utf8_percent_encode(&self.name, ASSET_COMPONENT_ENCODE_SET),
            utf8_percent_encode(&self.media_type, ASSET_COMPONENT_ENCODE_SET),
            self.size
        )?;
        if let Some(width) = self.width {
            write!(formatter, "&width={width}")?;
        }
        if let Some(height) = self.height {
            write!(formatter, "&height={height}")?;
        }
        formatter.write_str(">")
    }
}

fn query_value<'a>(field: &'a str, prefix: &str) -> Result<&'a str, AssetRefError> {
    field
        .strip_prefix(prefix)
        .ok_or(AssetRefError::InvalidQuery)
}

fn decode_component(value: &str) -> Result<String, AssetRefError> {
    validate_percent_encoding(value)?;
    percent_decode_str(value)
        .decode_utf8()
        .map(|decoded| decoded.into_owned())
        .map_err(|_| AssetRefError::InvalidUtf8)
}

fn validate_percent_encoding(value: &str) -> Result<(), AssetRefError> {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit()
            {
                return Err(AssetRefError::InvalidPercentEncoding);
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    Ok(())
}

fn is_canonical_uuid(value: &str) -> bool {
    if value.len() != 36 {
        return false;
    }
    value.bytes().enumerate().all(|(index, byte)| {
        if matches!(index, 8 | 13 | 18 | 23) {
            byte == b'-'
        } else {
            byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')
        }
    })
}

fn validate_filename(name: &str) -> Result<(), AssetRefError> {
    if name.is_empty() {
        return Err(AssetRefError::EmptyFilename);
    }
    if name.len() > MAX_ASSET_FILENAME_BYTES {
        return Err(AssetRefError::FilenameTooLong);
    }
    if name
        .chars()
        .any(|character| character.is_control() || matches!(character, '/' | '\\'))
    {
        return Err(AssetRefError::UnsafeFilename);
    }
    Ok(())
}

fn validate_media_type(media_type: &str) -> Result<(), AssetRefError> {
    if media_type.is_empty() {
        return Err(AssetRefError::EmptyMediaType);
    }
    if media_type.len() > MAX_ASSET_MEDIA_TYPE_BYTES {
        return Err(AssetRefError::MediaTypeTooLong);
    }
    let Some((type_name, subtype)) = media_type.split_once('/') else {
        return Err(AssetRefError::InvalidMediaType);
    };
    if type_name.is_empty()
        || subtype.is_empty()
        || !type_name.bytes().all(is_media_type_token_byte)
        || !subtype.bytes().all(is_media_type_token_byte)
    {
        return Err(AssetRefError::InvalidMediaType);
    }
    Ok(())
}

fn is_media_type_token_byte(byte: u8) -> bool {
    matches!(
        byte,
        b'a'..=b'z'
            | b'0'..=b'9'
            | b'!'
            | b'#'
            | b'$'
            | b'%'
            | b'&'
            | b'\''
            | b'*'
            | b'+'
            | b'-'
            | b'.'
            | b'^'
            | b'_'
            | b'`'
            | b'|'
            | b'~'
    )
}
