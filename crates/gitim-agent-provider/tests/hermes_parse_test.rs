//! Hermes-specific parse-shape tests that aren't covered by the shared
//! `acp::parse` module tests. Most parse coverage moved to
//! `src/acp/parse.rs` alongside the migrated symbols; this file retains
//! only assertions that pin hermes' own prompt-payload contract.

use gitim_agent_provider::hermes::build_prompt_payload;

#[test]
fn build_prompt_payload_does_not_inject_system_prompt() {
    // The runtime's system prompt must NOT enter hermes' conversation
    // history — it lives in SOUL.md (managed by hermes_profile) so that
    // hermes' frozen system-prompt slot owns it and the in-loop compressor
    // cannot summarise it away. Earlier versions of this function prepended
    // the runtime system prompt to the first user payload; that is exactly
    // the regression this test guards against.
    let payload = build_prompt_payload("events");

    assert_eq!(payload, "events");
}
