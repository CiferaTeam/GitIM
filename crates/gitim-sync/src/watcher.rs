use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::path::Path;
use tokio::sync::mpsc;
use tracing::info;

pub enum FileEvent {
    ThreadModified(String),
    MetaModified(String),
    FlowModified(String),
}

/// Returns the flow slug if `rel` is exactly `flows/<slug>/index.md`, otherwise `None`.
///
/// Run-state writes (`flows/<slug>/runs/<run_id>/state.yaml`) and any other
/// descendant paths are intentionally excluded so they do not trigger spurious
/// FlowModified events.
pub(crate) fn flow_template_slug(rel: &Path) -> Option<&str> {
    let comps: Vec<_> = rel.components().collect();
    if comps.len() != 3 {
        return None;
    }
    if comps[0].as_os_str() != "flows" {
        return None;
    }
    if comps[2].as_os_str() != "index.md" {
        return None;
    }
    comps[1].as_os_str().to_str()
}

pub async fn watch_repo(
    repo_root: &Path,
    tx: mpsc::Sender<FileEvent>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (notify_tx, mut notify_rx) = tokio::sync::mpsc::channel(100);

    let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
        if let Ok(event) = res {
            if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                let _ = notify_tx.blocking_send(event);
            }
        }
    })?;

    let channels_dir = repo_root.join("channels");
    let dm_dir = repo_root.join("dm");
    let flows_dir = repo_root.join("flows");

    if channels_dir.exists() {
        watcher.watch(&channels_dir, RecursiveMode::NonRecursive)?;
    }
    if dm_dir.exists() {
        watcher.watch(&dm_dir, RecursiveMode::NonRecursive)?;
    }
    if flows_dir.exists() {
        watcher.watch(&flows_dir, RecursiveMode::Recursive)?;
    }

    info!("file watcher started");

    let repo_root = repo_root.to_path_buf();
    tokio::spawn(async move {
        let _watcher = watcher;
        while let Some(event) = notify_rx.recv().await {
            for path in event.paths {
                let rel = match path.strip_prefix(&repo_root) {
                    Ok(r) => r.to_path_buf(),
                    Err(_) => continue,
                };

                if let Some(slug) = flow_template_slug(&rel) {
                    let _ = tx.send(FileEvent::FlowModified(slug.to_string())).await;
                    continue;
                }

                let filename = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
                if filename.ends_with(".thread") {
                    let name = filename.trim_end_matches(".thread").to_string();
                    let _ = tx.send(FileEvent::ThreadModified(name)).await;
                } else if filename.ends_with(".meta.yaml") {
                    let name = filename.trim_end_matches(".meta.yaml").to_string();
                    let _ = tx.send(FileEvent::MetaModified(name)).await;
                }
            }
        }
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::flow_template_slug;
    use std::path::Path;

    #[test]
    fn flow_index_md_matches() {
        assert_eq!(
            flow_template_slug(Path::new("flows/release/index.md")),
            Some("release")
        );
        assert_eq!(
            flow_template_slug(Path::new("flows/my-cool-flow/index.md")),
            Some("my-cool-flow")
        );
    }

    #[test]
    fn flow_run_state_file_does_not_emit_flow_modified() {
        // v1.5 run state writes must NOT match — this is the regression guard.
        assert_eq!(
            flow_template_slug(Path::new(
                "flows/release/runs/20260518T100000-abc123/state.yaml"
            )),
            None
        );
        // Any other descendant beyond the 3-component template path.
        assert_eq!(
            flow_template_slug(Path::new(
                "flows/release/runs/20260518T100000-abc123/log.txt"
            )),
            None
        );
        // A sibling file that is not index.md.
        assert_eq!(
            flow_template_slug(Path::new("flows/release/README.md")),
            None
        );
    }

    #[test]
    fn non_flow_paths_do_not_match() {
        assert_eq!(
            flow_template_slug(Path::new("channels/general.thread")),
            None
        );
        assert_eq!(flow_template_slug(Path::new("flows")), None);
        assert_eq!(flow_template_slug(Path::new("flows/release")), None);
        assert_eq!(flow_template_slug(Path::new("")), None);
    }
}
