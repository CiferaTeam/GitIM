/// Quick session runtime-local state management.
///
/// Each quick session has a local state file at:
/// `.gitim-runtime/quick-sessions/<id>.state.json`
///
/// This stores provider execution details (session tokens, usage, compaction state)
/// that are NOT git-synced — they are local to the runtime that owns the agent.
use gitim_core::types::QuickSessionRuntimeState;
use std::path::{Path, PathBuf};

pub fn quick_session_state_dir(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".gitim-runtime").join("quick-sessions")
}

pub fn quick_session_state_path(workspace_root: &Path, session_id: &str) -> PathBuf {
    quick_session_state_dir(workspace_root).join(format!("{}.state.json", session_id))
}

pub fn read_state(workspace_root: &Path, session_id: &str) -> Option<QuickSessionRuntimeState> {
    let path = quick_session_state_path(workspace_root, session_id);
    let content = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn write_state(
    workspace_root: &Path,
    session_id: &str,
    state: &QuickSessionRuntimeState,
) -> Result<(), String> {
    let path = quick_session_state_path(workspace_root, session_id);
    let parent = path
        .parent()
        .ok_or_else(|| "quick session state path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|e| format!("failed to create state dir: {}", e))?;
    let json = serde_json::to_string_pretty(state)
        .map_err(|e| format!("failed to serialize state: {}", e))?;
    std::fs::write(&path, json).map_err(|e| format!("failed to write state: {}", e))
}

pub fn delete_state(workspace_root: &Path, session_id: &str) -> Result<(), String> {
    let path = quick_session_state_path(workspace_root, session_id);
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("failed to delete state: {}", e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_read_write_delete_state() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let sid = "qs-01ARZ3NDEKTSV4RRFFQ69G5FAV";

        let state = QuickSessionRuntimeState {
            estimated_tokens: 100,
            ..Default::default()
        };

        write_state(root, sid, &state).unwrap();

        let read = read_state(root, sid).unwrap();
        assert_eq!(read.estimated_tokens, 100);

        delete_state(root, sid).unwrap();
        assert!(read_state(root, sid).is_none());
    }
}
