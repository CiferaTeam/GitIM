use notify::{Watcher, RecursiveMode, Event, EventKind};
use std::path::Path;
use tokio::sync::mpsc;
use tracing::info;

pub enum FileEvent {
    ThreadModified(String),
    MetaModified(String),
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

    if channels_dir.exists() {
        watcher.watch(&channels_dir, RecursiveMode::NonRecursive)?;
    }
    if dm_dir.exists() {
        watcher.watch(&dm_dir, RecursiveMode::NonRecursive)?;
    }

    info!("file watcher started");

    tokio::spawn(async move {
        let _watcher = watcher;
        while let Some(event) = notify_rx.recv().await {
            for path in event.paths {
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
