use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

use gitim_runtime::http::DEFAULT_PORT;

fn runtime_pid_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".gitim/runtime.pid"))
}

fn runtime_pid_file_points_to_current_process() -> bool {
    let Some(pid_path) = runtime_pid_path() else {
        return true;
    };
    pid_file_points_to_process(&pid_path, std::process::id())
}

fn pid_file_points_to_process(pid_path: &Path, pid: u32) -> bool {
    match std::fs::read_to_string(pid_path) {
        Ok(recorded) => recorded.trim() == pid.to_string(),
        Err(_) => true,
    }
}

fn cleanup_pid_file() {
    let Some(pid_path) = runtime_pid_path() else {
        return;
    };
    if runtime_pid_file_points_to_current_process() {
        let _ = std::fs::remove_file(pid_path);
    }
}

/// gitim-runtime: dual-mode binary.
///
/// No subcommand: runs the HTTP server (default; backs the WebUI and agent
/// lifecycle). With a subcommand: one-shot CLI that shells out to a running
/// runtime over HTTP, so AI agents and scripts can drive the runtime without
/// the WebUI.
///
/// Subcommand bodies are placeholders in this scaffolding pass — actual
/// behavior lands in later tasks (Tasks 6-12 of the runtime-cli plan).
#[derive(Parser, Debug)]
#[command(name = "gitim-runtime", version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,

    /// Port to bind the HTTP server on (server mode only).
    #[arg(long)]
    port: Option<u16>,

    /// Daemonize: fork-exec a detached server and exit (server mode only).
    #[arg(long, short = 'd')]
    daemon: bool,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Show runtime status (running/stopped, port, version).
    Status,
    /// Print the device-bound runtime ID.
    RuntimeId,
    /// List workspaces known to the runtime.
    Workspaces,
    /// List agents in a workspace.
    ListAgents {
        /// Workspace slug. Optional when exactly one workspace is configured.
        #[arg(long)]
        workspace: Option<String>,
        /// Include sensitive fields (repo_path, system_prompt, env). Env values
        /// still pass through secret-key redaction before printing.
        #[arg(long)]
        detailed: bool,
    },
    /// Provision a new agent in a workspace.
    AddAgent {
        /// Workspace slug. Optional when exactly one workspace is configured.
        #[arg(long)]
        workspace: Option<String>,
        /// Agent handler — lowercase a-z 0-9 hyphens, 1-39 chars. Required.
        /// Runtime enforces the format and uniqueness against the workspace.
        #[arg(long)]
        handler: String,
        /// Human-readable display name. Required.
        #[arg(long = "display-name")]
        display_name: String,
        /// LLM provider (claude / codex / hermes / opencode / pi).
        /// Runtime owns the whitelist — invalid values come back as 4xx.
        #[arg(long)]
        provider: String,
        /// Optional model override (e.g. "claude-opus-4-7"). Provider-specific
        /// semantics; passed through verbatim.
        #[arg(long)]
        model: Option<String>,
        /// Inline system prompt. Mutually exclusive with --system-prompt-file.
        #[arg(long = "system-prompt", conflicts_with = "system_prompt_file")]
        system_prompt: Option<String>,
        /// Read system prompt from a file (≤ 64KB).
        #[arg(long = "system-prompt-file")]
        system_prompt_file: Option<PathBuf>,
        /// Repeatable: --env KEY=VALUE. Empty values are allowed.
        #[arg(long, value_name = "KEY=VALUE")]
        env: Vec<String>,
        /// Optional human blurb shown on the agent card.
        #[arg(long)]
        introduction: Option<String>,
        /// Opt the new agent out of joining #general. Default: join.
        #[arg(long = "no-join-general")]
        no_join_general: bool,
        /// Hermes only: LLM provider id (e.g. "anthropic", "custom:foo").
        #[arg(long = "llm-provider")]
        llm_provider: Option<String>,
        /// Hermes only: model id to set as profile default.
        #[arg(long = "llm-model")]
        llm_model: Option<String>,
    },
    /// Depart an agent from the workspace.
    ///
    /// Default: ritual burn via `POST /agents/burn` — runs the archive
    /// protocol (audit commits + workspace-wide departure event) then
    /// removes the clone. Use `--hard` to skip the protocol and quietly
    /// delete via `POST /agents/remove` with `hard_delete: true`.
    BurnAgent {
        /// Workspace slug. Optional when exactly one workspace is configured.
        #[arg(long)]
        workspace: Option<String>,
        /// Agent id to burn. Note this is the agent **id**, which is what
        /// `/agents/list` returns under `agents[].id`. It happens to equal
        /// the handler today, but the wire shape is id-based on both
        /// `/burn` and `/remove`.
        #[arg(long)]
        id: String,
        /// Hard remove: bypass the ritual-burn audit protocol and call
        /// `/agents/remove { hard_delete: true }` instead. No SSE
        /// broadcast, no archive commits. Use only when the ritual path
        /// can't run (broken daemon, missing remote, dev resets).
        #[arg(long)]
        hard: bool,
    },
    /// Update an existing agent's editable fields.
    ///
    /// V1 supports omitting a flag (no-op) or setting a value. There is
    /// no "clear to null" path — `--clear-*` flags can be added in a
    /// future revision if the demand shows up. At least one update flag
    /// must be specified; an empty patch is treated as a user mistake
    /// and rejected client-side.
    UpdateAgent {
        /// Workspace slug. Optional when exactly one workspace is configured.
        #[arg(long)]
        workspace: Option<String>,
        /// Agent id to patch. Wire shape matches the path param of
        /// `PATCH /workspaces/{slug}/agents/{id}`.
        #[arg(long)]
        id: String,
        /// Inline replacement system prompt. Mutually exclusive with
        /// `--system-prompt-file`.
        #[arg(long = "system-prompt", conflicts_with = "system_prompt_file")]
        system_prompt: Option<String>,
        /// Read system prompt from a file (≤ 64KB).
        #[arg(long = "system-prompt-file")]
        system_prompt_file: Option<PathBuf>,
        /// Replacement model id (provider-specific). Stop the agent first
        /// — the runtime rejects model changes on running agents.
        #[arg(long)]
        model: Option<String>,
        /// Replacement introduction blurb.
        #[arg(long)]
        introduction: Option<String>,
        /// Repeatable: `--env KEY=VALUE`. Any occurrence replaces the
        /// agent's whole env map (the runtime contract is wholesale
        /// replace, not merge).
        #[arg(long, value_name = "KEY=VALUE")]
        env: Vec<String>,
        /// Write `.env` file content from this path (≤ 64KB). Writes are
        /// fail-fast on size; the file lands at the agent clone root with
        /// chmod 0600.
        #[arg(long = "dotenv-file")]
        dotenv_file: Option<PathBuf>,
    },
    /// Run provider preflight checks (binary present, version, hello round-trip).
    ///
    /// Calls `GET /preflight/{provider}` (root-level, not workspace-scoped).
    /// Response is provider-specific and passed through verbatim to stdout.
    /// `--llm-provider` / `--llm-model` are hermes-only — supplying them with
    /// any other provider is rejected client-side (exit 1) without an HTTP
    /// round-trip.
    Preflight {
        /// Provider to preflight: claude, codex, opencode, pi, hermes.
        /// Unknown values are rejected by the server with HTTP 400 — the
        /// CLI doesn't maintain its own whitelist so adding a provider on
        /// the runtime side doesn't require a coupled CLI change.
        #[arg(value_name = "PROVIDER")]
        provider: String,
        /// Hermes only: LLM provider id (e.g. "anthropic", "custom:foo").
        /// Forwarded as `?llm_provider=...` query param.
        #[arg(long = "llm-provider")]
        llm_provider: Option<String>,
        /// Hermes only: model id to use for the preflight hello.
        /// Forwarded as `?llm_model=...` query param.
        #[arg(long = "llm-model")]
        llm_model: Option<String>,
    },
    /// Manage optional remote runtime subscriptions for the fleet observer.
    Fleet {
        #[command(subcommand)]
        command: FleetCommand,
    },
}

#[derive(Subcommand, Debug)]
enum FleetCommand {
    /// List configured remote runtime subscriptions.
    List,
    /// Show live health/status for remote runtime subscriptions.
    Status,
    /// Add or replace a remote runtime subscription and start it immediately.
    Add {
        /// Stable node/runtime UUID. Used as the source identity in fleet events.
        #[arg(long = "node-id")]
        node_id: String,
        /// Base URL for the remote runtime, e.g. http://100.64.0.10:16868.
        #[arg(long = "base-url")]
        base_url: String,
        /// Current Tailscale IP or diagnostic address for the node.
        #[arg(long = "node-ip")]
        node_ip: Option<String>,
        /// Human-readable node label.
        #[arg(long = "node-name")]
        node_name: Option<String>,
        /// Remote workspace slug filter. Repeatable; omitted means auto-map all matching git remotes.
        #[arg(long = "workspace")]
        workspaces: Vec<String>,
    },
    /// Remove a remote runtime subscription and stop its active tasks.
    Remove {
        /// Stable node/runtime UUID to remove.
        #[arg(long = "node-id")]
        node_id: String,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Bypass `Args::parse()` because that calls `clap::Error::exit()` with
    // clap's default exit code 2 for parse failures, which conflicts with
    // the spec §4 mapping (1 = CLI / argv error, 2 = permanent server
    // error). We discriminate clap's error kinds ourselves so:
    //   - --help / --version → stdout + exit 0
    //   - any other parse error → stderr + exit 1
    let args = match Args::try_parse_from(std::env::args()) {
        Ok(a) => a,
        Err(e) => {
            use clap::error::ErrorKind;
            match e.kind() {
                // Help / version are not failures — clap routes their
                // formatted output through `e.print()` to stdout. Our
                // contract says exit 0 in this case.
                ErrorKind::DisplayHelp
                | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
                | ErrorKind::DisplayVersion => {
                    let _ = e.print();
                    std::process::exit(0);
                }
                // All real argv failures — unknown args, missing required,
                // value parse error, conflicts, etc. Print to stderr and
                // exit 1 per spec §4.
                _ => {
                    let _ = e.print();
                    std::process::exit(1);
                }
            }
        }
    };

    if let Err(msg) = validate_args(&args) {
        eprintln!("{msg}");
        // Spec §4: argv / config errors exit 1. clap's own parse errors
        // and our `validate_args` rejections both fall into this class;
        // 2 is reserved for runtime-side permanent failures.
        std::process::exit(1);
    }

    match args.command {
        None => run_server(args.port, args.daemon).await,
        Some(cmd) => run_cli(cmd).await,
    }
}

/// Reject argv combinations that clap accepts but the dispatch silently
/// ignores. Specifically: `--port` and `--daemon` are top-level fields on
/// `Args` (not on the subcommand variants and not `global = true`), so
/// `gitim-runtime --port 8080 status` parses fine, but `run_cli` doesn't
/// read either field — the CLI hits the default port instead of 8080.
/// Surfacing a clear error here is friendlier than letting the request go
/// to the wrong runtime. CLI users wanting to pin the runtime port should
/// set `GITIM_RUNTIME_PORT`, which `resolve_base_url` honors.
fn validate_args(args: &Args) -> Result<(), String> {
    if args.command.is_some() && (args.port.is_some() || args.daemon) {
        return Err(
            "--port and --daemon are server-mode flags and cannot be combined with a \
             subcommand. To target a specific runtime port from the CLI, set the \
             GITIM_RUNTIME_PORT environment variable."
                .to_string(),
        );
    }
    Ok(())
}

/// One-shot CLI dispatch. Subcommand bodies live in `cli::cmd_*` modules and
/// return `Result<i32, CliError>`; this function owns the exit-code mapping
/// and the stderr error envelope so each handler stays focused on the HTTP
/// composition.
///
/// Tracing is initialized at WARN level (not INFO like server mode) so the
/// CLI's JSON stdout output stays clean for downstream parsing.
///
/// Async because subcommand bodies issue HTTP requests via
/// `gitim_runtime::cli::Client` (reqwest non-blocking).
async fn run_cli(cmd: Command) -> Result<(), Box<dyn std::error::Error>> {
    use gitim_runtime::cli::{
        cmd_add_agent, cmd_burn_agent, cmd_fleet, cmd_list_agents, cmd_preflight, cmd_runtime_id,
        cmd_status, cmd_update_agent, cmd_workspaces, from_cli_error, resolve_base_url, CliError,
        Client, ErrorResponse,
    };

    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .init();

    // `--port` is server-only today; CLI discovers via the env / runtime.json
    // / DEFAULT_PORT chain. T13 may add a CLI-side `--port` flag — until then
    // the priority chain is the single source of truth.
    let client = Client::new(resolve_base_url(None));

    let result: Result<i32, CliError> = match cmd {
        Command::Status => cmd_status::run(&client).await,
        Command::RuntimeId => cmd_runtime_id::run(&client).await,
        Command::Workspaces => cmd_workspaces::run(&client).await,
        Command::ListAgents {
            workspace,
            detailed,
        } => cmd_list_agents::run(&client, workspace, detailed).await,
        Command::AddAgent {
            workspace,
            handler,
            display_name,
            provider,
            model,
            system_prompt,
            system_prompt_file,
            env,
            introduction,
            no_join_general,
            llm_provider,
            llm_model,
        } => {
            let args = cmd_add_agent::Args {
                workspace,
                handler,
                display_name,
                provider,
                model,
                system_prompt,
                system_prompt_file,
                env,
                introduction,
                no_join_general,
                llm_provider,
                llm_model,
            };
            cmd_add_agent::run(&client, args).await
        }
        Command::BurnAgent {
            workspace,
            id,
            hard,
        } => cmd_burn_agent::run(&client, workspace, id, hard).await,
        Command::UpdateAgent {
            workspace,
            id,
            system_prompt,
            system_prompt_file,
            model,
            introduction,
            env,
            dotenv_file,
        } => {
            let args = cmd_update_agent::Args {
                workspace,
                id,
                system_prompt,
                system_prompt_file,
                model,
                introduction,
                env,
                dotenv_file,
            };
            cmd_update_agent::run(&client, args).await
        }
        Command::Preflight {
            provider,
            llm_provider,
            llm_model,
        } => cmd_preflight::run(&client, provider, llm_provider, llm_model).await,
        Command::Fleet { command } => match command {
            FleetCommand::List => cmd_fleet::list(&client).await,
            FleetCommand::Status => cmd_fleet::status(&client).await,
            FleetCommand::Add {
                node_id,
                base_url,
                node_ip,
                node_name,
                workspaces,
            } => {
                let args = cmd_fleet::AddArgs {
                    node_id,
                    base_url,
                    node_ip,
                    node_name,
                    workspaces,
                };
                cmd_fleet::add(&client, args).await
            }
            FleetCommand::Remove { node_id } => cmd_fleet::remove(&client, node_id).await,
        },
    };

    match result {
        Ok(code) => std::process::exit(code),
        Err(err) => {
            // Stderr carries an ErrorResponse-shaped envelope so scripts can
            // parse a uniform `{ok, error, error_code?, preflight_detail?}`
            // from either runtime 4xx bodies or CLI-side failures. Mirroring
            // the wire shape keeps downstream tooling simple.
            let envelope = ErrorResponse {
                ok: false,
                error: err.to_string(),
                error_code: match &err {
                    CliError::ResponseErrorCode { code, .. } => Some(code.clone()),
                    _ => None,
                },
                preflight_detail: match &err {
                    CliError::ResponseErrorCode {
                        preflight_detail, ..
                    } => preflight_detail.clone(),
                    _ => None,
                },
            };
            // Best-effort serialize; if even this fails, fall back to the
            // Display string to avoid swallowing the original error.
            match serde_json::to_string_pretty(&envelope) {
                Ok(s) => eprintln!("{s}"),
                Err(_) => eprintln!("{err}"),
            }
            // Companion to the JSON envelope: emit human-greppable preflight
            // lines after the JSON when the error carries a `PreflightResult`.
            // Agents shell-out to the CLI and parse stderr with regex; pulling
            // the nested struct out as flat `Key: value` lines lets a one-line
            // grep grab the failure mode without a second JSON parser.
            //
            // Format intentionally indented so it visually groups under the
            // JSON envelope and stays distinguishable from a CLI-side error.
            if let CliError::ResponseErrorCode {
                preflight_detail: Some(pf),
                message,
                ..
            } = &err
            {
                print_preflight_detail_to_stderr(pf, message);
            }
            std::process::exit(from_cli_error(&err));
        }
    }
}

/// Render the nested [`PreflightResult`] as `Key: value` lines on stderr.
///
/// Wraps [`format_preflight_detail`] in `eprintln!` — split so the formatter
/// is unit-testable without stderr capture. See that function for the field
/// order, truncation, and `Detail:` suppression rules.
fn print_preflight_detail_to_stderr(
    pf: &gitim_runtime::preflight::PreflightResult,
    main_message: &str,
) {
    eprint!("{}", format_preflight_detail(pf, main_message));
}

/// Cap for the inlined provider CLI output. 200 chars is plenty for a stderr
/// glance; longer text lives in the JSON envelope above where a parser can
/// still grab it untruncated. Sized in chars not bytes because we operate on
/// `&str` and UTF-8 codepoints — see [`truncate_chars`].
const PREFLIGHT_PREVIEW_CHAR_CAP: usize = 200;

/// Pure formatter for the preflight stderr block.
///
/// Returns a multi-line string the caller writes to stderr (or captures in
/// tests). Field ordering is fixed so an agent regex parser
/// (e.g. `^Error kind: (\w+)$`) sees the same lines in the same positions
/// every time. Output preview is truncated to [`PREFLIGHT_PREVIEW_CHAR_CAP`]
/// chars to keep the stderr block scannable — the raw stdout/stderr from
/// the provider CLI can be megabytes (HTTP body of a rate-limit error, …).
///
/// `main_message` is the top-level `CliError::ResponseErrorCode.message`
/// (already printed inside the JSON envelope as `error`). We suppress
/// `Detail:` when `pf.error` equals that — repeating the same string twice
/// is noise. Different strings get both lines so the agent sees the
/// provider-specific detail in addition to the top-level summary.
fn format_preflight_detail(
    pf: &gitim_runtime::preflight::PreflightResult,
    main_message: &str,
) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "Preflight ({}):", pf.provider);
    if let Some(kind) = pf.error_kind {
        // serde rename = snake_case; serialize as a single value and strip
        // the surrounding quotes so the line reads `Error kind: not_installed`
        // instead of `Error kind: "not_installed"`.
        let raw = serde_json::to_string(&kind).unwrap_or_else(|_| String::from("\"other\""));
        let snake = raw.trim_matches('"');
        let _ = writeln!(out, "  Error kind: {snake}");
    }
    if let Some(failure_code) = &pf.failure_code {
        // Setup-level tag (e.g. `hermes_default_profile_no_llm`,
        // `missing_llm_provider`). Distinct from the top-level
        // `error_code` already present in the JSON envelope — surfacing
        // it inline gives the agent a stable tag without parsing JSON.
        let _ = writeln!(out, "  Failure code: {failure_code}");
    }
    if let Some(version) = &pf.version {
        let _ = writeln!(out, "  Provider version: {version}");
    }
    if let Some(model) = &pf.model_used {
        let _ = writeln!(out, "  Model: {model}");
    }
    if let Some(preview) = &pf.output_preview {
        let truncated = truncate_chars(preview, PREFLIGHT_PREVIEW_CHAR_CAP);
        let _ = writeln!(out, "  Output preview: {truncated}");
    }
    if let Some(detail) = &pf.error {
        // Suppress repetition: server `ErrorBody::with_preflight` typically
        // sets the top-level `error` to `pf.error.clone()`, so most paths
        // would print the same string twice. Only re-print when the strings
        // actually differ — that's the case where the provider supplied
        // extra context the agent should see (e.g. a model-not-found body).
        if detail != main_message {
            let _ = writeln!(out, "  Detail: {detail}");
        }
    }
    out
}

/// Truncate `s` to at most `max_chars` characters, appending `…` when cut.
/// Operates on chars (not bytes) so multibyte UTF-8 doesn't slice mid-codepoint
/// — provider error bodies often quote API payloads that include non-ASCII.
fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars).collect();
    out.push('…');
    out
}

/// Server mode: same boot path as before the CLI split. Initializes tracing,
/// runs env preflight, then either daemonizes or runs the shell directly.
async fn run_server(port: Option<u16>, daemon: bool) -> Result<(), Box<dyn std::error::Error>> {
    gitim_runtime::tool_path::ensure_common_tool_paths();

    tracing_subscriber::fmt::init();

    // Environment preflight: all three binaries must be version-aligned
    if let Err(e) = gitim_runtime::preflight::check_env() {
        eprintln!("{e}");
        std::process::exit(1);
    }

    let port = port.unwrap_or(DEFAULT_PORT);
    if daemon {
        return daemonize(port);
    }
    run_shell(port).await
}

fn daemonize(port: u16) -> Result<(), Box<dyn std::error::Error>> {
    let exe = std::env::current_exe()?;

    // Runtime + per-daemon logs both live in ~/.gitim/logs/ so a single
    // tail over the directory surfaces all agent activity.
    let log_path = gitim_runtime::daemon_log::runtime_log_path();
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let log_file = std::fs::File::create(&log_path)?;

    // PID file ownership lives with the process actually serving HTTP —
    // `run_shell()` writes it at startup. That way a future self-replace
    // path (fork-exec a fresh runtime with new binary) doesn't need to
    // also remember to rewrite the PID file from the exiting parent.
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["--port", &port.to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(log_file.try_clone()?)
        .stderr(log_file);
    let child = gitim_runtime::background::spawn_detached(&mut cmd)?;

    eprintln!(
        "runtime started in background (pid: {}, port: {port})",
        child.id()
    );
    eprintln!("log: {}", log_path.display());

    Ok(())
}

async fn run_shell(port: u16) -> Result<(), Box<dyn std::error::Error>> {
    // Capture canonical exe BEFORE any self-replace could run. After
    // replace_binaries swaps the on-disk file, Linux `current_exe()` returns
    // "<path> (deleted)" for this inode — too late then. Stored in
    // RuntimeState so the self-update endpoint can strict-mode-check the
    // install dir and pick the fork-exec target.
    let canonical_exe = std::env::current_exe()?.canonicalize()?;

    // Whoever is actually serving HTTP owns the PID file. On normal boot
    // this is just us writing our own pid; on self-replace restart the
    // freshly spawned runtime overwrites whatever the dying parent left.
    let pid_path = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".gitim/runtime.pid");
    if let Some(parent) = pid_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&pid_path, std::process::id().to_string())?;

    let (router, state) = gitim_runtime::http::create_router_with_exe(canonical_exe);
    // Record the port we're about to bind so the self-update async phase can
    // pass the same `--port` to the replacement runtime. `run_shell` is the
    // single writer; nothing else in the crate needs to mutate this.
    state.lock().unwrap().listen_port = port;

    // Materialize the device-bound runtime ID. First boot generates and
    // persists; subsequent boots read the existing UUID. Either way it lands
    // in RuntimeState before recover_from_config so /health responds with the
    // real ID even during the recovery window.
    // See docs/plans/runtime-id/00-design.md.
    let runtime_id = gitim_runtime::user_config::ensure_runtime_id();
    state.lock().unwrap().runtime_id = runtime_id.clone();
    eprintln!("runtime started, id: {runtime_id}");

    // Token + email propagation MUST run before `recover_from_config`, because
    // recovery spawns per-agent daemons and each daemon reads `me.json` /
    // `.git/config` into memory at startup. If we propagate after, the daemons
    // are already running with stale values and the fix won't take effect until
    // the user manually restarts them — which nobody knows to do.
    //
    // Both propagation passes are file-only (no state dependency), so we can
    // drive them straight from `user_config::read()` instead of from the
    // runtime state populated by recovery.
    let pre_recovery_paths: Vec<PathBuf> = gitim_runtime::user_config::read()
        .workspaces
        .iter()
        .map(|w| PathBuf::from(&w.path))
        .filter(|p| p.exists())
        .collect();

    // If config.json's token was edited while the runtime was down, clones
    // still carry the old token. Resync on startup so fetch/push don't fail.
    for workspace in &pre_recovery_paths {
        if let Err(e) = gitim_runtime::token_propagation::propagate_token(workspace) {
            tracing::warn!(error = %e, "token propagation on startup failed");
        }
    }

    // Backfill `github_email` for workspaces that predate the email feature
    // (or were provisioned when /user.email came back null). Net effect is
    // that existing github-mode workspaces start crediting commits to the
    // owner's contribution graph on the next runtime boot, no re-init and
    // no manual daemon restart needed.
    for workspace in &pre_recovery_paths {
        match gitim_runtime::email_propagation::backfill_github_email(
            workspace,
            gitim_runtime::email_propagation::GITHUB_API_BASE,
        )
        .await
        {
            Ok(true) => {
                tracing::info!(
                    workspace = %workspace.display(),
                    "email backfill applied",
                );
            }
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(error = %e, "email backfill on startup failed");
            }
        }
    }

    gitim_runtime::http::recover_from_config(state.clone()).await;
    gitim_runtime::fleet::recover_from_config(state.clone());

    // Idle watchdog: exit if no activity for 24 hours
    let idle_state = state.clone();
    tokio::spawn(async move {
        const IDLE_TIMEOUT_SECS: u64 = 24 * 60 * 60;
        const CHECK_INTERVAL_SECS: u64 = 60 * 60;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(CHECK_INTERVAL_SECS)).await;
            let last = idle_state
                .lock()
                .unwrap()
                .last_activity
                .load(std::sync::atomic::Ordering::Relaxed);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            if now.saturating_sub(last) >= IDLE_TIMEOUT_SECS {
                if gitim_runtime::http::has_active_agents(&idle_state) {
                    eprintln!("idle timeout reached but agents still active, deferring exit");
                    continue;
                }
                eprintln!("no activity for 24h — shutting down");
                if runtime_pid_file_points_to_current_process() {
                    cleanup_pid_file();
                    gitim_runtime::workspace::kill_managed_daemons(&idle_state);
                }
                std::process::exit(0);
            }
        }
    });

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    eprintln!("runtime shell listening on http://{addr}");

    // Self-update path fork-execs a fresh runtime and then `exit(0)`s the
    // parent. The child can briefly race the parent for the listening port:
    // parent hasn't released it yet when child first calls bind. Retry a few
    // times on AddrInUse so the child survives that ~100ms window instead of
    // dying and leaving the frontend polling a dead `/health`.
    // 10 x 100ms = 1s max wait, well over the observed race window.
    let listener = {
        let mut attempts = 0;
        loop {
            match tokio::net::TcpListener::bind(addr).await {
                Ok(l) => break l,
                Err(e) if attempts < 10 && e.kind() == std::io::ErrorKind::AddrInUse => {
                    attempts += 1;
                    tracing::warn!(
                        ?e,
                        attempts,
                        "port in use (likely restart race), retrying in 100ms"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                Err(e) => return Err(e.into()),
            }
        }
    };

    // Persist the actually-bound port as a CLI discovery hint. Best-effort:
    // a failure here doesn't block serving — the CLI falls back to
    // DEFAULT_PORT if this hint is missing or stale.
    if let Err(e) = gitim_runtime::user_config::write_listen_port(port) {
        tracing::warn!(error = %e, port, "failed to persist listen_port hint");
    }

    let mut server = tokio::spawn(async move { axum::serve(listener, router).await });

    // Wait for shutdown signal; also bail if the server itself errors out
    tokio::select! {
        _ = shutdown_signal() => {},
        result = &mut server => {
            if let Err(e) = result? {
                eprintln!("server error: {e}");
            }
        }
    }

    // SSE keep-alive connections block axum graceful shutdown indefinitely;
    // abort the server task so the process can exit cleanly.
    server.abort();

    // Kill all managed daemons on shutdown
    if runtime_pid_file_points_to_current_process() {
        cleanup_pid_file();
        gitim_runtime::workspace::kill_managed_daemons(&state);
        eprintln!("all daemons stopped");
    } else {
        eprintln!(
            "runtime pid changed; assuming replacement runtime took over, skipping daemon stop"
        );
    }
    Ok(())
}

async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};

    let mut sigterm = signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {},
        _ = sigterm.recv() => {},
    }

    eprintln!("\nshutting down...");
}

#[cfg(test)]
mod pid_file_tests {
    use super::*;

    #[test]
    fn pid_file_owner_matches_expected_process() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let path = tmp.path().join("runtime.pid");
        std::fs::write(&path, "12345\n").expect("pid file");

        assert!(pid_file_points_to_process(&path, 12345));
        assert!(!pid_file_points_to_process(&path, 54321));
    }

    #[test]
    fn missing_pid_file_keeps_current_process_responsible() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let path = tmp.path().join("missing.pid");

        assert!(pid_file_points_to_process(&path, 12345));
    }
}

#[cfg(test)]
mod argv_dispatch_tests {
    //! Argv parsing boundary tests. These verify the basic dispatch contract:
    //! no-subcommand → server mode, subcommand → CLI mode, and server-only
    //! flags (`--port`, `--daemon`) are rejected when a subcommand is present
    //! so they can't be silently ignored. The full per-subcommand argv test
    //! catalog lives in Task 13.
    use super::*;
    use clap::Parser;

    #[test]
    fn no_args_means_server_mode() {
        let args = Args::try_parse_from(["gitim-runtime"]).expect("parse must succeed");
        assert!(args.command.is_none());
        assert!(!args.daemon);
        assert!(args.port.is_none());
    }

    #[test]
    fn port_flag_at_top_level() {
        let args =
            Args::try_parse_from(["gitim-runtime", "--port", "5000"]).expect("parse must succeed");
        assert!(args.command.is_none());
        assert_eq!(args.port, Some(5000));
    }

    #[test]
    fn daemon_flag_at_top_level() {
        let args = Args::try_parse_from(["gitim-runtime", "-d"]).expect("parse must succeed");
        assert!(args.command.is_none());
        assert!(args.daemon);
    }

    #[test]
    fn subcommand_alone() {
        let args = Args::try_parse_from(["gitim-runtime", "status"]).expect("parse must succeed");
        assert!(matches!(args.command, Some(Command::Status)));
    }

    #[test]
    fn port_with_subcommand_rejected() {
        // --port appears *after* the subcommand → clap routes it to the
        // subcommand, doesn't find it there, and rejects with "unexpected
        // argument". This catches the obvious-looking misuse cleanly.
        let result = Args::try_parse_from(["gitim-runtime", "status", "--port", "8080"]);
        assert!(result.is_err());
    }

    #[test]
    fn port_before_subcommand_at_top_level_parses_but_validate_rejects() {
        // The sneakier form: --port BEFORE the subcommand. Clap parses
        // this fine — `args.port = Some(8080), args.command = Some(Status)`
        // — but `run_cli` ignores `args.port` and silently talks to the
        // default port. `validate_args` blocks the combination in main.
        let args = Args::try_parse_from(["gitim-runtime", "--port", "8080", "status"])
            .expect("clap accepts --port at top level even with a subcommand");
        assert_eq!(args.port, Some(8080));
        assert!(matches!(args.command, Some(Command::Status)));
        let err = validate_args(&args).expect_err("validate_args must reject this combination");
        assert!(
            err.contains("--port"),
            "error message must mention --port: {err}"
        );
        assert!(
            err.contains("GITIM_RUNTIME_PORT"),
            "error message must point users to the env var: {err}",
        );
    }

    #[test]
    fn daemon_before_subcommand_validate_rejects() {
        // Same shape as the --port case for --daemon (-d). Clap accepts it
        // as a top-level flag; we reject the combination at runtime.
        let args = Args::try_parse_from(["gitim-runtime", "-d", "status"])
            .expect("clap accepts -d at top level even with a subcommand");
        assert!(args.daemon);
        assert!(matches!(args.command, Some(Command::Status)));
        let err = validate_args(&args).expect_err("validate_args must reject this combination");
        assert!(
            err.contains("--daemon"),
            "error message must mention --daemon: {err}"
        );
    }

    #[test]
    fn port_alone_no_subcommand_ok() {
        // Server-mode use of --port is the supported case.
        let args = Args::try_parse_from(["gitim-runtime", "--port", "8080"])
            .expect("clap accepts --port in server mode");
        assert!(validate_args(&args).is_ok());
    }

    #[test]
    fn subcommand_alone_ok() {
        // Plain subcommand invocation must pass validation.
        let args = Args::try_parse_from(["gitim-runtime", "status"])
            .expect("clap accepts bare subcommand");
        assert!(validate_args(&args).is_ok());
    }

    #[test]
    fn no_args_validate_ok() {
        // Server-mode default invocation.
        let args = Args::try_parse_from(["gitim-runtime"]).expect("clap accepts no args");
        assert!(validate_args(&args).is_ok());
    }

    #[test]
    fn daemon_alone_no_subcommand_validates_ok() {
        // Server-mode `-d` (daemonize) is supported on its own.
        let args =
            Args::try_parse_from(["gitim-runtime", "-d"]).expect("clap accepts -d in server mode");
        assert!(args.daemon);
        assert!(args.command.is_none());
        assert!(validate_args(&args).is_ok());
    }

    #[test]
    fn port_and_daemon_alone_no_subcommand_validates_ok() {
        // Combined server-mode flags without a subcommand: the documented
        // way to spawn a detached server on a fixed port. Must validate.
        let args = Args::try_parse_from(["gitim-runtime", "--port", "5000", "-d"])
            .expect("clap accepts --port -d in server mode");
        assert_eq!(args.port, Some(5000));
        assert!(args.daemon);
        assert!(args.command.is_none());
        assert!(validate_args(&args).is_ok());
    }

    #[test]
    fn port_with_subcommand_validates_err_to_exit_1() {
        // Companion to `port_before_subcommand_at_top_level_parses_but_validate_rejects`
        // pinning the ergonomic identity: clap accepts the parse, validate
        // rejects, and `main` will exit 1 for that rejection (spec §4 —
        // CLI / argv class). This test only exercises the validate step;
        // the actual `process::exit(1)` is verified end-to-end by
        // `cli_subprocess_smoke`.
        let args = Args::try_parse_from(["gitim-runtime", "--port", "5000", "status"])
            .expect("clap accepts --port at top level even with a subcommand");
        assert!(validate_args(&args).is_err());
    }

    #[test]
    fn daemon_with_subcommand_validates_err() {
        // Same shape for `-d`. Locks the contract from the validate side
        // so a future refactor of `validate_args` can't silently let
        // server-mode flags through with a subcommand.
        let args = Args::try_parse_from(["gitim-runtime", "-d", "status"])
            .expect("clap accepts -d at top level even with a subcommand");
        assert!(validate_args(&args).is_err());
    }

    #[test]
    fn legacy_positional_rejected() {
        // The pre-CLI positional form (`gitim-runtime <url> <handler> <name>`)
        // must not parse as a subcommand or as bare server-mode args.
        let result =
            Args::try_parse_from(["gitim-runtime", "https://github.com/o/r", "handler", "name"]);
        assert!(result.is_err());
    }

    #[test]
    fn unknown_subcommand_rejected() {
        let result = Args::try_parse_from(["gitim-runtime", "fly-to-mars"]);
        assert!(result.is_err());
    }
}

#[cfg(test)]
mod argv_subcommand_tests {
    //! Per-subcommand argv parse catalog. Each subcommand gets a minimum-args
    //! happy parse, a fully-populated happy parse where relevant, missing-required
    //! fail cases, and conflict cases. These tests cover the clap surface only —
    //! handler behavior is tested separately via the cli::cmd_* modules.
    use super::*;
    use clap::Parser;
    use std::path::PathBuf;

    // ------------------------------------------------------------------
    // status
    // ------------------------------------------------------------------

    #[test]
    fn status_parses() {
        let args = Args::try_parse_from(["gitim-runtime", "status"]).expect("parse must succeed");
        assert!(matches!(args.command, Some(Command::Status)));
    }

    // ------------------------------------------------------------------
    // runtime-id
    // ------------------------------------------------------------------

    #[test]
    fn runtime_id_parses() {
        let args =
            Args::try_parse_from(["gitim-runtime", "runtime-id"]).expect("parse must succeed");
        assert!(matches!(args.command, Some(Command::RuntimeId)));
    }

    // ------------------------------------------------------------------
    // workspaces
    // ------------------------------------------------------------------

    #[test]
    fn workspaces_parses() {
        let args =
            Args::try_parse_from(["gitim-runtime", "workspaces"]).expect("parse must succeed");
        assert!(matches!(args.command, Some(Command::Workspaces)));
    }

    // ------------------------------------------------------------------
    // list-agents
    // ------------------------------------------------------------------

    #[test]
    fn list_agents_minimal_parses() {
        let args =
            Args::try_parse_from(["gitim-runtime", "list-agents"]).expect("parse must succeed");
        assert!(matches!(
            args.command,
            Some(Command::ListAgents {
                workspace: None,
                detailed: false
            })
        ));
    }

    #[test]
    fn list_agents_with_workspace_and_detailed() {
        let args = Args::try_parse_from([
            "gitim-runtime",
            "list-agents",
            "--workspace",
            "ws-a",
            "--detailed",
        ])
        .expect("parse must succeed");
        match args.command {
            Some(Command::ListAgents {
                workspace,
                detailed,
            }) => {
                assert_eq!(workspace.as_deref(), Some("ws-a"));
                assert!(detailed);
            }
            other => panic!("expected ListAgents, got {other:?}"),
        }
    }

    #[test]
    fn list_agents_detailed_only() {
        let args = Args::try_parse_from(["gitim-runtime", "list-agents", "--detailed"])
            .expect("parse must succeed");
        match args.command {
            Some(Command::ListAgents {
                workspace,
                detailed,
            }) => {
                assert!(workspace.is_none());
                assert!(detailed);
            }
            other => panic!("expected ListAgents, got {other:?}"),
        }
    }

    // ------------------------------------------------------------------
    // add-agent
    // ------------------------------------------------------------------

    #[test]
    fn add_agent_minimal_required_args() {
        let args = Args::try_parse_from([
            "gitim-runtime",
            "add-agent",
            "--handler",
            "tester",
            "--display-name",
            "Tester",
            "--provider",
            "claude",
        ])
        .expect("parse must succeed");
        match args.command {
            Some(Command::AddAgent {
                workspace,
                handler,
                display_name,
                provider,
                model,
                system_prompt,
                system_prompt_file,
                env,
                introduction,
                no_join_general,
                llm_provider,
                llm_model,
            }) => {
                assert!(workspace.is_none());
                assert_eq!(handler, "tester");
                assert_eq!(display_name, "Tester");
                assert_eq!(provider, "claude");
                assert!(model.is_none());
                assert!(system_prompt.is_none());
                assert!(system_prompt_file.is_none());
                assert!(env.is_empty());
                assert!(introduction.is_none());
                assert!(!no_join_general);
                assert!(llm_provider.is_none());
                assert!(llm_model.is_none());
            }
            other => panic!("expected AddAgent, got {other:?}"),
        }
    }

    #[test]
    fn add_agent_full_args() {
        let args = Args::try_parse_from([
            "gitim-runtime",
            "add-agent",
            "--workspace",
            "ws-a",
            "--handler",
            "alice",
            "--display-name",
            "Alice",
            "--provider",
            "hermes",
            "--model",
            "gpt-5",
            "--system-prompt",
            "be careful",
            "--env",
            "FOO=bar",
            "--env",
            "BAZ=qux",
            "--introduction",
            "Hi, I'm Alice",
            "--no-join-general",
            "--llm-provider",
            "anthropic",
            "--llm-model",
            "claude-opus-4-7",
        ])
        .expect("parse must succeed");
        match args.command {
            Some(Command::AddAgent {
                workspace,
                handler,
                display_name,
                provider,
                model,
                system_prompt,
                system_prompt_file,
                env,
                introduction,
                no_join_general,
                llm_provider,
                llm_model,
            }) => {
                assert_eq!(workspace.as_deref(), Some("ws-a"));
                assert_eq!(handler, "alice");
                assert_eq!(display_name, "Alice");
                assert_eq!(provider, "hermes");
                assert_eq!(model.as_deref(), Some("gpt-5"));
                assert_eq!(system_prompt.as_deref(), Some("be careful"));
                assert!(system_prompt_file.is_none());
                assert_eq!(env, vec!["FOO=bar".to_string(), "BAZ=qux".to_string()]);
                assert_eq!(introduction.as_deref(), Some("Hi, I'm Alice"));
                assert!(no_join_general);
                assert_eq!(llm_provider.as_deref(), Some("anthropic"));
                assert_eq!(llm_model.as_deref(), Some("claude-opus-4-7"));
            }
            other => panic!("expected AddAgent, got {other:?}"),
        }
    }

    #[test]
    fn add_agent_system_prompt_file_parses() {
        let args = Args::try_parse_from([
            "gitim-runtime",
            "add-agent",
            "--handler",
            "tester",
            "--display-name",
            "Tester",
            "--provider",
            "claude",
            "--system-prompt-file",
            "/tmp/prompt.md",
        ])
        .expect("parse must succeed");
        match args.command {
            Some(Command::AddAgent {
                system_prompt,
                system_prompt_file,
                ..
            }) => {
                assert!(system_prompt.is_none());
                assert_eq!(system_prompt_file, Some(PathBuf::from("/tmp/prompt.md")));
            }
            other => panic!("expected AddAgent, got {other:?}"),
        }
    }

    #[test]
    fn add_agent_missing_handler_fails() {
        let result = Args::try_parse_from([
            "gitim-runtime",
            "add-agent",
            "--display-name",
            "Tester",
            "--provider",
            "claude",
        ]);
        assert!(result.is_err());
    }

    #[test]
    fn add_agent_missing_display_name_fails() {
        let result = Args::try_parse_from([
            "gitim-runtime",
            "add-agent",
            "--handler",
            "tester",
            "--provider",
            "claude",
        ]);
        assert!(result.is_err());
    }

    #[test]
    fn add_agent_missing_provider_fails() {
        let result = Args::try_parse_from([
            "gitim-runtime",
            "add-agent",
            "--handler",
            "tester",
            "--display-name",
            "Tester",
        ]);
        assert!(result.is_err());
    }

    #[test]
    fn add_agent_system_prompt_conflicts() {
        // --system-prompt and --system-prompt-file are mutually exclusive.
        let result = Args::try_parse_from([
            "gitim-runtime",
            "add-agent",
            "--handler",
            "tester",
            "--display-name",
            "Tester",
            "--provider",
            "claude",
            "--system-prompt",
            "inline",
            "--system-prompt-file",
            "/tmp/y",
        ]);
        assert!(result.is_err());
    }

    // ------------------------------------------------------------------
    // burn-agent
    // ------------------------------------------------------------------

    #[test]
    fn burn_agent_minimal_required_args() {
        let args = Args::try_parse_from(["gitim-runtime", "burn-agent", "--id", "agent-1"])
            .expect("parse must succeed");
        match args.command {
            Some(Command::BurnAgent {
                workspace,
                id,
                hard,
            }) => {
                assert!(workspace.is_none());
                assert_eq!(id, "agent-1");
                assert!(!hard);
            }
            other => panic!("expected BurnAgent, got {other:?}"),
        }
    }

    #[test]
    fn burn_agent_with_hard() {
        let args = Args::try_parse_from(["gitim-runtime", "burn-agent", "--id", "x", "--hard"])
            .expect("parse must succeed");
        match args.command {
            Some(Command::BurnAgent {
                workspace,
                id,
                hard,
            }) => {
                assert!(workspace.is_none());
                assert_eq!(id, "x");
                assert!(hard);
            }
            other => panic!("expected BurnAgent, got {other:?}"),
        }
    }

    #[test]
    fn burn_agent_with_workspace() {
        let args = Args::try_parse_from([
            "gitim-runtime",
            "burn-agent",
            "--workspace",
            "ws-a",
            "--id",
            "agent-1",
        ])
        .expect("parse must succeed");
        match args.command {
            Some(Command::BurnAgent {
                workspace,
                id,
                hard,
            }) => {
                assert_eq!(workspace.as_deref(), Some("ws-a"));
                assert_eq!(id, "agent-1");
                assert!(!hard);
            }
            other => panic!("expected BurnAgent, got {other:?}"),
        }
    }

    #[test]
    fn burn_agent_missing_id_fails() {
        let result = Args::try_parse_from(["gitim-runtime", "burn-agent"]);
        assert!(result.is_err());
    }

    // ------------------------------------------------------------------
    // update-agent
    // ------------------------------------------------------------------

    #[test]
    fn update_agent_minimal_required_args() {
        // --id is the only parse-time required arg. An empty patch is the
        // CLI/runtime's job to reject, not clap's.
        let args = Args::try_parse_from(["gitim-runtime", "update-agent", "--id", "agent-1"])
            .expect("parse must succeed");
        match args.command {
            Some(Command::UpdateAgent {
                workspace,
                id,
                system_prompt,
                system_prompt_file,
                model,
                introduction,
                env,
                dotenv_file,
            }) => {
                assert!(workspace.is_none());
                assert_eq!(id, "agent-1");
                assert!(system_prompt.is_none());
                assert!(system_prompt_file.is_none());
                assert!(model.is_none());
                assert!(introduction.is_none());
                assert!(env.is_empty());
                assert!(dotenv_file.is_none());
            }
            other => panic!("expected UpdateAgent, got {other:?}"),
        }
    }

    #[test]
    fn update_agent_with_system_prompt_and_env() {
        let args = Args::try_parse_from([
            "gitim-runtime",
            "update-agent",
            "--workspace",
            "ws-a",
            "--id",
            "agent-1",
            "--system-prompt",
            "new prompt",
            "--env",
            "FOO=bar",
            "--env",
            "BAZ=qux",
            "--introduction",
            "new blurb",
        ])
        .expect("parse must succeed");
        match args.command {
            Some(Command::UpdateAgent {
                workspace,
                id,
                system_prompt,
                env,
                introduction,
                ..
            }) => {
                assert_eq!(workspace.as_deref(), Some("ws-a"));
                assert_eq!(id, "agent-1");
                assert_eq!(system_prompt.as_deref(), Some("new prompt"));
                assert_eq!(env, vec!["FOO=bar".to_string(), "BAZ=qux".to_string()]);
                assert_eq!(introduction.as_deref(), Some("new blurb"));
            }
            other => panic!("expected UpdateAgent, got {other:?}"),
        }
    }

    #[test]
    fn update_agent_with_model_and_dotenv_file() {
        let args = Args::try_parse_from([
            "gitim-runtime",
            "update-agent",
            "--id",
            "agent-1",
            "--model",
            "claude-opus-4-7",
            "--dotenv-file",
            "/tmp/.env",
        ])
        .expect("parse must succeed");
        match args.command {
            Some(Command::UpdateAgent {
                id,
                model,
                dotenv_file,
                ..
            }) => {
                assert_eq!(id, "agent-1");
                assert_eq!(model.as_deref(), Some("claude-opus-4-7"));
                assert_eq!(dotenv_file, Some(PathBuf::from("/tmp/.env")));
            }
            other => panic!("expected UpdateAgent, got {other:?}"),
        }
    }

    #[test]
    fn update_agent_missing_id_fails() {
        let result = Args::try_parse_from(["gitim-runtime", "update-agent"]);
        assert!(result.is_err());
    }

    #[test]
    fn update_agent_system_prompt_conflicts() {
        // Same mutually-exclusive pair as add-agent.
        let result = Args::try_parse_from([
            "gitim-runtime",
            "update-agent",
            "--id",
            "agent-1",
            "--system-prompt",
            "inline",
            "--system-prompt-file",
            "/tmp/y",
        ]);
        assert!(result.is_err());
    }

    // ------------------------------------------------------------------
    // preflight
    // ------------------------------------------------------------------

    #[test]
    fn preflight_positional_provider() {
        let args = Args::try_parse_from(["gitim-runtime", "preflight", "claude"])
            .expect("parse must succeed");
        match args.command {
            Some(Command::Preflight {
                provider,
                llm_provider,
                llm_model,
            }) => {
                assert_eq!(provider, "claude");
                assert!(llm_provider.is_none());
                assert!(llm_model.is_none());
            }
            other => panic!("expected Preflight, got {other:?}"),
        }
    }

    #[test]
    fn preflight_hermes_with_llm() {
        let args = Args::try_parse_from([
            "gitim-runtime",
            "preflight",
            "hermes",
            "--llm-provider",
            "gemini",
            "--llm-model",
            "gemini-2.0-flash-exp",
        ])
        .expect("parse must succeed");
        match args.command {
            Some(Command::Preflight {
                provider,
                llm_provider,
                llm_model,
            }) => {
                assert_eq!(provider, "hermes");
                assert_eq!(llm_provider.as_deref(), Some("gemini"));
                assert_eq!(llm_model.as_deref(), Some("gemini-2.0-flash-exp"));
            }
            other => panic!("expected Preflight, got {other:?}"),
        }
    }

    #[test]
    fn preflight_missing_positional_provider_fails() {
        let result = Args::try_parse_from(["gitim-runtime", "preflight"]);
        assert!(result.is_err());
    }

    #[test]
    fn fleet_list_parses() {
        let args =
            Args::try_parse_from(["gitim-runtime", "fleet", "list"]).expect("parse must succeed");
        match args.command {
            Some(Command::Fleet {
                command: FleetCommand::List,
            }) => {}
            other => panic!("expected Fleet::List, got {other:?}"),
        }
    }

    #[test]
    fn fleet_status_parses() {
        let args =
            Args::try_parse_from(["gitim-runtime", "fleet", "status"]).expect("parse must succeed");
        match args.command {
            Some(Command::Fleet {
                command: FleetCommand::Status,
            }) => {}
            other => panic!("expected Fleet::Status, got {other:?}"),
        }
    }

    #[test]
    fn fleet_add_parses_repeatable_workspaces() {
        let args = Args::try_parse_from([
            "gitim-runtime",
            "fleet",
            "add",
            "--node-id",
            "remote-a",
            "--base-url",
            "http://100.64.0.10:16868",
            "--node-ip",
            "100.64.0.10",
            "--node-name",
            "mac-mini",
            "--workspace",
            "room",
            "--workspace",
            "lab",
        ])
        .expect("parse must succeed");
        match args.command {
            Some(Command::Fleet {
                command:
                    FleetCommand::Add {
                        node_id,
                        base_url,
                        node_ip,
                        node_name,
                        workspaces,
                    },
            }) => {
                assert_eq!(node_id, "remote-a");
                assert_eq!(base_url, "http://100.64.0.10:16868");
                assert_eq!(node_ip.as_deref(), Some("100.64.0.10"));
                assert_eq!(node_name.as_deref(), Some("mac-mini"));
                assert_eq!(workspaces, vec!["room", "lab"]);
            }
            other => panic!("expected Fleet::Add, got {other:?}"),
        }
    }

    #[test]
    fn fleet_add_without_workspace_parses_for_auto_mapping() {
        let args = Args::try_parse_from([
            "gitim-runtime",
            "fleet",
            "add",
            "--node-id",
            "remote-a",
            "--base-url",
            "http://100.64.0.10:16868",
        ])
        .expect("parse must succeed");
        match args.command {
            Some(Command::Fleet {
                command:
                    FleetCommand::Add {
                        node_id,
                        base_url,
                        workspaces,
                        ..
                    },
            }) => {
                assert_eq!(node_id, "remote-a");
                assert_eq!(base_url, "http://100.64.0.10:16868");
                assert!(workspaces.is_empty());
            }
            other => panic!("expected Fleet::Add, got {other:?}"),
        }
    }

    #[test]
    fn fleet_remove_parses() {
        let args =
            Args::try_parse_from(["gitim-runtime", "fleet", "remove", "--node-id", "remote-a"])
                .expect("parse must succeed");
        match args.command {
            Some(Command::Fleet {
                command: FleetCommand::Remove { node_id },
            }) => {
                assert_eq!(node_id, "remote-a");
            }
            other => panic!("expected Fleet::Remove, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod preflight_stderr_format_tests {
    //! Format tests for the stderr block we render under the JSON envelope
    //! when the runtime ships a `preflight_detail`. The line ordering and
    //! field names are part of the agent contract — an agent shell-out
    //! greps these to branch on the failure mode. Drift here breaks
    //! callers, so every output field gets a positive and negative test.
    use super::{format_preflight_detail, truncate_chars, PREFLIGHT_PREVIEW_CHAR_CAP};
    use gitim_runtime::preflight::{ErrorKind, PreflightResult};

    fn baseline_failure() -> PreflightResult {
        PreflightResult {
            available: false,
            provider: "claude".to_string(),
            version: None,
            model_used: None,
            duration_ms: 12,
            output_preview: None,
            error: Some("boom".to_string()),
            error_kind: Some(ErrorKind::Other),
            failure_code: None,
        }
    }

    #[test]
    fn header_lists_provider() {
        let pf = baseline_failure();
        let out = format_preflight_detail(&pf, "");
        // First line is the section header — colon is intentional for
        // visual grouping under the JSON envelope.
        let first = out.lines().next().expect("at least one line");
        assert_eq!(first, "Preflight (claude):");
        assert!(out.starts_with("Preflight (claude):\n"));
    }

    #[test]
    fn error_kind_renders_as_snake_case_without_quotes() {
        let mut pf = baseline_failure();
        pf.error_kind = Some(ErrorKind::NotInstalled);
        let out = format_preflight_detail(&pf, "");
        assert!(
            out.contains("\n  Error kind: not_installed\n"),
            "expected `Error kind: not_installed` line, got:\n{out}",
        );
        // Negative: no surrounding quotes.
        assert!(!out.contains("\"not_installed\""), "stray quotes: {out:?}");
    }

    #[test]
    fn version_and_model_only_render_when_set() {
        let mut pf = baseline_failure();
        // Both absent in baseline.
        let out = format_preflight_detail(&pf, "");
        assert!(!out.contains("Provider version:"));
        assert!(!out.contains("Model:"));

        // Both present.
        pf.version = Some("1.2.3".to_string());
        pf.model_used = Some("claude-opus-4-7".to_string());
        let out = format_preflight_detail(&pf, "");
        assert!(out.contains("\n  Provider version: 1.2.3\n"));
        assert!(out.contains("\n  Model: claude-opus-4-7\n"));
    }

    #[test]
    fn output_preview_truncated_at_cap() {
        let mut pf = baseline_failure();
        // 300 ASCII chars — cap is 200 → truncate + ellipsis.
        pf.output_preview = Some("a".repeat(300));
        let out = format_preflight_detail(&pf, "");
        let preview_line = out
            .lines()
            .find(|l| l.starts_with("  Output preview:"))
            .expect("preview line present");
        assert!(
            preview_line.ends_with('…'),
            "truncation marker missing: {preview_line}"
        );
        // The line is "  Output preview: " (18 chars) + 200 a's + '…'
        // (1 char). Count chars not bytes since '…' is multibyte.
        assert_eq!(
            preview_line.chars().count(),
            18 + PREFLIGHT_PREVIEW_CHAR_CAP + 1,
            "unexpected length on truncated preview: {preview_line:?}",
        );
    }

    #[test]
    fn detail_suppressed_when_equal_to_main_message() {
        let mut pf = baseline_failure();
        pf.error = Some("boom".to_string());
        // main_message == pf.error → no duplicate Detail: line.
        let out = format_preflight_detail(&pf, "boom");
        assert!(
            !out.contains("Detail:"),
            "Detail must be suppressed when equal to main message: {out:?}",
        );
    }

    #[test]
    fn detail_emitted_when_different_from_main_message() {
        let mut pf = baseline_failure();
        pf.error = Some("provider-specific extra context".to_string());
        let out = format_preflight_detail(&pf, "top-level summary");
        assert!(
            out.contains("\n  Detail: provider-specific extra context\n"),
            "Detail line missing when distinct from main message: {out:?}",
        );
    }

    #[test]
    fn fully_populated_preflight_renders_all_expected_lines() {
        // End-to-end ordering sanity. If we ever reshuffle the field order
        // an agent regex pinned to "Error kind:" position would still
        // pass — but anything reading lines in order would break. Pin the
        // order explicitly.
        let pf = PreflightResult {
            available: false,
            provider: "claude".to_string(),
            version: Some("1.2.3".to_string()),
            model_used: Some("bogus-model".to_string()),
            duration_ms: 245,
            output_preview: Some("API returned: model not found".to_string()),
            error: Some("model not found body".to_string()),
            error_kind: Some(ErrorKind::Other),
            failure_code: None,
        };
        let out = format_preflight_detail(&pf, "model not found");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "Preflight (claude):");
        assert_eq!(lines[1], "  Error kind: other");
        assert_eq!(lines[2], "  Provider version: 1.2.3");
        assert_eq!(lines[3], "  Model: bogus-model");
        assert_eq!(lines[4], "  Output preview: API returned: model not found");
        assert_eq!(lines[5], "  Detail: model not found body");
        // Six fields, no trailing extras.
        assert_eq!(lines.len(), 6, "unexpected extra lines: {lines:?}");
    }

    #[test]
    fn failure_code_line_renders_between_error_kind_and_version() {
        // Locks the position so agent regex can grab `Failure code:` after
        // `Error kind:` deterministically.
        let pf = PreflightResult {
            available: false,
            provider: "hermes".to_string(),
            version: Some("0.9.0".to_string()),
            model_used: None,
            duration_ms: 12,
            output_preview: None,
            error: Some("no LLM configured".to_string()),
            error_kind: Some(ErrorKind::Other),
            failure_code: Some("hermes_default_profile_no_llm".to_string()),
        };
        let out = format_preflight_detail(&pf, "no LLM configured");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "Preflight (hermes):");
        assert_eq!(lines[1], "  Error kind: other");
        assert_eq!(lines[2], "  Failure code: hermes_default_profile_no_llm");
        assert_eq!(lines[3], "  Provider version: 0.9.0");
        // No Model line because model_used is None.
        // Detail suppressed because matches main_message.
        assert_eq!(lines.len(), 4, "unexpected extra lines: {lines:?}");
    }

    #[test]
    fn truncate_chars_passes_short_through() {
        assert_eq!(truncate_chars("hello", 200), "hello");
    }

    #[test]
    fn truncate_chars_cuts_long_with_ellipsis() {
        let s = "a".repeat(50);
        let out = truncate_chars(&s, 10);
        assert!(out.ends_with('…'));
        // 10 a's + '…' = 11 chars
        assert_eq!(out.chars().count(), 11);
    }

    #[test]
    fn truncate_chars_handles_multibyte_codepoints() {
        // Each '中' is 3 bytes but 1 char; truncation must count chars,
        // not bytes, or we'd slice mid-codepoint.
        let s: String = "中".repeat(20);
        let out = truncate_chars(&s, 5);
        // 5 chars + '…' = 6 chars total
        assert_eq!(out.chars().count(), 6);
        assert!(out.ends_with('…'));
    }
}
