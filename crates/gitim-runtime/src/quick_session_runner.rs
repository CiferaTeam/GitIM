/// Quick session runner — executes quick session turns through the provider.
///
/// Integration points (to be wired in Phase 3-6):
/// - `agent_loop.rs`: detect `quick_session_meta` poll changes, dispatch to runner
/// - `agent_work_queue.rs`: serialize main + quick session turns per agent
/// - `http.rs`: expose `POST .../title` endpoint, enforce title API gate
///
/// Current state: stub — compiles but does not execute provider calls.
/// Full implementation requires wiring into provider abstraction and SSE streaming.
use gitim_core::types::{QuickSessionMeta, QuickSessionRuntimeState, QuickSessionStatus};
use std::path::Path;

/// Enforce the title API gate: if status is `needs_title`, the agent must
/// call `set_quick_session_title` before sending assistant content.
///
/// Returns `Ok(())` if the session is ready for assistant content,
/// or an error message describing why content is blocked.
pub fn check_title_gate(meta: &QuickSessionMeta) -> Result<(), String> {
    if meta.status == QuickSessionStatus::NeedsTitle {
        return Err(
            "QUICK_SESSION_TITLE_REQUIRED: agent must call set_quick_session_title before sending assistant content"
                .to_string(),
        );
    }
    Ok(())
}

/// Build the provider prompt instruction for the title API gate.
/// Injected into the agent's system prompt for quick session turns.
pub fn title_gate_prompt_instruction() -> &'static str {
    "Before your first reply, you must call `set_quick_session_title` with a short title (max 80 characters) that summarizes this session. You will receive a typed error if you attempt to respond without setting a title first."
}

/// Placeholder for dispatching a quick session turn.
///
/// In the full implementation, this would:
/// 1. Load quick session state from `.gitim-runtime/quick-sessions/<id>.state.json`
/// 2. Build provider config from the selected agent's profile
/// 3. Inject title gate prompt instruction
/// 4. Run the agent turn through the provider (with title gate enforcement)
/// 5. Persist agent response to `discussion.thread` via daemon
/// 6. Update session state and status
pub async fn run_quick_session_turn(
    _workspace_root: &Path,
    _session_id: &str,
    _meta: &QuickSessionMeta,
    _state: &QuickSessionRuntimeState,
) -> Result<(), String> {
    // TODO: Wire into provider abstraction (agent_loop + provider layer)
    Err("quick session runner not yet wired to provider".to_string())
}
