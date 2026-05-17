use std::io::ErrorKind;

use crate::api::{Event, Response};
use crate::handlers::ensure_author_not_departed;
use crate::state::SharedState;
use gitim_core::flow::{
    flow_path, parse_flow_markdown, parse_flow_markdown_with_warnings, stringify_flow_markdown,
    validate_flow_document, validate_flow_for_storage, FlowDocument, FlowMeta, FlowSlug,
    FlowWarning,
};
use gitim_core::responses::{
    FlowNodeSummary, FlowSummary, FlowValidationItem, ListFlowsResponse, ShowFlowResponse,
    ValidateFlowResponse, WriteFlowResponse,
};

struct CommittedFlow {
    slug: String,
    path: String,
    commit_id: String,
}

pub async fn handle_flow_list(state: SharedState) -> Response {
    let root = state.repo_root.join("flows");
    let mut flows = Vec::new();

    if root.exists() {
        let entries = match std::fs::read_dir(&root) {
            Ok(e) => e,
            Err(e) => return Response::error(format!("failed to list flows: {}", e)),
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let Ok(slug) = FlowSlug::new(&name) else {
                continue;
            };
            let rel = flow_path(&slug);
            let abs = state.repo_root.join(&rel);
            let Ok(content) = std::fs::read_to_string(&abs) else {
                continue;
            };
            let Ok(doc) = parse_flow_markdown(&content) else {
                continue;
            };
            if validate_flow_document(&doc, slug.as_str()).is_err() {
                continue;
            }
            flows.push(FlowSummary {
                slug: slug.to_string(),
                name: doc.meta.name,
                description: doc.meta.description,
                node_count: doc.meta.nodes.len(),
                updated_at: doc.meta.updated_at,
            });
        }
    }
    flows.sort_by(|a, b| a.slug.cmp(&b.slug));
    Response::success(serde_json::to_value(ListFlowsResponse { flows }).unwrap())
}

pub async fn handle_flow_show(state: SharedState, slug: String) -> Response {
    let slug = match FlowSlug::new(&slug) {
        Ok(s) => s,
        Err(e) => return Response::error(format!("invalid slug: {}", e)),
    };
    let rel = flow_path(&slug);
    let abs = state.repo_root.join(&rel);
    let content = match std::fs::read_to_string(&abs) {
        Ok(c) => c,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Response::error(format!("flow not found: {}", slug));
        }
        Err(e) => return Response::error(format!("failed to read flow: {}", e)),
    };
    let doc = match parse_flow_markdown(&content) {
        Ok(d) => d,
        Err(e) => return Response::error(format!("invalid flow: {}", e)),
    };
    let payload = ShowFlowResponse {
        slug: doc.meta.slug.clone(),
        name: doc.meta.name.clone(),
        description: doc.meta.description.clone(),
        created_by: doc.meta.created_by.clone(),
        created_at: doc.meta.created_at.clone(),
        updated_at: doc.meta.updated_at.clone(),
        nodes: doc.meta.nodes.iter().map(FlowNodeSummary::from).collect(),
        raw_markdown: content,
    };
    Response::success(serde_json::to_value(payload).unwrap())
}

pub async fn handle_flow_create(
    state: SharedState,
    slug: String,
    name: String,
    description: String,
    author: String,
) -> Response {
    let slug = match FlowSlug::new(&slug) {
        Ok(s) => s,
        Err(e) => return Response::error(format!("invalid slug: {}", e)),
    };
    if let Err(resp) = ensure_author_not_departed(&state, &author) {
        return resp;
    }
    let rel = flow_path(&slug);
    let abs = state.repo_root.join(&rel);
    if abs.exists() {
        return Response::error(format!("flow already exists: {}", slug));
    }

    let stub = FlowDocument {
        meta: FlowMeta {
            schema_version: 1,
            slug: slug.to_string(),
            name,
            description,
            created_by: author.clone(),
            created_at: current_timestamp(),
            updated_at: None,
            nodes: vec![],
        },
    };

    match commit_flow_document_locked(&state, &slug, stub, "flow: create", &author) {
        Ok(c) => flow_write_success(&state, c),
        Err(resp) => resp,
    }
}

pub async fn handle_flow_remove(state: SharedState, slug: String, author: String) -> Response {
    let slug = match FlowSlug::new(&slug) {
        Ok(s) => s,
        Err(e) => return Response::error(format!("invalid slug: {}", e)),
    };
    if let Err(resp) = ensure_author_not_departed(&state, &author) {
        return resp;
    }
    let _guard = state.commit_lock.lock().expect("commit_lock poisoned");
    let rel = flow_path(&slug);
    let abs = state.repo_root.join(&rel);
    let trash_rel = std::path::PathBuf::from(".trash")
        .join("flows")
        .join(slug.as_str())
        .join("index.md");
    let trash_abs = state.repo_root.join(&trash_rel);

    if !abs.exists() {
        return Response::error(format!("flow not found: {}", slug));
    }
    if let Some(parent) = trash_abs.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return Response::error(format!("failed to create trash dir: {}", e));
        }
    }
    if let Err(e) = std::fs::rename(&abs, &trash_abs) {
        return Response::error(format!("failed to move to trash: {}", e));
    }
    let _ = std::fs::remove_dir(state.repo_root.join("flows").join(slug.as_str()));

    let from = rel.to_string_lossy().to_string();
    let to = trash_rel.to_string_lossy().to_string();
    let (author_name, author_email) = state.author_for(&author);

    // PORTABILITY: git_storage only exposes single-file commit. We commit both
    // paths in two commits; trash commit is best-effort.
    let commit_id_from = match state.git_storage.add_and_commit_only_as(
        &from,
        &format!("flow: remove {} @{}", slug, author),
        Some((&author_name, &author_email)),
    ) {
        Ok(c) => c,
        Err(e) => return Response::error(format!("flow remove (delete) commit failed: {}", e)),
    };
    let commit_id = match state.git_storage.add_and_commit_only_as(
        &to,
        &format!("flow: trash {} @{}", slug, author),
        Some((&author_name, &author_email)),
    ) {
        Ok(c) => c,
        Err(_) => commit_id_from, // best-effort; if trash commit fails, primary delete is still recorded
    };

    let _ = state.event_tx.send(Event::FlowChanged {
        slug: slug.to_string(),
    });
    state.push_notify.notify_one();

    Response::success(serde_json::json!({
        "slug": slug.to_string(),
        "status": "removed",
        "commit_id": commit_id,
    }))
}

pub async fn handle_flow_validate(state: SharedState, slug: String) -> Response {
    let slug_str = slug.clone();
    let slug = match FlowSlug::new(&slug) {
        Ok(s) => s,
        Err(e) => {
            return Response::success(
                serde_json::to_value(ValidateFlowResponse {
                    slug: slug_str,
                    ok: false,
                    items: vec![FlowValidationItem {
                        kind: "error".into(),
                        message: format!("invalid slug: {}", e),
                    }],
                })
                .unwrap(),
            );
        }
    };
    let rel = flow_path(&slug);
    let abs = state.repo_root.join(&rel);
    let content = match std::fs::read_to_string(&abs) {
        Ok(c) => c,
        Err(_) => {
            return Response::success(
                serde_json::to_value(ValidateFlowResponse {
                    slug: slug.to_string(),
                    ok: false,
                    items: vec![FlowValidationItem {
                        kind: "error".into(),
                        message: format!("flow not found: {}", slug),
                    }],
                })
                .unwrap(),
            );
        }
    };
    let file_size = content.len();
    let mut items = Vec::new();
    let parse_result = parse_flow_markdown_with_warnings(&content);
    match parse_result {
        Ok((doc, warnings)) => {
            for w in warnings {
                items.push(FlowValidationItem {
                    kind: "warning".into(),
                    message: format_warning(&w),
                });
            }
            if let Err(e) = validate_flow_document(&doc, slug.as_str()) {
                items.push(FlowValidationItem {
                    kind: "error".into(),
                    message: format!("{}", e),
                });
                return Response::success(
                    serde_json::to_value(ValidateFlowResponse {
                        slug: slug.to_string(),
                        ok: false,
                        items,
                    })
                    .unwrap(),
                );
            }
            for w in validate_flow_for_storage(&doc, file_size) {
                items.push(FlowValidationItem {
                    kind: "warning".into(),
                    message: format_warning(&w),
                });
            }
            Response::success(
                serde_json::to_value(ValidateFlowResponse {
                    slug: slug.to_string(),
                    ok: true,
                    items,
                })
                .unwrap(),
            )
        }
        Err(e) => {
            items.push(FlowValidationItem {
                kind: "error".into(),
                message: format!("{}", e),
            });
            Response::success(
                serde_json::to_value(ValidateFlowResponse {
                    slug: slug.to_string(),
                    ok: false,
                    items,
                })
                .unwrap(),
            )
        }
    }
}

fn commit_flow_document_locked(
    state: &SharedState,
    slug: &FlowSlug,
    mut doc: FlowDocument,
    message_prefix: &str,
    author: &str,
) -> Result<CommittedFlow, Response> {
    let _guard = state.commit_lock.lock().expect("commit_lock poisoned");
    doc.meta.updated_at = Some(current_timestamp());

    validate_flow_document(&doc, slug.as_str()).map_err(|e| Response::error(format!("{}", e)))?;
    let rel = flow_path(slug);
    let rendered = stringify_flow_markdown(&doc).map_err(|e| Response::error(format!("{}", e)))?;
    let abs = state.repo_root.join(&rel);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Response::error(format!("failed to create flow dir: {}", e)))?;
    }
    std::fs::write(&abs, rendered)
        .map_err(|e| Response::error(format!("failed to write flow: {}", e)))?;

    let path = rel.to_string_lossy().to_string();
    let message = format!("{} {} @{}", message_prefix, slug, author);
    let (author_name, author_email) = state.author_for(author);
    let commit_id = state
        .git_storage
        .add_and_commit_only_as(&path, &message, Some((&author_name, &author_email)))
        .map_err(|e| Response::error(format!("flow commit failed: {}", e)))?;

    Ok(CommittedFlow {
        slug: slug.to_string(),
        path,
        commit_id,
    })
}

fn flow_write_success(state: &SharedState, committed: CommittedFlow) -> Response {
    let _ = state.event_tx.send(Event::FlowChanged {
        slug: committed.slug.clone(),
    });
    state.push_notify.notify_one();
    let payload = WriteFlowResponse {
        slug: committed.slug,
        path: committed.path,
        status: "committed".to_string(),
        commit_id: committed.commit_id,
    };
    Response::success(serde_json::to_value(payload).unwrap())
}

fn format_warning(w: &FlowWarning) -> String {
    match w {
        FlowWarning::BodySectionMissing(id) => format!("body section missing for node: {}", id),
        FlowWarning::OrphanBodySection(id) => format!("orphan body section: {}", id),
        FlowWarning::OversizedFile { actual, limit } => {
            format!("file size {} exceeds limit {}", actual, limit)
        }
        FlowWarning::TooManyNodes { count, limit } => {
            format!("node count {} exceeds limit {}", count, limit)
        }
    }
}

fn current_timestamp() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}
