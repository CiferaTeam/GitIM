use std::path::Path;
use std::time::Duration;
use tokio::time;
use tracing::{info, warn};
use crate::git::GitRepo;

pub async fn start_sync_loop(repo_root: &Path, interval_secs: u32) {
    if interval_secs == 0 {
        info!("sync_interval=0, auto-sync disabled");
        return;
    }

    let repo = GitRepo::new(repo_root);

    if !repo.has_remote() {
        info!("no remote configured, sync loop disabled");
        return;
    }

    let interval = Duration::from_secs(interval_secs as u64);
    info!("sync loop started, interval={}s", interval_secs);

    let mut ticker = time::interval(interval);
    ticker.tick().await; // skip first immediate tick

    loop {
        ticker.tick().await;
        match repo.pull_rebase() {
            Ok(()) => info!("sync: pull complete"),
            Err(e) => warn!("sync: pull failed: {}", e),
        }
    }
}
