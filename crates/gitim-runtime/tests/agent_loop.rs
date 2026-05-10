mod common;

use gitim_runtime::agent_loop::detect_steering_trigger;
use gitim_runtime::poller::ChannelChange;

fn make_entry(author: &str, body: &str) -> serde_json::Value {
    serde_json::json!({
        "author": author,
        "body": body,
        "line_number": 1,
        "point_to": 0,
        "timestamp": "2026-04-14T00:00:00Z"
    })
}

fn make_changes(entries: Vec<(&str, &str)>) -> Vec<ChannelChange> {
    vec![ChannelChange {
        channel: "general".to_string(),
        kind: "message".to_string(),
        entries: entries
            .into_iter()
            .map(|(author, body)| make_entry(author, body))
            .collect(),
    }]
}

#[test]
fn test_steering_trigger_mention_and_keyword() {
    let changes = make_changes(vec![("alice", "@bot 急急急! 快来看这个bug")]);
    assert!(detect_steering_trigger(&changes, "bot"));
}

#[test]
fn test_steering_trigger_mention_without_keyword() {
    let changes = make_changes(vec![("alice", "@bot 你好，有空帮忙看看吗")]);
    assert!(!detect_steering_trigger(&changes, "bot"));
}

#[test]
fn test_steering_trigger_keyword_without_mention() {
    let changes = make_changes(vec![("alice", "急急急! 有个紧急问题")]);
    assert!(!detect_steering_trigger(&changes, "bot"));
}

#[test]
fn test_steering_trigger_self_authored_ignored() {
    let changes = make_changes(vec![("bot", "@bot 急急急!")]);
    assert!(!detect_steering_trigger(&changes, "bot"));
}

#[test]
fn test_steering_trigger_empty_changes() {
    let changes: Vec<ChannelChange> = vec![];
    assert!(!detect_steering_trigger(&changes, "bot"));
}

#[test]
fn test_steering_trigger_channel_meta_skipped() {
    let changes = vec![ChannelChange {
        channel: "general".to_string(),
        kind: "channel_meta".to_string(),
        entries: vec![make_entry("alice", "@bot 急急急!")],
    }];
    assert!(!detect_steering_trigger(&changes, "bot"));
}

use gitim_client::GitimClient;
use gitim_runtime::{provision_agent, AgentConfig, AgentLoop};

use common::{ensure_daemon_in_path, setup_bare_remote, short_tempdir, stop_daemon};

/// End-to-end test: send message → agent detects → claude processes → agent replies.
/// Requires `claude` CLI and `gitim` CLI in PATH.
/// Run with: cargo test -p gitim-runtime --test agent_loop -- --ignored --nocapture
#[tokio::test]
#[ignore]
async fn test_agent_loop_end_to_end() {
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let remote = setup_bare_remote(&tmp);
    let agents_dir = tmp.path().join("agents");
    std::fs::create_dir(&agents_dir).unwrap();

    let config = AgentConfig {
        handler: "loop-agent".into(),
        display_name: "Loop Agent".into(),
        remote_url: remote.to_str().unwrap().into(),
        github_email: None,
    };
    let handle = provision_agent(&agents_dir, &config, true).await.unwrap();
    let client = GitimClient::new(&handle.repo_root);
    eprintln!(
        "[setup] agent provisioned at {}",
        handle.repo_root.display()
    );

    let mut agent_loop = AgentLoop::with_defaults(&handle.repo_root).unwrap();

    // Initialize cursor
    let processed = agent_loop.run_once().await.unwrap();
    assert!(!processed, "first run should have no messages");
    eprintln!("[setup] cursor initialized");

    // Send trigger message
    let send_resp = client
        .send(
            "general",
            "This is a test. Please reply with: test-reply-ok",
            None,
            None,
        )
        .await
        .unwrap();
    assert!(send_resp.ok, "send failed: {:?}", send_resp.error);
    eprintln!("[trigger] sent message to general");

    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Agent loop processes the message
    let processed = agent_loop.run_once().await.unwrap();
    assert!(processed, "should have detected and processed the message");
    eprintln!("[agent] processed message via claude");

    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Verify agent replied
    let read_resp = client.read("general", Some(20), None).await.unwrap();
    assert!(read_resp.ok, "read failed: {:?}", read_resp.error);

    let entries = read_resp.data.unwrap();
    let messages = entries["entries"].as_array().unwrap();
    eprintln!("[verify] {} messages in general:", messages.len());
    for msg in messages {
        let author = msg["author"].as_str().unwrap_or("?");
        let body = msg["body"].as_str().unwrap_or("?");
        eprintln!("  @{}: {}", author, body);
    }

    assert!(
        messages.len() >= 2,
        "expected at least 2 messages (trigger + agent reply), got {}",
        messages.len()
    );

    stop_daemon(&handle.repo_root).await;
}

use gitim_agent_provider::ProviderUsage;
use gitim_runtime::agent_loop::compute_snapshot;
use gitim_runtime::state::UsageSource;

#[test]
fn snapshot_from_claude_provider_reported() {
    let snap = compute_snapshot(
        "sess-abc",
        Some(&ProviderUsage {
            input_tokens: Some(160_000),
            output_tokens: Some(500),
            used_percent: None,
            ..Default::default()
        }),
        42_000,
        Some(200_000),
        "2026-04-20T10:00:00Z",
    )
    .expect("snapshot");

    assert_eq!(snap.session_id, "sess-abc");
    assert_eq!(snap.input_tokens, Some(160_000));
    assert!((snap.used_percent - 80.0).abs() < 0.01);
    assert!(matches!(snap.source, UsageSource::ProviderReported));
}

#[test]
fn snapshot_from_claude_aggregates_cache_tokens() {
    // Turn 2+ of a cached Claude session: input_tokens alone reports 312,
    // but 159_500 tokens are coming in through cache_read. The percentage
    // must reflect the aggregate (~80%), not the uncached fraction (~0%).
    let snap = compute_snapshot(
        "sess-cached",
        Some(&ProviderUsage {
            input_tokens: Some(312),
            output_tokens: Some(180),
            used_percent: None,
            cache_read_tokens: Some(159_500),
            cache_creation_tokens: Some(220),
        }),
        0,
        Some(200_000),
        "2026-04-20T10:00:00Z",
    )
    .expect("snapshot");

    // 312 + 159_500 + 220 = 160_032  →  160_032 / 200_000 = 80.016%
    assert!(
        (snap.used_percent - 80.016).abs() < 0.01,
        "got {}, want ~80.016",
        snap.used_percent
    );
    assert!(matches!(snap.source, UsageSource::ProviderReported));
}

#[test]
fn snapshot_from_claude_without_cache_still_uses_input_tokens() {
    // No cache activity — behavior unchanged from the pre-fix path.
    let snap = compute_snapshot(
        "sess-nocache",
        Some(&ProviderUsage {
            input_tokens: Some(100_000),
            output_tokens: Some(400),
            used_percent: None,
            cache_read_tokens: None,
            cache_creation_tokens: None,
        }),
        0,
        Some(200_000),
        "2026-04-20T10:00:00Z",
    )
    .expect("snapshot");

    assert!((snap.used_percent - 50.0).abs() < 0.01);
}

#[test]
fn snapshot_from_codex_used_percent() {
    let snap = compute_snapshot(
        "sess-xyz",
        Some(&ProviderUsage {
            input_tokens: None,
            output_tokens: None,
            used_percent: Some(47.5),
            ..Default::default()
        }),
        0,
        None,
        "2026-04-20T10:00:00Z",
    )
    .expect("snapshot");

    assert!((snap.used_percent - 47.5).abs() < 0.01);
    assert!(matches!(snap.source, UsageSource::ProviderReported));
    assert!(snap.max_tokens.is_none());
}

#[test]
fn snapshot_falls_back_to_estimator() {
    let snap = compute_snapshot(
        "sess-fut",
        None,
        80_000,
        Some(100_000),
        "2026-04-20T10:00:00Z",
    )
    .expect("snapshot");

    assert!((snap.used_percent - 80.0).abs() < 0.01);
    assert!(matches!(snap.source, UsageSource::RuntimeEstimated));
}

#[test]
fn snapshot_returns_none_when_no_data_available() {
    let snap = compute_snapshot("sess", None, 0, None, "2026-04-20T10:00:00Z");
    assert!(snap.is_none());
}

#[test]
fn snapshot_clamps_above_100_with_warning_signal() {
    let snap = compute_snapshot(
        "sess",
        Some(&ProviderUsage {
            input_tokens: None,
            output_tokens: None,
            used_percent: Some(115.0),
            ..Default::default()
        }),
        0,
        None,
        "2026-04-20T10:00:00Z",
    )
    .expect("snapshot");
    assert!((snap.used_percent - 100.0).abs() < 0.01);
}

use gitim_runtime::agent_loop::just_crossed_threshold;
use gitim_runtime::context_window::WARN_AT_PERCENT;

#[test]
fn crossed_on_first_observation_above_threshold() {
    assert!(just_crossed_threshold(None, 85.0));
}

#[test]
fn not_crossed_below_threshold() {
    assert!(!just_crossed_threshold(Some(45.0), 62.0));
    assert!(!just_crossed_threshold(None, 30.0));
}

#[test]
fn crossed_when_previous_below_and_new_above() {
    assert!(just_crossed_threshold(Some(78.0), 82.0));
    assert!(just_crossed_threshold(Some(79.99), WARN_AT_PERCENT));
}

#[test]
fn not_crossed_when_already_above() {
    assert!(!just_crossed_threshold(Some(82.0), 90.0));
    assert!(!just_crossed_threshold(Some(WARN_AT_PERCENT), 95.0));
}

#[test]
fn not_crossed_when_dropping() {
    assert!(!just_crossed_threshold(Some(90.0), 40.0));
}

use gitim_runtime::agent_loop::build_usage_notice_preamble;

#[test]
fn preamble_contains_percentage() {
    let p = build_usage_notice_preamble(82.4);
    assert!(p.contains("82"), "preamble: {p}");
}

#[test]
fn preamble_mentions_reset_marker() {
    let p = build_usage_notice_preamble(85.0);
    assert!(p.contains("[[RESET]]"));
}

#[test]
fn preamble_marks_as_system_notice() {
    let p = build_usage_notice_preamble(85.0);
    assert!(p.starts_with("[系统通知]"));
}

#[test]
fn preamble_says_only_once() {
    let p = build_usage_notice_preamble(85.0);
    assert!(p.contains("仅发送一次"));
}

#[test]
fn preamble_frames_as_handoff_not_completion() {
    // Lock the "stop now, write orientation, hand off" framing — the whole
    // point of the revised preamble. If a future edit drifts back toward
    // "finish your tasks first", these assertions fail.
    let p = build_usage_notice_preamble(85.0);
    assert!(p.contains("立即"), "must convey stop-now urgency: {p}");
    assert!(p.contains("交接"), "must frame as handoff: {p}");
    assert!(
        p.contains("记忆文件"),
        "must name the persistence target: {p}"
    );
    assert!(
        p.contains("orientation"),
        "must name the handoff shape (not inventory): {p}"
    );
    assert!(
        !p.contains("请在本轮完成手头任务后"),
        "must NOT tell the agent to finish its work first: {p}"
    );
}

#[test]
fn preamble_does_not_blanket_ban_tool_use() {
    // Regression guard for the 2026-04-21 repro (sid f6cf86eb): the old
    // preamble said "停下所有新的工具调用和任务步骤" in step 1 and then
    // asked the agent to "在记忆文件里留一段 orientation" in step 2 —
    // which requires Read + Edit tool calls. The agent resolved the
    // contradiction by firing misdirected DMs before [[RESET]]. The
    // rewritten copy must name the allowed tool surface explicitly
    // (Read + Edit of memory files) and must NOT contain a blanket
    // "stop all tool calls" instruction.
    let p = build_usage_notice_preamble(85.0);
    assert!(
        !p.contains("停下所有新的工具调用"),
        "blanket tool-use ban contradicts the write-orientation step: {p}"
    );
    assert!(
        p.contains("Read") && p.contains("Edit"),
        "must name the allowed tools so the agent does not guess: {p}"
    );
    assert!(
        p.contains("不要发消息") || p.contains("不要回复用户"),
        "must explicitly prohibit the side-channel actions the agent \
         was observed to drift into (misdirected DMs in f6cf86eb): {p}"
    );
}
