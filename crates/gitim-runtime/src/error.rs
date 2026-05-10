use gitim_client::ClientError;

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("git clone failed: {0}")]
    GitCloneFailed(String),

    #[error("daemon start failed: {0}")]
    DaemonStartFailed(#[from] ClientError),

    #[error("onboard failed: {0}")]
    OnboardFailed(String),

    #[error("poll failed: {0}")]
    PollFailed(String),

    /// The agent's own user.meta.yaml has been moved to `archive/users/`
    /// — the agent self-departed via `gitim burn-self`, or another clone
    /// burned this handler and the change has synced in. Daemon surfaces
    /// this with `error_code: "self_departed"`; the runtime agent_loop
    /// must NOT back off and retry — it must drive self-cleanup
    /// (kill daemon + rm clone + ctx.agents removal + SSE) and exit
    /// the loop. See archive-protocol B.4.
    #[error("agent self-departed via burn-self")]
    SelfDeparted,

    #[error("provider failed: {0}")]
    ProviderFailed(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
