use std::fmt;

use serde::{Deserialize, Serialize};

use super::SkillError;

const ULID_LENGTH: usize = 26;
const MAX_SLUG_BYTES: usize = 64;
const MAX_ULID_TIMESTAMP: u64 = (1_u64 << 48) - 1;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SkillSlug(String);

impl SkillSlug {
    pub fn new(value: &str) -> Result<Self, SkillError> {
        if value.is_empty()
            || value.len() > MAX_SLUG_BYTES
            || value.starts_with('-')
            || value.ends_with('-')
        {
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
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SkillSlug {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
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
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub fn new(value: &str) -> Result<Self, SkillError> {
                let suffix = value
                    .strip_prefix($prefix)
                    .ok_or(SkillError::InvalidReference)?;
                if suffix.len() != ULID_LENGTH
                    || !suffix.chars().all(|character| {
                        matches!(
                            character,
                            '0'..='9'
                                | 'A'..='H'
                                | 'J'..='K'
                                | 'M'..='N'
                                | 'P'..='T'
                                | 'V'..='Z'
                        )
                    })
                {
                    return Err(SkillError::InvalidReference);
                }
                let parsed = ulid::Ulid::from_string(suffix)
                    .map_err(|_| SkillError::InvalidReference)?;
                if parsed.to_string() != suffix {
                    return Err(SkillError::InvalidReference);
                }
                Ok(Self(value.to_owned()))
            }

            pub fn generate() -> Self {
                Self(format!("{}{}", $prefix, random_ulid(current_timestamp_ms())))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
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
prefixed_ulid_id!(EventId, "e-");

impl EventId {
    pub fn generate_after(maximum: Option<&Self>) -> Result<Self, SkillError> {
        let Some(maximum) = maximum else {
            return Ok(Self::generate());
        };
        let suffix = maximum
            .0
            .strip_prefix("e-")
            .ok_or(SkillError::InvalidReference)?;
        let maximum_ulid =
            ulid::Ulid::from_string(suffix).map_err(|_| SkillError::InvalidReference)?;
        let maximum_timestamp = maximum_ulid.timestamp_ms();
        let now = current_timestamp_ms();

        if now > maximum_timestamp {
            let candidate = Self(format!("e-{}", random_ulid(now)));
            if candidate > *maximum {
                return Ok(candidate);
            }
        }

        let next_timestamp = maximum_timestamp
            .checked_add(1)
            .filter(|value| *value <= MAX_ULID_TIMESTAMP)
            .ok_or(SkillError::InvalidHistory)?;
        Ok(Self(format!("e-{}", random_ulid(next_timestamp))))
    }
}

fn current_timestamp_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

fn random_ulid(timestamp_ms: u64) -> ulid::Ulid {
    let mut random = [0_u8; 16];
    crate::preconditions::random_bytes(&mut random);
    ulid::Ulid::from_parts(timestamp_ms, u128::from_be_bytes(random))
}
