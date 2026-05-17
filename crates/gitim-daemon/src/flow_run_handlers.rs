use std::io::ErrorKind;

use crate::api::{Event, Response};
use crate::handlers::ensure_author_not_departed;
use crate::state::SharedState;
use gitim_core::flow::{
    flow_path, parse_flow_markdown, parse_run_state, run_path, stringify_run_state,
    validate_flow_document, validate_node_transition, FlowRun, FlowRunNode, FlowSlug, NodeStatus,
    RunId, RunStatus,
};
use gitim_core::responses::{
    CancelFlowRunResponse, FlowRunSummary, ListFlowRunsResponse, ShowFlowRunResponse,
    StartFlowRunResponse, UpdateFlowNodeResponse,
};
use gitim_core::types::ChannelName;

struct CommittedRun {
    run_id: String,
    flow_slug: String,
    channel: String,
    path: String,
    commit_id: String,
}

pub async fn handle_flow_run_start(
    state: SharedState,
    slug: String,
    channel: String,
    author: String,
) -> Response {
    if let Err(resp) = ensure_author_not_departed(&state, &author) {
        return resp;
    }
    let slug = match FlowSlug::new(&slug) {
        Ok(s) => s,
        Err(e) => return Response::error(format!("invalid slug: {}", e)),
    };
    let channel = match ChannelName::new(&channel) {
        Ok(c) => c,
        Err(e) => return Response::error(format!("invalid channel: {}", e)),
    };

    // validate template exists + parses
    let template_abs = state.repo_root.join(flow_path(&slug));
    let template_content = match std::fs::read_to_string(&template_abs) {
        Ok(c) => c,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Response::error_with_code(format!("flow not found: {}", slug), "not_found");
        }
        Err(e) => return Response::error(format!("failed to read flow: {}", e)),
    };
    let template = match parse_flow_markdown(&template_content) {
        Ok(t) => t,
        Err(e) => return Response::error(format!("invalid flow template: {}", e)),
    };
    if let Err(e) = validate_flow_document(&template, slug.as_str()) {
        return Response::error(format!("flow template invalid: {}", e));
    }

    // validate channel exists
    let channel_meta = state
        .repo_root
        .join(format!("channels/{}.meta.yaml", channel));
    if !channel_meta.exists() {
        return Response::error_with_code(format!("channel not found: {}", channel), "not_found");
    }

    // build the run
    let run_id = RunId::generate();
    let now = current_timestamp();
    let nodes: Vec<FlowRunNode> = template
        .meta
        .nodes
        .iter()
        .map(|n| FlowRunNode {
            id: n.id.clone(),
            status: NodeStatus::Pending,
            actor: None,
            started_at: None,
            completed_at: None,
            result_ref: None,
        })
        .collect();

    let run = FlowRun {
        schema_version: 1,
        run_id: run_id.to_string(),
        flow_slug: slug.to_string(),
        channel: channel.to_string(),
        started_at: now.clone(),
        started_by: author.clone(),
        status: RunStatus::InProgress,
        nodes,
        updated_at: now,
    };

    match commit_run_state_locked(&state, &run_id, &slug, run, "flow run: start", &author) {
        Ok(c) => {
            let _ = state.event_tx.send(Event::FlowRunStarted {
                run_id: c.run_id.clone(),
                flow_slug: c.flow_slug.clone(),
                channel: c.channel.clone(),
            });
            state.push_notify.notify_one();
            Response::success(
                serde_json::to_value(StartFlowRunResponse {
                    run_id: c.run_id,
                    flow_slug: c.flow_slug,
                    channel: c.channel,
                    path: c.path,
                    commit_id: c.commit_id,
                })
                .unwrap(),
            )
        }
        Err(resp) => resp,
    }
}

pub async fn handle_flow_run_list(
    state: SharedState,
    slug_filter: Option<String>,
    channel_filter: Option<String>,
    status_filter: Option<String>,
) -> Response {
    let flows_root = state.repo_root.join("flows");
    let mut summaries = Vec::new();
    if !flows_root.exists() {
        return Response::success(
            serde_json::to_value(ListFlowRunsResponse { runs: vec![] }).unwrap(),
        );
    }
    let slug_entries = match std::fs::read_dir(&flows_root) {
        Ok(e) => e,
        Err(e) => return Response::error(format!("failed to list flows: {}", e)),
    };
    for slug_entry in slug_entries.flatten() {
        let slug_name = slug_entry.file_name().to_string_lossy().to_string();
        if let Some(ref filter) = slug_filter {
            if filter != &slug_name {
                continue;
            }
        }
        if FlowSlug::new(&slug_name).is_err() {
            continue;
        }
        let runs_root = slug_entry.path().join("runs");
        if !runs_root.exists() {
            continue;
        }
        let run_entries = match std::fs::read_dir(&runs_root) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for run_entry in run_entries.flatten() {
            let run_name = run_entry.file_name().to_string_lossy().to_string();
            if RunId::new(&run_name).is_err() {
                continue;
            }
            let state_file = run_entry.path().join("state.yaml");
            let Ok(content) = std::fs::read_to_string(&state_file) else {
                continue;
            };
            let Ok(run) = parse_run_state(&content) else {
                continue;
            };
            if let Some(ref ch) = channel_filter {
                if &run.channel != ch {
                    continue;
                }
            }
            if let Some(ref st) = status_filter {
                let want = match st.as_str() {
                    "in_progress" => RunStatus::InProgress,
                    "done" => RunStatus::Done,
                    "failed" => RunStatus::Failed,
                    "cancelled" => RunStatus::Cancelled,
                    _ => continue,
                };
                if run.status != want {
                    continue;
                }
            }
            let nodes_done = run
                .nodes
                .iter()
                .filter(|n| n.status == NodeStatus::Done)
                .count();
            summaries.push(FlowRunSummary {
                run_id: run.run_id,
                flow_slug: run.flow_slug,
                channel: run.channel,
                status: run.status,
                started_by: run.started_by,
                started_at: run.started_at,
                updated_at: run.updated_at,
                node_count: run.nodes.len(),
                nodes_done,
            });
        }
    }
    // sort: newest first
    summaries.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    Response::success(serde_json::to_value(ListFlowRunsResponse { runs: summaries }).unwrap())
}

pub async fn handle_flow_run_show(state: SharedState, run_id: String) -> Response {
    let run_id_typed = match RunId::new(&run_id) {
        Ok(r) => r,
        Err(e) => return Response::error(format!("invalid run id: {}", e)),
    };
    let run = match find_run(&state, &run_id_typed) {
        Ok((_path, r)) => r,
        Err(resp) => return resp,
    };
    let payload: ShowFlowRunResponse = (&run).into();
    Response::success(serde_json::to_value(payload).unwrap())
}

pub async fn handle_flow_node_set(
    state: SharedState,
    run_id: String,
    node_id: String,
    status_str: String,
    actor: Option<String>,
    result_ref: Option<String>,
    author: String,
) -> Response {
    if let Err(resp) = ensure_author_not_departed(&state, &author) {
        return resp;
    }
    let run_id_typed = match RunId::new(&run_id) {
        Ok(r) => r,
        Err(e) => return Response::error(format!("invalid run id: {}", e)),
    };
    let new_status = match parse_node_status(&status_str) {
        Some(s) => s,
        None => {
            return Response::error(format!(
                "invalid status: {} (expected pending|in_progress|done|failed|skipped)",
                status_str
            ));
        }
    };

    let _guard = state.commit_lock.lock().expect("commit_lock poisoned");

    let (run_state_path, mut run) = match find_run(&state, &run_id_typed) {
        Ok(t) => t,
        Err(resp) => return resp,
    };

    if run.status.is_terminal() {
        return Response::error(format!(
            "run is terminal ({:?}); refuse to mutate",
            run.status
        ));
    }

    let node_idx = match run.nodes.iter().position(|n| n.id == node_id) {
        Some(i) => i,
        None => {
            return Response::error_with_code(format!("unknown node id: {}", node_id), "not_found");
        }
    };

    if let Err(e) = validate_node_transition(run.nodes[node_idx].status, new_status) {
        return Response::error(format!("{}", e));
    }

    let now = current_timestamp();
    let node = &mut run.nodes[node_idx];
    if node.status == NodeStatus::Pending && new_status == NodeStatus::InProgress {
        node.started_at = Some(now.clone());
    }
    if new_status.is_terminal() && node.completed_at.is_none() {
        if node.started_at.is_none() {
            node.started_at = Some(now.clone());
        }
        node.completed_at = Some(now.clone());
    }
    node.status = new_status;
    if actor.is_some() {
        node.actor = actor;
    }
    if result_ref.is_some() {
        node.result_ref = result_ref;
    }

    // auto-complete check
    let all_terminal = run.nodes.iter().all(|n| n.status.is_terminal());
    if all_terminal {
        let any_failed = run.nodes.iter().any(|n| n.status == NodeStatus::Failed);
        run.status = if any_failed {
            RunStatus::Failed
        } else {
            RunStatus::Done
        };
    }
    run.updated_at = now;

    let rendered = match stringify_run_state(&run) {
        Ok(r) => r,
        Err(e) => return Response::error(format!("stringify: {}", e)),
    };
    let path_str = run_state_path
        .strip_prefix(&state.repo_root)
        .unwrap_or(&run_state_path)
        .to_string_lossy()
        .to_string();
    let abs = state.repo_root.join(&path_str);
    if let Err(e) = std::fs::write(&abs, rendered) {
        return Response::error(format!("write: {}", e));
    }

    let (a_name, a_email) = state.author_for(&author);
    let commit_id = match state.git_storage.add_and_commit_only_as(
        &path_str,
        &format!(
            "flow run: node {} → {} @{}",
            node_id,
            new_status.as_str(),
            author
        ),
        Some((&a_name, &a_email)),
    ) {
        Ok(id) => id,
        Err(e) => return Response::error(format!("commit: {}", e)),
    };

    let _ = state.event_tx.send(Event::FlowRunNodeUpdated {
        run_id: run.run_id.clone(),
        node_id: node_id.clone(),
        status: new_status.as_str().to_string(),
    });
    if run.status.is_terminal() {
        let _ = state.event_tx.send(Event::FlowRunCompleted {
            run_id: run.run_id.clone(),
            status: run.status.as_str().to_string(),
        });
    }
    state.push_notify.notify_one();

    Response::success(
        serde_json::to_value(UpdateFlowNodeResponse {
            run_id: run.run_id,
            node_id,
            status: new_status,
            run_status: run.status,
            commit_id,
        })
        .unwrap(),
    )
}

pub async fn handle_flow_run_cancel(
    state: SharedState,
    run_id: String,
    author: String,
) -> Response {
    if let Err(resp) = ensure_author_not_departed(&state, &author) {
        return resp;
    }
    let run_id_typed = match RunId::new(&run_id) {
        Ok(r) => r,
        Err(e) => return Response::error(format!("invalid run id: {}", e)),
    };

    let _guard = state.commit_lock.lock().expect("commit_lock poisoned");

    let (run_state_path, mut run) = match find_run(&state, &run_id_typed) {
        Ok(t) => t,
        Err(resp) => return resp,
    };

    if run.status.is_terminal() {
        return Response::error(format!("run already terminal ({:?})", run.status));
    }

    run.status = RunStatus::Cancelled;
    run.updated_at = current_timestamp();

    let rendered = match stringify_run_state(&run) {
        Ok(r) => r,
        Err(e) => return Response::error(format!("stringify: {}", e)),
    };
    let path_str = run_state_path
        .strip_prefix(&state.repo_root)
        .unwrap_or(&run_state_path)
        .to_string_lossy()
        .to_string();
    let abs = state.repo_root.join(&path_str);
    if let Err(e) = std::fs::write(&abs, rendered) {
        return Response::error(format!("write: {}", e));
    }

    let (a_name, a_email) = state.author_for(&author);
    let commit_id = match state.git_storage.add_and_commit_only_as(
        &path_str,
        &format!("flow run: cancel {} @{}", run_id, author),
        Some((&a_name, &a_email)),
    ) {
        Ok(id) => id,
        Err(e) => return Response::error(format!("commit: {}", e)),
    };

    let _ = state.event_tx.send(Event::FlowRunCompleted {
        run_id: run.run_id.clone(),
        status: "cancelled".into(),
    });
    state.push_notify.notify_one();

    Response::success(
        serde_json::to_value(CancelFlowRunResponse {
            run_id: run.run_id,
            commit_id,
        })
        .unwrap(),
    )
}

fn commit_run_state_locked(
    state: &SharedState,
    run_id: &RunId,
    slug: &FlowSlug,
    run: FlowRun,
    message_prefix: &str,
    author: &str,
) -> Result<CommittedRun, Response> {
    let _guard = state.commit_lock.lock().expect("commit_lock poisoned");
    let rel = run_path(slug.as_str(), run_id);
    let rendered =
        stringify_run_state(&run).map_err(|e| Response::error(format!("stringify: {}", e)))?;
    let abs = state.repo_root.join(&rel);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Response::error(format!("mkdir: {}", e)))?;
    }
    std::fs::write(&abs, rendered).map_err(|e| Response::error(format!("write: {}", e)))?;
    let path = rel.to_string_lossy().to_string();
    let (a_name, a_email) = state.author_for(author);
    let commit_id = state
        .git_storage
        .add_and_commit_only_as(
            &path,
            &format!("{} {} @{}", message_prefix, run_id, author),
            Some((&a_name, &a_email)),
        )
        .map_err(|e| Response::error(format!("commit: {}", e)))?;
    Ok(CommittedRun {
        run_id: run_id.to_string(),
        flow_slug: run.flow_slug,
        channel: run.channel,
        path,
        commit_id,
    })
}

fn find_run(
    state: &SharedState,
    run_id: &RunId,
) -> Result<(std::path::PathBuf, FlowRun), Response> {
    let flows_root = state.repo_root.join("flows");
    if !flows_root.exists() {
        return Err(Response::error_with_code(
            format!("run not found: {}", run_id),
            "not_found",
        ));
    }
    let slug_entries = std::fs::read_dir(&flows_root)
        .map_err(|e| Response::error(format!("list flows: {}", e)))?;
    for slug_entry in slug_entries.flatten() {
        let candidate = slug_entry
            .path()
            .join("runs")
            .join(run_id.as_str())
            .join("state.yaml");
        if candidate.exists() {
            let content = std::fs::read_to_string(&candidate)
                .map_err(|e| Response::error(format!("read: {}", e)))?;
            let run =
                parse_run_state(&content).map_err(|e| Response::error(format!("parse: {}", e)))?;
            return Ok((candidate, run));
        }
    }
    Err(Response::error_with_code(
        format!("run not found: {}", run_id),
        "not_found",
    ))
}

fn parse_node_status(s: &str) -> Option<NodeStatus> {
    Some(match s {
        "pending" => NodeStatus::Pending,
        "in_progress" => NodeStatus::InProgress,
        "done" => NodeStatus::Done,
        "failed" => NodeStatus::Failed,
        "skipped" => NodeStatus::Skipped,
        _ => return None,
    })
}

fn current_timestamp() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}
