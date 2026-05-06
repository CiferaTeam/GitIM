# Cross-compile Release Pipeline — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 GitIM 后端三个 Rust binary (`gitim` / `gitim-daemon` / `gitim-runtime`) 的本地 release pipeline 扩展到 4 target (macOS arm64/x86_64 + Linux arm64/x86_64 musl),并加入 SHA256 完整性校验到 install / self-update 全链路。

**Architecture:** 增量扩展既有 `release.sh` / `install.sh` / `gitim-updater`。`cross-rs/cross` 走 Docker 编 Linux 两 target;本机 Apple Silicon + rustup target add 编 macOS 两 target;`gitim-updater` 的 `download_and_extract` 拆成 3 单职责函数 + `install_update` 编排,强制 SHA256SUMS 校验。Archive 命名契约冻结 (`gitim-v{tag}-{darwin,linux}-{arm64,x86_64}.tar.gz`),与 `detect_platform()` 已有输出对齐。Fail-fast 4 target build,任一失败整体中止。

**Tech Stack:** Bash 4+ / Rust 2021 / `cross-rs/cross` + Docker / `sha2` crate / `wiremock` / `tar` 0.4 / `gh` CLI / `shasum -a 256`

**Background docs:**
- 需求 + eng-review 决策: `docs/plans/cross-compile-release/00-requirements.md`
- CLAUDE.md: 项目整体架构
- TODOS.md: 已 defer 的 L2 signing / shellcheck

---

## File Structure

### Created

| 路径 | 职责 |
|---|---|
| `Cross.toml` | Pin cross Docker image tag,保 build reproducibility |

### Modified

| 路径 | 变更 |
|---|---|
| `release.sh` | 重写 build 部分: host-only → 4 target matrix + SHA256SUMS + `--target <slug>` + Docker smoke test + fail-fast |
| `install.sh` | `SUPPORTED_PLATFORMS` 扩到 4;加 SHA256SUMS fetch + verify |
| `crates/gitim-updater/Cargo.toml` | 加 `sha2` 依赖 |
| `crates/gitim-updater/src/lib.rs` | 拆 `download_and_extract` 为 3 单职责函数 + 新 `install_update` 编排 |
| `crates/gitim-updater/tests/download_replace.rs` | 重写覆盖新 API,加 SHA 校验的 regression guard |
| `crates/gitim-daemon/Cargo.toml` | dev-dep `reqwest` 加 `default-features = false` 清除 native-tls 泄漏 |
| `crates/gitim-cli/src/commands/update.rs` | Callsite update: `download_and_extract` → `install_update` |
| `crates/gitim-runtime/src/update.rs` | Callsite update + error shape 加 `sha_mismatch` error_code |
| `webui-v2/src/components/update-indicator.tsx` | 展示 `sha_mismatch` / `sha_file_missing` 等结构化错误 |

---

## Task ordering rationale

12 个 task 按 **"最小依赖链 + TDD + 频繁提交"** 排序:

1. Pre-work cleanup (T1-T3): 修泄漏 + 加依赖 + 建 Cross.toml — 独立小改,先落
2. `gitim-updater` 拆函数 + SHA 逻辑 (T4-T7): 纯 Rust TDD,每一步单元测试保护
3. Callsite 切换 + 老 API 删除 (T8): 完成 refactor 清尾
4. Runtime error contract 升级 (T9): critical gap,让上游能消费结构化错误
5. WebUI 展示 (T10): critical gap,前端闭环
6. Release pipeline 脚本重写 (T11-T12): 最后做,因为它的 SHA256SUMS 契约依赖前面的 updater 逻辑就位

---

### Task 1: 修 `gitim-daemon` dev-dep `reqwest` 的 native-tls 泄漏

**Files:**
- Modify: `crates/gitim-daemon/Cargo.toml` (dev-dependencies 段的 `reqwest` 行)

**Why:** 当前 `reqwest = { version = "0.12", features = ["json"] }` 没禁 default features,会激活 `native-tls` → 传递引入 `openssl-sys`。Cargo.lock 里的 `openssl-sys` 就是这条路径拉进来的。Release 构建虽然不受影响,但将来任何在 daemon dev scope 下做 cross-compile 会撞 openssl-sys 的 sysroot 地雷。

- [ ] **Step 1: 读取当前 reqwest 行**

```bash
grep -n 'reqwest' crates/gitim-daemon/Cargo.toml
```

Expected: `reqwest = { version = "0.12", features = ["json"] }` (dev-dependencies 下)

- [ ] **Step 2: 改为禁 default features + 显式启用 rustls-tls**

文件 `crates/gitim-daemon/Cargo.toml`,替换 dev-dependencies 下的 `reqwest` 行为:

```toml
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
```

- [ ] **Step 3: 验证 native-tls / openssl-sys 从 Cargo.lock 消失**

```bash
cargo update -p reqwest
grep -E '^name = "(openssl|openssl-sys|native-tls)"' Cargo.lock | sort -u
```

Expected: 空输出 (这三个 crate 都从 lockfile 移除)

- [ ] **Step 4: 跑 daemon test 确保没回归**

```bash
cargo test -p gitim-daemon --no-run
```

Expected: 编译通过 (不需跑 test,确认编译 OK 即可)

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-daemon/Cargo.toml Cargo.lock
git commit -m "fix(daemon): scope dev-dep reqwest to rustls-tls to drop openssl-sys leak"
```

---

### Task 2: 加 `sha2` 依赖到 `gitim-updater`

**Files:**
- Modify: `crates/gitim-updater/Cargo.toml` (dependencies 段)

- [ ] **Step 1: 在 `[dependencies]` 末尾加一行**

文件 `crates/gitim-updater/Cargo.toml`,在 `tracing.workspace = true` 行之后加:

```toml
sha2 = "0.10"
hex = "0.4"
```

(`hex` 用于把 `[u8; 32]` 编码成 lowercase hex string 方便比对)

- [ ] **Step 2: 验证依赖解析通过**

```bash
cargo check -p gitim-updater
```

Expected: 编译通过,无 warning

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-updater/Cargo.toml Cargo.lock
git commit -m "chore(updater): add sha2 + hex for SHA256 artifact verification"
```

---

### Task 3: 创建 `Cross.toml` pin image tag

**Files:**
- Create: `Cross.toml` (workspace 根)

**Why:** `cross` 默认拉 `ghcr.io/cross-rs/<target>:main` 会随上游漂移。Pin 到具体 release tag 保 release 可重复。选 `0.2.5` (2025 年稳定版,兼容 rustup 2024+ 工具链)。

- [ ] **Step 1: 新建 `Cross.toml`**

内容:

```toml
# Pinned cross-rs/cross Docker images. Updating these is a manual decision —
# don't follow `main`, because it breaks reproducibility of releases we shipped
# against a specific sysroot. Bump with intent, re-run full release, verify
# binaries still work on the oldest target distros we claim to support.

[target.x86_64-unknown-linux-musl]
image = "ghcr.io/cross-rs/x86_64-unknown-linux-musl:0.2.5"

[target.aarch64-unknown-linux-musl]
image = "ghcr.io/cross-rs/aarch64-unknown-linux-musl:0.2.5"
```

- [ ] **Step 2: 验证 cross 认识该配置**

```bash
# 需要本机已 cargo install cross,Docker 已开启
cross --version
cross build --target x86_64-unknown-linux-musl -p gitim-cli --release 2>&1 | head -20
```

Expected: Docker pull 看到 `ghcr.io/cross-rs/x86_64-unknown-linux-musl:0.2.5` tag;编译开始。**不要求这一步跑完 build** (x86_64 emulation 慢,只要看到开始拉 image 即可 Ctrl-C)。

- [ ] **Step 3: Commit**

```bash
git add Cross.toml
git commit -m "build: pin cross-rs Docker image tags to 0.2.5"
```

---

### Task 4: 加 `verify_sha256` 纯函数 (TDD)

**Files:**
- Modify: `crates/gitim-updater/src/lib.rs` (在 `detect_platform` 附近加新函数)
- Modify: `crates/gitim-updater/tests/download_replace.rs` (加 unit test)

**Why:** 纯函数最先上,TDD 最友好,后面 `install_update` 编排依赖它。

- [ ] **Step 1: 加 UpdateError 新变体**

文件 `crates/gitim-updater/src/lib.rs`,在 `enum UpdateError { ... }` 里加:

```rust
    #[error("sha256 mismatch: expected {expected}, actual {actual}")]
    Sha256Mismatch { expected: String, actual: String },

    #[error("sha256 line not found in SHA256SUMS for {0}")]
    Sha256LineMissing(String),
```

放在 `MissingBinary` 之后、`Io` 之前。

- [ ] **Step 2: 写失败测试 (before 实现)**

文件 `crates/gitim-updater/tests/download_replace.rs`,在文件末尾加:

```rust
// ---------- SHA256 verify ----------

#[test]
fn verify_sha256_matches_expected() {
    use gitim_updater::verify_sha256;
    // SHA256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
    let bytes = b"hello";
    let expected = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
    verify_sha256(bytes, expected).expect("matching SHA must pass");
}

#[test]
fn verify_sha256_rejects_mismatch() {
    use gitim_updater::{UpdateError, verify_sha256};
    let bytes = b"hello";
    let wrong = "0000000000000000000000000000000000000000000000000000000000000000";
    let err = verify_sha256(bytes, wrong).expect_err("wrong SHA must fail");
    match err {
        UpdateError::Sha256Mismatch { expected, actual } => {
            assert_eq!(expected, wrong);
            assert_eq!(actual.len(), 64);
            assert_ne!(actual, wrong);
        }
        other => panic!("expected Sha256Mismatch, got {:?}", other),
    }
}

#[test]
fn verify_sha256_case_insensitive() {
    use gitim_updater::verify_sha256;
    let bytes = b"hello";
    let upper = "2CF24DBA5FB0A30E26E83B2AC5B9E29E1B161E5C1FA7425E73043362938B9824";
    verify_sha256(bytes, upper).expect("uppercase hex must pass");
}

#[test]
fn verify_sha256_rejects_malformed_hex() {
    use gitim_updater::verify_sha256;
    let bytes = b"hello";
    // 63 chars (奇数 / 短)
    let malformed = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b982";
    assert!(verify_sha256(bytes, malformed).is_err());
}
```

- [ ] **Step 3: 跑测试验证失败**

```bash
cargo test -p gitim-updater verify_sha256 2>&1 | head -30
```

Expected: 编译失败 `cannot find function `verify_sha256` in crate `gitim_updater``

- [ ] **Step 4: 实现 `verify_sha256`**

文件 `crates/gitim-updater/src/lib.rs`,在 `fn parse_version` 后 (但在 `detect_platform` 前) 加:

```rust
/// Verify that `bytes` hash to `expected_hex` under SHA-256.
///
/// `expected_hex` is a 64-char lowercase hex string (uppercase tolerated) —
/// the canonical SHA-256 output format from `shasum -a 256` / `sha256sum`.
/// Anything shorter, longer, or non-hex is rejected as
/// `UpdateError::Sha256Mismatch` (fail closed — we never silently accept a
/// malformed expectation).
pub fn verify_sha256(bytes: &[u8], expected_hex: &str) -> Result<(), UpdateError> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let actual_bytes = hasher.finalize();
    let actual_hex = hex::encode(actual_bytes);

    // Case-insensitive compare — sha256sum on BSD/mac emits lowercase, GNU
    // coreutils also lowercase, but older shasum(1) on macOS emits uppercase
    // for some flags. Normalize both.
    let expected_norm = expected_hex.trim().to_lowercase();

    // Length guard: SHA-256 is always 32 bytes = 64 hex chars. Anything else
    // is malformed upstream data — treat as mismatch rather than a separate
    // error variant (callers only care "verify failed").
    if expected_norm.len() != 64 || !expected_norm.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(UpdateError::Sha256Mismatch {
            expected: expected_hex.to_string(),
            actual: actual_hex,
        });
    }

    if expected_norm != actual_hex {
        return Err(UpdateError::Sha256Mismatch {
            expected: expected_norm,
            actual: actual_hex,
        });
    }
    Ok(())
}
```

- [ ] **Step 5: 跑测试验证通过**

```bash
cargo test -p gitim-updater verify_sha256
```

Expected: 4 tests passed

- [ ] **Step 6: Commit**

```bash
git add crates/gitim-updater/src/lib.rs crates/gitim-updater/tests/download_replace.rs
git commit -m "feat(updater): add verify_sha256 pure function"
```

---

### Task 5: 从 `download_and_extract` 拆出 `download_bytes`

**Files:**
- Modify: `crates/gitim-updater/src/lib.rs` (拆 `download_and_extract:172-194` 的网络部分)
- Modify: `crates/gitim-updater/tests/download_replace.rs` (加 wiremock test)

**Why:** SRP — 网络 IO 和 tar 解包是两件事,拆开单测更容易 mock。旧 `download_and_extract` 暂时保留 (等 Task 8 再删),避免这一步破坏现有 callsite。

- [ ] **Step 1: 写失败测试**

文件 `crates/gitim-updater/tests/download_replace.rs`,在 `verify_sha256_*` 之后加:

```rust
// ---------- download_bytes ----------

#[tokio::test]
async fn download_bytes_happy_path() {
    use gitim_updater::download_bytes;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/blob.bin"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"payload-bytes".as_slice()))
        .mount(&server)
        .await;

    let url = format!("{}/blob.bin", server.uri());
    let bytes = download_bytes(&url).await.expect("download must succeed");
    assert_eq!(bytes.as_slice(), b"payload-bytes");
}

#[tokio::test]
async fn download_bytes_http_404() {
    use gitim_updater::{UpdateError, download_bytes};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/missing"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let url = format!("{}/missing", server.uri());
    match download_bytes(&url).await.expect_err("must fail") {
        UpdateError::HttpStatus(404) => (),
        other => panic!("expected HttpStatus(404), got {:?}", other),
    }
}

#[tokio::test]
async fn download_bytes_http_500() {
    use gitim_updater::{UpdateError, download_bytes};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/boom"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let url = format!("{}/boom", server.uri());
    match download_bytes(&url).await.expect_err("must fail") {
        UpdateError::HttpStatus(500) => (),
        other => panic!("expected HttpStatus(500), got {:?}", other),
    }
}
```

- [ ] **Step 2: 跑测试验证失败**

```bash
cargo test -p gitim-updater download_bytes 2>&1 | head -20
```

Expected: 编译失败 `cannot find function `download_bytes``

- [ ] **Step 3: 实现 `download_bytes`**

文件 `crates/gitim-updater/src/lib.rs`,在 `fetch_latest_tag` 之后、`download_and_extract` 之前加:

```rust
/// Fetch the full body of `url` into memory. Used for both small SHA256SUMS
/// text files and the release tarball (10-20 MB at current binary sizes —
/// well within RAM, streaming to disk not worth the complexity).
///
/// Non-2xx -> `UpdateError::HttpStatus(code)`.
pub async fn download_bytes(url: &str) -> Result<Vec<u8>, UpdateError> {
    let client = reqwest::Client::builder().user_agent(USER_AGENT).build()?;
    let resp = client.get(url).send().await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(UpdateError::HttpStatus(status.as_u16()));
    }
    let bytes = resp.bytes().await?;
    Ok(bytes.to_vec())
}
```

- [ ] **Step 4: 跑测试验证通过**

```bash
cargo test -p gitim-updater download_bytes
```

Expected: 3 tests passed

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-updater/src/lib.rs crates/gitim-updater/tests/download_replace.rs
git commit -m "feat(updater): extract download_bytes from download_and_extract"
```

---

### Task 6: 从 `download_and_extract` 拆出 `extract_tarball`

**Files:**
- Modify: `crates/gitim-updater/src/lib.rs` (拆 `download_and_extract` 的 tar 解包部分)
- Modify: `crates/gitim-updater/tests/download_replace.rs` (加 tempfile + tar builder test)

**Why:** SRP 闭环。把 `tar::Archive::new` + `unpack` 变成 `extract_tarball(bytes, dest)` 纯 sync 函数 (无网络),测试用 in-memory tar builder 直接构造字节流。

- [ ] **Step 1: 写失败测试**

文件 `crates/gitim-updater/tests/download_replace.rs`,在 `download_bytes_*` 之后加:

```rust
// ---------- extract_tarball ----------

fn build_tar_gz(files: &[(&str, &[u8])]) -> Vec<u8> {
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use tar::{Builder, Header};

    let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
    {
        let mut tar = Builder::new(&mut gz);
        for (path, content) in files {
            let mut h = Header::new_gnu();
            h.set_path(path).unwrap();
            h.set_size(content.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            tar.append(&h, *content).unwrap();
        }
        tar.finish().unwrap();
    }
    gz.finish().unwrap()
}

#[test]
fn extract_tarball_happy_path() {
    use gitim_updater::extract_tarball;
    let bytes = build_tar_gz(&[
        ("gitim-v9.9.9-darwin-arm64/gitim", b"fake-bin-1"),
        ("gitim-v9.9.9-darwin-arm64/gitim-daemon", b"fake-bin-2"),
    ]);
    let dest = tempfile::tempdir().unwrap();
    extract_tarball(&bytes, dest.path()).expect("extract must succeed");
    let entry = dest.path().join("gitim-v9.9.9-darwin-arm64/gitim");
    assert!(entry.exists(), "extracted file must exist");
    assert_eq!(std::fs::read(&entry).unwrap(), b"fake-bin-1");
}

#[test]
fn extract_tarball_rejects_corrupt_bytes() {
    use gitim_updater::{UpdateError, extract_tarball};
    let garbage = vec![0xFFu8; 1024];
    let dest = tempfile::tempdir().unwrap();
    match extract_tarball(&garbage, dest.path()).expect_err("garbage must fail") {
        UpdateError::Extract(_) => (),
        other => panic!("expected Extract, got {:?}", other),
    }
}
```

- [ ] **Step 2: 跑测试验证失败**

```bash
cargo test -p gitim-updater extract_tarball 2>&1 | head -20
```

Expected: 编译失败 `cannot find function `extract_tarball``

- [ ] **Step 3: 实现 `extract_tarball`**

文件 `crates/gitim-updater/src/lib.rs`,在 `download_bytes` 之后、`download_and_extract` 之前加:

```rust
/// Extract a gzipped-tar byte slice into `dest` on disk.
///
/// Pure sync; no network. The `tar` 0.4 default `Archive::unpack` rejects
/// absolute paths and `..` traversal — we rely on that for defense in depth.
/// Do not call `archive.set_preserve_permissions(true)` or relax the path
/// checks without re-evaluating the trust model.
pub fn extract_tarball(bytes: &[u8], dest: &Path) -> Result<(), UpdateError> {
    let decoder = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(decoder);
    archive
        .unpack(dest)
        .map_err(|e| UpdateError::Extract(format!("tar unpack failed: {e}")))?;
    Ok(())
}
```

- [ ] **Step 4: 跑测试验证通过**

```bash
cargo test -p gitim-updater extract_tarball
```

Expected: 2 tests passed

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-updater/src/lib.rs crates/gitim-updater/tests/download_replace.rs
git commit -m "feat(updater): extract extract_tarball from download_and_extract"
```

---

### Task 7: 加 `install_update` 编排函数 (含 SHA 校验 regression guard)

**Files:**
- Modify: `crates/gitim-updater/src/lib.rs` (加 `install_update` + `parse_sha256sums_line` helper)
- Modify: `crates/gitim-updater/tests/download_replace.rs` (加 wiremock 双 endpoint test)

**Why:** 这是新 API 的核心。`install_update(tag, platform, dest)` 串联 `download_bytes` (两次) + `parse_sha256sums_line` + `verify_sha256` + `extract_tarball`。SHA mismatch 必须 `fail-closed`,不 extract,不污染 dest dir。

- [ ] **Step 1: 写失败测试 (含 regression guard)**

文件 `crates/gitim-updater/tests/download_replace.rs`,在 `extract_tarball_*` 之后加:

```rust
// ---------- install_update (orchestration) ----------

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

fn archive_name(tag: &str, platform: &str) -> String {
    format!("gitim-{tag}-{platform}.tar.gz")
}

fn sha256sums_body(entries: &[(&str, &str)]) -> String {
    // `shasum -a 256` format: "<hex>  <filename>\n"
    let mut s = String::new();
    for (hex_sum, name) in entries {
        s.push_str(hex_sum);
        s.push_str("  ");
        s.push_str(name);
        s.push('\n');
    }
    s
}

#[tokio::test]
async fn install_update_happy_path() {
    use gitim_updater::install_update;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let tag = "v9.9.9";
    let platform = "darwin-arm64";
    let tarball = build_tar_gz(&[(
        &format!("gitim-{tag}-{platform}/gitim"),
        b"binary-contents",
    )]);
    let expected_hex = sha256_hex(&tarball);
    let sha_body = sha256sums_body(&[(&expected_hex, &archive_name(tag, platform))]);

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/releases/download/{tag}/SHA256SUMS")))
        .respond_with(ResponseTemplate::new(200).set_body_string(sha_body))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/releases/download/{tag}/{}",
            archive_name(tag, platform)
        )))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(tarball))
        .mount(&server)
        .await;

    let dest = tempfile::tempdir().unwrap();
    let base = format!("{}/releases/download", server.uri());
    install_update(&base, tag, platform, dest.path())
        .await
        .expect("happy path must succeed");
    let entry = dest.path().join(format!("gitim-{tag}-{platform}/gitim"));
    assert!(entry.exists(), "extracted binary must be on disk");
}

#[tokio::test]
async fn install_update_sha_mismatch_does_not_extract() {
    // REGRESSION GUARD: a poisoned tarball with a valid SHA file must NEVER
    // be extracted. This is the core self-update attack surface.
    use gitim_updater::{UpdateError, install_update};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let tag = "v9.9.9";
    let platform = "darwin-arm64";
    let real_tarball = build_tar_gz(&[(
        &format!("gitim-{tag}-{platform}/gitim"),
        b"original-bytes",
    )]);
    let poisoned_tarball = build_tar_gz(&[(
        &format!("gitim-{tag}-{platform}/gitim"),
        b"MALICIOUS_PAYLOAD",
    )]);
    // SHA references the original; server serves the poisoned tarball.
    let real_hex = sha256_hex(&real_tarball);
    let sha_body = sha256sums_body(&[(&real_hex, &archive_name(tag, platform))]);

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/releases/download/{tag}/SHA256SUMS")))
        .respond_with(ResponseTemplate::new(200).set_body_string(sha_body))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/releases/download/{tag}/{}",
            archive_name(tag, platform)
        )))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(poisoned_tarball))
        .mount(&server)
        .await;

    let dest = tempfile::tempdir().unwrap();
    let base = format!("{}/releases/download", server.uri());
    let err = install_update(&base, tag, platform, dest.path())
        .await
        .expect_err("poisoned tarball must be rejected");
    match err {
        UpdateError::Sha256Mismatch { .. } => (),
        other => panic!("expected Sha256Mismatch, got {:?}", other),
    }
    // fail-closed: destination must be empty (no extracted files).
    let entries: Vec<_> = std::fs::read_dir(dest.path())
        .unwrap()
        .flatten()
        .collect();
    assert!(
        entries.is_empty(),
        "destination must stay empty on SHA mismatch, got {entries:?}"
    );
}

#[tokio::test]
async fn install_update_sha_file_missing_fails_closed() {
    use gitim_updater::{UpdateError, install_update};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let tag = "v9.9.9";
    let platform = "darwin-arm64";

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/releases/download/{tag}/SHA256SUMS")))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let dest = tempfile::tempdir().unwrap();
    let base = format!("{}/releases/download", server.uri());
    match install_update(&base, tag, platform, dest.path())
        .await
        .expect_err("missing SHA file must fail")
    {
        UpdateError::HttpStatus(404) => (),
        other => panic!("expected HttpStatus(404), got {:?}", other),
    }
}

#[tokio::test]
async fn install_update_sha_line_missing_fails_closed() {
    use gitim_updater::{UpdateError, install_update};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let tag = "v9.9.9";
    let platform = "darwin-arm64";
    // SHA file exists but lists a DIFFERENT platform only.
    let other_hex = sha256_hex(b"other");
    let sha_body =
        sha256sums_body(&[(&other_hex, &archive_name(tag, "linux-x86_64"))]);

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/releases/download/{tag}/SHA256SUMS")))
        .respond_with(ResponseTemplate::new(200).set_body_string(sha_body))
        .mount(&server)
        .await;

    let dest = tempfile::tempdir().unwrap();
    let base = format!("{}/releases/download", server.uri());
    match install_update(&base, tag, platform, dest.path())
        .await
        .expect_err("missing SHA line must fail")
    {
        UpdateError::Sha256LineMissing(name) => {
            assert_eq!(name, archive_name(tag, platform));
        }
        other => panic!("expected Sha256LineMissing, got {:?}", other),
    }
}
```

- [ ] **Step 2: 跑测试验证失败**

```bash
cargo test -p gitim-updater install_update 2>&1 | head -30
```

Expected: 编译失败 `cannot find function `install_update``

- [ ] **Step 3: 实现 `parse_sha256sums_line` + `install_update`**

文件 `crates/gitim-updater/src/lib.rs`,在 `extract_tarball` 之后加:

```rust
/// Parse one line out of a `SHA256SUMS` file, matching by exact trailing
/// filename. Lines follow GNU/BSD format: `<64 hex>  <filename>\n` (two spaces
/// for binary mode, one space + `*` for text mode — we accept both).
///
/// Returns `Sha256LineMissing(archive_name)` if `SHA256SUMS` has no matching
/// line. Malformed lines are skipped silently — one bad line shouldn't poison
/// the rest of the file, and we fail-close on "no match" anyway.
pub fn parse_sha256sums_line(body: &str, archive_name: &str) -> Result<String, UpdateError> {
    for line in body.lines() {
        // Split on first whitespace run. First token = hex, rest = name.
        let mut parts = line.splitn(2, char::is_whitespace);
        let Some(hex_tok) = parts.next() else { continue };
        let Some(rest) = parts.next() else { continue };
        // BSD/GNU text mode prefixes filename with `*`; strip.
        let name = rest.trim_start().trim_start_matches('*').trim();
        if name == archive_name {
            return Ok(hex_tok.trim().to_string());
        }
    }
    Err(UpdateError::Sha256LineMissing(archive_name.to_string()))
}

/// Orchestrate a self-update:
///   1. Fetch SHA256SUMS
///   2. Parse expected hash for `gitim-{tag}-{platform}.tar.gz`
///   3. Download the tarball
///   4. Verify SHA — on mismatch, fail-closed and do NOT extract
///   5. Extract into `dest`
///
/// `base_download_url` is the URL prefix up to (not including) the tag-scoped
/// path segment. In production: `https://github.com/<repo>/releases/download`.
/// The function appends `/<tag>/SHA256SUMS` and `/<tag>/gitim-...tar.gz`.
pub async fn install_update(
    base_download_url: &str,
    tag: &str,
    platform: &str,
    dest: &Path,
) -> Result<(), UpdateError> {
    let archive = format!("gitim-{tag}-{platform}.tar.gz");
    let sha_url = format!("{base_download_url}/{tag}/SHA256SUMS");
    let tarball_url = format!("{base_download_url}/{tag}/{archive}");

    let sha_body_bytes = download_bytes(&sha_url).await?;
    let sha_body = String::from_utf8(sha_body_bytes)
        .map_err(|e| UpdateError::Extract(format!("SHA256SUMS not UTF-8: {e}")))?;
    let expected_hex = parse_sha256sums_line(&sha_body, &archive)?;

    let tarball_bytes = download_bytes(&tarball_url).await?;
    verify_sha256(&tarball_bytes, &expected_hex)?;
    extract_tarball(&tarball_bytes, dest)?;
    Ok(())
}
```

- [ ] **Step 4: 跑测试验证通过**

```bash
cargo test -p gitim-updater install_update
```

Expected: 4 tests passed (happy / sha_mismatch / sha_file_missing / sha_line_missing)

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-updater/src/lib.rs crates/gitim-updater/tests/download_replace.rs
git commit -m "feat(updater): add install_update with SHA256 fail-closed verification"
```

---

### Task 8: 删除旧 `download_and_extract` + 切换 callsite

**Files:**
- Modify: `crates/gitim-updater/src/lib.rs` (删除 `download_and_extract:172-194`,更新 module doc)
- Modify: `crates/gitim-updater/tests/download_replace.rs` (删除 `download_and_extract_happy_path:56-100` 旧 test)
- Modify: `crates/gitim-cli/src/commands/update.rs:10-11,97` (切换到 `install_update`)
- Modify: `crates/gitim-runtime/src/update.rs:258` (切换到 `install_update`)
- Modify: `crates/gitim-runtime/tests/update_handler.rs:27-28` (更新注释引用)

**Why:** 完成 refactor 清尾。callsite 从构造 download URL + 调 `download_and_extract` 变为直接给 `(base, tag, platform, dest)`。

- [ ] **Step 1: 删除 `download_and_extract` 实现**

文件 `crates/gitim-updater/src/lib.rs`:
- 删除整个 `pub async fn download_and_extract(...)` 函数 (当前 lines 172-194,注意 `##[derive]`/doc comment 对齐)
- 更新 module doc (文件顶部 `//!` 块): `fetch_latest_tag, download_and_extract` → `fetch_latest_tag, install_update`

- [ ] **Step 2: 删除老 `download_and_extract_happy_path` 测试**

文件 `crates/gitim-updater/tests/download_replace.rs`:
- 删除整个 `async fn download_and_extract_happy_path()` 测试函数 (目前 `async fn download_and_extract_happy_path` + 下方所有 body 到下一个 `#[test]`)
- 同时删除 `use gitim_updater::{BINARIES, UpdateError, download_and_extract, replace_binaries};` 里的 `download_and_extract,` 片段

- [ ] **Step 3: 切换 `gitim-cli` callsite**

文件 `crates/gitim-cli/src/commands/update.rs`:

Line 10-11 imports:
```rust
    detect_platform, download_and_extract, download_url, fetch_latest_tag, is_newer,
    replace_binaries,
```
改为:
```rust
    detect_platform, fetch_latest_tag, install_update, is_newer, replace_binaries,
    RELEASES_REPO,
```

(删 `download_and_extract`, `download_url`;加 `install_update`, `RELEASES_REPO`)

Line ~97:
```rust
    if let Err(e) = download_and_extract(&url, tmp.path()).await {
```
改为:
```rust
    let base = format!("https://github.com/{RELEASES_REPO}/releases/download");
    if let Err(e) = install_update(&base, &tag, &platform, tmp.path()).await {
```

删掉上面构造 `url` 的那两行 (`download_url(&tag, &platform)` 那处,grep `download_url` 定位)。

- [ ] **Step 4: 切换 `gitim-runtime` callsite**

文件 `crates/gitim-runtime/src/update.rs`:

Line 258:
```rust
    gitim_updater::download_and_extract(&url, tmp.path())
```
改为:
```rust
    let base = format!("https://github.com/{}/releases/download", gitim_updater::RELEASES_REPO);
    gitim_updater::install_update(&base, &latest_tag, &platform, tmp.path())
```

同时删除该文件里构造 `url` 的行 (grep `download_url`)。

- [ ] **Step 5: 更新 `gitim-runtime` test 注释**

文件 `crates/gitim-runtime/tests/update_handler.rs:27-28`:

```
//! - `gitim-updater` has wiremock coverage of `fetch_latest_tag`,
//!   `download_and_extract`, and `HttpStatus` vs `Network` error shapes.
```
改为:
```
//! - `gitim-updater` has wiremock coverage of `fetch_latest_tag`,
//!   `install_update`, and `HttpStatus` vs `Network` error shapes.
```

- [ ] **Step 6: 全量 build 验证**

```bash
cargo build --workspace
```

Expected: 编译通过,无 `unused import` warning (我们 deny warnings)

- [ ] **Step 7: 跑所有受影响 test**

```bash
cargo test -p gitim-updater
cargo test -p gitim-cli --no-run
cargo test -p gitim-runtime --no-run
```

Expected: gitim-updater 全部通过;cli / runtime 编译通过

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "refactor(updater): drop download_and_extract in favor of install_update"
```

---

### Task 9: Runtime update 错误 shape 加 `sha_mismatch` error_code

**Files:**
- Modify: `crates/gitim-runtime/src/update.rs` (mapping `gitim_updater::UpdateError` → HTTP response 的地方)
- Modify: `crates/gitim-runtime/tests/update_handler.rs` 或相邻 (加 test)

**Why (critical gap from Phase 3):** Runtime `/runtime/update-and-restart` endpoint 要暴露 SHA mismatch / SHA file missing 作为结构化 `error_code`,让 webui 能 toast 具体错误,而不是 generic "update failed"。

- [ ] **Step 1: 定位 update.rs 里 UpdateError mapping 代码**

```bash
grep -n 'UpdateError' crates/gitim-runtime/src/update.rs | head -20
```

找到把 `gitim_updater::UpdateError` 映射为 runtime `UpdateError`(当前在 line 229 附近) 的分支。读该函数上下文(前后 30 行)确认当前 error shape 是 `{ error_code: String, detail: String }` 还是别的。

- [ ] **Step 2: 补全 error_code mapping**

在 `.map_err(|e| UpdateError { ... })` 的 closure 里,对 `gitim_updater::UpdateError::Sha256Mismatch { .. }` 和 `Sha256LineMissing(..)` 和 `HttpStatus(404)` (针对 SHA file missing 场景) 给出明确 `error_code`:

- `Sha256Mismatch { .. }` → `error_code: "sha_mismatch"`
- `Sha256LineMissing(_)` → `error_code: "sha_line_missing"`
- `HttpStatus(404)` (在 SHA fetch 上下文) → `error_code: "sha_file_missing"` 或保持现有 404 code 如果已有

具体代码(需按实际 UpdateError 结构调整 — 实现者读 struct def 后写):

```rust
Err(gitim_updater::UpdateError::Sha256Mismatch { expected, actual }) => UpdateError {
    error_code: "sha_mismatch".into(),
    detail: format!("expected sha256 {expected}, got {actual}"),
},
Err(gitim_updater::UpdateError::Sha256LineMissing(name)) => UpdateError {
    error_code: "sha_line_missing".into(),
    detail: format!("SHA256SUMS has no entry for {name}"),
},
```

- [ ] **Step 3: 加 HTTP endpoint test**

`crates/gitim-runtime/tests/update_handler.rs` 加测试: wiremock 返回 SHA mismatch 场景,断言 runtime endpoint 返回 `error_code == "sha_mismatch"`。参考该文件现有测试 pattern。

```rust
#[tokio::test]
async fn update_endpoint_reports_sha_mismatch() {
    // 用 wiremock 构造: SHA256SUMS 包含 hash A,tarball 实际 bytes 的 hash 是 B
    // 调 update endpoint,断言 response.error_code == "sha_mismatch"
    // 实现参考本文件现有 wiremock setup
    // ...
}
```

(详细 fixture 参考本文件现有 test。如果测试结构复杂,把 sha fixture 抽成 helper)

- [ ] **Step 4: 运行 test**

```bash
cargo test -p gitim-runtime update_endpoint_reports_sha_mismatch
```

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-runtime/src/update.rs crates/gitim-runtime/tests/update_handler.rs
git commit -m "feat(runtime): expose sha_mismatch / sha_line_missing in update error contract"
```

---

### Task 10: WebUI 展示结构化 SHA 错误

**Files:**
- Modify: `webui-v2/src/components/update-indicator.tsx` (错误分支处理)
- Modify: `webui-v2/src/lib/client.ts` 或对应 API caller (如果 error shape type 需要扩)

**Why (critical gap):** 上一步 runtime 已经返回 `error_code`,前端要 map 到人类可读 toast。没这一步,maintainer 点了升级失败后只看到 generic "update failed"。

- [ ] **Step 1: 定位 update-indicator 里触发升级的函数**

```bash
grep -n 'update-and-restart\|triggerUpdate\|update\s*(' webui-v2/src/components/update-indicator.tsx
```

找到点了黄 ⚠ 后 fetch `/runtime/update-and-restart` 的 handler。

- [ ] **Step 2: 读 current error handling**

阅读该 handler,确认当前失败路径是:
- 静默 console.error
- toast "update failed"
- 弹 dialog

按现有 toast 基础设施 (项目用 `sonner` 或自制) 选一致路径。

- [ ] **Step 3: 加 error_code → 人类消息 map**

```tsx
// 放在 update-indicator.tsx 顶部或抽到 lib/update-errors.ts
const UPDATE_ERROR_MESSAGES: Record<string, string> = {
  sha_mismatch: "校验失败:下载的安装包哈希与官方发布不一致,已拒绝安装。请稍后重试或联系维护者。",
  sha_line_missing: "校验失败:SHA256SUMS 未列出当前平台,疑似 release 不完整。请查看 Release 页面。",
  sha_file_missing: "校验失败:该版本未提供 SHA256SUMS(可能是 v0.6.0 之前的旧版本),请先手动升级到 v0.6.0+。",
};

function friendlyUpdateError(errorCode: string, detail: string): string {
  return UPDATE_ERROR_MESSAGES[errorCode] ?? `升级失败: ${detail || errorCode}`;
}
```

在 handler catch 分支:
```tsx
const errText = await res.text();
const parsed = tryParseJson(errText); // existing helper 或 JSON.parse 包 try
toast.error(friendlyUpdateError(parsed?.error_code, parsed?.detail || errText));
```

(关键: 不要让 toast 只显示 "failed",要把 error_code 对应的中文说明显示出来)

- [ ] **Step 4: 手动 smoke (本机跑 webui-v2 dev server)**

```bash
cd webui-v2
npm run dev
```

浏览器打开 dev URL,用 devtools Network 面板手动把 `/runtime/update-and-restart` 的响应篡改成 `{ "error_code": "sha_mismatch", "detail": "..." }` (devtools → Network → Right-click → Override headers / response),点黄 ⚠,观察 toast 是否显示中文说明。

(如果 devtools override 不方便,临时修改 handler 注入 mock error 验证一次,验完回滚)

- [ ] **Step 5: Commit**

```bash
git add webui-v2/src/components/update-indicator.tsx
# 如果抽了 lib/update-errors.ts,也 add
git commit -m "feat(webui): surface structured SHA errors in update indicator"
```

---

### Task 11: `release.sh` 重写 — 4 target build + SHA + fail-fast + `--target`

**Files:**
- Modify: `release.sh` (主体 rewrite,保留 dry-run / notes / gh auth 等)

**Why:** 核心任务。从 host-only build 变成 4 target matrix,生成 `SHA256SUMS`,fail-fast,加 `--target <slug>` 单 target 调试路径,Linux binary 产出后 docker run alpine smoke test。

- [ ] **Step 1: 备份现有 release.sh 到 staging (可选 safety)**

不需要复制文件 (git 已经管),但读一遍当前 release.sh 确认"要保留"的部分:
- version 派生 (line 14)
- tag verify (line 19-22)
- notes file (line 25-28)
- dry-run flag (line 8-11)
- gh auth (line 76-79)
- gh release upload (line 91-99)

这些保留,重写中间的 build + package + SHA。

- [ ] **Step 2: 重写 `release.sh`**

文件 `release.sh`,完整新内容:

```bash
#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
RELEASES_REPO="CiferaTeam/gitim-releases"

# ---------- Argument parsing ----------
DRY_RUN=false
ONLY_TARGET=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run) DRY_RUN=true; shift ;;
    --target)  ONLY_TARGET="$2"; shift 2 ;;
    *) echo "Usage: $0 [--dry-run] [--target <slug>]"; exit 1 ;;
  esac
done

$DRY_RUN && echo "==> DRY RUN (will not publish)"

# ---------- Read version from Cargo workspace ----------
VERSION=$(grep 'version = "' "$ROOT/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')
TAG="v${VERSION}"
echo "==> Version: $VERSION"

# ---------- Verify tag exists ----------
if ! git rev-parse "$TAG" &>/dev/null; then
  echo "Error: tag $TAG not found. Run ./bump.sh first."
  exit 1
fi

# ---------- Verify release notes ----------
NOTES_FILE="$ROOT/docs/releases/${TAG}.md"
if [ ! -f "$NOTES_FILE" ]; then
  echo "Warning: release notes not found at $NOTES_FILE"
  NOTES_FILE=""
fi

# ---------- Target matrix ----------
#
# Each entry: rust_target:slug:tool
#   rust_target  — value for `--target`
#   slug         — filename segment (must match gitim-updater::detect_platform())
#   tool         — `cargo` for native / `cross` for Docker-based cross
#
# IMPORTANT: slugs are CONTRACT with gitim-updater / install.sh. Do not change
# without updating both.
ALL_TARGETS=(
  "aarch64-apple-darwin:darwin-arm64:cargo"
  "x86_64-apple-darwin:darwin-x86_64:cargo"
  "aarch64-unknown-linux-musl:linux-arm64:cross"
  "x86_64-unknown-linux-musl:linux-x86_64:cross"
)

if [ -n "$ONLY_TARGET" ]; then
  FILTERED=()
  for t in "${ALL_TARGETS[@]}"; do
    IFS=: read -r _r slug _tool <<< "$t"
    [ "$slug" = "$ONLY_TARGET" ] && FILTERED+=("$t")
  done
  if [ ${#FILTERED[@]} -eq 0 ]; then
    echo "Error: unknown --target slug '$ONLY_TARGET'"
    echo "Valid slugs: darwin-arm64 / darwin-x86_64 / linux-arm64 / linux-x86_64"
    exit 1
  fi
  TARGETS=("${FILTERED[@]}")
  echo "==> Single-target build: $ONLY_TARGET"
else
  TARGETS=("${ALL_TARGETS[@]}")
  echo "==> Full matrix: 4 targets"
fi

# ---------- Prepare staging ----------
STAGING="$ROOT/target/release-dist"
rm -rf "$STAGING"
mkdir -p "$STAGING"

# ---------- build_target function ----------
#
# Args: rust_target, slug, tool
#
# Produces: $STAGING/gitim-${TAG}-${slug}.tar.gz
#
# For linux targets, smoke-tests the gitim binary via `docker run alpine`.
build_target() {
  local rust_target="$1" slug="$2" tool="$3"
  local archive_name="gitim-${TAG}-${slug}"
  local out_dir="$STAGING/$archive_name"
  echo ""
  echo "==> [$slug] building (tool=$tool, rust_target=$rust_target)"

  case "$tool" in
    cargo)
      # Ensure rustup target installed for macOS cross (x86_64 from arm64 host)
      rustup target add "$rust_target" >/dev/null 2>&1 || true
      cargo build --release --target "$rust_target" \
        -p gitim-cli -p gitim-daemon -p gitim-runtime
      ;;
    cross)
      # Requires Docker running + `cargo install cross`
      cross build --release --target "$rust_target" \
        -p gitim-cli -p gitim-daemon -p gitim-runtime
      ;;
    *) echo "Error: unknown build tool '$tool'"; exit 1 ;;
  esac

  mkdir -p "$out_dir"
  cp "$ROOT/target/$rust_target/release/gitim"         "$out_dir/"
  cp "$ROOT/target/$rust_target/release/gitim-daemon"  "$out_dir/"
  cp "$ROOT/target/$rust_target/release/gitim-runtime" "$out_dir/"
  chmod +x "$out_dir"/*

  # Smoke test: for Linux targets, verify the binary starts inside Alpine.
  # For macOS targets on Apple Silicon host, native arm64 runs directly;
  # x86_64-apple-darwin runs via Rosetta 2 if installed — skip to keep
  # release reproducible on hosts without Rosetta.
  case "$slug" in
    linux-*)
      local arch_tag
      case "$slug" in
        linux-x86_64) arch_tag="linux/amd64" ;;
        linux-arm64)  arch_tag="linux/arm64" ;;
      esac
      echo "==> [$slug] smoke test (docker alpine, $arch_tag)"
      docker run --rm --platform "$arch_tag" \
        -v "$out_dir/gitim:/gitim:ro" \
        alpine:3 /gitim --version >/dev/null
      ;;
  esac

  # Tar it up
  (cd "$STAGING" && tar czf "${archive_name}.tar.gz" "$archive_name")
  rm -rf "$out_dir"  # keep only the tarball
  echo "==> [$slug] packaged: ${archive_name}.tar.gz"
}

# ---------- Run matrix (fail-fast) ----------
for t in "${TARGETS[@]}"; do
  IFS=: read -r rust_target slug tool <<< "$t"
  build_target "$rust_target" "$slug" "$tool"
done

# ---------- SHA256SUMS ----------
(cd "$STAGING" && shasum -a 256 gitim-${TAG}-*.tar.gz > SHA256SUMS)
echo ""
echo "==> SHA256SUMS:"
cat "$STAGING/SHA256SUMS"

# ---------- Dry run exit ----------
if $DRY_RUN; then
  echo ""
  echo "==> Dry run complete. Would publish to $RELEASES_REPO:"
  (cd "$STAGING" && ls -la gitim-${TAG}-*.tar.gz SHA256SUMS)
  exit 0
fi

# ---------- gh auth ----------
if ! gh auth status >/dev/null 2>&1; then
  echo "Error: gh not authenticated. Run: gh auth login"
  exit 1
fi

# ---------- Create / upload release ----------
# Fail-fast invariant: if any build above failed, `set -e` killed the script
# before we reach here. So any upload at this point is atomic-ish — either
# all assets for this matrix run land in the Release, or none (we exited early).
echo ""
echo "==> Publishing ${TAG} to ${RELEASES_REPO}..."

NOTES_ARGS=()
if [ -n "$NOTES_FILE" ]; then
  NOTES_ARGS=(--notes-file "$NOTES_FILE")
else
  NOTES_ARGS=(--notes "GitIM ${TAG} release")
fi

cd "$STAGING"
ASSETS=(gitim-${TAG}-*.tar.gz SHA256SUMS)

if gh release view "$TAG" --repo "$RELEASES_REPO" >/dev/null 2>&1; then
  echo "    Release $TAG exists, re-uploading assets..."
  gh release upload "$TAG" "${ASSETS[@]}" --repo "$RELEASES_REPO" --clobber
else
  gh release create "$TAG" "${ASSETS[@]}" \
    --repo "$RELEASES_REPO" \
    --title "GitIM ${TAG}" \
    "${NOTES_ARGS[@]}"
fi

echo ""
echo "==> Published ${TAG}"
echo "    https://github.com/${RELEASES_REPO}/releases/tag/${TAG}"
echo ""
echo "    Install:"
echo "    curl -sSf https://raw.githubusercontent.com/${RELEASES_REPO}/main/install.sh | sh"
echo ""
echo "    Update:"
echo "    gitim update"
```

- [ ] **Step 3: 验证 `--dry-run` 跑完整 matrix (不 upload)**

```bash
# Docker 必须开启。本机要 cargo install cross。macOS x86_64 target 必须已 rustup add。
./release.sh --dry-run
```

Expected: 4 tarball + SHA256SUMS 在 `target/release-dist/`,输出列出文件;**不调 gh release**。大致耗时 15-20 分钟 (QEMU)。

- [ ] **Step 4: 验证 `--target` 单 target 路径**

```bash
./release.sh --dry-run --target darwin-arm64
```

Expected: 只编 1 target,快速 (~1 分钟),生成 `gitim-v{tag}-darwin-arm64.tar.gz` + SHA256SUMS (只含一行)

- [ ] **Step 5: 验证未知 slug 被拒**

```bash
./release.sh --dry-run --target windows-x86_64
```

Expected: 报错 `Error: unknown --target slug 'windows-x86_64'`,退出 1

- [ ] **Step 6: Commit**

```bash
git add release.sh
git commit -m "build(release): 4-target matrix with SHA256SUMS + fail-fast + --target filter"
```

---

### Task 12: `install.sh` — 白名单扩 4 + SHA 校验

**Files:**
- Modify: `install.sh:16` (SUPPORTED_PLATFORMS)
- Modify: `install.sh:44-57` (加 SHA256SUMS fetch + verify step)

- [ ] **Step 1: 扩 SUPPORTED_PLATFORMS**

文件 `install.sh:16`:
```bash
SUPPORTED_PLATFORMS="darwin-arm64"
```
改为:
```bash
SUPPORTED_PLATFORMS="darwin-arm64 darwin-x86_64 linux-arm64 linux-x86_64"
```

- [ ] **Step 2: 在 download tarball 之前插入 SHA 校验**

找到 `install.sh` 里 `# ---------- Download ----------` 段 (line ~44),在 `HTTP_CODE=$(curl -sSL -w ... "$DOWNLOAD_URL")` 成功之后、`# ---------- Extract ----------` 之前,插入:

```bash
# ---------- Verify SHA256 ----------
SHA_URL="https://github.com/${RELEASES_REPO}/releases/download/${TAG}/SHA256SUMS"
echo "==> Verifying SHA256..."
SHA_FILE="$TMPDIR/SHA256SUMS"
if ! curl -sSfL -o "$SHA_FILE" "$SHA_URL"; then
  echo "Error: SHA256SUMS not found at $SHA_URL"
  echo "This release may be pre-v0.6.0. Upgrade path:"
  echo "  1. Install v0.5.x manually (or skip SHA check: SKIP_SHA=1 sh install.sh)"
  echo "  2. Run \`gitim update\` after install to jump to the current version."
  # Allow explicit bypass for one-shot recovery; never default to off.
  if [ "${SKIP_SHA:-0}" != "1" ]; then
    exit 1
  fi
  echo "==> SKIP_SHA=1 — skipping SHA verification (unsafe)"
else
  EXPECTED_SHA=$(grep " $ARCHIVE_NAME$" "$SHA_FILE" | awk '{print $1}' | head -1)
  if [ -z "$EXPECTED_SHA" ]; then
    echo "Error: SHA256SUMS has no line for $ARCHIVE_NAME"
    exit 1
  fi
  ACTUAL_SHA=$(shasum -a 256 "$TMPDIR/$ARCHIVE_NAME" | awk '{print $1}')
  if [ "$EXPECTED_SHA" != "$ACTUAL_SHA" ]; then
    echo "Error: SHA256 mismatch"
    echo "  expected: $EXPECTED_SHA"
    echo "  actual:   $ACTUAL_SHA"
    rm -f "$TMPDIR/$ARCHIVE_NAME"
    exit 1
  fi
  echo "==> SHA256 verified."
fi
```

(注意: `grep " $ARCHIVE_NAME$"` 前导空格锁尾部匹配,避免子串匹配混淆)

- [ ] **Step 3: 手动 smoke test (需要先发一个 pre-release 作为 fixture,或本地 mock)**

本地 mock 方案 — 生成一个 fake SHA256SUMS,测匹配/不匹配:

```bash
# 构造假 tarball
mkdir -p /tmp/itest
echo "fake" > /tmp/itest/gitim-v9.9.9-darwin-arm64.tar.gz
REAL_SHA=$(shasum -a 256 /tmp/itest/gitim-v9.9.9-darwin-arm64.tar.gz | awk '{print $1}')

# 手动执行 install.sh 的 SHA 校验片段 (inline shell test):
EXPECTED_SHA="$REAL_SHA"
ACTUAL_SHA=$(shasum -a 256 /tmp/itest/gitim-v9.9.9-darwin-arm64.tar.gz | awk '{print $1}')
[ "$EXPECTED_SHA" = "$ACTUAL_SHA" ] && echo "match OK" || echo "FAIL"

# 验证 mismatch 场景
EXPECTED_SHA="0000000000000000000000000000000000000000000000000000000000000000"
[ "$EXPECTED_SHA" != "$ACTUAL_SHA" ] && echo "mismatch detected OK" || echo "FAIL"
```

Expected: "match OK" + "mismatch detected OK"

(完整 E2E smoke 需要真实发一个带 SHA 的 pre-release,做为 Task 12 之外的手工验证 checklist,见下文 "Post-plan verification")

- [ ] **Step 4: shellcheck (可选,如果本机有)**

```bash
command -v shellcheck >/dev/null && shellcheck install.sh release.sh || echo "shellcheck not installed, skipping lint"
```

Expected: 无 error / 若干 warning 可接受 (记下来未来修)

- [ ] **Step 5: Commit**

```bash
git add install.sh
git commit -m "feat(install): expand platform whitelist to 4; enforce SHA256 verification"
```

---

## Post-plan manual verification (不是 task,是 checklist)

Plan 实施完成后,在真实网络做一次端到端验证:

- [ ] 本机跑 `./release.sh --dry-run` 全 matrix 通过 (4 tarball + SHA256SUMS on disk)
- [ ] 把 dry-run 产物手动 upload 到一个 **pre-release** (`v0.6.0-rc.1`,`--prerelease`),不覆盖 latest
- [ ] 新开一个临时目录,用临时 `GITIM_VERSION=0.6.0-rc.1` 跑 `curl ... install.sh | sh`,验证 SHA 校验通过 + binary 能跑
- [ ] 手动污染 pre-release 的 tarball (重新上传一个被改过的 tar.gz),观察 install.sh 拒绝安装 + 报 mismatch
- [ ] WebUI 跑 update 流程,验证 toast 展示结构化错误
- [ ] 满意后 delete pre-release,正式 `./release.sh`(无 dry-run)发 v0.6.0

---

## Self-Review Checklist (writing-plans 要求)

**Spec coverage:**
- ✅ Target matrix (4 targets): Task 11
- ✅ Linux musl: Cross.toml (T3) + release.sh (T11)
- ✅ macOS x86_64 from Apple Silicon: release.sh rustup add + cargo --target (T11)
- ✅ Archive naming 契约: release.sh build_target (T11) + updater detect_platform (unchanged)
- ✅ SHA256SUMS integrity: release.sh (T11) + install.sh (T12) + updater install_update (T7)
- ✅ Fail-fast: release.sh `set -e` (T11)
- ✅ Dev-dep native-tls 清理: Task 1
- ✅ Callsite 切换: Task 8
- ✅ Critical gap (WebUI SHA error UX): Task 9 + Task 10
- ✅ TDD regression guard (SHA mismatch 不 extract): Task 7

**Placeholders 扫查:**
- Task 9 Step 2 说 "需按实际 UpdateError 结构调整 — 实现者读 struct def 后写" — 合规,因为实现者必须先阅 existing code,避免 plan 里硬编码错误 shape
- Task 10 Step 3 code sample 里的 `tryParseJson` 标注 "existing helper 或 JSON.parse 包 try" — 合规,两种 fallback 都具体化
- Task 10 Step 4 devtools override 步骤给了明确两条路径 (override 或临时注入 mock),符合"完整动作"标准
- Task 11 `DOWNLOAD_BASE` 等变量命名是具体的,不是 placeholder

**Type consistency:**
- `install_update(base, tag, platform, dest)` 签名在 T7 定义,T8 所有 callsite 使用一致
- `verify_sha256(bytes, expected_hex)`(T4)、`download_bytes(url)`(T5)、`extract_tarball(bytes, dest)`(T6) 在 T7 的 `install_update` 里组合使用,签名完全对齐
- `UpdateError::Sha256Mismatch { expected, actual }` / `Sha256LineMissing(String)` 在 T4、T7 测试、T9 mapping 使用一致
- `SHA256SUMS` 文件名在 T7 / T11 / T12 三处一致
- Archive 命名 `gitim-{tag}-{platform}.tar.gz` 三处一致
