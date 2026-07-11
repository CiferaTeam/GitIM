#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use gitim_agent_provider::{
    mock::MockProvider, ExecOptions, Provider, ProviderConfig, ProviderError, ProviderUsage,
    Session,
};
use gitim_core::responses::{
    ClaimQuickSessionTurnResponse, ListQuickSessionsResponse, MarkQuickSessionErrorResponse,
    QuickSessionDetail, QuickSessionListItem, ReadQuickSessionResponse,
};
use gitim_core::types::{
    apply_quick_session_transition, Handler, Message, QuickSessionMeta, QuickSessionStatus,
    QuickSessionTransition, ThreadEntry,
};
use gitim_runtime::http::{ActivityScope, AgentActivityEvent};
use gitim_runtime::quick_session_executor::{
    QuickSessionBackend, QuickSessionExecutor, QuickSessionExecutorConfig,
    QuickSessionProviderFactory, QuickSessionRunOutcome,
};
use gitim_runtime::quick_session_state::QuickSessionRuntimeState;
use gitim_runtime::AgentState;
use tokio::sync::broadcast;

const SESSION_ID: &str = "qs-01JZZZZZZZZZZZZZZZZZZZZZZZ";

#[derive(Clone, Copy)]
enum ProviderAction {
    Complete,
    MissingTitle,
    MissingReply,
    QueueHumanThenComplete,
    QueueHumanMissingReply,
    Archive,
    CompleteAndReset,
    BumpGeneration,
    ReplaceAttempt,
}

#[derive(Default)]
struct BackendState {
    detail: Option<QuickSessionDetail>,
    mark_error_calls: usize,
}

#[derive(Clone, Default)]
struct FakeBackend {
    inner: Arc<Mutex<BackendState>>,
}

impl FakeBackend {
    fn with_detail(detail: QuickSessionDetail) -> Self {
        Self {
            inner: Arc::new(Mutex::new(BackendState {
                detail: Some(detail),
                mark_error_calls: 0,
            })),
        }
    }

    fn detail(&self) -> QuickSessionDetail {
        self.inner.lock().unwrap().detail.clone().unwrap()
    }

    fn transition(&self, transition: QuickSessionTransition) {
        let mut guard = self.inner.lock().unwrap();
        let detail = guard.detail.as_mut().unwrap();
        apply_quick_session_transition(&mut detail.meta, transition).unwrap();
    }

    fn append_message(&self, author: &str, point_to: u64, body: &str) -> u64 {
        let mut guard = self.inner.lock().unwrap();
        let detail = guard.detail.as_mut().unwrap();
        let line = detail
            .entries
            .last()
            .map(ThreadEntry::line_number)
            .unwrap_or(0)
            + 1;
        detail.entries.push(message(line, point_to, author, body));
        line
    }
}

#[async_trait]
impl QuickSessionBackend for FakeBackend {
    async fn list(
        &self,
        _agent_id: &str,
        actionable: bool,
    ) -> Result<ListQuickSessionsResponse, String> {
        let guard = self.inner.lock().unwrap();
        let sessions = guard
            .detail
            .iter()
            .filter(|detail| {
                !actionable
                    || (matches!(
                        detail.meta.status,
                        QuickSessionStatus::NeedsTitle | QuickSessionStatus::Active
                    ) && detail.meta.last_human_line > detail.meta.last_completed_input_line)
            })
            .map(|detail| QuickSessionListItem::from_meta(&detail.meta, detail.archived))
            .collect();
        Ok(ListQuickSessionsResponse { sessions })
    }

    async fn read(&self, _session_id: &str) -> Result<ReadQuickSessionResponse, String> {
        Ok(ReadQuickSessionResponse {
            session: self.detail(),
        })
    }

    async fn claim(
        &self,
        session_id: &str,
        input_line: u64,
        attempt_id: &str,
    ) -> Result<ClaimQuickSessionTurnResponse, String> {
        self.transition(QuickSessionTransition::Claim {
            actor: "bob".to_string(),
            input_line,
            attempt_id: attempt_id.to_string(),
            now: "2026-07-11T00:00:10Z".to_string(),
        });
        let meta = self.detail().meta;
        Ok(ClaimQuickSessionTurnResponse {
            session_id: session_id.to_string(),
            input_line,
            attempt_id: attempt_id.to_string(),
            status: meta.status,
            revision: meta.revision,
        })
    }

    async fn mark_error(
        &self,
        session_id: &str,
        attempt_id: &str,
        error: &str,
    ) -> Result<MarkQuickSessionErrorResponse, String> {
        let mut guard = self.inner.lock().unwrap();
        guard.mark_error_calls += 1;
        let detail = guard.detail.as_mut().unwrap();
        apply_quick_session_transition(
            &mut detail.meta,
            QuickSessionTransition::MarkError {
                actor: "bob".to_string(),
                attempt_id: attempt_id.to_string(),
                error: error.to_string(),
                now: "2026-07-11T00:00:20Z".to_string(),
            },
        )
        .map_err(|error| error.to_string())?;
        Ok(MarkQuickSessionErrorResponse {
            session_id: session_id.to_string(),
            status: detail.meta.status,
            revision: detail.meta.revision,
        })
    }
}

#[derive(Clone)]
struct RecordingFactory {
    backend: FakeBackend,
    action: ProviderAction,
    workspace_root: PathBuf,
    self_managed: bool,
    calls: Arc<Mutex<Vec<(ProviderConfig, String, ExecOptions)>>>,
}

impl RecordingFactory {
    fn new(backend: FakeBackend, action: ProviderAction, workspace_root: PathBuf) -> Self {
        Self {
            backend,
            action,
            workspace_root,
            self_managed: false,
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl QuickSessionProviderFactory for RecordingFactory {
    fn create(
        &self,
        _provider_type: &str,
        config: ProviderConfig,
    ) -> Result<Box<dyn Provider>, String> {
        Ok(Box::new(RecordingProvider {
            backend: self.backend.clone(),
            action: self.action,
            workspace_root: self.workspace_root.clone(),
            self_managed: self.self_managed,
            config,
            calls: self.calls.clone(),
        }))
    }
}

struct RecordingProvider {
    backend: FakeBackend,
    action: ProviderAction,
    workspace_root: PathBuf,
    self_managed: bool,
    config: ProviderConfig,
    calls: Arc<Mutex<Vec<(ProviderConfig, String, ExecOptions)>>>,
}

#[async_trait]
impl Provider for RecordingProvider {
    fn self_managed_context(&self) -> bool {
        self.self_managed
    }

    async fn execute(&self, prompt: &str, opts: ExecOptions) -> Result<Session, ProviderError> {
        self.calls
            .lock()
            .unwrap()
            .push((self.config.clone(), prompt.to_string(), opts.clone()));
        let attempt = value_after(prompt, "Attempt: ");
        let input_line: u64 = value_after(prompt, "Input line: L").parse().unwrap();
        if matches!(self.action, ProviderAction::Archive) {
            let mut guard = self.backend.inner.lock().unwrap();
            let detail = guard.detail.as_mut().unwrap();
            detail.archived = true;
            detail.meta.status = QuickSessionStatus::Archived;
            detail.meta.attempt_id = None;
            detail.meta.processing_input_line = None;
            detail.meta.processing_started_at = None;
            detail.meta.archived_at = Some("2026-07-11T00:00:11Z".to_string());
            detail.meta.archived_from = Some(QuickSessionStatus::NeedsTitle);
            detail.meta.revision += 1;
        } else if matches!(self.action, ProviderAction::BumpGeneration) {
            let mut state =
                QuickSessionRuntimeState::load(&self.workspace_root, SESSION_ID).unwrap();
            state.bump_context_generation();
            state.save(&self.workspace_root, SESSION_ID).unwrap();
        } else if matches!(self.action, ProviderAction::ReplaceAttempt) {
            let mut guard = self.backend.inner.lock().unwrap();
            let detail = guard.detail.as_mut().unwrap();
            detail.meta.attempt_id = Some("qa-01K00000000000000000000000".to_string());
            detail.meta.revision += 1;
        } else {
            if !matches!(self.action, ProviderAction::MissingTitle) {
                self.backend.transition(QuickSessionTransition::SetTitle {
                    actor: "bob".to_string(),
                    attempt_id: attempt.clone(),
                    title: "Investigate auth".to_string(),
                    now: "2026-07-11T00:00:12Z".to_string(),
                });
            }
            if matches!(
                self.action,
                ProviderAction::QueueHumanThenComplete | ProviderAction::QueueHumanMissingReply
            ) {
                let line = self.backend.append_message("alice", 0, "one more thing");
                self.backend
                    .transition(QuickSessionTransition::HumanMessage {
                        actor: "alice".to_string(),
                        line_number: line,
                        request_id: Some("req-2".to_string()),
                        preview: "one more thing".to_string(),
                        now: "2026-07-11T00:00:13Z".to_string(),
                    });
            }
            if matches!(self.action, ProviderAction::CompleteAndReset) {
                self.backend.transition(QuickSessionTransition::SetSummary {
                    actor: "bob".to_string(),
                    attempt_id: attempt.clone(),
                    summary: "Durable auth summary".to_string(),
                    now: "2026-07-11T00:00:14Z".to_string(),
                });
            }
            if !matches!(
                self.action,
                ProviderAction::MissingTitle
                    | ProviderAction::MissingReply
                    | ProviderAction::QueueHumanMissingReply
            ) {
                let output_line = self.backend.append_message("bob", input_line, "done");
                self.backend.transition(QuickSessionTransition::AgentReply {
                    actor: "bob".to_string(),
                    input_line,
                    attempt_id: attempt.clone(),
                    output_line,
                    preview: "done".to_string(),
                    now: "2026-07-11T00:00:15Z".to_string(),
                });
            }
        }
        let output = if matches!(self.action, ProviderAction::CompleteAndReset) {
            "done [[RESET]]"
        } else {
            "done"
        };
        let usage = ProviderUsage {
            input_tokens: Some(9_000),
            output_tokens: Some(100),
            ..Default::default()
        };
        MockProvider::with_response(output.to_string())
            .with_usage(usage)
            .execute(prompt, opts)
            .await
    }
}

fn value_after(prompt: &str, marker: &str) -> String {
    prompt
        .lines()
        .find_map(|line| line.strip_prefix(marker))
        .unwrap_or_else(|| panic!("missing marker {marker:?} in prompt:\n{prompt}"))
        .to_string()
}

fn message(line: u64, point_to: u64, author: &str, body: &str) -> ThreadEntry {
    ThreadEntry::Message(Message {
        line_number: line,
        point_to,
        author: Handler::new(author).unwrap(),
        timestamp: "2026-07-11T00:00:00Z".to_string(),
        body: body.to_string(),
        mentions: Vec::new(),
        links: Vec::new(),
    })
}

fn initial_detail() -> QuickSessionDetail {
    let mut meta = QuickSessionMeta::new(
        SESSION_ID.to_string(),
        "bob".to_string(),
        "alice".to_string(),
        "2026-07-11T00:00:00Z".to_string(),
    );
    apply_quick_session_transition(
        &mut meta,
        QuickSessionTransition::HumanMessage {
            actor: "alice".to_string(),
            line_number: 1,
            request_id: Some("req-1".to_string()),
            preview: "Check auth".to_string(),
            now: "2026-07-11T00:00:01Z".to_string(),
        },
    )
    .unwrap();
    QuickSessionDetail {
        meta,
        entries: vec![message(1, 0, "alice", "Check auth")],
        archived: false,
    }
}

fn harness(
    action: ProviderAction,
    self_managed: bool,
) -> (
    tempfile::TempDir,
    FakeBackend,
    RecordingFactory,
    QuickSessionExecutor,
    broadcast::Receiver<AgentActivityEvent>,
) {
    let temp = tempfile::tempdir().unwrap();
    let repo_root = temp.path().join("agent");
    std::fs::create_dir_all(repo_root.join(".gitim")).unwrap();
    let backend = FakeBackend::with_detail(initial_detail());
    let mut factory = RecordingFactory::new(backend.clone(), action, temp.path().to_path_buf());
    factory.self_managed = self_managed;
    let (tx, rx) = broadcast::channel(32);
    let config = QuickSessionExecutorConfig {
        repo_root,
        workspace_root: temp.path().to_path_buf(),
        workspace_id: "workspace".to_string(),
        handler: "bob".to_string(),
        provider_type: "mock".to_string(),
        provider_config: ProviderConfig {
            executable_path: Some("mock-bin".to_string()),
            env: HashMap::from([("TOKEN".to_string(), "secret".to_string())]),
        },
        model: Some("mock-model".to_string()),
        effort: Some("high".to_string()),
        custom_system_prompt: Some("custom instruction".to_string()),
        activity_tx: Some(tx),
    };
    let executor = QuickSessionExecutor::with_dependencies(
        config,
        Arc::new(backend.clone()),
        Arc::new(factory.clone()),
    );
    (temp, backend, factory, executor, rx)
}

#[tokio::test]
async fn provider_turn_inherits_config_but_not_primary_token() {
    let (temp, _backend, factory, executor, _rx) = harness(ProviderAction::Complete, false);
    let repo_root = temp.path().join("agent");
    let primary = AgentState {
        session_token: Some("primary-token".to_string()),
        ..AgentState::default()
    };
    primary.save(&repo_root).unwrap();

    assert!(matches!(
        executor.execute(SESSION_ID).await.unwrap(),
        QuickSessionRunOutcome::Completed { .. }
    ));

    let calls = factory.calls.lock().unwrap();
    let (config, _prompt, opts) = &calls[0];
    assert_eq!(config.env.get("TOKEN").map(String::as_str), Some("secret"));
    assert_eq!(config.executable_path.as_deref(), Some("mock-bin"));
    assert_eq!(opts.model.as_deref(), Some("mock-model"));
    assert_eq!(opts.effort.as_deref(), Some("high"));
    assert_eq!(opts.resume_token, None);
    assert_eq!(
        AgentState::load(&repo_root)
            .unwrap()
            .session_token
            .as_deref(),
        Some("primary-token")
    );
}

#[tokio::test]
async fn fresh_provider_instance_is_created_for_each_quick_session_turn() {
    let (_temp, backend, factory, executor, _rx) = harness(ProviderAction::Complete, false);
    executor.execute(SESSION_ID).await.unwrap();
    let line = backend.append_message("alice", 0, "follow up");
    backend.transition(QuickSessionTransition::HumanMessage {
        actor: "alice".to_string(),
        line_number: line,
        request_id: Some("req-follow-up".to_string()),
        preview: "follow up".to_string(),
        now: "2026-07-11T00:00:30Z".to_string(),
    });
    executor.execute(SESSION_ID).await.unwrap();

    let calls = factory.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert!(calls[0].2.resume_token.is_none());
    assert!(calls[1].2.resume_token.is_some());
}

#[tokio::test]
async fn prompt_is_bounded_and_names_exact_claim_context() {
    let (_temp, _backend, factory, executor, _rx) = harness(ProviderAction::Complete, false);
    executor.execute(SESSION_ID).await.unwrap();
    let calls = factory.calls.lock().unwrap();
    let (_config, prompt, opts) = &calls[0];
    let attempt = value_after(prompt, "Attempt: ");
    assert!(attempt.starts_with("qa-") && attempt.len() == 29);
    assert!(prompt.contains(&format!("Session id: {SESSION_ID}")));
    assert!(prompt.contains(&format!("Ref: session:{SESSION_ID}")));
    assert!(prompt.contains("Input line: L1"));
    assert!(prompt.contains("Title: (unset)"));
    assert!(prompt.contains("Summary: (none)"));
    assert!(prompt.contains("@alice: Check auth"));
    assert!(prompt.len() <= 16_000);
    let system = opts.system_prompt.as_deref().unwrap();
    assert!(system.contains("custom instruction"));
    assert!(system.contains("gitim session title"));
    assert!(system.contains("Set the title before the first reply"));
}

#[tokio::test]
async fn title_and_reply_complete_the_claim_and_emit_scoped_events() {
    let (_temp, backend, _factory, executor, mut rx) = harness(ProviderAction::Complete, false);
    let outcome = executor.execute(SESSION_ID).await.unwrap();
    assert_eq!(
        outcome,
        QuickSessionRunOutcome::Completed { requeue: false }
    );
    let detail = backend.detail();
    assert_eq!(detail.meta.status, QuickSessionStatus::Active);
    assert_eq!(detail.meta.title.as_deref(), Some("Investigate auth"));
    assert_eq!(detail.meta.last_completed_input_line, Some(1));
    let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert!(!events.is_empty());
    assert!(events.iter().all(|event| {
        event.scope == ActivityScope::QuickSession
            && event.session_id.as_deref() == Some(SESSION_ID)
            && event.r#ref.as_deref() == Some("session:qs-01JZZZZZZZZZZZZZZZZZZZZZZZ")
            && event.session_revision.is_some()
            && event
                .attempt_id
                .as_deref()
                .is_some_and(|id| id.starts_with("qa-"))
            && event.context_generation == Some(0)
    }));
}

#[tokio::test]
async fn missing_title_or_reply_marks_error_once() {
    for action in [ProviderAction::MissingTitle, ProviderAction::MissingReply] {
        let (_temp, backend, _factory, executor, _rx) = harness(action, false);
        assert_eq!(
            executor.execute(SESSION_ID).await.unwrap(),
            QuickSessionRunOutcome::Failed { requeue: false }
        );
        assert_eq!(backend.inner.lock().unwrap().mark_error_calls, 1);
        assert_eq!(backend.detail().meta.status, QuickSessionStatus::Error);
        assert_eq!(
            executor.execute(SESSION_ID).await.unwrap(),
            QuickSessionRunOutcome::Noop
        );
        assert_eq!(backend.inner.lock().unwrap().mark_error_calls, 1);
    }
}

#[tokio::test]
async fn queued_human_input_remains_actionable_after_reply() {
    let (_temp, backend, _factory, executor, _rx) =
        harness(ProviderAction::QueueHumanThenComplete, false);
    assert_eq!(
        executor.execute(SESSION_ID).await.unwrap(),
        QuickSessionRunOutcome::Completed { requeue: true }
    );
    let meta = backend.detail().meta;
    assert!(meta.last_human_line > meta.last_completed_input_line);
}

#[tokio::test]
async fn queued_human_input_remains_actionable_after_failed_turn() {
    let (_temp, backend, _factory, executor, _rx) =
        harness(ProviderAction::QueueHumanMissingReply, false);
    assert_eq!(
        executor.execute(SESSION_ID).await.unwrap(),
        QuickSessionRunOutcome::Failed { requeue: true }
    );
    let meta = backend.detail().meta;
    assert!(matches!(
        meta.status,
        QuickSessionStatus::NeedsTitle | QuickSessionStatus::Active
    ));
    assert!(meta.last_human_line > meta.last_completed_input_line);
}

#[tokio::test]
async fn archive_during_running_rejects_late_completion() {
    let (_temp, backend, _factory, executor, _rx) = harness(ProviderAction::Archive, false);
    assert_eq!(
        executor.execute(SESSION_ID).await.unwrap(),
        QuickSessionRunOutcome::Discarded
    );
    assert_eq!(backend.detail().meta.status, QuickSessionStatus::Archived);
    assert_eq!(backend.inner.lock().unwrap().mark_error_calls, 0);
}

#[tokio::test]
async fn late_attempt_or_generation_is_discarded() {
    let (_temp, backend, _factory, executor, _rx) = harness(ProviderAction::ReplaceAttempt, false);
    assert_eq!(
        executor.execute(SESSION_ID).await.unwrap(),
        QuickSessionRunOutcome::Discarded
    );
    assert_eq!(backend.inner.lock().unwrap().mark_error_calls, 0);

    let (_temp, backend, _factory, executor, _rx) = harness(ProviderAction::BumpGeneration, false);
    assert_eq!(
        executor.execute(SESSION_ID).await.unwrap(),
        QuickSessionRunOutcome::Discarded
    );
    assert_eq!(backend.inner.lock().unwrap().mark_error_calls, 0);
}

#[tokio::test]
async fn usage_and_reset_change_only_quick_state() {
    let (temp, _backend, _factory, executor, _rx) =
        harness(ProviderAction::CompleteAndReset, false);
    let repo_root = temp.path().join("agent");
    AgentState {
        session_token: Some("primary-token".to_string()),
        estimated_tokens: 123,
        ..AgentState::default()
    }
    .save(&repo_root)
    .unwrap();

    assert!(matches!(
        executor.execute(SESSION_ID).await.unwrap(),
        QuickSessionRunOutcome::Completed { .. }
    ));
    let quick = QuickSessionRuntimeState::load(temp.path(), SESSION_ID).unwrap();
    assert!(quick.session_token.is_none());
    assert!(quick.session_usage.is_none());
    assert_eq!(quick.context_generation, 1);
    let primary = AgentState::load(&repo_root).unwrap();
    assert_eq!(primary.session_token.as_deref(), Some("primary-token"));
    assert_eq!(primary.estimated_tokens, 123);
}

#[tokio::test]
async fn usage_warning_arms_context_handoff_on_the_next_quick_turn() {
    let (temp, backend, factory, executor, _rx) = harness(ProviderAction::Complete, false);
    executor.execute(SESSION_ID).await.unwrap();
    assert!(
        QuickSessionRuntimeState::load(temp.path(), SESSION_ID)
            .unwrap()
            .reset_required
    );
    let line = backend.append_message("alice", 0, "continue");
    backend.transition(QuickSessionTransition::HumanMessage {
        actor: "alice".to_string(),
        line_number: line,
        request_id: Some("req-continue".to_string()),
        preview: "continue".to_string(),
        now: "2026-07-11T00:00:31Z".to_string(),
    });
    executor.execute(SESSION_ID).await.unwrap();
    let calls = factory.calls.lock().unwrap();
    assert!(calls[1].1.contains("Context handoff required"));
    assert!(calls[1].1.contains("gitim session summarize"));
    assert!(calls[1].1.contains("[[RESET]]"));
}

#[tokio::test]
async fn self_managed_provider_skips_runtime_reset() {
    let (temp, _backend, _factory, executor, _rx) = harness(ProviderAction::CompleteAndReset, true);
    QuickSessionRuntimeState {
        session_token: Some("quick-token".to_string()),
        ..QuickSessionRuntimeState::default()
    }
    .save(temp.path(), SESSION_ID)
    .unwrap();
    executor.execute(SESSION_ID).await.unwrap();
    let quick = QuickSessionRuntimeState::load(temp.path(), SESSION_ID).unwrap();
    assert_eq!(quick.session_token.as_deref(), Some("quick-token"));
    assert_eq!(quick.context_generation, 0);
    assert!(!quick.reset_required);
}

#[tokio::test]
async fn startup_scan_finds_pre_cursor_actionable_session() {
    let (_temp, _backend, _factory, executor, _rx) = harness(ProviderAction::Complete, false);
    assert_eq!(
        executor.recover_actionable().await.unwrap(),
        vec![SESSION_ID]
    );
}

#[tokio::test]
async fn stale_running_claim_becomes_error_without_execution() {
    let (_temp, backend, factory, executor, _rx) = harness(ProviderAction::Complete, false);
    backend.transition(QuickSessionTransition::Claim {
        actor: "bob".to_string(),
        input_line: 1,
        attempt_id: "qa-01JYYYYYYYYYYYYYYYYYYYYYYY".to_string(),
        now: "2026-07-11T00:00:02Z".to_string(),
    });
    assert!(executor.recover_actionable().await.unwrap().is_empty());
    assert_eq!(backend.detail().meta.status, QuickSessionStatus::Error);
    assert_eq!(backend.inner.lock().unwrap().mark_error_calls, 1);
    assert!(factory.calls.lock().unwrap().is_empty());
}
