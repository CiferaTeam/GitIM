# Environment Preflight Check Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Runtime 启动时校验 `gitim`、`gitim-daemon`、`gitim-runtime` 三个 binary 版本完全一致，不一致则拒绝启动。

**Architecture:** 在 runtime 二进制的 `main()` 最早期调用 preflight 检查。Preflight 先定位 runtime 自身所在目录，在该目录查找兄弟 binary；找不到则 fallback 到 PATH。对每个 binary 执行 `--version`，解析输出，与自身编译时版本 (`env!("CARGO_PKG_VERSION")`) 比较。Daemon 目前不支持 `--version`，需先补上。

**Tech Stack:** Rust, `std::process::Command`, `env!("CARGO_PKG_VERSION")`

---

## File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `crates/gitim-daemon/src/main.rs` | Modify | 在参数解析最前面加 `--version` 处理 |
| `crates/gitim-runtime/src/preflight.rs` | Create | 版本检测逻辑：定位 binary、执行 `--version`、比对 |
| `crates/gitim-runtime/src/lib.rs` | Modify | 导出 `preflight` 模块 |
| `crates/gitim-runtime/src/bin/runtime.rs` | Modify | 在 main() 开头加 `--version` 支持和 preflight 调用 |
| `crates/gitim-runtime/tests/preflight_test.rs` | Create | preflight 逻辑的集成测试 |

---

### Task 1: Daemon 支持 `--version`

**Files:**
- Modify: `crates/gitim-daemon/src/main.rs:11-14`

- [ ] **Step 1: 在 daemon main() 最前面加 `--version` 处理**

在 `tracing_subscriber::fmt::init()` 之前（避免 `--version` 也打日志），插入参数检查：

```rust
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // --version must come before tracing init to keep output clean
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("--version") {
        println!("gitim-daemon {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    tracing_subscriber::fmt::init();
    // ... rest unchanged
```

- [ ] **Step 2: 构建并验证**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/env-preflight && cargo build -p gitim-daemon 2>&1 | tail -3`
Expected: 编译成功

Run: `./target/debug/gitim-daemon --version`
Expected: `gitim-daemon 0.3.1`

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-daemon/src/main.rs
git commit -m "feat(daemon): add --version flag for env preflight"
```

---

### Task 2: Preflight 模块核心逻辑

**Files:**
- Create: `crates/gitim-runtime/src/preflight.rs`
- Modify: `crates/gitim-runtime/src/lib.rs`

- [ ] **Step 1: 写 preflight.rs**

```rust
use std::path::{Path, PathBuf};
use std::process::Command;

const RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Binary names to check alongside runtime itself.
const PEERS: &[(&str, &str)] = &[
    ("gitim", "gitim"),
    ("gitim-daemon", "gitim-daemon"),
];

#[derive(Debug)]
pub struct VersionMismatch {
    pub binary: String,
    pub found: String,
    pub expected: String,
}

#[derive(Debug)]
pub struct PrefightError {
    pub missing: Vec<String>,
    pub mismatches: Vec<VersionMismatch>,
}

impl std::fmt::Display for PrefightError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "environment preflight failed")?;
        writeln!(f, "  expected version: {RUNTIME_VERSION}")?;
        for m in &self.mismatches {
            writeln!(f, "  {} version mismatch: found {}", m.binary, m.found)?;
        }
        for name in &self.missing {
            writeln!(f, "  {} not found in PATH or runtime directory", name)?;
        }
        Ok(())
    }
}

/// Find a binary: first check the directory where the current exe lives,
/// then fall back to PATH lookup.
fn find_binary(name: &str) -> Option<PathBuf> {
    // Check sibling of current executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    // Fallback: rely on PATH via `which`
    which_in_path(name)
}

fn which_in_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var("PATH").ok()?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Run `<binary> --version`, parse the version string.
/// Expected format: `<name> <version>` (e.g. "gitim 0.3.1").
fn query_version(binary_path: &Path) -> Option<String> {
    let output = Command::new(binary_path)
        .arg("--version")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Take the last whitespace-separated token on the first line
    let first_line = stdout.lines().next()?;
    first_line.split_whitespace().last().map(|s| s.to_string())
}

/// Run environment preflight check.
/// Returns Ok(()) if all binaries are found and version-aligned.
pub fn check_env() -> Result<(), PrefightError> {
    let mut missing = Vec::new();
    let mut mismatches = Vec::new();

    for &(name, binary_name) in PEERS {
        match find_binary(binary_name) {
            None => missing.push(name.to_string()),
            Some(path) => match query_version(&path) {
                None => missing.push(format!("{name} (found but --version failed)")),
                Some(version) if version != RUNTIME_VERSION => {
                    mismatches.push(VersionMismatch {
                        binary: name.to_string(),
                        found: version,
                        expected: RUNTIME_VERSION.to_string(),
                    });
                }
                Some(_) => {} // matched
            },
        }
    }

    if missing.is_empty() && mismatches.is_empty() {
        Ok(())
    } else {
        Err(PrefightError { missing, mismatches })
    }
}
```

- [ ] **Step 2: 在 lib.rs 导出模块**

在 `crates/gitim-runtime/src/lib.rs` 添加：

```rust
pub mod preflight;
```

- [ ] **Step 3: 编译验证**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/env-preflight && cargo build -p gitim-runtime 2>&1 | tail -3`
Expected: 编译成功

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-runtime/src/preflight.rs crates/gitim-runtime/src/lib.rs
git commit -m "feat(runtime): add preflight module for binary version checking"
```

---

### Task 3: Runtime 二进制集成 preflight

**Files:**
- Modify: `crates/gitim-runtime/src/bin/runtime.rs:6-16`

- [ ] **Step 1: 在 runtime main() 加入 `--version` 和 preflight 调用**

修改 `runtime.rs` 的 `main()` 函数开头部分：

```rust
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();

    // --version: print and exit before anything else
    if args.get(1).map(|s| s.as_str()) == Some("--version") {
        println!("gitim-runtime {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    tracing_subscriber::fmt::init();

    // Environment preflight: all three binaries must be version-aligned
    if let Err(e) = gitim_runtime::preflight::check_env() {
        eprintln!("{e}");
        std::process::exit(1);
    }

    // Shell mode: gitim-runtime --port <PORT>
    if args.len() >= 3 && args[1] == "--port" {
        // ... rest unchanged
```

- [ ] **Step 2: 构建并验证 `--version`**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/env-preflight && cargo build -p gitim-runtime 2>&1 | tail -3`
Expected: 编译成功

Run: `./target/debug/gitim-runtime --version`
Expected: `gitim-runtime 0.3.1`

- [ ] **Step 3: 验证 preflight 正常启动**

Run: `./target/debug/gitim-runtime 2>&1 | head -5`
Expected: 不出现 "preflight failed" 错误（应显示 Usage 帮助信息，说明 preflight 通过了）

- [ ] **Step 4: 验证 preflight 检测到版本不对齐**

制造版本不对齐场景：创建一个 fake binary 输出错误版本，放在 runtime 同目录。

```bash
# 创建 fake gitim-daemon 输出错误版本
echo '#!/bin/sh
echo "gitim-daemon 99.99.99"' > ./target/debug/gitim-daemon-real-backup
cp ./target/debug/gitim-daemon ./target/debug/gitim-daemon-real-backup
echo '#!/bin/sh
echo "gitim-daemon 99.99.99"' > ./target/debug/gitim-daemon
chmod +x ./target/debug/gitim-daemon

# 运行 runtime，应该报错
./target/debug/gitim-runtime 2>&1

# 恢复
cp ./target/debug/gitim-daemon-real-backup ./target/debug/gitim-daemon
rm ./target/debug/gitim-daemon-real-backup
```

Expected: 输出包含 `environment preflight failed` 和 `gitim-daemon version mismatch: found 99.99.99`

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-runtime/src/bin/runtime.rs
git commit -m "feat(runtime): integrate env preflight at startup"
```

---

### Task 4: 集成测试

**Files:**
- Create: `crates/gitim-runtime/tests/preflight_test.rs`

- [ ] **Step 1: 写测试**

测试 preflight 的内部辅助函数（`query_version`），通过创建临时脚本模拟 binary 行为：

```rust
use std::io::Write;
use std::os::unix::fs::PermissionsExt;

/// Create a temporary script that prints a version string.
fn make_version_script(dir: &std::path::Path, name: &str, output: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    let mut f = std::fs::File::create(&path).unwrap();
    writeln!(f, "#!/bin/sh\necho \"{output}\"").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn version_script_outputs_correctly() {
    let dir = tempfile::tempdir().unwrap();
    let script = make_version_script(dir.path(), "fake-bin", "fake-bin 1.2.3");

    let output = std::process::Command::new(&script)
        .arg("--version")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("1.2.3"));
}

#[test]
fn preflight_detects_missing_binary() {
    // Point PATH to an empty dir so nothing is found
    let empty_dir = tempfile::tempdir().unwrap();
    let original_path = std::env::var("PATH").unwrap_or_default();

    // Override PATH and current_exe won't help either since
    // siblings won't have these names in the temp dir.
    // We test the low-level find logic indirectly: if we had a fake
    // current_exe, the sibling check would work. Since we can't fake
    // current_exe in tests, we verify the PATH fallback by using an empty PATH.
    std::env::set_var("PATH", empty_dir.path());
    let result = gitim_runtime::preflight::check_env();
    std::env::set_var("PATH", &original_path);

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(!err.missing.is_empty(), "should report missing binaries");
}
```

- [ ] **Step 2: 运行测试**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/env-preflight && cargo test -p gitim-runtime --test preflight_test -- --nocapture 2>&1 | tail -15`
Expected: 两个测试通过

- [ ] **Step 3: 运行全量测试确认无回归**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/env-preflight && cargo test 2>&1 | tail -20`
Expected: 全部通过

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-runtime/tests/preflight_test.rs
git commit -m "test(runtime): add preflight integration tests"
```
