use std::fs;
use std::io;
use std::path::Path;

use regex::Regex;

/// Ensure `.gitim/config.yaml` in `repo_dir` has `indexer.enabled = <enabled>`.
///
/// Three cases:
/// - File exists with `indexer:\n  enabled:` → regex-replace the value.
/// - File exists with `indexer:` but no `enabled:` → append `enabled:` under it.
/// - File absent or no `indexer:` section → append the full `indexer:` block.
pub fn ensure_config_indexer_enabled(repo_dir: &Path, enabled: bool) -> io::Result<()> {
    let config_path = repo_dir.join(".gitim/config.yaml");
    let value = enabled.to_string();

    if config_path.exists() {
        let mut content = fs::read_to_string(&config_path)?;
        // literal regex: compile-time invariant, cannot fail at runtime
        #[allow(clippy::unwrap_used)]
        let re = Regex::new(r"(?m)(indexer:\s*\n\s*enabled:)\s*(true|false)").unwrap();
        if re.is_match(&content) {
            content = re.replace(&content, format!("$1 {value}")).to_string();
        } else if content.contains("indexer:") {
            content = content.replacen("indexer:", &format!("indexer:\n  enabled: {value}"), 1);
        } else {
            content.push_str(&format!("\nindexer:\n  enabled: {value}\n"));
        }
        fs::write(&config_path, content)?;
    } else {
        let gitim_dir = repo_dir.join(".gitim");
        fs::create_dir_all(&gitim_dir)?;
        let content = format!("version: 1\nindexer:\n  enabled: {value}\n");
        fs::write(&config_path, content)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn read_config(tmp: &TempDir) -> crate::types::config::Config {
        let content =
            fs::read_to_string(tmp.path().join(".gitim/config.yaml")).expect("config missing");
        serde_yaml::from_str(&content).expect("invalid yaml")
    }

    #[test]
    fn writes_indexer_enabled_true_no_config() {
        let tmp = TempDir::new().unwrap();
        ensure_config_indexer_enabled(tmp.path(), true).unwrap();
        assert!(read_config(&tmp).indexer.enabled);
    }

    #[test]
    fn writes_indexer_enabled_true_existing_config_without_indexer() {
        let tmp = TempDir::new().unwrap();
        let gitim_dir = tmp.path().join(".gitim");
        fs::create_dir_all(&gitim_dir).unwrap();
        fs::write(
            gitim_dir.join("config.yaml"),
            "version: 1\ndaemon:\n  debug_http: false\n",
        )
        .unwrap();
        ensure_config_indexer_enabled(tmp.path(), true).unwrap();
        let cfg = read_config(&tmp);
        assert!(cfg.indexer.enabled);
        assert!(!cfg.daemon.debug_http);
    }

    #[test]
    fn writes_indexer_enabled_true_existing_config_with_indexer_false() {
        let tmp = TempDir::new().unwrap();
        let gitim_dir = tmp.path().join(".gitim");
        fs::create_dir_all(&gitim_dir).unwrap();
        fs::write(
            gitim_dir.join("config.yaml"),
            "version: 1\nindexer:\n  enabled: false\n",
        )
        .unwrap();
        ensure_config_indexer_enabled(tmp.path(), true).unwrap();
        assert!(read_config(&tmp).indexer.enabled);
    }

    #[test]
    fn writes_indexer_enabled_false() {
        let tmp = TempDir::new().unwrap();
        ensure_config_indexer_enabled(tmp.path(), false).unwrap();
        assert!(!read_config(&tmp).indexer.enabled);
    }
}
