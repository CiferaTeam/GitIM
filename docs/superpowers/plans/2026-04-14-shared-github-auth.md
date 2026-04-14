# Shared GitHub Auth Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Allow multiple agents to use a GitHub repo with a single shared credential, specifying handler + display_name manually instead of requiring per-agent GitHub tokens.

**Architecture:** CLI validation and auth construction allow `--handler` + `--display-name` as an alternative to `--token` in GitHub mode. When handler is provided, the CLI sends a Git-variant auth payload to the daemon (which already handles it). Clone/create falls back to `git clone` when `gh` CLI is unavailable. Daemon renames `inferred_at` → `onboarded_at` in me.json for semantic accuracy.

**Tech Stack:** Rust (daemon + CLI), TypeScript (legacy CLI)

**Key files overview:**
- `crates/gitim-daemon/src/onboard.rs` — me.json writing (daemon)
- `crates/gitim-cli/src/commands/onboard.rs` — Rust CLI onboard (validate, auth, clone)
- `legacy/cli/src/commands/onboard.ts` — Legacy TS CLI onboard (same logic)
- `crates/gitim-cli/src/main.rs` — Rust CLI arg help text

---

### Task 1: Daemon — rename `inferred_at` → `onboarded_at` in me.json

**Files:**
- Modify: `crates/gitim-daemon/src/onboard.rs:182` (write_me_json)
- Modify: `crates/gitim-daemon/src/onboard.rs:522` (test assertion)

- [ ] **Step 1: Update the test assertion first**

In `crates/gitim-daemon/src/onboard.rs`, find the test `write_me_json_creates_file` (line ~522).
Change:

```rust
        assert!(content["inferred_at"].as_str().is_some());
```

to:

```rust
        assert!(content["onboarded_at"].as_str().is_some());
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/shared-github-auth && cargo test -p gitim-daemon write_me_json_creates_file -- --nocapture`

Expected: FAIL — me.json still writes `inferred_at`, assertion on `onboarded_at` fails.

- [ ] **Step 3: Update write_me_json**

In `crates/gitim-daemon/src/onboard.rs`, find `write_me_json` (line ~182). Change:

```rust
        "inferred_at": now,
```

to:

```rust
        "onboarded_at": now,
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p gitim-daemon write_me_json_creates_file -- --nocapture`

Expected: PASS

- [ ] **Step 5: Run all daemon tests to check for regressions**

Run: `cargo test -p gitim-daemon`

Expected: All tests pass. No other code reads `inferred_at` from me.json — verified via grep during analysis.

- [ ] **Step 6: Commit**

```bash
git add crates/gitim-daemon/src/onboard.rs
git commit -m "refactor: rename inferred_at to onboarded_at in me.json"
```

---

### Task 2: Rust CLI — relax validation for GitHub mode

**Files:**
- Modify: `crates/gitim-cli/src/commands/onboard.rs:59-83` (validate_params)
- Modify: `crates/gitim-cli/src/main.rs:137-141` (help text)

- [ ] **Step 1: Update validate_params**

In `crates/gitim-cli/src/commands/onboard.rs`, replace the `validate_params` function (lines 59-83):

```rust
fn validate_params(git_server: &GitServer, args: &OnboardArgs) {
    match git_server {
        GitServer::Git => {
            if args.handler.is_none() {
                eprintln!("Error: git 本地模式需要 --handler");
                process::exit(1);
            }
            if args.display_name.is_none() {
                eprintln!("Error: git 本地模式需要 --display-name");
                process::exit(1);
            }
        }
        GitServer::Github => {
            // GitHub mode: either handler+display_name (shared auth) or token (API inference)
            let has_handler = args.handler.is_some() && args.display_name.is_some();
            let has_token = args.token.is_some();
            if !has_handler && !has_token {
                eprintln!("Error: github 模式需要 --handler + --display-name 或 --token");
                process::exit(1);
            }
        }
        other => {
            let name = other.as_str();
            if args.token.is_none() {
                eprintln!("Error: {name} 模式需要 --token");
                process::exit(1);
            }
            if matches!(other, GitServer::Gitea | GitServer::Gitlab) && args.url.is_none() {
                eprintln!("Error: {name} 模式需要 --url（服务地址）");
                process::exit(1);
            }
        }
    }
}
```

- [ ] **Step 2: Update help text in main.rs**

In `crates/gitim-cli/src/main.rs`, update the arg comments (lines 137-141):

```rust
        /// Handler (required for git mode; optional for github with --display-name)
        #[arg(long)]
        handler: Option<String>,
        /// Display name (required for git mode; optional for github with --handler)
        #[arg(long)]
        display_name: Option<String>,
```

- [ ] **Step 3: Build to verify compilation**

Run: `cargo build -p gitim-cli`

Expected: Compiles without errors.

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-cli/src/commands/onboard.rs crates/gitim-cli/src/main.rs
git commit -m "feat(cli): allow handler+display-name for github mode"
```

---

### Task 3: Rust CLI — build_auth sends Git variant when handler provided

**Files:**
- Modify: `crates/gitim-cli/src/commands/onboard.rs:85-103` (build_auth)

- [ ] **Step 1: Update build_auth**

In `crates/gitim-cli/src/commands/onboard.rs`, replace the `build_auth` function (lines 85-103):

```rust
fn build_auth(git_server: &GitServer, args: &OnboardArgs) -> Value {
    // If handler + display_name are provided, always use Git-style auth
    // (works for both git and github modes with shared credentials)
    if let (Some(handler), Some(display_name)) = (&args.handler, &args.display_name) {
        return json!({
            "handler": handler,
            "display_name": display_name,
        });
    }

    match git_server {
        GitServer::Git => {
            // validate_params guarantees handler+display_name for git mode,
            // so this branch is unreachable — but keep it for safety
            json!({
                "handler": args.handler.as_ref().unwrap(),
                "display_name": args.display_name.as_ref().unwrap(),
            })
        }
        other => {
            let mut auth = json!({ "token": args.token.as_ref().unwrap() });
            if matches!(other, GitServer::Gitea | GitServer::Gitlab) {
                if let Some(url) = &args.url {
                    auth["url"] = json!(url);
                }
            }
            auth
        }
    }
}
```

- [ ] **Step 2: Build to verify**

Run: `cargo build -p gitim-cli`

Expected: Compiles without errors.

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-cli/src/commands/onboard.rs
git commit -m "feat(cli): build git-variant auth when handler provided in github mode"
```

---

### Task 4: Rust CLI — clone_or_create_repo GitHub fallback

**Files:**
- Modify: `crates/gitim-cli/src/commands/onboard.rs:141-178` (clone_or_create_repo Github branch)

- [ ] **Step 1: Update the Github branch in clone_or_create_repo**

In `crates/gitim-cli/src/commands/onboard.rs`, replace the `GitServer::Github` arm (lines 141-178):

```rust
        GitServer::Github => {
            let gh_target = match org {
                Some(o) => format!("{o}/{repo_name}"),
                None => repo_name.to_string(),
            };

            // Try gh CLI first (uses gh's own auth)
            let clone_ok = Command::new("gh")
                .args(["repo", "clone", &gh_target, target_dir.to_str().unwrap_or(repo_name)])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);

            if clone_ok {
                return target_dir;
            }

            // gh clone failed — try gh create
            let parent = target_dir.parent().unwrap_or_else(|| Path::new("."));
            let create_ok = Command::new("gh")
                .args(["repo", "create", &gh_target, "--private", "--clone"])
                .current_dir(parent)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);

            if create_ok {
                return target_dir;
            }

            // gh not available or failed — fallback to git clone
            // Construct URL: use token for HTTPS if available, else try SSH
            let org_name = org.unwrap_or_else(|| {
                eprintln!("Error: gh 不可用时，github 模式需要指定 org");
                eprintln!("  → 用法: gitim onboard <repo> <org> --git-server github --handler ...");
                process::exit(1);
            });

            let clone_url = if let Some(token) = &args.token {
                format!("https://x-access-token:{token}@github.com/{org_name}/{repo_name}.git")
            } else {
                format!("git@github.com:{org_name}/{repo_name}.git")
            };

            let git_clone_ok = Command::new("git")
                .args(["clone", &clone_url, target_dir.to_str().unwrap_or(repo_name)])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);

            if git_clone_ok {
                return target_dir;
            }

            // All attempts failed — create local repo as last resort
            eprintln!("Warning: 无法克隆远程仓库，创建本地 git 仓库");
            fs::create_dir_all(&target_dir).unwrap_or_else(|e| {
                eprintln!("Error: cannot create directory: {e}");
                process::exit(1);
            });
            let init_ok = Command::new("git")
                .args(["init"])
                .current_dir(&target_dir)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !init_ok {
                eprintln!("Error: git init 失败");
                process::exit(1);
            }

            // Add remote for future push
            let remote_url = if let Some(token) = &args.token {
                format!("https://x-access-token:{token}@github.com/{org_name}/{repo_name}.git")
            } else {
                format!("git@github.com:{org_name}/{repo_name}.git")
            };
            let _ = Command::new("git")
                .args(["remote", "add", "origin", &remote_url])
                .current_dir(&target_dir)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();

            target_dir
        }
```

- [ ] **Step 2: Build to verify**

Run: `cargo build -p gitim-cli`

Expected: Compiles without errors.

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-cli/src/commands/onboard.rs
git commit -m "feat(cli): github clone fallback to git clone when gh unavailable"
```

---

### Task 5: Legacy TS CLI — same changes

**Files:**
- Modify: `legacy/cli/src/commands/onboard.ts:25-37` (buildAuth)
- Modify: `legacy/cli/src/commands/onboard.ts:57-77` (validateParams)
- Modify: `legacy/cli/src/commands/onboard.ts:100-168` (cloneOrCreateRepo Github branch)
- Modify: `legacy/cli/src/index.ts:39-40` (help text)

- [ ] **Step 1: Update validateParams**

In `legacy/cli/src/commands/onboard.ts`, replace `validateParams` (lines 57-77):

```typescript
function validateParams(gitServer: GitServer, options: OnboardOptions): void {
  if (gitServer === 'git') {
    if (!options.handler) {
      console.error('Error: git 本地模式需要 --handler');
      process.exit(1);
    }
    if (!options.displayName) {
      console.error('Error: git 本地模式需要 --display-name');
      process.exit(1);
    }
  } else if (gitServer === 'github') {
    const hasHandler = options.handler && options.displayName;
    const hasToken = !!options.token;
    if (!hasHandler && !hasToken) {
      console.error('Error: github 模式需要 --handler + --display-name 或 --token');
      process.exit(1);
    }
  } else {
    if (!options.token) {
      console.error(`Error: ${gitServer} 模式需要 --token`);
      process.exit(1);
    }
    if ((gitServer === 'gitea' || gitServer === 'gitlab') && !options.url) {
      console.error(`Error: ${gitServer} 模式需要 --url（服务地址）`);
      process.exit(1);
    }
  }
}
```

- [ ] **Step 2: Update buildAuth**

In `legacy/cli/src/commands/onboard.ts`, replace `buildAuth` (lines 25-37):

```typescript
function buildAuth(gitServer: GitServer, options: OnboardOptions): Record<string, string> {
  // If handler + display_name provided, use Git-style auth (shared credentials)
  if (options.handler && options.displayName) {
    return {
      handler: options.handler,
      display_name: options.displayName,
    };
  }

  if (gitServer === 'git') {
    return {
      handler: options.handler!,
      display_name: options.displayName!,
    };
  }
  const auth: Record<string, string> = { token: options.token! };
  if ((gitServer === 'gitea' || gitServer === 'gitlab') && options.url) {
    auth.url = options.url;
  }
  return auth;
}
```

- [ ] **Step 3: Update cloneOrCreateRepo Github branch**

In `legacy/cli/src/commands/onboard.ts`, replace the github block inside `cloneOrCreateRepo` (lines 100-124):

```typescript
  if (gitServer === 'github') {
    const ghTarget = org ? `${org}/${repoName}` : repoName;

    // Try gh CLI first
    try {
      execFileSync('gh', ['repo', 'clone', ghTarget, targetDir], { stdio: 'ignore' });
      return targetDir;
    } catch {
      // gh clone failed
    }

    try {
      execFileSync('gh', ['repo', 'create', ghTarget, '--private', '--clone'], {
        cwd: path.dirname(targetDir),
        stdio: 'ignore',
      });
      return targetDir;
    } catch {
      // gh create failed
    }

    // Fallback: git clone (needs org for URL construction)
    if (!org) {
      console.error('Error: gh 不可用时，github 模式需要指定 org');
      console.error('  → 用法: gitim onboard <repo> <org> --git-server github --handler ...');
      process.exit(1);
    }

    const cloneUrl = options.token
      ? `https://x-access-token:${options.token}@github.com/${org}/${repoName}.git`
      : `git@github.com:${org}/${repoName}.git`;

    try {
      execFileSync('git', ['clone', cloneUrl, targetDir], { stdio: 'ignore' });
      return targetDir;
    } catch {
      // git clone also failed — init locally
    }

    console.error('Warning: 无法克隆远程仓库，创建本地 git 仓库');
    fs.mkdirSync(targetDir, { recursive: true });
    try {
      execFileSync('git', ['init'], { cwd: targetDir, stdio: 'ignore' });
    } catch {
      console.error('Error: git init 失败');
      process.exit(1);
    }

    // Add remote for future push
    const remoteUrl = options.token
      ? `https://x-access-token:${options.token}@github.com/${org}/${repoName}.git`
      : `git@github.com:${org}/${repoName}.git`;
    try {
      execFileSync('git', ['remote', 'add', 'origin', remoteUrl], { cwd: targetDir, stdio: 'ignore' });
    } catch {
      // remote might already exist
    }

    return targetDir;
  }
```

- [ ] **Step 4: Update help text in index.ts**

In `legacy/cli/src/index.ts`, update lines 39-40:

```typescript
  .option('--handler <handler>', 'Handler（git 必填；github 可选，配合 --display-name 替代 --token）')
  .option('--display-name <name>', '显示名称（git 必填；github 可选，配合 --handler 替代 --token）')
```

- [ ] **Step 5: Build to verify**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/shared-github-auth/legacy/cli && npm run build`

Expected: Compiles without errors. (If no build script, `npx tsc --noEmit` to type-check.)

- [ ] **Step 6: Commit**

```bash
git add legacy/cli/src/commands/onboard.ts legacy/cli/src/index.ts
git commit -m "feat(legacy-cli): allow handler+display-name for github mode with fallback"
```

---

### Task 6: Integration smoke test

No automated test infrastructure for CLI commands. Manual verification:

- [ ] **Step 1: Verify all Rust tests pass**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/shared-github-auth && cargo test 2>&1 | grep -E "^test result:" | tail -5`

Expected: All `ok`, no failures.

- [ ] **Step 2: Verify Rust CLI builds and shows updated help**

Run: `cargo run -p gitim-cli -- onboard --help`

Expected: `--handler` and `--display-name` help text reflects github mode support.

- [ ] **Step 3: Verify build for legacy CLI**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/shared-github-auth/legacy/cli && npm run build 2>&1 || npx tsc --noEmit 2>&1`

Expected: No type errors.

- [ ] **Step 4: Final commit (if any remaining changes)**

Only if previous steps revealed issues that needed fixing.
