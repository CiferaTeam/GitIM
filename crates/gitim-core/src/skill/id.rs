use serde::{Deserialize, Serialize};

use super::SkillError;

const ULID_LENGTH: usize = 26;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SkillSlug(String);

impl SkillSlug {
    pub fn new(value: &str) -> Result<Self, SkillError> {
        if value.is_empty() || value.len() > 64 || value.starts_with('-') {
            return Err(SkillError::InvalidSlug);
        }

        let mut previous_hyphen = false;
        for character in value.chars() {
            match character {
                'a'..='z' | '0'..='9' => previous_hyphen = false,
                '-' if !previous_hyphen => previous_hyphen = true,
                _ => return Err(SkillError::InvalidSlug),
            }
        }

        if previous_hyphen {
            return Err(SkillError::InvalidSlug);
        }

        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SkillSlug {
    type Error = SkillError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}

impl From<SkillSlug> for String {
    fn from(value: SkillSlug) -> Self {
        value.0
    }
}

macro_rules! prefixed_ulid_id {
    ($name:ident, $prefix:literal) => {
        #[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub fn new(value: &str) -> Result<Self, SkillError> {
                let suffix = value
                    .strip_prefix($prefix)
                    .ok_or(SkillError::InvalidPackage)?;
                if suffix.len() != ULID_LENGTH
                    || !suffix
                        .chars()
                        .all(|character| matches!(character, '0'..='9' | 'A'..='H' | 'J'..='K' | 'M'..='N' | 'P'..='T' | 'V'..='Z'))
                {
                    return Err(SkillError::InvalidPackage);
                }
                Ok(Self(value.to_owned()))
            }

            pub fn generate() -> Self {
                let mut random = [0_u8; 16];
                crate::preconditions::random_bytes(&mut random);
                let timestamp_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;
                let ulid = ulid::Ulid::from_parts(timestamp_ms, u128::from_be_bytes(random));
                Self(format!("{}{}", $prefix, ulid))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = SkillError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(&value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

prefixed_ulid_id!(RevisionId, "r-");
prefixed_ulid_id!(ProposalId, "p-");
prefixed_ulid_id!(RequestId, "q-");
