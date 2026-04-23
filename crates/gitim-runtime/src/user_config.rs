use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceEntry {
    pub slug: String,
    pub workspace_name: String,
    pub path: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct UserConfig {
    #[serde(default)]
    pub workspaces: Vec<WorkspaceEntry>,
}

pub fn config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".gitim/runtime.json"))
}

pub fn read() -> UserConfig {
    read_from(config_path().as_deref())
}

pub fn read_from(path: Option<&Path>) -> UserConfig {
    let Some(p) = path else {
        return UserConfig::default();
    };
    let Ok(content) = std::fs::read_to_string(p) else {
        return UserConfig::default();
    };
    serde_json::from_str(&content).unwrap_or_default()
}

pub fn write(cfg: &UserConfig) -> std::io::Result<()> {
    match config_path() {
        Some(p) => write_to(cfg, &p),
        None => Ok(()),
    }
}

pub fn write_to(cfg: &UserConfig, path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(cfg).unwrap();
    std::fs::write(path, json)
}

impl UserConfig {
    pub fn upsert(&mut self, entry: WorkspaceEntry) {
        if let Some(existing) = self.workspaces.iter_mut().find(|e| e.slug == entry.slug) {
            *existing = entry;
        } else {
            self.workspaces.push(entry);
        }
    }

    pub fn remove(&mut self, slug: &str) -> bool {
        let before = self.workspaces.len();
        self.workspaces.retain(|e| e.slug != slug);
        self.workspaces.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample(slug: &str, name: &str, path: &str) -> WorkspaceEntry {
        WorkspaceEntry {
            slug: slug.to_string(),
            workspace_name: name.to_string(),
            path: path.to_string(),
        }
    }

    #[test]
    fn read_missing_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("does-not-exist.json");
        let cfg = read_from(Some(&missing));
        assert!(cfg.workspaces.is_empty());
    }

    #[test]
    fn read_parse_error_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let bad = tmp.path().join("runtime.json");
        std::fs::write(&bad, "{ this is not json").unwrap();
        let cfg = read_from(Some(&bad));
        assert!(cfg.workspaces.is_empty());
    }

    #[test]
    fn write_then_read_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("runtime.json");
        let mut cfg = UserConfig::default();
        cfg.workspaces
            .push(sample("frontend", "Frontend", "/ws/frontend"));
        cfg.workspaces
            .push(sample("backend", "Backend", "/ws/backend"));
        write_to(&cfg, &path).unwrap();

        let loaded = read_from(Some(&path));
        assert_eq!(loaded.workspaces, cfg.workspaces);
    }

    #[test]
    fn upsert_adds_entry() {
        let mut cfg = UserConfig::default();
        cfg.upsert(sample("frontend", "Frontend", "/ws/frontend"));
        assert_eq!(cfg.workspaces.len(), 1);
        assert_eq!(cfg.workspaces[0].slug, "frontend");
    }

    #[test]
    fn upsert_updates_existing_by_slug() {
        let mut cfg = UserConfig::default();
        cfg.upsert(sample("frontend", "Frontend", "/ws/frontend"));
        cfg.upsert(sample("frontend", "Frontend v2", "/ws/frontend-v2"));

        assert_eq!(cfg.workspaces.len(), 1);
        assert_eq!(cfg.workspaces[0].workspace_name, "Frontend v2");
        assert_eq!(cfg.workspaces[0].path, "/ws/frontend-v2");
    }

    #[test]
    fn remove_by_slug() {
        let mut cfg = UserConfig::default();
        cfg.upsert(sample("frontend", "Frontend", "/ws/frontend"));
        cfg.upsert(sample("backend", "Backend", "/ws/backend"));

        assert!(cfg.remove("frontend"));
        assert_eq!(cfg.workspaces.len(), 1);
        assert_eq!(cfg.workspaces[0].slug, "backend");
        assert!(!cfg.remove("frontend"));
    }

    #[test]
    fn write_creates_parent_dir() {
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("a/b/c/runtime.json");
        assert!(!nested.parent().unwrap().exists());

        let mut cfg = UserConfig::default();
        cfg.upsert(sample("frontend", "Frontend", "/ws/frontend"));
        write_to(&cfg, &nested).unwrap();

        assert!(nested.exists());
        let loaded = read_from(Some(&nested));
        assert_eq!(loaded.workspaces, cfg.workspaces);
    }
}
