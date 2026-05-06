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
    let file = gitim_core::parser::parse_thread(text)
        .map_err(|e| JsError::new(&e.to_string()))?;
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
    let handler = gitim_core::types::Handler::new(author)
        .map_err(|e| JsError::new(&e.to_string()))?;
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
    let handler = gitim_core::types::Handler::new(author)
        .map_err(|e| JsError::new(&e.to_string()))?;
    let meta: serde_json::Value = serde_json::from_str(meta_json)
        .map_err(|e| JsError::new(&e.to_string()))?;
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
    let ha = gitim_core::types::Handler::new(a)
        .map_err(|e| JsError::new(&e.to_string()))?;
    let hb = gitim_core::types::Handler::new(b)
        .map_err(|e| JsError::new(&e.to_string()))?;
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
    let local: gitim_core::types::ChannelMeta = serde_yaml::from_str(local_yaml)
        .map_err(|e| JsError::new(&e.to_string()))?;
    let remote: gitim_core::types::ChannelMeta = serde_yaml::from_str(remote_yaml)
        .map_err(|e| JsError::new(&e.to_string()))?;
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
        &mappings,
        &additions,
    ))
}

#[wasm_bindgen(js_name = "resolveContentPure")]
pub fn resolve_content_pure(
    additions_json: &str,
    remote_json: &str,
) -> Result<JsValue, JsError> {
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
