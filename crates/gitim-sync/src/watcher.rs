use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::path::Path;
use tokio::sync::mpsc;
use tracing::info;

pub enum FileEvent {
    ThreadModified(String),
    MetaModified(String),
    FlowModified(String),
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
                let comps: Vec<_> = rel.components().collect();

                if comps.first().and_then(|c| c.as_os_str().to_str()) == Some("flows") {
                    if let Some(slug_comp) = comps.get(1) {
                        if let Some(slug) = slug_comp.as_os_str().to_str() {
                            let _ = tx.send(FileEvent::FlowModified(slug.to_string())).await;
                            continue;
                        }
                    }
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
