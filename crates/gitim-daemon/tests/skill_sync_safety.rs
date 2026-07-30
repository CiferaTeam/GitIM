#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

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

#[test]
fn daemon_writer_categories_publish_through_quarantine_replay() {
    let cases = [
        ("user_archive", "archive/users/bob.meta.yaml"),
        ("channel_archive", "archive/channels/general/general.thread"),
        ("dm_archive", "archive/dm/alice--bob.thread"),
        ("onboard", "users/new-agent.meta.yaml"),
        (
            "card_archive",
            "archive/channels/general/cards/C1/card.meta.yaml",
        ),
        (
            "reconcile",
            "archive/channels/legacy/cards/C2/card.meta.yaml",
        ),
    ];

    for (category, ordinary_path) in cases {
        let fixture = tempfile::tempdir().unwrap();
        let remote = fixture.path().join("origin.git");
        run_git(
            fixture.path(),
            &["init", "--bare", remote.to_str().unwrap()],
        );
        let local = fixture.path().join("local");
        run_git(
            fixture.path(),
            &["clone", remote.to_str().unwrap(), local.to_str().unwrap()],
        );
        run_git(&local, &["config", "user.name", "alice"]);
        run_git(&local, &["config", "user.email", "alice@example.com"]);
        fs::write(local.join(".keep"), "\n").unwrap();
        commit_all(&local, "seed");
        run_git(&local, &["branch", "-M", "main"]);
        run_git(&local, &["push", "-u", "origin", "main"]);

        let (event_tx, _) = tokio::sync::broadcast::channel(4);
        let state = Arc::new(gitim_daemon::state::AppState::new(
            local.clone(),
            gitim_core::types::Config::default(),
            event_tx,
            Some("alice".to_owned()),
        ));
        fs::create_dir_all(local.join("skills/invalid")).unwrap();
        fs::write(local.join("skills/invalid/SKILL.md"), "# bypass\n").unwrap();
        commit_all(&local, "invalid Skill bypass");
        let target = local.join(ordinary_path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, format!("{category}\n")).unwrap();
        let commit_message = if category == "user_archive" {
            "archive: depart user @bob\n\nGitim-Skills-Tree: absent"
        } else {
            category
        };
        commit_all(&local, commit_message);

        state.push_working_branch_with_retry(category).unwrap();

        let remote_tip = git_output(&local, &["rev-parse", "refs/remotes/origin/main"]);
        assert_eq!(
            git_output(&local, &["show", &format!("{remote_tip}:{ordinary_path}")]),
            category
        );
        assert!(
            git_status(
                &local,
                &[
                    "cat-file",
                    "-e",
                    &format!("{remote_tip}:skills/invalid/SKILL.md")
                ]
            )
            .is_none(),
            "{category} published bypassed Skill content"
        );
        let quarantine_refs = git_output(
            &local,
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
            "{category} did not retain a quarantine ref"
        );
    }
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
