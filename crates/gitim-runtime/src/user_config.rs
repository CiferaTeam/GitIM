use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceEntry {
    pub slug: String,
    pub workspace_name: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FleetWorkspaceMapping {
    pub remote_workspace_id: String,
    pub local_workspace_id: String,
    pub workspace_identity: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FleetNodeEntry {
    pub node_id: String,
    pub base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_ip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_name: Option<String>,
    #[serde(default)]
    pub workspaces: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspace_mappings: Vec<FleetWorkspaceMapping>,
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
    /// Best-effort hint of the port the runtime last bound on. Written after
    /// a successful `TcpListener::bind` in server mode (`run_shell`) and read
    /// by the CLI to discover where a running runtime is serving HTTP.
    /// Absent (`None`) when the field has never been written — e.g. legacy
    /// runtime.json predating this feature, or a runtime that crashed before
    /// the first bind. Stale values are tolerated: CLI falls back to
    /// `DEFAULT_PORT` if the persisted port refuses connections.
    #[serde(default)]
    pub listen_port: Option<u16>,
    /// Optional remote runtime subscriptions for the local runtime's fleet
    /// collector. These are persisted here so hot-added subscriptions can be
    /// restored on the next runtime boot.
    #[serde(default)]
    pub fleet_nodes: Vec<FleetNodeEntry>,
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

/// Best-effort persistence of the bound listen port. Reads the existing
/// config, sets `listen_port`, writes it back as a merge so `runtime_id` and
/// `workspaces` survive untouched. Callers (`run_shell`) must NOT treat a
/// failure here as fatal — the runtime keeps serving even if the hint can't
/// be written; the CLI will fall back to `DEFAULT_PORT`.
pub fn write_listen_port(port: u16) -> std::io::Result<()> {
    match config_path() {
        Some(p) => write_listen_port_at(port, &p),
        None => Ok(()),
    }
}

pub fn write_listen_port_at(port: u16, path: &Path) -> std::io::Result<()> {
    let mut cfg = read_from(Some(path));
    cfg.listen_port = Some(port);
    write_to(&cfg, path)
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

    pub fn upsert_fleet_node(&mut self, entry: FleetNodeEntry) {
        if let Some(existing) = self
            .fleet_nodes
            .iter_mut()
            .find(|e| e.node_id == entry.node_id)
        {
            *existing = entry;
        } else {
            self.fleet_nodes.push(entry);
        }
    }

    pub fn remove_fleet_node(&mut self, node_id: &str) -> bool {
        let before = self.fleet_nodes.len();
        self.fleet_nodes.retain(|e| e.node_id != node_id);
        self.fleet_nodes.len() != before
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
    fn write_listen_port_creates_file_when_missing() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("runtime.json");
        assert!(!path.exists());
        write_listen_port_at(16868, &path).unwrap();
        let cfg = read_from(Some(&path));
        assert_eq!(cfg.listen_port, Some(16868));
        assert!(cfg.runtime_id.is_empty());
        assert!(cfg.workspaces.is_empty());
    }

    #[test]
    fn write_listen_port_preserves_runtime_id() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("runtime.json");
        let mut cfg = UserConfig {
            runtime_id: "abc".to_string(),
            ..UserConfig::default()
        };
        cfg.upsert(sample("frontend", "Frontend", "/ws/frontend"));
        cfg.upsert(sample("backend", "Backend", "/ws/backend"));
        write_to(&cfg, &path).unwrap();

        write_listen_port_at(17000, &path).unwrap();
        let after = read_from(Some(&path));
        assert_eq!(after.runtime_id, "abc");
        assert_eq!(after.workspaces.len(), 2);
        assert_eq!(after.workspaces[0].slug, "frontend");
        assert_eq!(after.workspaces[1].slug, "backend");
        assert_eq!(after.listen_port, Some(17000));
    }

    #[test]
    fn write_listen_port_updates_existing_port() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("runtime.json");
        let cfg = UserConfig {
            listen_port: Some(16868),
            ..UserConfig::default()
        };
        write_to(&cfg, &path).unwrap();

        write_listen_port_at(17000, &path).unwrap();
        let after = read_from(Some(&path));
        assert_eq!(after.listen_port, Some(17000));
    }

    #[test]
    fn read_listen_port_legacy_file_no_port_field() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("runtime.json");
        std::fs::write(&path, r#"{"runtime_id":"xxx","workspaces":[]}"#).unwrap();
        let cfg = read_from(Some(&path));
        assert_eq!(cfg.listen_port, None);
        assert_eq!(cfg.runtime_id, "xxx");
    }

    fn fleet_node(
        node_id: &str,
        base_url: &str,
        workspaces: impl IntoIterator<Item = &'static str>,
    ) -> FleetNodeEntry {
        FleetNodeEntry {
            node_id: node_id.to_string(),
            base_url: base_url.to_string(),
            node_ip: Some("100.64.0.10".to_string()),
            node_name: Some("mac-mini".to_string()),
            workspaces: workspaces.into_iter().map(str::to_string).collect(),
            workspace_mappings: Vec::new(),
        }
    }

    #[test]
    fn fleet_nodes_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("runtime.json");
        let mut cfg = UserConfig::default();
        cfg.upsert_fleet_node(fleet_node(
            "node-a",
            "http://100.64.0.10:16868",
            ["room", "lab"],
        ));

        write_to(&cfg, &path).unwrap();
        let loaded = read_from(Some(&path));

        assert_eq!(loaded.fleet_nodes.len(), 1);
        assert_eq!(loaded.fleet_nodes[0].node_id, "node-a");
        assert_eq!(loaded.fleet_nodes[0].base_url, "http://100.64.0.10:16868");
        assert_eq!(
            loaded.fleet_nodes[0].node_ip.as_deref(),
            Some("100.64.0.10")
        );
        assert_eq!(loaded.fleet_nodes[0].node_name.as_deref(), Some("mac-mini"));
        assert_eq!(loaded.fleet_nodes[0].workspaces, vec!["room", "lab"]);
    }

    #[test]
    fn fleet_node_upsert_updates_by_node_id() {
        let mut cfg = UserConfig::default();
        cfg.upsert_fleet_node(fleet_node("node-a", "http://old:16868", ["room"]));
        cfg.upsert_fleet_node(fleet_node("node-a", "http://new:16868", ["room", "lab"]));

        assert_eq!(cfg.fleet_nodes.len(), 1);
        assert_eq!(cfg.fleet_nodes[0].base_url, "http://new:16868");
        assert_eq!(cfg.fleet_nodes[0].workspaces, vec!["room", "lab"]);
    }

    #[test]
    fn remove_fleet_node_by_node_id() {
        let mut cfg = UserConfig::default();
        cfg.upsert_fleet_node(fleet_node("node-a", "http://a:16868", ["room"]));
        cfg.upsert_fleet_node(fleet_node("node-b", "http://b:16868", ["room"]));

        assert!(cfg.remove_fleet_node("node-a"));
        assert_eq!(cfg.fleet_nodes.len(), 1);
        assert_eq!(cfg.fleet_nodes[0].node_id, "node-b");
        assert!(!cfg.remove_fleet_node("node-a"));
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
