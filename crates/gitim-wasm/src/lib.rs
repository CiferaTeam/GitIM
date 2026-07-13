use std::collections::HashMap;
use std::path::PathBuf;

use wasm_bindgen::prelude::*;

// --- identity ---

#[wasm_bindgen(js_name = "githubIdentityFromUserJson")]
pub fn github_identity_from_user_json(user_json: &str) -> Result<JsValue, JsError> {
    let identity = gitim_core::identity::github_identity_from_user_json(user_json)
        .map_err(|e| JsError::new(&e.to_string()))?;
    serde_wasm_bindgen::to_value(&identity).map_err(|e| JsError::new(&e.to_string()))
}

// --- parse / format ---

#[wasm_bindgen(js_name = "parseThread")]
pub fn parse_thread(text: &str) -> Result<JsValue, JsError> {
    let file = gitim_core::parser::parse_thread(text).map_err(|e| JsError::new(&e.to_string()))?;
    serde_wasm_bindgen::to_value(&file).map_err(|e| JsError::new(&e.to_string()))
}

#[wasm_bindgen(js_name = "formatMessage")]
pub fn format_message(
    line_number: u64,
    point_to: u64,
    author: &str,
    timestamp: &str,
    body: &str,
) -> Result<String, JsError> {
    let handler =
        gitim_core::types::Handler::new(author).map_err(|e| JsError::new(&e.to_string()))?;
    Ok(gitim_core::formatter::format_message(
        line_number,
        point_to,
        &handler,
        timestamp,
        body,
    ))
}

#[wasm_bindgen(js_name = "formatEvent")]
pub fn format_event(
    line_number: u64,
    author: &str,
    timestamp: &str,
    event_type: &str,
    meta_json: &str,
) -> Result<String, JsError> {
    let handler =
        gitim_core::types::Handler::new(author).map_err(|e| JsError::new(&e.to_string()))?;
    let meta: serde_json::Value =
        serde_json::from_str(meta_json).map_err(|e| JsError::new(&e.to_string()))?;
    Ok(gitim_core::formatter::format_event(
        line_number,
        &handler,
        timestamp,
        event_type,
        &meta,
    ))
}

// --- validation ---

#[wasm_bindgen(js_name = "validateAppend")]
pub fn validate_append(
    existing: &str,
    new_lines: &str,
    users: JsValue,
    senders: JsValue,
) -> Result<(), JsError> {
    let users_vec: Vec<String> =
        serde_wasm_bindgen::from_value(users).map_err(|e| JsError::new(&e.to_string()))?;
    let senders_vec: Vec<String> =
        serde_wasm_bindgen::from_value(senders).map_err(|e| JsError::new(&e.to_string()))?;
    let users_refs: Vec<&str> = users_vec.iter().map(|s| s.as_str()).collect();
    let senders_refs: Vec<&str> = senders_vec.iter().map(|s| s.as_str()).collect();
    gitim_core::validator::compliance::validate_append(
        existing,
        new_lines,
        &users_refs,
        &senders_refs,
    )
    .map_err(|e| JsError::new(&e.to_string()))?;
    Ok(())
}

#[wasm_bindgen(js_name = "validateJoin")]
pub fn validate_join(
    author: &str,
    targets: JsValue,
    users: JsValue,
    members: JsValue,
) -> Result<(), JsError> {
    let targets_vec: Vec<String> =
        serde_wasm_bindgen::from_value(targets).map_err(|e| JsError::new(&e.to_string()))?;
    let users_vec: Vec<String> =
        serde_wasm_bindgen::from_value(users).map_err(|e| JsError::new(&e.to_string()))?;
    let members_vec: Vec<String> =
        serde_wasm_bindgen::from_value(members).map_err(|e| JsError::new(&e.to_string()))?;
    let t: Vec<&str> = targets_vec.iter().map(|s| s.as_str()).collect();
    let u: Vec<&str> = users_vec.iter().map(|s| s.as_str()).collect();
    let m: Vec<&str> = members_vec.iter().map(|s| s.as_str()).collect();
    gitim_core::validator::im_rules::validate_join(author, &t, &u, &m)
        .map_err(|e| JsError::new(&e.to_string()))
}

#[wasm_bindgen(js_name = "validateLeave")]
pub fn validate_leave(
    author: &str,
    targets: JsValue,
    users: JsValue,
    members: JsValue,
) -> Result<(), JsError> {
    let targets_vec: Vec<String> =
        serde_wasm_bindgen::from_value(targets).map_err(|e| JsError::new(&e.to_string()))?;
    let users_vec: Vec<String> =
        serde_wasm_bindgen::from_value(users).map_err(|e| JsError::new(&e.to_string()))?;
    let members_vec: Vec<String> =
        serde_wasm_bindgen::from_value(members).map_err(|e| JsError::new(&e.to_string()))?;
    let t: Vec<&str> = targets_vec.iter().map(|s| s.as_str()).collect();
    let u: Vec<&str> = users_vec.iter().map(|s| s.as_str()).collect();
    let m: Vec<&str> = members_vec.iter().map(|s| s.as_str()).collect();
    gitim_core::validator::im_rules::validate_leave(author, &t, &u, &m)
        .map_err(|e| JsError::new(&e.to_string()))
}

#[wasm_bindgen(js_name = "validateHandler")]
pub fn validate_handler(handler: &str) -> Result<String, JsError> {
    let h = gitim_core::types::Handler::new(handler).map_err(|e| JsError::new(&e.to_string()))?;
    Ok(h.as_str().to_string())
}

#[wasm_bindgen(js_name = "validateQuickSessionId")]
pub fn validate_quick_session_id_wasm(id: &str) -> Result<(), JsError> {
    gitim_core::types::validate_quick_session_id(id)
        .map_err(|error| JsError::new(&error.to_string()))
}

#[wasm_bindgen(js_name = "parseQuickSessionMeta")]
pub fn parse_quick_session_meta_wasm(yaml: &str) -> Result<JsValue, JsError> {
    let meta: gitim_core::types::QuickSessionMeta =
        serde_yaml::from_str(yaml).map_err(|error| JsError::new(&error.to_string()))?;
    gitim_core::types::validate_quick_session_meta(&meta)
        .map_err(|error| JsError::new(&error.to_string()))?;
    serde_wasm_bindgen::to_value(&meta).map_err(|error| JsError::new(&error.to_string()))
}

#[wasm_bindgen(js_name = "serializeQuickSessionMeta")]
pub fn serialize_quick_session_meta_wasm(meta: JsValue) -> Result<String, JsError> {
    let meta: gitim_core::types::QuickSessionMeta =
        serde_wasm_bindgen::from_value(meta).map_err(|error| JsError::new(&error.to_string()))?;
    gitim_core::types::validate_quick_session_meta(&meta)
        .map_err(|error| JsError::new(&error.to_string()))?;
    serde_yaml::to_string(&meta).map_err(|error| JsError::new(&error.to_string()))
}

#[derive(serde::Serialize)]
struct QuickSessionTransitionResult {
    meta: gitim_core::types::QuickSessionMeta,
    outcome: gitim_core::types::TransitionOutcome,
}

#[wasm_bindgen(js_name = "applyQuickSessionTransition")]
pub fn apply_quick_session_transition_wasm(
    meta: JsValue,
    transition: JsValue,
) -> Result<JsValue, JsError> {
    let mut meta: gitim_core::types::QuickSessionMeta =
        serde_wasm_bindgen::from_value(meta).map_err(|error| JsError::new(&error.to_string()))?;
    let transition: gitim_core::types::QuickSessionTransition =
        serde_wasm_bindgen::from_value(transition)
            .map_err(|error| JsError::new(&error.to_string()))?;
    let outcome = gitim_core::types::apply_quick_session_transition(&mut meta, transition)
        .map_err(|error| JsError::new(&error.to_string()))?;
    serde_wasm_bindgen::to_value(&QuickSessionTransitionResult { meta, outcome })
        .map_err(|error| JsError::new(&error.to_string()))
}

// `parseChannelMeta` / `parseUserMeta` mirror the daemon's *read* path, which
// is a lenient `serde_yaml::from_str::<ChannelMeta/UserMeta>` (see
// gitim-daemon handlers — channel listing/poll/read all deserialize without
// the strict field-constraint validator). daemon-web only ever reads meta, so
// it needs this lenient parse, not `validateChannelMeta` (which additionally
// enforces write-time constraints and would be stricter than the daemon).
#[wasm_bindgen(js_name = "parseChannelMeta")]
pub fn parse_channel_meta(yaml: &str) -> Result<JsValue, JsError> {
    let meta: gitim_core::types::ChannelMeta =
        serde_yaml::from_str(yaml).map_err(|e| JsError::new(&e.to_string()))?;
    serde_wasm_bindgen::to_value(&meta).map_err(|e| JsError::new(&e.to_string()))
}

#[wasm_bindgen(js_name = "parseUserMeta")]
pub fn parse_user_meta(yaml: &str) -> Result<JsValue, JsError> {
    let meta: gitim_core::types::UserMeta =
        serde_yaml::from_str(yaml).map_err(|e| JsError::new(&e.to_string()))?;
    serde_wasm_bindgen::to_value(&meta).map_err(|e| JsError::new(&e.to_string()))
}

#[wasm_bindgen(js_name = "validateUserMeta")]
pub fn validate_user_meta(yaml: &str) -> Result<JsValue, JsError> {
    let meta = gitim_core::validator::validate_user_meta(yaml)
        .map_err(|e| JsError::new(&e.to_string()))?;
    serde_wasm_bindgen::to_value(&meta).map_err(|e| JsError::new(&e.to_string()))
}

#[wasm_bindgen(js_name = "validateChannelMeta")]
pub fn validate_channel_meta(yaml: &str) -> Result<JsValue, JsError> {
    let meta = gitim_core::validator::validate_channel_meta(yaml)
        .map_err(|e| JsError::new(&e.to_string()))?;
    serde_wasm_bindgen::to_value(&meta).map_err(|e| JsError::new(&e.to_string()))
}

#[wasm_bindgen(js_name = "parseCardMeta")]
pub fn parse_card_meta(yaml: &str) -> Result<JsValue, JsError> {
    let meta =
        gitim_core::types::parse_card_meta_yaml(yaml).map_err(|e| JsError::new(&e.to_string()))?;
    serde_wasm_bindgen::to_value(&meta).map_err(|e| JsError::new(&e.to_string()))
}

#[wasm_bindgen(js_name = "stringifyCardMeta")]
pub fn stringify_card_meta(meta: JsValue) -> Result<String, JsError> {
    let meta: gitim_core::types::CardMeta =
        serde_wasm_bindgen::from_value(meta).map_err(|e| JsError::new(&e.to_string()))?;
    gitim_core::types::stringify_card_meta_yaml(&meta).map_err(|e| JsError::new(&e.to_string()))
}

#[wasm_bindgen(js_name = "validateCardMeta")]
pub fn validate_card_meta(meta: JsValue) -> Result<(), JsError> {
    let meta: gitim_core::types::CardMeta =
        serde_wasm_bindgen::from_value(meta).map_err(|e| JsError::new(&e.to_string()))?;
    gitim_core::types::validate_card_meta(&meta).map_err(|e| JsError::new(&e.to_string()))
}

#[wasm_bindgen(js_name = "validateCardId")]
pub fn validate_card_id(card_id: &str) -> Result<(), JsError> {
    gitim_core::types::validate_card_id(card_id).map_err(|e| JsError::new(&e.to_string()))
}

#[wasm_bindgen(js_name = "validateCardLabels")]
pub fn validate_card_labels(labels: JsValue) -> Result<(), JsError> {
    let labels: Vec<String> =
        serde_wasm_bindgen::from_value(labels).map_err(|e| JsError::new(&e.to_string()))?;
    gitim_core::types::validate_labels(&labels, gitim_core::types::CARD_MAX_LABELS)
        .map_err(|e| JsError::new(&e.to_string()))
}

#[wasm_bindgen(js_name = "parseBoardMarkdown")]
pub fn parse_board_markdown(markdown: &str) -> Result<JsValue, JsError> {
    let board = gitim_core::types::parse_board_markdown(markdown)
        .map_err(|e| JsError::new(&e.to_string()))?;
    serde_wasm_bindgen::to_value(&board).map_err(|e| JsError::new(&e.to_string()))
}

#[wasm_bindgen(js_name = "stringifyBoardMarkdown")]
pub fn stringify_board_markdown(board: JsValue) -> Result<String, JsError> {
    let board: gitim_core::types::BoardDocument =
        serde_wasm_bindgen::from_value(board).map_err(|e| JsError::new(&e.to_string()))?;
    gitim_core::types::stringify_board_markdown(&board).map_err(|e| JsError::new(&e.to_string()))
}

#[wasm_bindgen(js_name = "defaultBoard")]
pub fn default_board(handler: &str, timestamp: &str) -> Result<JsValue, JsError> {
    let board = gitim_core::types::default_board(handler, timestamp)
        .map_err(|e| JsError::new(&e.to_string()))?;
    serde_wasm_bindgen::to_value(&board).map_err(|e| JsError::new(&e.to_string()))
}

#[wasm_bindgen(js_name = "setBoardField")]
pub fn set_board_field(board: JsValue, field: &str, value: &str) -> Result<JsValue, JsError> {
    let mut board: gitim_core::types::BoardDocument =
        serde_wasm_bindgen::from_value(board).map_err(|e| JsError::new(&e.to_string()))?;
    gitim_core::types::set_board_field(&mut board, field, value)
        .map_err(|e| JsError::new(&e.to_string()))?;
    serde_wasm_bindgen::to_value(&board).map_err(|e| JsError::new(&e.to_string()))
}

#[wasm_bindgen(js_name = "setBoardSection")]
pub fn set_board_section(board: JsValue, section: &str, value: &str) -> Result<JsValue, JsError> {
    let mut board: gitim_core::types::BoardDocument =
        serde_wasm_bindgen::from_value(board).map_err(|e| JsError::new(&e.to_string()))?;
    gitim_core::types::set_board_section(&mut board, section, value)
        .map_err(|e| JsError::new(&e.to_string()))?;
    serde_wasm_bindgen::to_value(&board).map_err(|e| JsError::new(&e.to_string()))
}

#[wasm_bindgen(js_name = "appendBoardSection")]
pub fn append_board_section(
    board: JsValue,
    section: &str,
    value: &str,
) -> Result<JsValue, JsError> {
    let mut board: gitim_core::types::BoardDocument =
        serde_wasm_bindgen::from_value(board).map_err(|e| JsError::new(&e.to_string()))?;
    gitim_core::types::append_board_section(&mut board, section, value)
        .map_err(|e| JsError::new(&e.to_string()))?;
    serde_wasm_bindgen::to_value(&board).map_err(|e| JsError::new(&e.to_string()))
}

// --- extraction ---

#[wasm_bindgen(js_name = "extractMentions")]
pub fn extract_mentions(body: &str) -> Result<JsValue, JsError> {
    let mentions = gitim_core::mention::extract_mentions(body);
    let strs: Vec<&str> = mentions.iter().map(|h| h.as_str()).collect();
    serde_wasm_bindgen::to_value(&strs).map_err(|e| JsError::new(&e.to_string()))
}

#[wasm_bindgen(js_name = "extractLinks")]
pub fn extract_links(body: &str) -> Result<JsValue, JsError> {
    let links = gitim_core::link::extract_links(body);
    serde_wasm_bindgen::to_value(&links).map_err(|e| JsError::new(&e.to_string()))
}

// --- DM ---

#[wasm_bindgen(js_name = "dmFilename")]
pub fn dm_filename(a: &str, b: &str) -> Result<String, JsError> {
    let ha = gitim_core::types::Handler::new(a).map_err(|e| JsError::new(&e.to_string()))?;
    let hb = gitim_core::types::Handler::new(b).map_err(|e| JsError::new(&e.to_string()))?;
    Ok(gitim_core::dm::dm_filename(&ha, &hb))
}

// --- sync pure functions ---

#[wasm_bindgen(js_name = "renumberBatch")]
pub fn renumber_batch(batch: &str, max_existing: u64) -> Result<String, JsError> {
    gitim_sync::renumber::renumber_batch(batch, max_existing)
        .map_err(|e| JsError::new(&e.to_string()))
}

#[wasm_bindgen(js_name = "mergeChannelMeta")]
pub fn merge_channel_meta(local_yaml: &str, remote_yaml: &str) -> Result<JsValue, JsError> {
    let local: gitim_core::types::ChannelMeta =
        serde_yaml::from_str(local_yaml).map_err(|e| JsError::new(&e.to_string()))?;
    let remote: gitim_core::types::ChannelMeta =
        serde_yaml::from_str(remote_yaml).map_err(|e| JsError::new(&e.to_string()))?;
    let merged = gitim_sync::conflict::merge_channel_meta(&local, &remote);
    serde_wasm_bindgen::to_value(&merged).map_err(|e| JsError::new(&e.to_string()))
}

#[wasm_bindgen(js_name = "buildRebaseCommitMsg")]
pub fn build_rebase_commit_msg(
    mappings_json: &str,
    additions_json: &str,
) -> Result<String, JsError> {
    let mappings: Vec<gitim_sync::conflict::RenumberMapping> =
        serde_json::from_str(mappings_json).map_err(|e| JsError::new(&e.to_string()))?;
    let additions: HashMap<PathBuf, String> =
        serde_json::from_str(additions_json).map_err(|e| JsError::new(&e.to_string()))?;
    Ok(gitim_sync::conflict::build_rebase_commit_msg(
        &mappings, &additions,
    ))
}

#[wasm_bindgen(js_name = "resolveContentPure")]
pub fn resolve_content_pure(additions_json: &str, remote_json: &str) -> Result<JsValue, JsError> {
    let additions: HashMap<PathBuf, String> =
        serde_json::from_str(additions_json).map_err(|e| JsError::new(&e.to_string()))?;
    let remote: HashMap<PathBuf, String> =
        serde_json::from_str(remote_json).map_err(|e| JsError::new(&e.to_string()))?;
    let (files, mappings) = gitim_sync::conflict::resolve_content_pure(&additions, &remote)
        .map_err(|e| JsError::new(&e.to_string()))?;

    #[derive(serde::Serialize)]
    struct ResolveResult {
        files: Vec<gitim_sync::conflict::ResolvedFile>,
        mappings: Vec<gitim_sync::conflict::RenumberMapping>,
    }

    let result = ResolveResult { files, mappings };
    serde_wasm_bindgen::to_value(&result).map_err(|e| JsError::new(&e.to_string()))
}

#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use wasm_bindgen_test::wasm_bindgen_test;

    const ASSET_REF: &str = "<^v1/3c6a295e-744a-41dc-ba60-5c21bb94e5a2/sha256:8f2c4d7d7e931a62c18f6f24c8e388d72524d4c4cd6f88e9538f7d4a66c72a88?name=asset.txt&type=text%2Fplain&size=42>";

    #[wasm_bindgen_test]
    fn extract_links_preserves_nested_asset_wire_shape() -> Result<(), String> {
        let js_value = super::extract_links(ASSET_REF).map_err(|error| format!("{error:?}"))?;
        let value: serde_json::Value =
            serde_wasm_bindgen::from_value(js_value).map_err(|error| error.to_string())?;

        assert_eq!(value[0]["kind"]["kind"], "asset");
        assert_eq!(
            value[0]["kind"]["asset"]["sha256"],
            "8f2c4d7d7e931a62c18f6f24c8e388d72524d4c4cd6f88e9538f7d4a66c72a88"
        );
        assert_eq!(value[0]["raw"], ASSET_REF);
        Ok(())
    }
}
