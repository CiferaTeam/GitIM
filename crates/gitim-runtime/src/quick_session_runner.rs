/// Shared quick session execution helpers.
use gitim_core::types::{QuickSessionMeta, QuickSessionStatus};

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
