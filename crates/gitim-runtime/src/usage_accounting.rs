use std::path::Path;
use std::sync::atomic::Ordering;

use gitim_agent_provider::ProviderUsage;
use serde::Serialize;

use crate::http::SharedRuntimeState;
use crate::state::SessionUsageSnapshot;
use crate::usage_log::{AgentUsageLog, UsageSummary};

pub(crate) struct UsageAccountingContext<'a> {
    pub workspace_root: &'a Path,
    pub workspace_id: &'a str,
    pub handler: &'a str,
    pub provider_type: &'a str,
    pub model: &'a str,
    pub provider_reports_usage: bool,
    pub runtime_state: Option<&'a SharedRuntimeState>,
}

pub(crate) fn accumulate_usage(
    context: UsageAccountingContext<'_>,
    delta: Option<&ProviderUsage>,
) -> UsageSummary {
    let mut log = AgentUsageLog::load_or_default(
        context.workspace_root,
        context.handler,
        context.provider_type,
        context.model,
        context.provider_reports_usage,
    );
    let now = chrono::Utc::now();
    let today = now.format("%Y-%m-%d").to_string();
    log.accumulate(&today, delta, &now.to_rfc3339());
    if let Err(error) = log.save(context.workspace_root, &today) {
        tracing::warn!(
            handler = %context.handler,
            error = %error,
            "failed to save token usage log"
        );
        if let Some(runtime_state) = context.runtime_state {
            if let Ok(state) = runtime_state.lock() {
                state.usage_save_failures.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    let summary = log.summary(&today);
    if let Some(runtime_state) = context.runtime_state {
        if let Ok(mut state) = runtime_state.lock() {
            if let Some(workspace) = state.workspaces.get_mut(context.workspace_id) {
                if let Some(agent) = workspace.agents.get_mut(context.handler) {
                    agent.usage_summary = Some(summary.clone());
                }
            }
        }
    }
    summary
}

#[derive(Serialize)]
struct UsageEventPayload<'a> {
    #[serde(flatten)]
    snapshot: &'a SessionUsageSnapshot,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage_summary: Option<&'a UsageSummary>,
}

pub(crate) fn serialize_usage_event(
    snapshot: &SessionUsageSnapshot,
    usage_summary: Option<&UsageSummary>,
) -> String {
    serde_json::to_string(&UsageEventPayload {
        snapshot,
        usage_summary,
    })
    .unwrap_or_default()
}
