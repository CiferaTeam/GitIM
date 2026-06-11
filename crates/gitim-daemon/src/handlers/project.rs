use crate::api::Response;
use crate::card_handlers::push_with_retry;
use crate::handlers::ensure_author_not_departed;
use crate::state::SharedState;

use gitim_core::types::{Handler, ProjectMeta, ProjectSlug};
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

    info!("project created: {project_slug} by @{author}");
    Response::success(serde_json::json!({"slug": project_slug.as_str()}))
}

pub async fn handle_list_projects(state: SharedState) -> Response {
    let projects_dir = state.repo_root.join("projects");
    if !projects_dir.exists() {
        return Response::success(serde_json::json!({"projects": []}));
    }

    let mut projects = Vec::new();
    let rd = match std::fs::read_dir(&projects_dir) {
        Ok(rd) => rd,
        Err(e) => return Response::error(format!("failed to read projects dir: {e}")),
    };

    for entry in rd.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(slug) = name.strip_suffix(".meta.yaml") else {
            continue;
        };
        let yaml = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        if let Ok(meta) = validate_project_meta(&yaml) {
            projects.push(serde_json::json!({
                "slug": slug,
                "display_name": meta.display_name,
                "created_by": meta.created_by,
                "created_at": meta.created_at,
                "introduction": meta.introduction,
            }));
        }
    }

    projects.sort_by(|a, b| {
        a["slug"]
            .as_str()
            .unwrap_or("")
            .cmp(b["slug"].as_str().unwrap_or(""))
    });

    Response::success(serde_json::json!({"projects": projects}))
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
