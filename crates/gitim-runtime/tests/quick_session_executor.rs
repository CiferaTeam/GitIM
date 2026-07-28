#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
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
use gitim_runtime::http::{
    ActivityScope, AgentActivityEvent, AgentInfo, RuntimeState, SharedRuntimeState,
};
use gitim_runtime::quick_session_executor::{
    QuickSessionBackend, QuickSessionExecutor, QuickSessionExecutorConfig,
    QuickSessionProviderFactory, QuickSessionRunOutcome,
};
use gitim_runtime::quick_session_state::QuickSessionRuntimeState;
use gitim_runtime::workspace::WorkspaceContext;
use gitim_runtime::AgentState;
use tokio::sync::{broadcast, Notify};

const SESSION_ID: &str = "qs-01JZZZZZZZZZZZZZZZZZZZZZZZ";

#[derive(Clone)]
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
    CompleteThenFlood,
    ArchiveThenUnarchive,
    WaitThenComplete {
        entered: Arc<Notify>,
        release: Arc<Notify>,
    },
    DetachedSession {
        release: Arc<Notify>,
        exited: Arc<Notify>,
        writes: Arc<AtomicUsize>,
    },
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
        status: Option<QuickSessionStatus>,
    ) -> Result<ListQuickSessionsResponse, String> {
        let guard = self.inner.lock().unwrap();
        let sessions = guard
            .detail
            .iter()
            .filter(|detail| {
                status.is_none_or(|status| detail.meta.status == status)
                    && (!actionable
                        || (matches!(
                            detail.meta.status,
                            QuickSessionStatus::NeedsTitle | QuickSessionStatus::Active
                        ) && detail.meta.last_human_line
                            > detail.meta.last_completed_input_line))
            })
            .map(|detail| QuickSessionListItem::from_meta(&detail.meta, detail.archived))
            .collect();
        Ok(ListQuickSessionsResponse { sessions })
    }

    async fn read(&self, _session_id: &str) -> Result<ReadQuickSessionResponse, String> {
        let mut session = self.detail();
        let drain = session.entries.len().saturating_sub(24);
        session.entries.drain(..drain);
        Ok(ReadQuickSessionResponse { session })
    }

    async fn read_since(
        &self,
        _session_id: &str,
        since: u64,
        limit: usize,
    ) -> Result<ReadQuickSessionResponse, String> {
        let mut session = self.detail();
        session.entries = session
            .entries
            .into_iter()
            .filter(|entry| entry.line_number() > since)
            .take(limit)
            .collect();
        Ok(ReadQuickSessionResponse { session })
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
struct CrowdedBackend {
    completed: Arc<Vec<QuickSessionDetail>>,
    actionable: QuickSessionDetail,
    running: Arc<Mutex<QuickSessionDetail>>,
    mark_error_calls: Arc<Mutex<usize>>,
}

#[derive(Clone)]
struct PartialFailureRecoveryBackend {
    sessions: Arc<Mutex<HashMap<String, QuickSessionDetail>>>,
    read_failure: String,
    mark_failure: String,
    marked: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl QuickSessionBackend for PartialFailureRecoveryBackend {
    async fn list(
        &self,
        _agent_id: &str,
        actionable: bool,
        status: Option<QuickSessionStatus>,
    ) -> Result<ListQuickSessionsResponse, String> {
        let sessions = self
            .sessions
            .lock()
            .unwrap()
            .values()
            .filter(|detail| {
                !actionable && status.is_none_or(|status| detail.meta.status == status)
            })
            .map(|detail| QuickSessionListItem::from_meta(&detail.meta, false))
            .collect();
        Ok(ListQuickSessionsResponse { sessions })
    }

    async fn read(&self, session_id: &str) -> Result<ReadQuickSessionResponse, String> {
        if session_id == self.read_failure {
            return Err("corrupt session detail".to_string());
        }
        self.sessions
            .lock()
            .unwrap()
            .get(session_id)
            .cloned()
            .map(|session| ReadQuickSessionResponse { session })
            .ok_or_else(|| "missing session".to_string())
    }

    async fn read_since(
        &self,
        _session_id: &str,
        _since: u64,
        _limit: usize,
    ) -> Result<ReadQuickSessionResponse, String> {
        Err("read_since not expected".to_string())
    }

    async fn claim(
        &self,
        _session_id: &str,
        _input_line: u64,
        _attempt_id: &str,
    ) -> Result<ClaimQuickSessionTurnResponse, String> {
        Err("claim not expected".to_string())
    }

    async fn mark_error(
        &self,
        session_id: &str,
        attempt_id: &str,
        error: &str,
    ) -> Result<MarkQuickSessionErrorResponse, String> {
        if session_id == self.mark_failure {
            return Err("mark rejected".to_string());
        }
        let mut sessions = self.sessions.lock().unwrap();
        let detail = sessions.get_mut(session_id).unwrap();
        apply_quick_session_transition(
            &mut detail.meta,
            QuickSessionTransition::MarkError {
                actor: "bob".to_string(),
                attempt_id: attempt_id.to_string(),
                error: error.to_string(),
                now: "2026-07-11T00:10:00Z".to_string(),
            },
        )
        .map_err(|error| error.to_string())?;
        self.marked.lock().unwrap().push(session_id.to_string());
        Ok(MarkQuickSessionErrorResponse {
            session_id: session_id.to_string(),
            status: detail.meta.status,
            revision: detail.meta.revision,
        })
    }
}

#[async_trait]
impl QuickSessionBackend for CrowdedBackend {
    async fn list(
        &self,
        _agent_id: &str,
        actionable: bool,
        status: Option<QuickSessionStatus>,
    ) -> Result<ListQuickSessionsResponse, String> {
        let sessions = if status == Some(QuickSessionStatus::Running) {
            vec![QuickSessionListItem::from_meta(
                &self.running.lock().unwrap().meta,
                false,
            )]
        } else if actionable {
            vec![QuickSessionListItem::from_meta(
                &self.actionable.meta,
                false,
            )]
        } else {
            self.completed
                .iter()
                .take(100)
                .map(|detail| QuickSessionListItem::from_meta(&detail.meta, false))
                .collect()
        };
        Ok(ListQuickSessionsResponse { sessions })
    }

    async fn read(&self, session_id: &str) -> Result<ReadQuickSessionResponse, String> {
        let session = if self.actionable.meta.id == session_id {
            self.actionable.clone()
        } else if self.running.lock().unwrap().meta.id == session_id {
            self.running.lock().unwrap().clone()
        } else {
            self.completed
                .iter()
                .find(|detail| detail.meta.id == session_id)
                .cloned()
                .ok_or_else(|| "missing crowded session".to_string())?
        };
        Ok(ReadQuickSessionResponse { session })
    }

    async fn read_since(
        &self,
        session_id: &str,
        since: u64,
        limit: usize,
    ) -> Result<ReadQuickSessionResponse, String> {
        let mut response = self.read(session_id).await?;
        response.session.entries = response
            .session
            .entries
            .into_iter()
            .filter(|entry| entry.line_number() > since)
            .take(limit)
            .collect();
        Ok(response)
    }

    async fn claim(
        &self,
        _session_id: &str,
        _input_line: u64,
        _attempt_id: &str,
    ) -> Result<ClaimQuickSessionTurnResponse, String> {
        Err("claim not expected during recovery".to_string())
    }

    async fn mark_error(
        &self,
        session_id: &str,
        attempt_id: &str,
        error: &str,
    ) -> Result<MarkQuickSessionErrorResponse, String> {
        *self.mark_error_calls.lock().unwrap() += 1;
        let mut detail = self.running.lock().unwrap();
        assert_eq!(detail.meta.id, session_id);
        apply_quick_session_transition(
            &mut detail.meta,
            QuickSessionTransition::MarkError {
                actor: "bob".to_string(),
                attempt_id: attempt_id.to_string(),
                error: error.to_string(),
                now: "2026-07-11T00:10:00Z".to_string(),
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
            action: self.action.clone(),
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
        if let ProviderAction::DetachedSession {
            release,
            exited,
            writes,
        } = &self.action
        {
            let (event_tx, event_rx) = tokio::sync::mpsc::channel(1);
            let (result_tx, result_rx) = tokio::sync::oneshot::channel();
            let release = release.clone();
            let exited = exited.clone();
            let writes = writes.clone();
            let task = tokio::spawn(async move {
                struct ExitSignal(Arc<Notify>);
                impl Drop for ExitSignal {
                    fn drop(&mut self) {
                        self.0.notify_one();
                    }
                }
                let _exit_signal = ExitSignal(exited);
                let _ = event_tx
                    .send(gitim_agent_provider::Event::Status {
                        status: "provider-session-ready".to_string(),
                    })
                    .await;
                release.notified().await;
                writes.fetch_add(1, Ordering::SeqCst);
                drop(result_tx);
            });
            return Ok(Session::new(
                event_rx,
                result_rx,
                task.abort_handle(),
                tokio_util::sync::CancellationToken::new(),
            ));
        }
        if let ProviderAction::WaitThenComplete { entered, release } = &self.action {
            entered.notify_one();
            release.notified().await;
        }
        if matches!(&self.action, ProviderAction::Archive) {
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
        } else if matches!(&self.action, ProviderAction::BumpGeneration) {
            let mut state =
                QuickSessionRuntimeState::load(&self.workspace_root, SESSION_ID).unwrap();
            state.bump_context_generation();
            state.save(&self.workspace_root, SESSION_ID).unwrap();
        } else if matches!(&self.action, ProviderAction::ReplaceAttempt) {
            let mut guard = self.backend.inner.lock().unwrap();
            let detail = guard.detail.as_mut().unwrap();
            detail.meta.attempt_id = Some("qa-01K00000000000000000000000".to_string());
            detail.meta.revision += 1;
        } else if matches!(&self.action, ProviderAction::ArchiveThenUnarchive) {
            let mut guard = self.backend.inner.lock().unwrap();
            let detail = guard.detail.as_mut().unwrap();
            detail.meta.status = QuickSessionStatus::NeedsTitle;
            detail.meta.attempt_id = None;
            detail.meta.processing_input_line = None;
            detail.meta.processing_started_at = None;
            detail.meta.archived_at = None;
            detail.meta.archived_from = None;
            detail.meta.revision += 2;
        } else {
            if !matches!(&self.action, ProviderAction::MissingTitle) {
                self.backend.transition(QuickSessionTransition::SetTitle {
                    actor: "bob".to_string(),
                    attempt_id: attempt.clone(),
                    title: "Investigate auth".to_string(),
                    now: "2026-07-11T00:00:12Z".to_string(),
                });
            }
            if matches!(
                &self.action,
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
            if matches!(&self.action, ProviderAction::CompleteAndReset) {
                self.backend.transition(QuickSessionTransition::SetSummary {
                    actor: "bob".to_string(),
                    attempt_id: attempt.clone(),
                    summary: "Durable auth summary".to_string(),
                    now: "2026-07-11T00:00:14Z".to_string(),
                });
            }
            if !matches!(
                &self.action,
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
                if matches!(&self.action, ProviderAction::CompleteThenFlood) {
                    for index in 0..30 {
                        let body = format!("queued-{index}");
                        let line = self.backend.append_message("alice", 0, &body);
                        self.backend
                            .transition(QuickSessionTransition::HumanMessage {
                                actor: "alice".to_string(),
                                line_number: line,
                                request_id: Some(format!("flood-{index}")),
                                preview: body,
                                now: "2026-07-11T00:00:16Z".to_string(),
                            });
                    }
                }
            }
        }
        let output = if matches!(&self.action, ProviderAction::CompleteAndReset) {
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

fn completed_detail(index: usize) -> QuickSessionDetail {
    let id = format!("qs-{index:026}");
    let mut detail = initial_detail();
    detail.meta.id = id;
    detail.meta.title = Some("Completed".to_string());
    detail.meta.status = QuickSessionStatus::Active;
    detail.meta.last_completed_attempt_id = Some("qa-01JZZZZZZZZZZZZZZZZZZZZZZZ".to_string());
    detail.meta.last_completed_input_line = Some(1);
    detail.meta.last_completed_line = Some(2);
    detail.entries.push(message(2, 1, "bob", "done"));
    detail
}

fn running_detail() -> QuickSessionDetail {
    running_detail_with(
        "qs-01K00000000000000000000000",
        "qa-01K00000000000000000000000",
    )
}

fn running_detail_with(id: &str, attempt_id: &str) -> QuickSessionDetail {
    let mut detail = initial_detail();
    detail.meta.id = id.to_string();
    apply_quick_session_transition(
        &mut detail.meta,
        QuickSessionTransition::Claim {
            actor: "bob".to_string(),
            input_line: 1,
            attempt_id: attempt_id.to_string(),
            now: "20260711T000002Z".to_string(),
        },
    )
    .unwrap();
    detail
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
    harness_with_runtime(action, self_managed, None)
}

fn harness_with_runtime(
    action: ProviderAction,
    self_managed: bool,
    runtime_state: Option<SharedRuntimeState>,
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
        runtime_state,
    };
    let executor = QuickSessionExecutor::with_dependencies(
        config,
        Arc::new(backend.clone()),
        Arc::new(factory.clone()),
    );
    (temp, backend, factory, executor, rx)
}

fn shared_runtime_state(
    workspace_root: &std::path::Path,
    repo_root: &std::path::Path,
) -> SharedRuntimeState {
    let mut context = WorkspaceContext::new(
        "workspace".to_string(),
        "workspace".to_string(),
        workspace_root.to_path_buf(),
    );
    context.agents.insert(
        "bob".to_string(),
        AgentInfo {
            id: "bob".to_string(),
            handler: "bob".to_string(),
            display_name: "Bob".to_string(),
            status: "running".to_string(),
            last_activity: None,
            messages_processed: 0,
            repo_path: repo_root.display().to_string(),
            provider: Some("mock".to_string()),
            model: Some("mock-model".to_string()),
            effort: None,
            system_prompt: None,
            introduction: None,
            env: HashMap::new(),
            error_message: None,
            session_usage: None,
            llm_provider: None,
            llm_model: None,
            usage_summary: None,
            saturation_summary: None,
            is_working: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            loop_handle: None,
        },
    );
    let state = Arc::new(Mutex::new(RuntimeState::default()));
    state
        .lock()
        .unwrap()
        .workspaces
        .insert("workspace".to_string(), context);
    state
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
async fn oversized_claimed_message_stays_meaningfully_present_in_bounded_prompt() {
    let (_temp, backend, factory, executor, _rx) = harness(ProviderAction::Complete, false);
    let marker = "CLAIMED-CONTENT-MARKER";
    let body = format!("{marker} {}", "x".repeat(64 * 1024));
    {
        let mut guard = backend.inner.lock().unwrap();
        let detail = guard.detail.as_mut().unwrap();
        detail.entries = vec![message(1, 0, "alice", &body)];
        detail.meta.last_message_preview = marker.to_string();
    }

    executor.execute(SESSION_ID).await.unwrap();
    let calls = factory.calls.lock().unwrap();
    let prompt = &calls[0].1;
    assert!(prompt.contains("L1 @alice:"));
    assert!(prompt.contains(marker));
    assert!(prompt.contains("[truncated]"));
    assert!(prompt.chars().count() <= 12_000);
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
async fn completion_verification_survives_more_than_twenty_four_queued_messages() {
    let (temp, backend, _factory, executor, _rx) =
        harness(ProviderAction::CompleteThenFlood, false);
    assert_eq!(
        executor.execute(SESSION_ID).await.unwrap(),
        QuickSessionRunOutcome::Completed { requeue: true }
    );
    let local = QuickSessionRuntimeState::load(temp.path(), SESSION_ID).unwrap();
    assert!(local.session_token.is_some());
    assert!(backend.detail().meta.last_completed_line.is_some());
    assert!(local.session_usage.is_some());
}

#[tokio::test]
async fn archive_then_unarchive_discards_late_result_without_state_acceptance() {
    let (temp, backend, _factory, executor, _rx) =
        harness(ProviderAction::ArchiveThenUnarchive, false);
    let original = QuickSessionRuntimeState {
        session_token: Some("original-token".to_string()),
        context_generation: 7,
        ..QuickSessionRuntimeState::default()
    };
    original.save(temp.path(), SESSION_ID).unwrap();

    assert_eq!(
        executor.execute(SESSION_ID).await.unwrap(),
        QuickSessionRunOutcome::Discarded
    );
    assert_eq!(backend.inner.lock().unwrap().mark_error_calls, 0);
    let local = QuickSessionRuntimeState::load(temp.path(), SESSION_ID).unwrap();
    assert_eq!(local, original);
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
async fn restart_without_local_state_recovers_stale_compact_claim_without_execution() {
    let (temp, backend, factory, executor, _rx) = harness(ProviderAction::Complete, false);
    backend.transition(QuickSessionTransition::Claim {
        actor: "bob".to_string(),
        input_line: 1,
        attempt_id: "qa-01JYYYYYYYYYYYYYYYYYYYYYYY".to_string(),
        now: "20260711T000002Z".to_string(),
    });
    assert!(
        !QuickSessionRuntimeState::state_path(temp.path(), SESSION_ID)
            .unwrap()
            .exists()
    );
    let executor = executor.with_stale_claim_after(std::time::Duration::ZERO);
    assert!(executor.recover_actionable().await.unwrap().is_empty());
    assert_eq!(backend.detail().meta.status, QuickSessionStatus::Error);
    assert_eq!(backend.inner.lock().unwrap().mark_error_calls, 1);
    assert!(executor.recover_actionable().await.unwrap().is_empty());
    assert_eq!(backend.inner.lock().unwrap().mark_error_calls, 1);
    assert!(factory.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn non_compact_claim_timestamp_is_not_treated_as_stale() {
    // Only the daemon's compact format is a production claim timestamp; an
    // RFC3339 value is unparseable and must never trip the staleness path.
    let (_temp, backend, factory, executor, _rx) = harness(ProviderAction::Complete, false);
    backend.transition(QuickSessionTransition::Claim {
        actor: "bob".to_string(),
        input_line: 1,
        attempt_id: "qa-01JYYYYYYYYYYYYYYYYYYYYYYY".to_string(),
        now: "2026-07-11T00:00:02Z".to_string(),
    });

    let executor = executor.with_stale_claim_after(std::time::Duration::ZERO);
    assert!(executor.recover_actionable().await.unwrap().is_empty());
    assert_eq!(backend.detail().meta.status, QuickSessionStatus::Running);
    assert_eq!(backend.inner.lock().unwrap().mark_error_calls, 0);
    assert!(factory.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn malformed_compact_claim_timestamp_is_not_treated_as_stale() {
    let (_temp, backend, factory, executor, _rx) = harness(ProviderAction::Complete, false);
    backend.transition(QuickSessionTransition::Claim {
        actor: "bob".to_string(),
        input_line: 1,
        attempt_id: "qa-01JYYYYYYYYYYYYYYYYYYYYYYY".to_string(),
        now: "20260711 000002Z".to_string(),
    });

    let executor = executor.with_stale_claim_after(std::time::Duration::ZERO);
    assert!(executor.recover_actionable().await.unwrap().is_empty());
    assert_eq!(backend.detail().meta.status, QuickSessionStatus::Running);
    assert_eq!(backend.inner.lock().unwrap().mark_error_calls, 0);
    assert!(factory.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn fresh_unowned_running_claim_remains_untouched() {
    let (_temp, backend, factory, executor, _rx) = harness(ProviderAction::Complete, false);
    backend.transition(QuickSessionTransition::Claim {
        actor: "bob".to_string(),
        input_line: 1,
        attempt_id: "qa-01JYYYYYYYYYYYYYYYYYYYYYYY".to_string(),
        now: chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string(),
    });

    assert!(executor.recover_actionable().await.unwrap().is_empty());
    assert_eq!(backend.detail().meta.status, QuickSessionStatus::Running);
    assert_eq!(backend.inner.lock().unwrap().mark_error_calls, 0);
    assert!(factory.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn recovery_scan_leaves_a_live_owned_claim_untouched() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (_temp, backend, _factory, executor, _rx) = harness(
        ProviderAction::WaitThenComplete {
            entered: entered.clone(),
            release: release.clone(),
        },
        false,
    );
    let recovery_executor = executor.clone();
    let task = tokio::spawn(async move { executor.execute(SESSION_ID).await });
    entered.notified().await;

    assert!(recovery_executor
        .recover_actionable()
        .await
        .unwrap()
        .is_empty());
    assert_eq!(backend.detail().meta.status, QuickSessionStatus::Running);
    assert_eq!(backend.inner.lock().unwrap().mark_error_calls, 0);

    release.notify_one();
    assert!(matches!(
        task.await.unwrap().unwrap(),
        QuickSessionRunOutcome::Completed { .. }
    ));
}

#[tokio::test]
async fn cancelled_executor_clears_active_attempt_ownership() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (temp, backend, _factory, executor, _rx) = harness(
        ProviderAction::WaitThenComplete {
            entered: entered.clone(),
            release,
        },
        false,
    );
    let task = tokio::spawn(async move { executor.execute(SESSION_ID).await });
    entered.notified().await;
    assert!(QuickSessionRuntimeState::load(temp.path(), SESSION_ID)
        .unwrap()
        .active_attempt_id
        .is_some());

    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(backend.detail().meta.status, QuickSessionStatus::Running);
    assert_eq!(
        QuickSessionRuntimeState::load(temp.path(), SESSION_ID)
            .unwrap()
            .active_attempt_id,
        None
    );
}

#[tokio::test]
async fn cancellation_after_session_return_aborts_provider_before_late_write() {
    let release = Arc::new(Notify::new());
    let exited = Arc::new(Notify::new());
    let writes = Arc::new(AtomicUsize::new(0));
    let (temp, backend, _factory, executor, mut rx) = harness(
        ProviderAction::DetachedSession {
            release: release.clone(),
            exited: exited.clone(),
            writes: writes.clone(),
        },
        false,
    );
    let task = tokio::spawn(async move { executor.execute(SESSION_ID).await });
    loop {
        let event = rx.recv().await.unwrap();
        if event.event_type == "status" && event.detail == "provider-session-ready" {
            break;
        }
    }

    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    release.notify_waiters();
    exited.notified().await;

    assert_eq!(writes.load(Ordering::SeqCst), 0);
    assert_eq!(backend.detail().meta.status, QuickSessionStatus::Running);
    assert_eq!(
        QuickSessionRuntimeState::load(temp.path(), SESSION_ID)
            .unwrap()
            .active_attempt_id,
        None
    );
}

#[tokio::test]
async fn archiving_a_running_session_discards_late_result_after_turn_completes() {
    let release = Arc::new(Notify::new());
    let exited = Arc::new(Notify::new());
    let writes = Arc::new(AtomicUsize::new(0));
    let (temp, backend, _factory, executor, mut rx) = harness(
        ProviderAction::DetachedSession {
            release: release.clone(),
            exited: exited.clone(),
            writes: writes.clone(),
        },
        false,
    );
    let task = tokio::spawn(async move { executor.execute(SESSION_ID).await });
    loop {
        let event = rx.recv().await.unwrap();
        if event.event_type == "status" && event.detail == "provider-session-ready" {
            break;
        }
    }

    backend.transition(QuickSessionTransition::Archive {
        actor: "alice".to_string(),
        now: "2026-07-11T00:00:11Z".to_string(),
    });
    // Claim loss no longer aborts the in-flight turn; the provider runs to
    // completion and the late result is rejected by the ownership check.
    release.notify_one();
    let outcome = tokio::time::timeout(std::time::Duration::from_millis(250), task)
        .await
        .expect("discarded completion should be bounded")
        .unwrap()
        .unwrap();
    assert_eq!(outcome, QuickSessionRunOutcome::Discarded);

    tokio::time::timeout(std::time::Duration::from_millis(250), exited.notified())
        .await
        .expect("provider task should run to completion");
    assert_eq!(writes.load(Ordering::SeqCst), 1);
    assert_eq!(backend.detail().meta.status, QuickSessionStatus::Archived);
    assert_eq!(backend.inner.lock().unwrap().mark_error_calls, 0);
    let local = QuickSessionRuntimeState::load(temp.path(), SESSION_ID).unwrap();
    assert_eq!(local.active_attempt_id, None);
    assert_eq!(local.session_token, None);
    assert_eq!(local.session_usage, None);
}

#[tokio::test]
async fn archive_then_unarchive_cannot_revive_a_running_provider_attempt() {
    let release = Arc::new(Notify::new());
    let exited = Arc::new(Notify::new());
    let writes = Arc::new(AtomicUsize::new(0));
    let (temp, backend, _factory, executor, mut rx) = harness(
        ProviderAction::DetachedSession {
            release: release.clone(),
            exited: exited.clone(),
            writes: writes.clone(),
        },
        false,
    );
    let task = tokio::spawn(async move { executor.execute(SESSION_ID).await });
    loop {
        let event = rx.recv().await.unwrap();
        if event.event_type == "status" && event.detail == "provider-session-ready" {
            break;
        }
    }

    backend.transition(QuickSessionTransition::Archive {
        actor: "alice".to_string(),
        now: "2026-07-11T00:00:11Z".to_string(),
    });
    backend.transition(QuickSessionTransition::Unarchive {
        actor: "alice".to_string(),
        now: "2026-07-11T00:00:12Z".to_string(),
    });
    // The stale attempt runs to completion; its late result cannot revive the
    // session and is rejected by the ownership check.
    release.notify_one();
    let outcome = tokio::time::timeout(std::time::Duration::from_millis(250), task)
        .await
        .expect("discarded completion should be bounded")
        .unwrap()
        .unwrap();
    assert_eq!(outcome, QuickSessionRunOutcome::Discarded);

    tokio::time::timeout(std::time::Duration::from_millis(250), exited.notified())
        .await
        .expect("provider task should run to completion");
    assert_eq!(writes.load(Ordering::SeqCst), 1);
    assert_eq!(backend.detail().meta.status, QuickSessionStatus::NeedsTitle);
    assert_eq!(backend.inner.lock().unwrap().mark_error_calls, 0);
    assert_eq!(
        QuickSessionRuntimeState::load(temp.path(), SESSION_ID)
            .unwrap()
            .active_attempt_id,
        None
    );
}

#[tokio::test]
async fn matching_crash_marker_recovers_immediately_and_clears_ownership() {
    let (temp, backend, _factory, executor, _rx) = harness(ProviderAction::Complete, false);
    let attempt_id = "qa-01JYYYYYYYYYYYYYYYYYYYYYYY";
    backend.transition(QuickSessionTransition::Claim {
        actor: "bob".to_string(),
        input_line: 1,
        attempt_id: attempt_id.to_string(),
        now: chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string(),
    });
    QuickSessionRuntimeState {
        active_attempt_id: Some(attempt_id.to_string()),
        ..QuickSessionRuntimeState::default()
    }
    .save(temp.path(), SESSION_ID)
    .unwrap();

    assert!(executor.recover_actionable().await.unwrap().is_empty());
    assert_eq!(backend.detail().meta.status, QuickSessionStatus::Error);
    assert_eq!(backend.inner.lock().unwrap().mark_error_calls, 1);
    assert_eq!(
        QuickSessionRuntimeState::load(temp.path(), SESSION_ID)
            .unwrap()
            .active_attempt_id,
        None
    );
}

#[tokio::test]
async fn recovery_finds_old_actionable_and_running_behind_completed_sessions() {
    let temp = tempfile::tempdir().unwrap();
    let fake = FakeBackend::with_detail(initial_detail());
    let factory = RecordingFactory::new(fake, ProviderAction::Complete, temp.path().to_path_buf());
    let backend = CrowdedBackend {
        completed: Arc::new((0..101).map(completed_detail).collect()),
        actionable: initial_detail(),
        running: Arc::new(Mutex::new(running_detail())),
        mark_error_calls: Arc::new(Mutex::new(0)),
    };
    let config = QuickSessionExecutorConfig {
        repo_root: temp.path().join("agent"),
        workspace_root: temp.path().to_path_buf(),
        workspace_id: "workspace".to_string(),
        handler: "bob".to_string(),
        provider_type: "mock".to_string(),
        provider_config: ProviderConfig::default(),
        model: None,
        effort: None,
        custom_system_prompt: None,
        activity_tx: None,
        runtime_state: None,
    };
    let executor = QuickSessionExecutor::with_dependencies(
        config,
        Arc::new(backend.clone()),
        Arc::new(factory),
    )
    .with_stale_claim_after(std::time::Duration::ZERO);

    let recovered = executor.recover_actionable().await.unwrap();
    assert_eq!(recovered, vec![SESSION_ID.to_string()]);
    assert_eq!(*backend.mark_error_calls.lock().unwrap(), 1);
    assert_eq!(
        backend.running.lock().unwrap().meta.status,
        QuickSessionStatus::Error
    );
}

#[tokio::test]
async fn recovery_continues_after_per_session_read_and_mark_failures() {
    let temp = tempfile::tempdir().unwrap();
    let read_failure = "qs-01K00000000000000000000001".to_string();
    let mark_failure = "qs-01K00000000000000000000002".to_string();
    let recoverable = "qs-01K00000000000000000000003".to_string();
    let sessions = HashMap::from([
        (
            read_failure.clone(),
            running_detail_with(&read_failure, "qa-01K00000000000000000000001"),
        ),
        (
            mark_failure.clone(),
            running_detail_with(&mark_failure, "qa-01K00000000000000000000002"),
        ),
        (
            recoverable.clone(),
            running_detail_with(&recoverable, "qa-01K00000000000000000000003"),
        ),
    ]);
    let backend = PartialFailureRecoveryBackend {
        sessions: Arc::new(Mutex::new(sessions)),
        read_failure,
        mark_failure: mark_failure.clone(),
        marked: Arc::new(Mutex::new(Vec::new())),
    };
    let fake = FakeBackend::with_detail(initial_detail());
    let factory = RecordingFactory::new(fake, ProviderAction::Complete, temp.path().to_path_buf());
    let executor = QuickSessionExecutor::with_dependencies(
        QuickSessionExecutorConfig {
            repo_root: temp.path().join("agent"),
            workspace_root: temp.path().to_path_buf(),
            workspace_id: "workspace".to_string(),
            handler: "bob".to_string(),
            provider_type: "mock".to_string(),
            provider_config: ProviderConfig::default(),
            model: None,
            effort: None,
            custom_system_prompt: None,
            activity_tx: None,
            runtime_state: None,
        },
        Arc::new(backend.clone()),
        Arc::new(factory),
    )
    .with_stale_claim_after(std::time::Duration::ZERO);

    assert!(executor.recover_actionable().await.unwrap().is_empty());
    assert_eq!(
        backend.marked.lock().unwrap().as_slice(),
        std::slice::from_ref(&recoverable)
    );
    let sessions = backend.sessions.lock().unwrap();
    assert_eq!(
        sessions[&mark_failure].meta.status,
        QuickSessionStatus::Running
    );
    assert_eq!(
        sessions[&recoverable].meta.status,
        QuickSessionStatus::Error
    );
}

#[tokio::test]
async fn quick_session_usage_updates_live_summary_without_primary_snapshot() {
    let state = shared_runtime_state(
        std::path::Path::new("/tmp/ws"),
        std::path::Path::new("/tmp/ws/bob"),
    );
    let (_temp, _backend, _factory, executor, mut rx) =
        harness_with_runtime(ProviderAction::Complete, false, Some(state.clone()));

    executor.execute(SESSION_ID).await.unwrap();

    let guard = state.lock().unwrap();
    let info = &guard.workspaces["workspace"].agents["bob"];
    assert!(
        info.session_usage.is_none(),
        "primary snapshot must stay isolated"
    );
    let summary = info.usage_summary.as_ref().expect("live usage summary");
    assert_eq!(summary.totals.turns, 1);
    assert_eq!(summary.totals.input, 9_000);
    drop(guard);

    let usage_payloads: Vec<serde_json::Value> = std::iter::from_fn(|| rx.try_recv().ok())
        .filter(|event| event.event_type == "usage")
        .filter_map(|event| serde_json::from_str(&event.detail).ok())
        .collect();
    assert!(!usage_payloads.is_empty());
    assert!(usage_payloads
        .iter()
        .all(|payload| payload.get("used_percent").is_some()));
    assert!(usage_payloads
        .iter()
        .any(|payload| payload.get("usage_summary").is_some()));
}

#[tokio::test]
async fn quick_session_usage_save_failure_increments_runtime_health_counter() {
    let state = shared_runtime_state(
        std::path::Path::new("/tmp/ws"),
        std::path::Path::new("/tmp/ws/bob"),
    );
    let (temp, _backend, _factory, executor, _rx) =
        harness_with_runtime(ProviderAction::Complete, false, Some(state.clone()));
    std::fs::create_dir_all(temp.path().join(".gitim-runtime")).unwrap();
    std::fs::write(temp.path().join(".gitim-runtime/usage"), "blocks directory").unwrap();

    executor.execute(SESSION_ID).await.unwrap();

    assert_eq!(
        state
            .lock()
            .unwrap()
            .usage_save_failures
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
}
