#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use common::{ensure_daemon_in_path, setup_bare_remote, short_tempdir, stop_daemon};
use gitim_agent_provider::{
    Event, ExecOptions, ExecResult, ExecStatus, Provider, ProviderError, ProviderUsageReport,
    Session,
};
use gitim_client::{ApiResponse, GitimClient};
use gitim_core::auth_payload::AuthPayload;
use gitim_core::skill::{
    RequestId, SkillCreateRequest, SkillMutationResult, SkillWorkspaceBootstrapRequest,
};
use gitim_runtime::agent::provision_human;
use gitim_runtime::{provision_agent, AgentConfig, AgentLoop};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

const SKILL_SLUG: &str = "pinned-release-check";
const SKILL_BODY_MARKER: &str = "PINNED_SKILL_BODY_7E41";
const ANSWER: &str = "pinned Skill loaded before this answer";
const CONTRACT_FIRST_LINE: &str =
    "GitIM provides optional shared Skills and does not load them automatically.";
const TEST_BINARY_TARGET: &str = "skill-agent-load";

struct PathGuard {
    original: Option<std::ffi::OsString>,
}

impl PathGuard {
    fn prepend(paths: &[&Path]) -> Self {
        let original = std::env::var_os("PATH");
        let mut entries = paths
            .iter()
            .map(|path| path.to_path_buf())
            .collect::<Vec<_>>();
        if let Some(current) = original.as_deref() {
            entries.extend(std::env::split_paths(current));
        }
        let joined = std::env::join_paths(entries).expect("join test PATH");
        std::env::set_var("PATH", joined);
        Self { original }
    }
}

impl Drop for PathGuard {
    fn drop(&mut self) {
        match self.original.take() {
            Some(path) => std::env::set_var("PATH", path),
            None => std::env::remove_var("PATH"),
        }
    }
}

fn build_current_test_binaries() -> PathBuf {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical workspace root");
    let target_dir = workspace_root.join("target").join(TEST_BINARY_TARGET);
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = std::process::Command::new(cargo)
        .current_dir(&workspace_root)
        .args([
            "build",
            "--locked",
            "--target-dir",
            target_dir.to_str().expect("UTF-8 target directory"),
            "-p",
            "gitim-cli",
            "--bin",
            "gitim",
            "-p",
            "gitim-daemon",
            "--bin",
            "gitim-daemon",
        ])
        .output()
        .expect("build current GitIM test binaries in isolated target");
    assert!(
        output.status.success(),
        "failed to build current GitIM test binaries:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let bin_dir = target_dir.join("debug");
    for binary in ["gitim", "gitim-daemon"] {
        assert!(
            bin_dir.join(binary).is_file(),
            "isolated build did not produce {binary}"
        );
    }
    bin_dir
}

#[cfg(unix)]
fn install_stale_binary_markers(directory: &Path, marker: &Path) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::create_dir_all(directory).expect("create stale marker binary directory");
    for binary in ["gitim", "gitim-daemon"] {
        let path = directory.join(binary);
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\nprintf '%s\\n' '{binary}' >> '{}'\nexit 97\n",
                marker.display()
            ),
        )
        .expect("write stale marker binary");
        let mut permissions = std::fs::metadata(&path)
            .expect("read stale marker metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).expect("make stale marker executable");
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum TranscriptEntry {
    Shell(String),
    ToolOutput(String),
    Answer(String),
}

struct SkillLoadingProvider {
    pinned_ref: String,
    transcript: Arc<Mutex<Vec<TranscriptEntry>>>,
}

#[async_trait]
impl Provider for SkillLoadingProvider {
    async fn execute(&self, prompt: &str, opts: ExecOptions) -> Result<Session, ProviderError> {
        let pinned_ref = self.pinned_ref.clone();
        let transcript = Arc::clone(&self.transcript);
        let cwd = opts.cwd.clone().expect("runtime provider cwd");
        let channel = prompt
            .find("[#")
            .and_then(|start| {
                let suffix = &prompt[start + 2..];
                suffix.find(']').map(|end| suffix[..end].to_owned())
            })
            .expect("message channel in runtime prompt");
        let contract_present = opts
            .system_prompt
            .as_deref()
            .is_some_and(|system_prompt| system_prompt.contains(CONTRACT_FIRST_LINE));
        let should_load = contract_present && prompt.contains(&pinned_ref);
        let (event_tx, event_rx) = mpsc::channel(8);
        let (result_tx, result_rx) = oneshot::channel();

        let task = tokio::spawn(async move {
            let result = run_scripted_turn(
                event_tx,
                transcript,
                &cwd,
                &channel,
                &pinned_ref,
                should_load,
            )
            .await;
            let _ = result_tx.send(result);
        });

        Ok(Session::new(
            event_rx,
            result_rx,
            task.abort_handle(),
            CancellationToken::new(),
        ))
    }
}

async fn run_scripted_turn(
    event_tx: mpsc::Sender<Event>,
    transcript: Arc<Mutex<Vec<TranscriptEntry>>>,
    cwd: &Path,
    channel: &str,
    pinned_ref: &str,
    should_load: bool,
) -> ExecResult {
    if should_load {
        let load_command = format!("gitim skill load {pinned_ref}");
        transcript
            .lock()
            .unwrap()
            .push(TranscriptEntry::Shell(load_command.clone()));
        let _ = event_tx
            .send(Event::ToolUse {
                tool: "bash".to_owned(),
                call_id: "load-skill".to_owned(),
                input: serde_json::json!({ "command": load_command }),
            })
            .await;
        let output = Command::new("gitim")
            .args(["skill", "load", pinned_ref])
            .current_dir(cwd)
            .output()
            .await
            .expect("run gitim skill load");
        let load_output = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        transcript
            .lock()
            .unwrap()
            .push(TranscriptEntry::ToolOutput(load_output.clone()));
        let _ = event_tx
            .send(Event::ToolResult {
                call_id: "load-skill".to_owned(),
                output: load_output,
            })
            .await;
        if !output.status.success() {
            return failed_result("gitim skill load failed");
        }
    }

    let send_output = Command::new("gitim")
        .args(["send", channel, ANSWER])
        .current_dir(cwd)
        .output()
        .await
        .expect("send answer through GitIM");
    if !send_output.status.success() {
        return failed_result("gitim send failed");
    }
    transcript
        .lock()
        .unwrap()
        .push(TranscriptEntry::Answer(ANSWER.to_owned()));
    let _ = event_tx
        .send(Event::Text {
            content: ANSWER.to_owned(),
        })
        .await;

    ExecResult {
        status: ExecStatus::Completed,
        output: ANSWER.to_owned(),
        error: None,
        duration_ms: 1,
        session_token: None,
        usage: None,
        usage_report: ProviderUsageReport::default(),
    }
}

fn failed_result(error: &str) -> ExecResult {
    ExecResult {
        status: ExecStatus::Failed,
        output: String::new(),
        error: Some(error.to_owned()),
        duration_ms: 1,
        session_token: None,
        usage: None,
        usage_report: ProviderUsageReport::default(),
    }
}

fn response_data(response: ApiResponse, operation: &str) -> serde_json::Value {
    assert!(response.ok, "{operation} failed: {:?}", response.error);
    response
        .data
        .unwrap_or_else(|| panic!("{operation} response missing data"))
}

async fn publish_fixture(human_repo: &Path, source: PathBuf) -> String {
    let client = GitimClient::new(human_repo);
    let bootstrap = client
        .request_with_timeout(
            "skill_workspace_bootstrap",
            serde_json::json!({
                "request": SkillWorkspaceBootstrapRequest {
                    request_id: RequestId::generate(),
                }
            }),
            Duration::from_secs(30),
        )
        .await
        .unwrap();
    response_data(bootstrap, "Skill workspace bootstrap");

    let create = client
        .request_with_timeout(
            "skill_create",
            serde_json::json!({
                "request": SkillCreateRequest {
                    request_id: RequestId::generate(),
                    slug: gitim_core::skill::SkillSlug::new(SKILL_SLUG).unwrap(),
                    display_name: "Pinned release check".to_owned(),
                    description: "Pinned loading fixture".to_owned(),
                    source_directory: source,
                }
            }),
            Duration::from_secs(30),
        )
        .await
        .unwrap();
    let result: SkillMutationResult =
        serde_json::from_value(response_data(create, "Skill create")["result"].clone()).unwrap();
    let canonical = result.canonical_ref.unwrap();
    format!(
        "skill:{}@{}",
        canonical.slug.as_str(),
        canonical.revision.unwrap().as_str()
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pinned_message_loads_exact_published_revision_before_answer() {
    let current_bin_dir = tokio::task::spawn_blocking(build_current_test_binaries)
        .await
        .expect("current binary build task");
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let stale_bin_dir = tmp.path().join("stale-bin");
    let stale_marker = tmp.path().join("stale-binary-invoked");
    install_stale_binary_markers(&stale_bin_dir, &stale_marker);
    let _path_guard = PathGuard::prepend(&[&current_bin_dir, &stale_bin_dir]);
    let remote = setup_bare_remote(&tmp);
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    let human_repo = provision_human(
        &workspace,
        remote.to_str().unwrap(),
        "git",
        AuthPayload::Git {
            handler: "alice".to_owned(),
            display_name: "Alice".to_owned(),
            github_email: None,
        },
    )
    .await
    .unwrap();
    let source = tmp.path().join("published-skill");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(
        source.join("SKILL.md"),
        format!(
            "---\nname: {SKILL_SLUG}\ndescription: Pinned loading fixture\n---\n\n{SKILL_BODY_MARKER}\n"
        ),
    )
    .unwrap();
    let pinned_ref = publish_fixture(&human_repo, source).await;

    let agents_dir = workspace.join("agents");
    std::fs::create_dir_all(&agents_dir).unwrap();
    let agent = provision_agent(
        &agents_dir,
        &AgentConfig {
            handler: "skill-agent".to_owned(),
            display_name: "Skill Agent".to_owned(),
            remote_url: remote.to_string_lossy().into_owned(),
            github_email: None,
        },
        true,
    )
    .await
    .unwrap();

    let transcript = Arc::new(Mutex::new(Vec::new()));
    let mut agent_loop = AgentLoop::with_provider(&agent.repo_root, "mock", "skill-agent").unwrap();
    agent_loop.replace_provider_for_test(Box::new(SkillLoadingProvider {
        pinned_ref: pinned_ref.clone(),
        transcript: Arc::clone(&transcript),
    }));
    agent_loop.init().await.unwrap();

    let message = format!("<@skill-agent> follow {pinned_ref} and report the result");
    let human_client = GitimClient::new(&human_repo);
    let mut sent = None;
    for _ in 0..40 {
        let response = human_client
            .send("general", &message, None, None)
            .await
            .unwrap();
        if response.ok {
            sent = Some(response);
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(
        sent.is_some(),
        "human daemon did not learn the agent identity"
    );

    let mut processed = false;
    for _ in 0..40 {
        if agent_loop.run_once().await.unwrap() {
            processed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(processed, "agent did not process the pinned Skill message");

    let entries = transcript.lock().unwrap().clone();
    let expected_load = TranscriptEntry::Shell(format!("gitim skill load {pinned_ref}"));
    let load_position = entries
        .iter()
        .position(|entry| entry == &expected_load)
        .expect("exact pinned gitim skill load command in transcript");
    let output_position = entries
        .iter()
        .position(|entry| {
            matches!(entry, TranscriptEntry::ToolOutput(output) if output.contains(SKILL_BODY_MARKER))
        })
        .expect("published Skill body in load output");
    let answer_position = entries
        .iter()
        .position(|entry| entry == &TranscriptEntry::Answer(ANSWER.to_owned()))
        .expect("answer in transcript");
    assert!(load_position < output_position);
    assert!(output_position < answer_position);

    for native_skill_path in [
        ".agents/skills",
        ".claude/skills",
        ".codex/skills",
        ".cursor/skills",
        ".gemini/skills",
        ".opencode/skills",
    ] {
        assert!(
            !agent.repo_root.join(native_skill_path).exists(),
            "runtime-native Skill path created: {native_skill_path}"
        );
    }

    stop_daemon(&agent.repo_root).await;
    stop_daemon(&human_repo).await;
    assert!(
        !stale_marker.exists(),
        "a stale PATH binary was invoked instead of the isolated current build"
    );
}
