use crate::api::{Event, Response};
use crate::card_handlers::push_with_retry;
use crate::handlers::ensure_author_not_departed;
use crate::state::SharedState;

use gitim_core::responses::{ListProjectsResponse, ProjectEntry};
use gitim_core::types::{ChannelMeta, ChannelName, Handler, ProjectMeta, ProjectSlug};
use gitim_core::validator::validate_project_meta;
use tracing::info;

pub async fn handle_create_project(
    state: SharedState,
    slug: String,
    display_name: String,
    introduction: String,
    author: String,
) -> Response {
    // 1. Validate author
    if Handler::new(&author).is_err() {
        return Response::error(format!("invalid author: {author}"));
    }
    if let Err(resp) = ensure_author_not_departed(&state, &author) {
        return resp;
    }
    {
        let users = state.users.read().await;
        if !users.contains(&author) {
            return Response::error(format!("unknown user: {author}"));
        }
    }

    // 2. Validate slug
    let project_slug = match ProjectSlug::new(&slug) {
        Ok(s) => s,
        Err(e) => return Response::error(format!("invalid_slug: {e}")),
    };

    // 3. Check project doesn't already exist
    let projects_dir = state.repo_root.join("projects");
    let meta_path = projects_dir.join(format!("{project_slug}.meta.yaml"));
    if meta_path.exists() {
        return Response::error_with_code(
            format!("project '{slug}' already exists"),
            "project_exists",
        );
    }

    // 4. Create projects/ dir
    if let Err(e) = std::fs::create_dir_all(&projects_dir) {
        return Response::error(format!("failed to create projects dir: {e}"));
    }

    // 5. Build + serialize meta
    let now = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let meta = ProjectMeta {
        display_name,
        created_by: author.clone(),
        created_at: now,
        introduction,
    };

    let yaml = match Response::yaml_string(&meta, "project meta") {
        Ok(s) => s,
        Err(resp) => return resp,
    };

    // re-validate via validator to catch bound violations
    if let Err(e) = validate_project_meta(&yaml) {
        return Response::error(format!("project meta validation: {e}"));
    }

    // 6. commit_lock + write + git commit
    let commit_guard = state.commit_lock.lock().unwrap_or_else(|e| e.into_inner());
    if let Err(e) = std::fs::write(&meta_path, &yaml) {
        return Response::error(format!("write project meta: {e}"));
    }

    let meta_rel = format!("projects/{project_slug}.meta.yaml");
    let commit_msg = format!("project: create {project_slug} by @{author}");
    let (author_name, author_email) = state.author_for(&author);
    if let Err(e) = state.git_storage.add_and_commit_as(
        &[&meta_rel],
        &commit_msg,
        Some((&author_name, &author_email)),
    ) {
        // Clean up the half-written state
        let _ = std::fs::remove_file(&meta_path);
        return Response::error(format!("commit create_project: {e}"));
    }
    drop(commit_guard);

    // 7. Push with retry (skip if no remote)
    if let Err(e) = push_with_retry(&state, "create_project").await {
        return Response::error(e);
    }

    let _ = state.event_tx.send(Event::ProjectCreated {
        slug: project_slug.to_string(),
    });

    info!("project created: {project_slug} by @{author}");
    Response::success(serde_json::json!({"slug": project_slug.as_str()}))
}

pub async fn handle_list_projects(state: SharedState) -> Response {
    let projects_dir = state.repo_root.join("projects");
    if !projects_dir.exists() {
        return Response::json(ListProjectsResponse { projects: vec![] });
    }

    // --- Step 1: collect valid slugs sorted alphabetically ---
    let mut slugs: Vec<String> = Vec::new();
    let rd = match std::fs::read_dir(&projects_dir) {
        Ok(rd) => rd,
        Err(e) => return Response::error(format!("failed to read projects dir: {e}")),
    };
    for entry in rd.flatten() {
        let fname = entry.file_name();
        let Some(name) = fname.to_str() else { continue };
        let Some(slug) = name.strip_suffix(".meta.yaml") else {
            continue;
        };
        if ProjectSlug::new(slug).is_ok() {
            slugs.push(slug.to_string());
        }
    }
    slugs.sort();

    // --- Step 2: build project→channel count map by scanning channels/ (active only) ---
    // Archived channels live under archive/channels/ and are excluded.
    let channels_dir = state.repo_root.join("channels");
    let mut slug_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    if channels_dir.exists() {
        if let Ok(crd) = std::fs::read_dir(&channels_dir) {
            for cent in crd.flatten() {
                let cname = cent.file_name();
                let Some(cn) = cname.to_str() else { continue };
                if !cn.ends_with(".meta.yaml") {
                    continue;
                }
                let path = cent.path();
                let yaml = match std::fs::read_to_string(&path) {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let cm: Result<ChannelMeta, _> = serde_yaml::from_str(&yaml);
                if let Ok(cm) = cm {
                    if let Some(p) = cm.project {
                        *slug_counts.entry(p).or_insert(0) += 1;
                    }
                }
            }
        }
    }

    // --- Step 3: build response entries ---
    let mut entries: Vec<ProjectEntry> = Vec::new();
    for slug in &slugs {
        let meta_path = projects_dir.join(format!("{slug}.meta.yaml"));
        let yaml = match std::fs::read_to_string(&meta_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let meta: ProjectMeta = match serde_yaml::from_str(&yaml) {
            Ok(m) => m,
            Err(_) => {
                tracing::warn!("project meta corrupted, skipping: {slug}");
                continue;
            }
        };
        let channel_count = slug_counts.get(slug).copied().unwrap_or(0);
        entries.push(ProjectEntry {
            slug: slug.clone(),
            display_name: meta.display_name,
            created_by: meta.created_by,
            created_at: meta.created_at,
            introduction: meta.introduction,
            channel_count,
        });
    }

    Response::json(ListProjectsResponse { projects: entries })
}

pub async fn handle_set_channel_project(
    state: SharedState,
    channel: String,
    project: Option<String>,
    author: String,
) -> Response {
    // 1. Validate author
    if Handler::new(&author).is_err() {
        return Response::error(format!("invalid author: {author}"));
    }
    if let Err(resp) = ensure_author_not_departed(&state, &author) {
        return resp;
    }
    {
        let users = state.users.read().await;
        if !users.contains(&author) {
            return Response::error(format!("unknown user: {author}"));
        }
    }

    // 2. Validate channel name
    let channel_name = match ChannelName::new(&channel) {
        Ok(n) => n,
        Err(e) => return Response::error(format!("invalid channel name: {e}")),
    };

    // 3. Validate project (if Some) exists and is parseable
    if let Some(ref p_slug) = project {
        let validated_slug = match ProjectSlug::new(p_slug) {
            Ok(s) => s,
            Err(e) => return Response::error(format!("invalid_slug: {e}")),
        };
        let p_meta_path = state
            .repo_root
            .join(format!("projects/{validated_slug}.meta.yaml"));
        if !p_meta_path.exists() {
            return Response::error_with_code(
                format!("project '{p_slug}' does not exist"),
                "project_not_found",
            );
        }
        let p_yaml = match std::fs::read_to_string(&p_meta_path) {
            Ok(s) => s,
            Err(e) => return Response::error(format!("read project meta: {e}")),
        };
        if validate_project_meta(&p_yaml).is_err() {
            return Response::error_with_code(
                format!("project '{p_slug}' meta is corrupted"),
                "project_meta_corrupted",
            );
        }
    }

    // 4. Find channel meta — active only; archived channels are frozen
    let active_meta_path = state
        .repo_root
        .join(format!("channels/{channel_name}.meta.yaml"));
    let archived_meta_path = state
        .repo_root
        .join(format!("archive/channels/{channel_name}.meta.yaml"));
    if !active_meta_path.exists() {
        if archived_meta_path.exists() {
            return Response::error_with_code(
                format!("channel '{channel}' is archived; meta is frozen"),
                "channel_archived",
            );
        }
        return Response::error_with_code(
            format!("channel '{channel}' does not exist"),
            "channel_not_found",
        );
    }

    // 5. Read + mutate channel meta
    let yaml = match std::fs::read_to_string(&active_meta_path) {
        Ok(s) => s,
        Err(e) => return Response::error(format!("read channel meta: {e}")),
    };
    let mut meta: ChannelMeta = match serde_yaml::from_str(&yaml) {
        Ok(m) => m,
        Err(e) => return Response::error(format!("parse channel meta: {e}")),
    };

    let old_project = meta.project.clone();
    meta.project = project.clone();

    let new_yaml = match serde_yaml::to_string(&meta) {
        Ok(s) => s,
        Err(e) => return Response::error(format!("ser channel meta: {e}")),
    };

    // 6. commit_lock + write + commit (rollback on failure)
    let commit_guard = state.commit_lock.lock().unwrap_or_else(|e| e.into_inner());
    if let Err(e) = std::fs::write(&active_meta_path, &new_yaml) {
        return Response::error(format!("write channel meta: {e}"));
    }

    let meta_rel = format!("channels/{channel_name}.meta.yaml");
    let commit_msg = match (old_project.as_deref(), project.as_deref()) {
        (None, Some(p)) => {
            format!("channel: assign #{channel_name} to project '{p}' by @{author}")
        }
        (Some(p), None) => {
            format!("channel: remove #{channel_name} from project '{p}' by @{author}")
        }
        (Some(from), Some(to)) => {
            format!("channel: move #{channel_name} from project '{from}' to '{to}' by @{author}")
        }
        (None, None) => format!("channel: #{channel_name} project unchanged by @{author}"),
    };

    let (author_name, author_email) = state.author_for(&author);
    if let Err(e) = state.git_storage.add_and_commit_as(
        &[&meta_rel],
        &commit_msg,
        Some((&author_name, &author_email)),
    ) {
        // Rollback the disk write
        let _ = std::fs::write(&active_meta_path, &yaml);
        return Response::error(format!("commit set_channel_project: {e}"));
    }
    drop(commit_guard);

    // 7. Push with retry (skip if no remote)
    if let Err(e) = push_with_retry(&state, "set_channel_project").await {
        return Response::error(e);
    }

    let _ = state.event_tx.send(Event::ChannelProjectChanged {
        channel: channel_name.to_string(),
        project: project.clone(),
    });

    info!(
        "set_channel_project: #{channel_name} {old:?} → {new:?} by @{author}",
        old = old_project,
        new = project
    );
    Response::success(serde_json::json!({
        "channel": channel_name.as_str(),
        "project": project,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::{handle_archive_channel, handle_create_channel};
    use crate::state::AppState;
    use gitim_core::types::config::Config;
    use std::sync::Arc;
    use tokio::sync::broadcast;

    fn setup_state(tmp: &std::path::Path) -> SharedState {
        let remote = tmp.join("remote.git");
        std::fs::create_dir_all(&remote).unwrap();
        std::process::Command::new("git")
            .args(["init", "--bare"])
            .current_dir(&remote)
            .output()
            .unwrap();
        let repo = tmp.join("repo");
        std::process::Command::new("git")
            .args(["clone", remote.to_str().unwrap(), repo.to_str().unwrap()])
            .output()
            .unwrap();
        for (k, v) in [("user.email", "test@test.com"), ("user.name", "Test")] {
            std::process::Command::new("git")
                .args(["config", k, v])
                .current_dir(&repo)
                .output()
                .unwrap();
        }
        std::fs::write(repo.join(".keep"), "").unwrap();
        std::process::Command::new("git")
            .args(["add", ".keep"])
            .current_dir(&repo)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(&repo)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["push", "-u", "origin", "HEAD"])
            .current_dir(&repo)
            .output()
            .unwrap();
        let (tx, _) = broadcast::channel(16);
        Arc::new(AppState::new(repo, Config::default(), tx, None))
    }

    async fn register(state: &SharedState, handler: &str) {
        use crate::handlers::user::handle_register_user;
        let resp = handle_register_user(
            state.clone(),
            handler.to_string(),
            "Display".to_string(),
            "member".to_string(),
            "GitIM user".to_string(),
        )
        .await;
        assert!(resp.ok, "register_user failed: {:?}", resp.error);
    }

    #[tokio::test]
    async fn create_happy_path() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_state(tmp.path());
        register(&state, "alice").await;

        let resp = handle_create_project(
            state.clone(),
            "design".into(),
            "Design Sprint".into(),
            "All UX work".into(),
            "alice".into(),
        )
        .await;
        assert!(resp.ok, "{:?}", resp.error);

        let meta_path = state.repo_root.join("projects/design.meta.yaml");
        assert!(meta_path.exists());
    }

    #[tokio::test]
    async fn duplicate_returns_project_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_state(tmp.path());
        register(&state, "alice").await;

        let r1 = handle_create_project(
            state.clone(),
            "design".into(),
            "D".into(),
            "some intro".into(),
            "alice".into(),
        )
        .await;
        assert!(r1.ok);

        let r2 = handle_create_project(
            state.clone(),
            "design".into(),
            "D".into(),
            "some intro".into(),
            "alice".into(),
        )
        .await;
        assert!(!r2.ok);
        assert_eq!(r2.error_code.as_deref(), Some("project_exists"));
    }

    #[tokio::test]
    async fn invalid_slug_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_state(tmp.path());
        register(&state, "alice").await;

        let r = handle_create_project(
            state.clone(),
            "UPPER".into(),
            "D".into(),
            "some intro".into(),
            "alice".into(),
        )
        .await;
        assert!(!r.ok);
        assert!(
            r.error.as_deref().unwrap_or("").contains("invalid_slug"),
            "expected invalid_slug in error, got: {:?}",
            r.error
        );
    }

    #[tokio::test]
    async fn reserved_slug_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_state(tmp.path());
        register(&state, "alice").await;

        let r = handle_create_project(
            state.clone(),
            "channels".into(),
            "D".into(),
            "some intro".into(),
            "alice".into(),
        )
        .await;
        assert!(!r.ok);
        assert!(
            r.error.as_deref().unwrap_or("").contains("invalid_slug"),
            "expected invalid_slug in error, got: {:?}",
            r.error
        );
    }

    // --- handle_set_channel_project tests ---

    /// Create a channel with the given name (creator = author).
    async fn setup_channel(state: &SharedState, channel: &str, author: &str) {
        let resp = handle_create_channel(
            state.clone(),
            channel.to_string(),
            None,
            None,
            author.to_string(),
            vec![],
        )
        .await;
        assert!(
            resp.ok,
            "setup_channel failed for '{channel}': {:?}",
            resp.error
        );
    }

    /// Archive the given channel (creator must match the registered author).
    async fn do_archive_channel(state: &SharedState, channel: &str, author: &str) {
        let resp =
            handle_archive_channel(state.clone(), channel.to_string(), author.to_string()).await;
        assert!(
            resp.ok,
            "do_archive_channel failed for '{channel}': {:?}",
            resp.error
        );
    }

    #[tokio::test]
    async fn set_assign_happy() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_state(tmp.path());
        register(&state, "alice").await;
        setup_channel(&state, "dev", "alice").await;
        handle_create_project(
            state.clone(),
            "design".into(),
            "D".into(),
            "intro".into(),
            "alice".into(),
        )
        .await;

        let r = handle_set_channel_project(
            state.clone(),
            "dev".into(),
            Some("design".into()),
            "alice".into(),
        )
        .await;
        assert!(r.ok, "{:?}", r.error);

        let yaml = std::fs::read_to_string(state.repo_root.join("channels/dev.meta.yaml")).unwrap();
        assert!(
            yaml.contains("project: design"),
            "project not set; yaml:\n{yaml}"
        );
    }

    #[tokio::test]
    async fn set_unassign_happy() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_state(tmp.path());
        register(&state, "alice").await;
        setup_channel(&state, "dev", "alice").await;
        handle_create_project(
            state.clone(),
            "design".into(),
            "D".into(),
            "intro".into(),
            "alice".into(),
        )
        .await;
        // Assign first
        handle_set_channel_project(
            state.clone(),
            "dev".into(),
            Some("design".into()),
            "alice".into(),
        )
        .await;

        // Then unassign (project: None)
        let r = handle_set_channel_project(state.clone(), "dev".into(), None, "alice".into()).await;
        assert!(r.ok, "{:?}", r.error);

        let yaml = std::fs::read_to_string(state.repo_root.join("channels/dev.meta.yaml")).unwrap();
        assert!(
            !yaml.contains("project:"),
            "project field should be absent after unassign; yaml:\n{yaml}"
        );
    }

    #[tokio::test]
    async fn project_not_found_returns_code() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_state(tmp.path());
        register(&state, "alice").await;
        setup_channel(&state, "dev", "alice").await;

        let r = handle_set_channel_project(
            state.clone(),
            "dev".into(),
            Some("ghost".into()),
            "alice".into(),
        )
        .await;
        assert!(!r.ok);
        assert_eq!(r.error_code.as_deref(), Some("project_not_found"));

        // Channel meta must be unchanged (no project field written)
        let yaml = std::fs::read_to_string(state.repo_root.join("channels/dev.meta.yaml")).unwrap();
        assert!(
            !yaml.contains("project:"),
            "channel meta must not be mutated; yaml:\n{yaml}"
        );
    }

    #[tokio::test]
    async fn archived_channel_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_state(tmp.path());
        register(&state, "alice").await;
        setup_channel(&state, "dev", "alice").await;
        handle_create_project(
            state.clone(),
            "design".into(),
            "D".into(),
            "intro".into(),
            "alice".into(),
        )
        .await;
        do_archive_channel(&state, "dev", "alice").await;

        let r = handle_set_channel_project(
            state.clone(),
            "dev".into(),
            Some("design".into()),
            "alice".into(),
        )
        .await;
        assert!(!r.ok);
        assert_eq!(r.error_code.as_deref(), Some("channel_archived"));
    }

    #[tokio::test]
    async fn project_meta_corrupted_returns_code() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_state(tmp.path());
        register(&state, "alice").await;
        setup_channel(&state, "dev", "alice").await;

        // Write a corrupted project meta directly — no valid YAML structure
        std::fs::create_dir_all(state.repo_root.join("projects")).unwrap();
        std::fs::write(
            state.repo_root.join("projects/design.meta.yaml"),
            "this: is: not: valid: yaml: at: all:::",
        )
        .unwrap();

        let r = handle_set_channel_project(
            state.clone(),
            "dev".into(),
            Some("design".into()),
            "alice".into(),
        )
        .await;
        assert!(!r.ok);
        assert_eq!(r.error_code.as_deref(), Some("project_meta_corrupted"));
    }

    // --- Task 8: handle_list_projects tests ---

    #[tokio::test]
    async fn list_projects_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_state(tmp.path());
        register(&state, "alice").await;

        let r = handle_list_projects(state).await;
        assert!(r.ok, "{:?}", r.error);
        let data: gitim_core::responses::ListProjectsResponse =
            serde_json::from_value(r.data.unwrap()).unwrap();
        assert!(data.projects.is_empty());
    }

    #[tokio::test]
    async fn list_projects_with_channel_counts() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_state(tmp.path());
        register(&state, "alice").await;

        // Create two projects
        handle_create_project(
            state.clone(),
            "design".into(),
            "Design Sprint".into(),
            "All UX work".into(),
            "alice".into(),
        )
        .await;
        handle_create_project(
            state.clone(),
            "infra".into(),
            "Infrastructure".into(),
            "All infra work".into(),
            "alice".into(),
        )
        .await;

        // Create three channels
        setup_channel(&state, "dev", "alice").await;
        setup_channel(&state, "ml", "alice").await;
        setup_channel(&state, "ops", "alice").await;

        // Assign dev + ml to design; leave ops unassigned
        handle_set_channel_project(
            state.clone(),
            "dev".into(),
            Some("design".into()),
            "alice".into(),
        )
        .await;
        handle_set_channel_project(
            state.clone(),
            "ml".into(),
            Some("design".into()),
            "alice".into(),
        )
        .await;

        let r = handle_list_projects(state).await;
        assert!(r.ok, "{:?}", r.error);
        let data: gitim_core::responses::ListProjectsResponse =
            serde_json::from_value(r.data.unwrap()).unwrap();

        assert_eq!(data.projects.len(), 2);
        // Results must be sorted alphabetically by slug
        assert_eq!(data.projects[0].slug, "design");
        assert_eq!(data.projects[1].slug, "infra");

        let design = data.projects.iter().find(|p| p.slug == "design").unwrap();
        let infra = data.projects.iter().find(|p| p.slug == "infra").unwrap();
        assert_eq!(design.channel_count, 2);
        assert_eq!(infra.channel_count, 0);
    }

    #[tokio::test]
    async fn list_projects_archived_channels_not_counted() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_state(tmp.path());
        register(&state, "alice").await;

        handle_create_project(
            state.clone(),
            "design".into(),
            "Design".into(),
            "intro".into(),
            "alice".into(),
        )
        .await;
        setup_channel(&state, "dev", "alice").await;
        setup_channel(&state, "old", "alice").await;

        // Assign both to design
        handle_set_channel_project(
            state.clone(),
            "dev".into(),
            Some("design".into()),
            "alice".into(),
        )
        .await;
        handle_set_channel_project(
            state.clone(),
            "old".into(),
            Some("design".into()),
            "alice".into(),
        )
        .await;

        // Archive 'old' — its meta moves to archive/channels/ and must not be counted
        do_archive_channel(&state, "old", "alice").await;

        let r = handle_list_projects(state).await;
        assert!(r.ok, "{:?}", r.error);
        let data: gitim_core::responses::ListProjectsResponse =
            serde_json::from_value(r.data.unwrap()).unwrap();

        let design = data.projects.iter().find(|p| p.slug == "design").unwrap();
        // 'old' was archived and must not be counted
        assert_eq!(
            design.channel_count, 1,
            "archived channel must not inflate channel_count"
        );
    }

    // --- Task 8b: SSE event tests ---

    #[tokio::test]
    async fn create_project_pushes_project_created_event() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_state(tmp.path());
        register(&state, "alice").await;

        let mut rx = state.event_tx.subscribe();

        let r = handle_create_project(
            state.clone(),
            "design".into(),
            "Design".into(),
            "intro".into(),
            "alice".into(),
        )
        .await;
        assert!(r.ok, "{:?}", r.error);

        let ev = rx.try_recv().expect("expected ProjectCreated event");
        match ev {
            Event::ProjectCreated { slug } => assert_eq!(slug, "design"),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn create_project_failure_no_event() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_state(tmp.path());
        register(&state, "alice").await;

        // Create once to trigger duplicate_error on second call
        handle_create_project(
            state.clone(),
            "design".into(),
            "D".into(),
            "intro".into(),
            "alice".into(),
        )
        .await;

        let mut rx = state.event_tx.subscribe();

        // This will fail with project_exists
        let r = handle_create_project(
            state.clone(),
            "design".into(),
            "D".into(),
            "intro".into(),
            "alice".into(),
        )
        .await;
        assert!(!r.ok);
        assert!(rx.try_recv().is_err(), "failure path must not push event");
    }

    #[tokio::test]
    async fn set_channel_project_assign_pushes_event() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_state(tmp.path());
        register(&state, "alice").await;
        setup_channel(&state, "dev", "alice").await;
        handle_create_project(
            state.clone(),
            "design".into(),
            "D".into(),
            "intro".into(),
            "alice".into(),
        )
        .await;

        let mut rx = state.event_tx.subscribe();

        let r = handle_set_channel_project(
            state.clone(),
            "dev".into(),
            Some("design".into()),
            "alice".into(),
        )
        .await;
        assert!(r.ok, "{:?}", r.error);

        let ev = rx.try_recv().expect("expected ChannelProjectChanged event");
        match ev {
            Event::ChannelProjectChanged { channel, project } => {
                assert_eq!(channel, "dev");
                assert_eq!(project, Some("design".to_string()));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn set_channel_project_clear_pushes_event_with_none() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_state(tmp.path());
        register(&state, "alice").await;
        setup_channel(&state, "dev", "alice").await;
        handle_create_project(
            state.clone(),
            "design".into(),
            "D".into(),
            "intro".into(),
            "alice".into(),
        )
        .await;
        // Assign first
        handle_set_channel_project(
            state.clone(),
            "dev".into(),
            Some("design".into()),
            "alice".into(),
        )
        .await;

        let mut rx = state.event_tx.subscribe();

        // Now clear
        let r = handle_set_channel_project(state.clone(), "dev".into(), None, "alice".into()).await;
        assert!(r.ok, "{:?}", r.error);

        let ev = rx
            .try_recv()
            .expect("expected ChannelProjectChanged event on clear");
        match ev {
            Event::ChannelProjectChanged { channel, project } => {
                assert_eq!(channel, "dev");
                assert!(project.is_none(), "project should be None on clear");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn set_channel_project_failure_no_event() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_state(tmp.path());
        register(&state, "alice").await;
        setup_channel(&state, "dev", "alice").await;
        // Note: project "ghost" does not exist

        let mut rx = state.event_tx.subscribe();

        let r = handle_set_channel_project(
            state.clone(),
            "dev".into(),
            Some("ghost".into()),
            "alice".into(),
        )
        .await;
        assert!(!r.ok);
        assert_eq!(r.error_code.as_deref(), Some("project_not_found"));
        assert!(rx.try_recv().is_err(), "failure path must not push event");
    }
}
