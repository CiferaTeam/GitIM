#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gitim_core::skill::{
    validate_package_entries, PackageEntry, RequestId, SkillCreateRequest, SkillListQuery,
    SkillMutationRequest, SkillReference, SkillResourceQuery, SkillShowQuery, SkillSlug,
    SkillWorkspaceBootstrapRequest,
};
use gitim_daemon::skill_store::SkillStore;
use gitim_sync::git::GitStorage;
use gitim_sync::skill::checkpoint::SkillCheckpointStore;
use gitim_sync::skill::guard::SkillSyncGuard;
use gitim_sync::skill::transaction::{
    execute_remote_skill_transaction, RemoteSkillTransactionRequest,
};
use tempfile::TempDir;

fn git<I, S>(root: &Path, args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn configure(root: &Path) {
    git(root, ["config", "user.name", "Alice"]);
    git(root, ["config", "user.email", "alice@example.com"]);
    git(root, ["config", "commit.gpgsign", "false"]);
}

struct Fixture {
    _root: TempDir,
    repo: PathBuf,
    accepted: String,
    rejected: String,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let remote = root.path().join("origin.git");
        git(
            root.path(),
            ["init", "--bare", "-b", "main", remote.to_str().unwrap()],
        );
        let seed = root.path().join("seed");
        fs::create_dir(&seed).unwrap();
        git(&seed, ["init", "-b", "main"]);
        configure(&seed);
        fs::create_dir_all(seed.join("users")).unwrap();
        fs::write(
            seed.join("users/alice.meta.yaml"),
            "display_name: Alice\nrole: human\nintroduction: Owner\n",
        )
        .unwrap();
        git(&seed, ["add", "."]);
        git(&seed, ["commit", "-m", "initialize"]);
        git(
            &seed,
            [
                OsStr::new("remote"),
                OsStr::new("add"),
                OsStr::new("origin"),
                remote.as_os_str(),
            ],
        );
        git(&seed, ["push", "-u", "origin", "main"]);

        let repo = root.path().join("repo");
        git(
            root.path(),
            [OsStr::new("clone"), remote.as_os_str(), repo.as_os_str()],
        );
        configure(&repo);
        let storage = GitStorage::new(&repo);
        let guard = SkillSyncGuard::new(&repo).unwrap();
        let transaction = |request, package| {
            execute_remote_skill_transaction(
                &storage,
                &guard,
                RemoteSkillTransactionRequest {
                    request,
                    actor: "alice".to_owned(),
                    author_email: "alice@example.com".to_owned(),
                    now: "2026-07-31T00:00:00Z".to_owned(),
                    package,
                },
            )
            .unwrap()
        };
        transaction(
            SkillMutationRequest::WorkspaceBootstrap(SkillWorkspaceBootstrapRequest {
                request_id: RequestId::generate(),
            }),
            None,
        );
        let slug = SkillSlug::new("read-race").unwrap();
        transaction(
            SkillMutationRequest::Create(SkillCreateRequest {
                request_id: RequestId::generate(),
                slug: slug.clone(),
                display_name: "Accepted display".to_owned(),
                description: "Accepted description".to_owned(),
                source_directory: repo.join("unused"),
            }),
            Some(
                validate_package_entries(
                    &slug,
                    vec![
                        PackageEntry::new(
                            "SKILL.md",
                            b"---\nname: read-race\ndescription: Accepted\n---\n\naccepted body\n"
                                .to_vec(),
                        ),
                        PackageEntry::new("references/value.txt", b"accepted resource\n".to_vec()),
                    ],
                )
                .unwrap(),
            ),
        );
        let checkpoint = SkillCheckpointStore::new(&repo)
            .unwrap()
            .load()
            .unwrap()
            .unwrap();
        let accepted = checkpoint.skills["read-race"].tree.commit_oid.clone();
        git(&repo, ["reset", "--hard", &accepted]);
        let revision = fs::read_dir(repo.join("skills/read-race/revisions"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .file_name();
        let package = repo
            .join("skills/read-race/revisions")
            .join(revision)
            .join("package");
        fs::write(
            package.join("SKILL.md"),
            "---\nname: read-race\ndescription: Rejected\n---\n\nrejected body\n",
        )
        .unwrap();
        fs::write(package.join("references/value.txt"), "rejected resource\n").unwrap();
        let meta_path = repo.join("skills/read-race/skill.meta.yaml");
        let meta = fs::read_to_string(&meta_path).unwrap().replace(
            "display_name: Accepted display",
            "display_name: Rejected display",
        );
        fs::write(meta_path, meta).unwrap();
        git(&repo, ["add", "."]);
        git(&repo, ["commit", "-m", "rejected checkout"]);
        let rejected = String::from_utf8(git(&repo, ["rev-parse", "HEAD"]).stdout)
            .unwrap()
            .trim()
            .to_owned();
        git(&repo, ["reset", "--hard", &accepted]);

        Self {
            _root: root,
            repo,
            accepted,
            rejected,
        }
    }
}

struct PathGuard {
    original: Option<OsString>,
}

impl Drop for PathGuard {
    fn drop(&mut self) {
        match self.original.take() {
            Some(path) => std::env::set_var("PATH", path),
            None => std::env::remove_var("PATH"),
        }
    }
}

struct Gate {
    _path: PathGuard,
    real_git: PathBuf,
    ready: PathBuf,
    release: PathBuf,
    count: PathBuf,
}

impl Gate {
    fn install(root: &Path) -> Self {
        let original = std::env::var_os("PATH");
        let real_git = original
            .as_ref()
            .and_then(|path| {
                std::env::split_paths(path)
                    .map(|directory| directory.join("git"))
                    .find(|candidate| candidate.is_file())
            })
            .unwrap();
        let bin = root.join("bin");
        fs::create_dir(&bin).unwrap();
        let wrapper = bin.join("git");
        fs::write(
            &wrapper,
            r#"#!/bin/sh
gate=false
for argument in "$@"; do
  if [ "$argument" = ":(literal)skills/read-race" ]; then
    gate=true
  fi
done
if [ "$gate" = true ]; then
  count=0
  if [ -f "$GITIM_READ_GATE_COUNT" ]; then
    count=$(cat "$GITIM_READ_GATE_COUNT")
  fi
  count=$((count + 1))
  printf '%s\n' "$count" > "$GITIM_READ_GATE_COUNT"
  if [ "$count" = "$GITIM_READ_GATE_TARGET" ]; then
    output=$(mktemp)
    "$GITIM_REAL_GIT" "$@" > "$output"
    status=$?
    : > "$GITIM_READ_GATE_READY"
    while [ ! -f "$GITIM_READ_GATE_RELEASE" ]; do
      sleep 0.01
    done
    cat "$output"
    rm -f "$output"
    exit "$status"
  fi
fi
exec "$GITIM_REAL_GIT" "$@"
"#,
        )
        .unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var("GITIM_REAL_GIT", &real_git);
        let ready = root.join("ready");
        let release = root.join("release");
        let count = root.join("count");
        std::env::set_var("GITIM_READ_GATE_READY", &ready);
        std::env::set_var("GITIM_READ_GATE_RELEASE", &release);
        std::env::set_var("GITIM_READ_GATE_COUNT", &count);
        let mut paths = vec![bin];
        if let Some(original) = &original {
            paths.extend(std::env::split_paths(original));
        }
        std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
        Self {
            _path: PathGuard { original },
            real_git,
            ready,
            release,
            count,
        }
    }

    fn interleave<T: Send + 'static>(
        &self,
        fixture: &Fixture,
        target: usize,
        operation: impl FnOnce() -> T + Send + 'static,
    ) -> T {
        for path in [&self.ready, &self.release, &self.count] {
            if path.exists() {
                fs::remove_file(path).unwrap();
            }
        }
        std::env::set_var("GITIM_READ_GATE_TARGET", target.to_string());
        let worker = std::thread::spawn(operation);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.ready.exists() {
            assert!(
                Instant::now() < deadline,
                "read did not reach equality gate"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let checkout = Command::new(&self.real_git)
            .args(["reset", "--hard", &fixture.rejected])
            .current_dir(&fixture.repo)
            .output()
            .unwrap();
        assert!(checkout.status.success());
        fs::write(&self.release, []).unwrap();
        worker.join().unwrap()
    }

    fn restore_accepted(&self, fixture: &Fixture) {
        let checkout = Command::new(&self.real_git)
            .args(["reset", "--hard", &fixture.accepted])
            .current_dir(&fixture.repo)
            .output()
            .unwrap();
        assert!(checkout.status.success());
    }
}

#[test]
fn accepted_reads_never_materialize_rejected_or_mixed_worktree_bytes() {
    let fixture = Fixture::new();
    let gate_dir = tempfile::tempdir().unwrap();
    let gate = Gate::install(gate_dir.path());
    let slug = SkillSlug::new("read-race").unwrap();

    let list_store = Arc::new(SkillStore::new(&fixture.repo));
    let list_worker = Arc::clone(&list_store);
    let listed = gate.interleave(&fixture, 1, move || {
        list_worker
            .list(SkillListQuery {
                archived: false,
                limit: 20,
                cursor: None,
            })
            .unwrap()
    });
    gate.restore_accepted(&fixture);
    let cached = list_store
        .list(SkillListQuery {
            archived: false,
            limit: 20,
            cursor: None,
        })
        .unwrap();

    let show_store = SkillStore::new(&fixture.repo);
    let show_slug = slug.clone();
    let shown = gate.interleave(&fixture, 1, move || {
        show_store
            .show(SkillShowQuery {
                slug: show_slug,
                revision: None,
            })
            .unwrap()
    });
    gate.restore_accepted(&fixture);

    let load_store = SkillStore::new(&fixture.repo);
    let load_slug = slug.clone();
    let loaded = gate.interleave(&fixture, 2, move || {
        load_store
            .load(&SkillReference {
                slug: load_slug,
                revision: None,
            })
            .unwrap()
    });
    gate.restore_accepted(&fixture);

    let resource_store = SkillStore::new(&fixture.repo);
    let resource_slug = slug;
    let resource = gate.interleave(&fixture, 2, move || {
        resource_store
            .resource(SkillResourceQuery {
                reference: SkillReference {
                    slug: resource_slug,
                    revision: None,
                },
                path: "references/value.txt".to_owned(),
            })
            .unwrap()
    });
    gate.restore_accepted(&fixture);

    assert_eq!(listed.skills[0].display_name, "Accepted display");
    assert_eq!(cached.skills[0].display_name, "Accepted display");
    assert_eq!(shown.meta.display_name, "Accepted display");
    assert!(loaded.skill_markdown.contains("accepted body"));
    assert_eq!(resource.bytes, b"accepted resource\n");
}
