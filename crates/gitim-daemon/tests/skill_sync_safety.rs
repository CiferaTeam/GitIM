#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use gitim_daemon::api::Request;
use gitim_daemon::handlers::handle_request;

#[test]
fn daemon_working_branch_publication_has_one_state_facade() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations = Vec::new();
    visit_rust_files(&src, &mut |path, source| {
        for (line_index, line) in source.lines().enumerate() {
            if line.contains("git_storage.push(")
                || line.contains("push_working_branch_unchecked(")
                || line.contains("git_storage.rebase_onto_origin(")
                || line.contains("git_storage.pull_rebase(")
                || line.contains("git_storage.discard_unpushed(")
                || line.contains("rotate::follow_redirect(")
            {
                violations.push(format!("{}:{}", path.display(), line_index + 1));
            }
        }
    });
    assert!(
        violations.is_empty(),
        "daemon bypasses AppState guarded sync facade at {violations:?}"
    );

    let state_source = fs::read_to_string(src.join("state.rs")).unwrap();
    let callback_start = state_source
        .find("if synced_state.is_redirected()")
        .unwrap();
    let callback_tail = &state_source[callback_start..];
    let callback_end = callback_tail.find("\n                    }\n").unwrap();
    assert!(
        !callback_tail[..callback_end].contains(".commit_lock"),
        "on_synced must not acquire commit_lock around guarded redirect follow"
    );
}

#[test]
fn unchecked_working_branch_push_is_private_to_the_guard_implementation() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let sync_src = crates.join("gitim-sync/src");
    let mut violations = Vec::new();
    visit_rust_files(&sync_src, &mut |path, source| {
        let allowed = path.ends_with("git.rs") || path.ends_with("skill/guard.rs");
        if allowed {
            return;
        }
        for (line_index, line) in source.lines().enumerate() {
            if line.contains("push_working_branch_unchecked(") {
                violations.push(format!("{}:{}", path.display(), line_index + 1));
            }
        }
    });
    assert!(
        violations.is_empty(),
        "unchecked working-branch push escaped git/guard implementation: {violations:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_writer_categories_publish_through_real_production_paths() {
    for case in WriterCase::ALL {
        let fixture = ProductionFixture::new(case).await;
        let response = match case {
            WriterCase::UserArchive => Some(
                handle_request(
                    request(serde_json::json!({
                        "method": "archive_user",
                        "handler": "bob",
                        "author": "alice"
                    })),
                    fixture.state.clone(),
                )
                .await,
            ),
            WriterCase::ChannelArchive => Some(
                handle_request(
                    request(serde_json::json!({
                        "method": "archive_channel",
                        "channel": "general",
                        "author": "alice"
                    })),
                    fixture.state.clone(),
                )
                .await,
            ),
            WriterCase::DmArchive => Some(
                handle_request(
                    request(serde_json::json!({
                        "method": "archive_dm",
                        "peer": "bob",
                        "author": "alice"
                    })),
                    fixture.state.clone(),
                )
                .await,
            ),
            WriterCase::Onboard => Some(
                handle_request(
                    request(serde_json::json!({
                        "method": "onboard",
                        "git_server": "git",
                        "auth": {
                            "type": "git",
                            "handler": "new-agent",
                            "display_name": "New Agent"
                        },
                        "join_general": false
                    })),
                    fixture.state.clone(),
                )
                .await,
            ),
            WriterCase::CardArchive => Some(
                handle_request(
                    request(serde_json::json!({
                        "method": "archive_card",
                        "channel": "general",
                        "card_id": "c1",
                        "author": "alice"
                    })),
                    fixture.state.clone(),
                )
                .await,
            ),
            WriterCase::Reconcile => {
                let migrated =
                    gitim_daemon::reconcile::reconcile_orphan_cards(fixture.state.clone())
                        .await
                        .unwrap();
                assert_eq!(migrated, 1, "{case:?} did not invoke reconcile writer");
                None
            }
        };
        if let Some(response) = response {
            assert!(
                response.ok,
                "{case:?} production request failed: {:?}",
                response.error
            );
        }
        fixture.assert_sanitized_publication(case);
    }
}

#[test]
fn daemon_redirect_follow_completes_without_an_outer_lock_deadlock() {
    let directory = tempfile::tempdir().unwrap();
    let remote = directory.path().join("origin.git");
    run_git(
        directory.path(),
        &["init", "--bare", "-b", "main", remote.to_str().unwrap()],
    );
    let rotator = directory.path().join("rotator");
    run_git(
        directory.path(),
        &["clone", remote.to_str().unwrap(), rotator.to_str().unwrap()],
    );
    run_git(&rotator, &["config", "user.name", "rotator"]);
    run_git(&rotator, &["config", "user.email", "rotator@example.com"]);
    fs::write(rotator.join(".gitignore"), ".gitim/\n").unwrap();
    for index in 0..3 {
        fs::write(rotator.join(format!("seed-{index}.txt")), "seed\n").unwrap();
        commit_all(&rotator, &format!("seed {index}"));
    }
    run_git(&rotator, &["push", "-u", "origin", "main"]);

    let follower = directory.path().join("follower");
    run_git(
        directory.path(),
        &[
            "clone",
            remote.to_str().unwrap(),
            follower.to_str().unwrap(),
        ],
    );
    run_git(&follower, &["config", "user.name", "follower"]);
    run_git(&follower, &["config", "user.email", "follower@example.com"]);

    let (rotator_events, _) = tokio::sync::broadcast::channel(4);
    let rotator_state = Arc::new(gitim_daemon::state::AppState::new(
        rotator,
        gitim_core::types::Config::default(),
        rotator_events,
        None,
    ));
    assert!(rotator_state.attempt_rotation_for_test(3).unwrap());

    let (follower_events, _) = tokio::sync::broadcast::channel(4);
    let follower_state = Arc::new(gitim_daemon::state::AppState::new(
        follower.clone(),
        gitim_core::types::Config::default(),
        follower_events,
        None,
    ));
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = follower_state.integrate_working_branch(
            gitim_sync::skill::guard::IntegrationOperation::FollowEpochRedirect,
        );
        let _ = done_tx.send(result.map(|_| ()));
    });

    done_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert_eq!(
        git_output(&follower, &["branch", "--show-current"]),
        "main-epoch-2"
    );
}

#[test]
fn daemon_writer_sources_use_the_state_facade_for_each_category() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let categories = [
        ("user archive", "handlers/user.rs", "archive_user"),
        ("channel archive", "handlers/channel.rs", "archive_channel"),
        ("DM archive", "handlers/dm.rs", "archive_dm"),
        ("onboard", "onboard.rs", "push_working_branch"),
        ("card", "card_handlers.rs", "push_working_branch_with_retry"),
        (
            "reconcile",
            "reconcile.rs",
            "push_working_branch_with_retry",
        ),
    ];
    for (category, relative, facade_call) in categories {
        let source = fs::read_to_string(src.join(relative)).unwrap();
        assert!(
            source.contains(facade_call),
            "{category} writer is not routed through AppState"
        );
    }
}

#[derive(Clone, Copy, Debug)]
enum WriterCase {
    UserArchive,
    ChannelArchive,
    DmArchive,
    Onboard,
    CardArchive,
    Reconcile,
}

impl WriterCase {
    const ALL: [Self; 6] = [
        Self::UserArchive,
        Self::ChannelArchive,
        Self::DmArchive,
        Self::CardArchive,
        Self::Reconcile,
        Self::Onboard,
    ];

    const fn expected_path(self) -> &'static str {
        match self {
            Self::UserArchive => "archive/users/bob.meta.yaml",
            Self::ChannelArchive => "archive/channels/general.thread",
            Self::DmArchive => "archive/dm/alice--bob.thread",
            Self::Onboard => "users/new-agent.meta.yaml",
            Self::CardArchive => "archive/channels/general/cards/c1/card.meta.yaml",
            Self::Reconcile => "archive/channels/legacy/cards/c1/card.meta.yaml",
        }
    }
}

struct ProductionFixture {
    _directory: tempfile::TempDir,
    local: PathBuf,
    state: Arc<gitim_daemon::state::AppState>,
}

impl ProductionFixture {
    async fn new(case: WriterCase) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let remote = directory.path().join("origin.git");
        run_git(
            directory.path(),
            &["init", "--bare", remote.to_str().unwrap()],
        );
        let local = directory.path().join("local");
        run_git(
            directory.path(),
            &["clone", remote.to_str().unwrap(), local.to_str().unwrap()],
        );
        run_git(&local, &["config", "user.name", "alice"]);
        run_git(&local, &["config", "user.email", "alice@example.com"]);
        fs::write(local.join(".gitignore"), ".gitim/\n").unwrap();
        seed_writer_case(&local, case);
        commit_all(&local, "seed production writer fixture");
        run_git(&local, &["branch", "-M", "main"]);
        run_git(&local, &["push", "-u", "origin", "main"]);

        let users: &[&str] = if matches!(case, WriterCase::Onboard) {
            &[]
        } else {
            &["alice", "bob"]
        };
        let current_user = (!matches!(case, WriterCase::Onboard)).then(|| "alice".to_owned());
        let (event_tx, _) = tokio::sync::broadcast::channel(16);
        let state = Arc::new(gitim_daemon::state::AppState::new(
            local.clone(),
            gitim_core::types::Config::default(),
            event_tx,
            current_user,
        ));
        {
            let mut state_users = state.users.write().await;
            *state_users = users.iter().map(|handler| (*handler).to_owned()).collect();
        }

        fs::create_dir_all(local.join("skills/invalid")).unwrap();
        fs::write(local.join("skills/invalid/SKILL.md"), "# bypass\n").unwrap();
        commit_all(&local, "invalid Skill bypass before production writer");

        Self {
            _directory: directory,
            local,
            state,
        }
    }

    fn assert_sanitized_publication(&self, case: WriterCase) {
        let remote_tip = git_output(&self.local, &["rev-parse", "refs/remotes/origin/main"]);
        assert!(
            git_status(
                &self.local,
                &[
                    "cat-file",
                    "-e",
                    &format!("{remote_tip}:{}", case.expected_path())
                ]
            )
            .is_some(),
            "{case:?} ordinary production delta was not published"
        );
        assert!(
            git_status(
                &self.local,
                &[
                    "cat-file",
                    "-e",
                    &format!("{remote_tip}:skills/invalid/SKILL.md")
                ]
            )
            .is_none(),
            "{case:?} published bypassed Skill content"
        );
        let quarantine_refs = git_output(
            &self.local,
            &[
                "for-each-ref",
                "--format=%(refname)",
                "refs/gitim/quarantine",
            ],
        );
        assert!(
            quarantine_refs
                .lines()
                .any(|reference| reference.starts_with("refs/gitim/quarantine/skill-")),
            "{case:?} did not retain a quarantine ref"
        );
        assert!(
            !self.local.join(".gitim/skill-quarantine.json").exists(),
            "{case:?} left a completed quarantine journal"
        );

        if matches!(case, WriterCase::UserArchive) {
            let quarantine_ref = quarantine_refs.lines().next().unwrap();
            let message = git_output(&self.local, &["show", "-s", "--format=%B", quarantine_ref]);
            assert!(
                message.contains("Gitim-Skills-Tree: absent"),
                "user archive did not bind its trailer to accepted origin Skill state: {message}"
            );
        }
    }
}

fn request(value: serde_json::Value) -> Request {
    serde_json::from_value(value).unwrap()
}

fn seed_writer_case(root: &Path, case: WriterCase) {
    if matches!(case, WriterCase::Onboard) {
        fs::write(root.join(".keep"), "\n").unwrap();
        return;
    }
    write_user(root, "alice", "Alice");
    write_user(root, "bob", "Bob");
    match case {
        WriterCase::UserArchive => {}
        WriterCase::ChannelArchive => write_channel(root, "general", false),
        WriterCase::DmArchive => {
            fs::create_dir_all(root.join("dm")).unwrap();
            fs::write(
                root.join("dm/alice--bob.thread"),
                "[L000001][P000000][@alice][20260730T120000Z] hello\n",
            )
            .unwrap();
        }
        WriterCase::CardArchive => {
            write_channel(root, "general", false);
            write_card(root, "channels/general/cards/c1");
        }
        WriterCase::Reconcile => {
            write_channel(root, "legacy", true);
            write_card(root, "channels/legacy/cards/c1");
        }
        WriterCase::Onboard => {}
    }
}

fn write_user(root: &Path, handler: &str, display_name: &str) {
    fs::create_dir_all(root.join("users")).unwrap();
    fs::write(
        root.join(format!("users/{handler}.meta.yaml")),
        format!("display_name: {display_name}\nrole: member\nintroduction: test user\n"),
    )
    .unwrap();
}

fn write_channel(root: &Path, channel: &str, archived: bool) {
    let prefix = if archived {
        "archive/channels"
    } else {
        "channels"
    };
    fs::create_dir_all(root.join(prefix)).unwrap();
    fs::write(
        root.join(format!("{prefix}/{channel}.meta.yaml")),
        format!(
            "display_name: {channel}\ncreated_by: alice\ncreated_at: \"20260730T120000Z\"\nintroduction: test channel\nmembers:\n- alice\n- bob\n"
        ),
    )
    .unwrap();
    fs::write(
        root.join(format!("{prefix}/{channel}.thread")),
        "[L000001][P000000][@alice][20260730T120000Z] seeded\n",
    )
    .unwrap();
}

fn write_card(root: &Path, relative: &str) {
    let directory = root.join(relative);
    fs::create_dir_all(&directory).unwrap();
    let channel = if relative.contains("/legacy/") {
        "legacy"
    } else {
        "general"
    };
    fs::write(
        directory.join("card.meta.yaml"),
        format!(
            "title: Test card\nchannel: {channel}\nstatus: todo\nlabels: []\nassignee: null\ncreated_by: alice\ncreated_at: \"20260730T120000Z\"\nupdated_at: \"20260730T120000Z\"\n"
        ),
    )
    .unwrap();
    fs::write(directory.join("discussion.thread"), "").unwrap();
}

fn visit_rust_files(directory: &Path, visitor: &mut impl FnMut(&Path, &str)) {
    for entry in fs::read_dir(directory).unwrap() {
        let path: PathBuf = entry.unwrap().path();
        if path.is_dir() {
            visit_rust_files(&path, visitor);
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            visitor(&path, &fs::read_to_string(&path).unwrap());
        }
    }
}

fn run_git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_output(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn git_status(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).unwrap().trim().to_owned())
}

fn commit_all(root: &Path, message: &str) {
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "-m", message]);
}
