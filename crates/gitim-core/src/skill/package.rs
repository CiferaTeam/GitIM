use sha2::{Digest, Sha256};

use super::{
    portable_path::valid_portable_relative_paths, ResourceDescriptor, SkillError, SkillSlug,
};

pub const MAX_SKILL_MD_BYTES: usize = 64 * 1024;
pub const MAX_PACKAGE_FILE_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_PACKAGE_FILES: usize = 256;
pub const MAX_PACKAGE_BYTES: usize = 10 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackageEntryKind {
    Regular,
    Directory,
    Symlink,
    BlockDevice,
    CharacterDevice,
    Fifo,
    Socket,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageEntry {
    pub path: String,
    pub bytes: Vec<u8>,
    pub kind: PackageEntryKind,
}

impl PackageEntry {
    pub fn new(path: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self::with_kind(path, bytes, PackageEntryKind::Regular)
    }

    pub fn with_kind(path: impl Into<String>, bytes: Vec<u8>, kind: PackageEntryKind) -> Self {
        Self {
            path: path.into(),
            bytes,
            kind,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedPackage {
    pub entries: Vec<PackageEntry>,
    pub skill_markdown: Vec<u8>,
    pub content_sha256: String,
    pub resources: Vec<ResourceDescriptor>,
}

pub fn validate_package_entries(
    slug: &SkillSlug,
    entries: Vec<PackageEntry>,
) -> Result<ValidatedPackage, SkillError> {
    let entries = validate_entries(entries)?;
    let skill_markdown = entries
        .iter()
        .find(|entry| entry.path == "SKILL.md")
        .map(|entry| entry.bytes.clone())
        .ok_or(SkillError::InvalidPackage)?;

    validate_skill_markdown(slug, &skill_markdown)?;
    let content_sha256 = hash_entries(&entries);
    let resources = entries
        .iter()
        .filter(|entry| entry.path != "SKILL.md")
        .map(|entry| ResourceDescriptor {
            path: entry.path.clone(),
            byte_size: entry.bytes.len() as u64,
            media_type: media_type_for_path(&entry.path).to_owned(),
            text: std::str::from_utf8(&entry.bytes).is_ok(),
        })
        .collect();

    Ok(ValidatedPackage {
        entries,
        skill_markdown,
        content_sha256,
        resources,
    })
}

pub fn canonical_package_sha256(entries: &[PackageEntry]) -> Result<String, SkillError> {
    let entries = validate_entries(entries.to_vec())?;
    Ok(hash_entries(&entries))
}

pub fn media_type_for_path(path: &str) -> &'static str {
    let extension = path
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase());

    match extension.as_deref() {
        Some("md") => "text/markdown",
        Some("txt") => "text/plain",
        Some("json") => "application/json",
        Some("yaml" | "yml") => "application/yaml",
        Some("toml") => "application/toml",
        Some("sh") => "text/x-shellscript",
        Some("py") => "text/x-python",
        Some("js" | "jsx") => "text/javascript",
        Some("ts" | "tsx") => "text/typescript",
        Some("rs") => "text/x-rust",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("pdf") => "application/pdf",
        _ => "application/octet-stream",
    }
}

pub fn truncate_utf8_bytes(value: &str, max_bytes: usize) -> &str {
    let mut end = value.len().min(max_bytes);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn validate_entries(mut entries: Vec<PackageEntry>) -> Result<Vec<PackageEntry>, SkillError> {
    if entries.len() > MAX_PACKAGE_FILES {
        return Err(SkillError::PackageTooLarge);
    }

    let mut total_bytes = 0_usize;
    if !valid_portable_relative_paths(entries.iter().map(|entry| entry.path.as_str())) {
        return Err(SkillError::InvalidPackage);
    }
    for entry in &entries {
        if entry.kind != PackageEntryKind::Regular {
            return Err(SkillError::InvalidPackage);
        }
        if entry.bytes.len() > MAX_PACKAGE_FILE_BYTES {
            return Err(SkillError::PackageTooLarge);
        }
        total_bytes = total_bytes
            .checked_add(entry.bytes.len())
            .ok_or(SkillError::PackageTooLarge)?;
        if total_bytes > MAX_PACKAGE_BYTES {
            return Err(SkillError::PackageTooLarge);
        }
    }

    entries.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(entries)
}

fn validate_skill_markdown(slug: &SkillSlug, bytes: &[u8]) -> Result<(), SkillError> {
    if bytes.len() > MAX_SKILL_MD_BYTES {
        return Err(SkillError::PackageTooLarge);
    }

    let markdown = std::str::from_utf8(bytes).map_err(|_| SkillError::InvalidPackage)?;
    let frontmatter = first_frontmatter(markdown).ok_or(SkillError::InvalidPackage)?;
    let value: serde_yaml::Value =
        serde_yaml::from_str(frontmatter).map_err(|_| SkillError::InvalidPackage)?;
    let mapping = value.as_mapping().ok_or(SkillError::InvalidPackage)?;
    let name = mapping
        .get(serde_yaml::Value::String("name".to_owned()))
        .and_then(serde_yaml::Value::as_str)
        .ok_or(SkillError::InvalidPackage)?;
    let description = mapping
        .get(serde_yaml::Value::String("description".to_owned()))
        .and_then(serde_yaml::Value::as_str)
        .ok_or(SkillError::InvalidPackage)?;

    if name != slug.as_str() || description.is_empty() || description.chars().count() > 1024 {
        return Err(SkillError::InvalidPackage);
    }

    Ok(())
}

fn first_frontmatter(markdown: &str) -> Option<&str> {
    let body = markdown
        .strip_prefix("---\n")
        .or_else(|| markdown.strip_prefix("---\r\n"))?;
    let mut offset = 0;
    for line in body.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed == "---" {
            return Some(&body[..offset]);
        }
        offset += line.len();
    }
    None
}

fn hash_entries(entries: &[PackageEntry]) -> String {
    let mut hash = Sha256::new();
    for entry in entries {
        hash.update((entry.path.len() as u32).to_be_bytes());
        hash.update(entry.path.as_bytes());
        hash.update((entry.bytes.len() as u64).to_be_bytes());
        hash.update(&entry.bytes);
    }
    format!("{:x}", hash.finalize())
}
