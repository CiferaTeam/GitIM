use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{RevisionId, SkillError, SkillSlug};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillReference {
    pub slug: SkillSlug,
    pub revision: Option<RevisionId>,
}

impl SkillReference {
    pub fn pinned(slug: SkillSlug, revision: RevisionId) -> Self {
        Self {
            slug,
            revision: Some(revision),
        }
    }
}

impl fmt::Display for SkillReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "skill:{}", self.slug)?;
        if let Some(revision) = &self.revision {
            write!(formatter, "@{revision}")?;
        }
        Ok(())
    }
}

impl Serialize for SkillReference {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for SkillReference {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        parse_skill_reference(&value).map_err(serde::de::Error::custom)
    }
}

pub fn parse_skill_reference(value: &str) -> Result<SkillReference, SkillError> {
    let body = value
        .strip_prefix("skill:")
        .ok_or(SkillError::InvalidReference)?;
    parse_body(body)
}

pub fn parse_skill_reference_or_shorthand(value: &str) -> Result<SkillReference, SkillError> {
    match value.strip_prefix("skill:") {
        Some(body) => parse_body(body),
        None => parse_body(value),
    }
}

fn parse_body(body: &str) -> Result<SkillReference, SkillError> {
    let (slug, revision) = match body.split_once('@') {
        Some((slug, revision)) if !revision.is_empty() && !revision.contains('@') => {
            (SkillSlug::new(slug)?, Some(RevisionId::new(revision)?))
        }
        Some(_) => return Err(SkillError::InvalidReference),
        None => (SkillSlug::new(body)?, None),
    };
    Ok(SkillReference { slug, revision })
}
