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
    /// Stable device-bound UUID for this runtime install. Empty when
    /// uninitialized — `ensure_runtime_id` materializes it on first call.
    /// See docs/plans/runtime-id/00-design.md.
    #[serde(default)]
    pub runtime_id: String,
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

/// Read or generate the device-bound runtime ID.
///
/// Behavior:
/// - If `path` exists and `runtime_id` parses as a UUID → return it as-is.
/// - Otherwise (missing file, missing field, empty string, malformed UUID)
///   → generate a new v4 UUID, write it back into the same file (preserving
///   `workspaces`), and return the new ID.
/// - Write failures are logged via `tracing::warn!` but do NOT propagate;
///   the in-memory UUID is still returned so runtime startup can proceed.
///   Next startup will retry the write.
///
/// See docs/plans/runtime-id/00-design.md for the full design and
/// non-goals (no platform-native device ID, no git sync, no agent injection).
pub fn ensure_runtime_id_at(path: &Path) -> String {
    let mut cfg = read_from(Some(path));
    if uuid::Uuid::parse_str(&cfg.runtime_id).is_ok() {
        return cfg.runtime_id;
    }
    let new_id = uuid::Uuid::new_v4().to_string();
    cfg.runtime_id = new_id.clone();
    if let Err(e) = write_to(&cfg, path) {
        tracing::warn!(
            error = %e,
            path = %path.display(),
            "failed to persist runtime_id; will retry on next startup"
        );
    }
    new_id
}

/// Production entry point: resolves `~/.gitim/runtime.json` and delegates to
/// `ensure_runtime_id_at`. If `dirs::home_dir()` returns `None` (rare —
/// containers, no-HOME environments), generates a fresh in-memory UUID
/// without persisting it, matching the existing `write()` noop semantics.
/// In that case the runtime keeps a stable ID for its current process
/// lifetime but rolls a new one on each restart — acceptable for the
/// tail-edge case.
pub fn ensure_runtime_id() -> String {
    match config_path() {
        Some(p) => ensure_runtime_id_at(&p),
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            tracing::warn!(
                runtime_id = %id,
                "dirs::home_dir() returned None; runtime_id not persisted, will reroll on restart"
            );
            id
        }
    }
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

    #[test]
    fn legacy_config_without_runtime_id_loads() {
        // 旧 schema 没有 runtime_id 字段;serde(default) 应让它解析为空字符串。
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("runtime.json");
        let legacy = r#"{"workspaces":[{"slug":"a","workspace_name":"A","path":"/x"}]}"#;
        std::fs::write(&path, legacy).unwrap();
        let cfg = read_from(Some(&path));
        assert_eq!(cfg.runtime_id, "");
        assert_eq!(cfg.workspaces.len(), 1);
        assert_eq!(cfg.workspaces[0].slug, "a");
    }

    #[test]
    fn ensure_runtime_id_creates_when_missing() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("runtime.json");
        assert!(!path.exists());
        let id = ensure_runtime_id_at(&path);
        assert_eq!(id.len(), 36);
        assert_eq!(id.matches('-').count(), 4);
        assert!(uuid::Uuid::parse_str(&id).is_ok());
        let cfg = read_from(Some(&path));
        assert_eq!(cfg.runtime_id, id);
    }

    #[test]
    fn ensure_runtime_id_returns_same_on_second_call() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("runtime.json");
        let first = ensure_runtime_id_at(&path);
        let second = ensure_runtime_id_at(&path);
        assert_eq!(first, second);
    }

    #[test]
    fn ensure_runtime_id_regenerates_on_corruption() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("runtime.json");
        std::fs::write(&path, r#"{"runtime_id":"not-a-uuid","workspaces":[]}"#).unwrap();
        let id = ensure_runtime_id_at(&path);
        assert_ne!(id, "not-a-uuid");
        assert!(uuid::Uuid::parse_str(&id).is_ok());
        let cfg = read_from(Some(&path));
        assert_eq!(cfg.runtime_id, id);
    }

    #[test]
    fn ensure_runtime_id_regenerates_on_empty() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("runtime.json");
        std::fs::write(&path, r#"{"runtime_id":"","workspaces":[]}"#).unwrap();
        let id = ensure_runtime_id_at(&path);
        assert!(uuid::Uuid::parse_str(&id).is_ok());
        assert_eq!(read_from(Some(&path)).runtime_id, id);
    }

    #[test]
    fn ensure_runtime_id_preserves_workspaces() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("runtime.json");
        let mut cfg = UserConfig::default();
        cfg.upsert(sample("frontend", "Frontend", "/ws/frontend"));
        cfg.upsert(sample("backend", "Backend", "/ws/backend"));
        write_to(&cfg, &path).unwrap();
        let id = ensure_runtime_id_at(&path);
        assert!(!id.is_empty());
        let after = read_from(Some(&path));
        assert_eq!(after.runtime_id, id);
        assert_eq!(after.workspaces.len(), 2);
        assert_eq!(after.workspaces[0].slug, "frontend");
        assert_eq!(after.workspaces[1].slug, "backend");
    }

    #[test]
    #[ignore = "writes to real ~/.gitim/runtime.json; run manually with --ignored"]
    fn ensure_runtime_id_returns_valid_uuid() {
        // Manual smoke test for the home_dir-bound production wrapper.
        // Marked #[ignore] because it touches the real $HOME — running it in
        // CI or in a developer's normal `cargo test` would write/mutate
        // ~/.gitim/runtime.json. The integration tests in
        // tests/runtime_id.rs cover the wiring without this side effect.
        let id = ensure_runtime_id();
        assert!(uuid::Uuid::parse_str(&id).is_ok());
    }
}
