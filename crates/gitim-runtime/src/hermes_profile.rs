use crate::error::RuntimeError;
use std::path::PathBuf;

/// Returns the hermes profile name for a given agent handler.
/// Profile name format is `gitim-<handler>`.
pub fn profile_name(handler: &str) -> String {
    format!("gitim-{}", handler)
}

/// Returns the hermes profile directory path for a given agent handler.
/// Returns `<home>/.hermes/profiles/gitim-<handler>`.
pub fn profile_dir(handler: &str) -> Result<PathBuf, RuntimeError> {
    let home = dirs::home_dir().ok_or_else(|| {
        RuntimeError::OnboardFailed("home directory not found".to_string())
    })?;
    Ok(home.join(".hermes/profiles").join(profile_name(handler)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_name_for_alice() {
        assert_eq!(profile_name("alice"), "gitim-alice");
    }

    #[test]
    fn profile_dir_for_alice() {
        let result = profile_dir("alice").unwrap();
        let expected = dirs::home_dir()
            .unwrap()
            .join(".hermes/profiles/gitim-alice");
        assert_eq!(result, expected);
    }
}
