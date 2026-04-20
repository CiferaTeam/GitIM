# Cross-compile Release Pipeline — Requirements & Eng Review

> 任务: 把 GitIM (后端 3 binary: `gitim` / `gitim-daemon` / `gitim-runtime`) 的 release 扩展到 4 target 矩阵,本地脚本驱动,增量式扩展现有 `release.sh` / `install.sh` / `gitim-updater` 基础设施。
>
> **Status**: Phase 2 grill-me 决策锁定 + Phase 3 eng-review 完成。待 Phase 4 writing-plans 生成 step-by-step 实施 plan。

## Target Matrix

| target (Rust) | slug (archive / updater) | 工具链 | 编译 host |
|---|---|---|---|
| `aarch64-apple-darwin` | `darwin-arm64` | cargo + 本机 Xcode CLT | Apple Silicon native |
| `x86_64-apple-darwin` | `darwin-x86_64` | cargo + `rustup target add` + Xcode CLT universal SDK | Apple Silicon cross |
| `aarch64-unknown-linux-musl` | `linux-arm64` | `cross build` (Docker 镜像 aarch64 native) | Apple Silicon native via Docker |
| `x86_64-unknown-linux-musl` | `linux-x86_64` | `cross build` (Docker 镜像 x86_64 via QEMU) | Apple Silicon via QEMU emulation |

**不发:** Windows (WSL 兜底 — daemon 用 `UnixListener` 无 cfg(unix) 保护,scope 外)

**Archive 命名契约(冻结):** `gitim-v{tag}-{slug}.tar.gz` —— 对齐 `crates/gitim-updater/src/lib.rs:82` `detect_platform()` 输出。**不带 `-musl` 后缀**。

## 决策清单 (Phase 2 grill-me + Phase 3 eng-review)

| # | 决策点 | 锁定 | 出处 |
|---|---|---|---|
| 1 | 目标平台 | Linux + macOS Intel + macOS arm64 | grill Q1 |
| 2 | Linux libc | musl 静态 | grill Q2 |
| 3 | Linux arch | x86_64 + aarch64 都发 | grill Q3 |
| 4 | 执行模型 | 本地 `release.sh`,无 CI | grill Q4 |
| 5 | Cross 工具链 | `cross-rs/cross` (Docker-based) | grill Q5 |
| 6 | macOS 两 target 编译 | Apple Silicon host + rustup target + Xcode CLT 自带 SDK | 推导 |
| 7 | Archive 命名 | `gitim-v{tag}-{darwin,linux}-{arm64,x86_64}.tar.gz`,契约冻结 | 代码分析 |
| 8 | Integrity | L1: `SHA256SUMS` 强制校验;无 signing | grill Q6 |
| 9 | macOS codesign/notarize | 不做,不留 opt-in 钩子 | grill Q7 |
| 10 | 前端 (webui-v2) | Out of scope,release 只 ship 3 Rust binary | 用户澄清 |
| 11 | Releases repo | `CiferaTeam/gitim-releases` (沿用) | 代码分析 |
| 12 | 4 target build 失败处理 | **Fail-fast,整体中止,不 upload 任何 asset** | eng Q A2 |
| 13 | Apple Silicon QEMU 编译耗时 | 接受 (~20min release 端到端),不引入 OrbStack | eng Q A4 |
| 14 | updater SHA 融入方式 | 拆 3 单职责函数 (`download_bytes` / `verify_sha256` / `extract_tarball`) + `install_update` 编排;删除旧 `download_and_extract` | eng Q Q3 |
| 15 | 老 Release 兼容性 | v0.6.0 起强制 SHA;v0.5.x 老 updater 升到 v0.6.0 走单次无校验窗 (老 updater 不知 SHA 存在) | eng A5 |

## Data Flow

```
───────────── maintainer local (Apple Silicon) ─────────────
bump.sh [UNCHANGED]                              release.sh [REWRITE]
  └─ patch Cargo.toml version + git tag           ├─ verify tag exists
  └─ gen docs/releases/v{tag}.md                  ├─ for each of 4 targets:
                                                  │    build_target(rust_target, slug, tool)
                                                  │      [tool=cargo|cross]
                                                  │      → cp 3 binary to staging/gitim-v{tag}-{slug}/
                                                  │      → tar czf
                                                  │    SMOKE TEST (docker run alpine for linux)
                                                  ├─ shasum -a 256 *.tar.gz > SHA256SUMS
                                                  └─ gh release upload (5 assets atomic)
                                                       (fail-fast: 任一失败,一概不上传)

───────────── end user ─────────────
install.sh [EXTEND]                           gitim-updater [REFACTOR]
  detect_platform                              install_update(url, sha_url, dest) 编排:
  fetch SHA256SUMS                               1. download_bytes(sha_url)
  download tarball                                  → parse → 拿 expected SHA
  verify_sha (reject on mismatch)                2. download_bytes(tarball_url)
  tar xzf → $HOME/.gitim/bin                     3. verify_sha256(bytes, expected)
  echo PATH guidance                             4. extract_tarball(bytes, tmp_dir)
                                                 5. replace_binaries(tmp_dir, install_dir, ...)
                                                    [已有 rollback 机制保持不变]
```

## 修改文件清单

| 文件 | 动作 | 说明 |
|---|---|---|
| `release.sh` | **Rewrite** | 4 target build matrix,SHA256SUMS 生成,fail-fast,docker smoke test,可选 `--target <slug>` 调试 |
| `install.sh` | Extend | `SUPPORTED_PLATFORMS` 扩到 4;fetch SHA256SUMS + verify |
| `crates/gitim-updater/src/lib.rs` | Refactor | 拆 3 单职责函数 + `install_update` 编排;删除 `download_and_extract` |
| `crates/gitim-updater/Cargo.toml` | Extend | 加 `sha2` 依赖 |
| `crates/gitim-updater/tests/*` | Extend | 新增 SHA 校验 unit + integration tests (wiremock) |
| `Cross.toml` | **New** | Pin cross Docker image tag (e.g., `ghcr.io/cross-rs/x86_64-unknown-linux-musl:0.2.5`) |
| `crates/gitim-daemon/Cargo.toml` | Fix | dev-dep `reqwest` 加 `default-features = false, features = ["json", "rustls-tls"]` 清除 native-tls 泄漏 (Q4) |
| `crates/gitim-runtime/src/http.rs` 和 `crates/gitim-cli/src/commands/update.rs` | Callsite update | `download_and_extract` 被替换为 `install_update` |
| `docs/plans/cross-compile-release/*.md` | New | 本文档 + 后续 plan 文件 |
| `bump.sh` | **不动** | |
| `install-from-source.sh` | **不动** | |

## NOT in scope

| 项 | 原因 |
|---|---|
| Windows 发布 | Daemon Unix socket 无 cfg(unix) 保护,需 IPC 重写;用户选 WSL 兜底 |
| macOS codesign / notarize | 依赖 Apple Developer $99/年外部系统,用户拒 |
| WebUI (webui-v2) 打包 | 非用户产品面,只有 maintainer 部署 |
| SHA256SUMS 的 minisign / GPG 签名 (L2) | Key 管理复杂度过高,单一 maintainer 威胁模型下 L1 已够 |
| CI/GitHub Actions 自动 release | 用户明确选本地脚本路线 |
| aarch64 Linux 扩展到 glibc | 锁定 musl 单路线,避免分叉 |
| Fallback: SHA256SUMS 缺失时静默跳过校验 | fail-closed 是安全基线 |
| install.sh 的 bats 自动化测试 | 依赖外部工具,手动 smoke + updater 单元测试已覆盖 |

## What already exists

| 组件 | 现状复用度 | 备注 |
|---|---|---|
| `bump.sh` | 100% | 版本派生 + release notes 生成 (claude CLI fallback) + tag + commit |
| `install-from-source.sh` | 100% | `cargo install --path` 路径 |
| `CiferaTeam/gitim-releases` | 100% | release 目标 repo |
| `gitim-updater::detect_platform()` | 100% | 4 canonical slug 输出已就位 |
| `gitim-updater::download_url()` / `latest_release_api_url()` | 100% | URL 组装逻辑 |
| `gitim-updater::replace_binaries()` | 100% | 原子替换 + rollback,已 battle-tested |
| `gitim-updater::find_binary()` / `walkdir()` | 100% | 从 tarball 里找 binary,支持嵌套目录结构 |
| `release.sh` | 60% | upload + dry-run 机制保留,build 部分 rewrite |
| `install.sh` | 70% | 下载/解压/PATH 指南保留,白名单 + SHA verify 新增 |

## Failure Modes

| 代码路径 | 失败场景 | 测试覆盖 | 错误处理 | 用户可见? |
|---|---|---|---|---|
| `release.sh` build 某 target 失败 | QEMU 抖动 / Docker 挂 / rust 编译错 | 手动 (fail-fast 靠 `set -e`) | ✓ `set -euo pipefail` | ✓ 终端红色 |
| `release.sh` SHA 生成失败 | `shasum` 二进制缺失 (不可能) | N/A | `set -e` | ✓ |
| `release.sh` gh upload 失败 | 网络 / token 过期 | 手动 | `set -e`,无回滚 | ✓ |
| `install.sh` SHA mismatch | Release 污染 / 不完整下载 | **计划加 (手动 smoke)** | **计划加 (exit 1 + 清 tmp)** | ✓ 明确错误 |
| `gitim-updater` SHA mismatch | 同上,WebUI 触发路径 | **计划加 (wiremock unit)** | **计划加 (UpdateError + 清 tmp)** | **需前端 error 展示** |
| `gitim-updater` SHA256SUMS 404 | 老 Release 无 SHA 文件 | **计划加** | fail-closed: 拒升 | ✓ 错误提示 "该版本不受支持,请升 v0.6.0+" |
| `gitim-updater` v0.5.x → v0.6.0 单次跳跃 | 老 updater 不验 SHA | 人工一次验证 | N/A (老逻辑) | 无校验窗 (已知风险) |
| Cross.toml image 被 upstream 删除 | cross repo 打 tag | 手动 | `cross build` 报错 | ✓ |

**Critical gaps flagged:**

1. WebUI 触发 `/runtime/update-and-restart` 时 SHA mismatch 错误如何回显给用户? 前端当前的 "黄⚠ 一键升级" UX 可能不展示详细错误码。**建议**: runtime API 返回 `{ error_code: "sha_mismatch", message }`,前端 toast 显示。**标为 critical**,需 plan 里覆盖。

2. 老 Release (v0.5.x) 无 SHA 场景: v0.6.0+ 的 updater 必须 fail-closed,不能 fallback 到 no-verify。这是安全边界,不可松。

## Performance

- Apple Silicon QEMU 编译 x86_64 Linux: ~8-10min (accept per A4)
- cross image 首次 pull: 100-500MB/image x 2 target = 一次性 1GB 级
- SHA256 计算: tarball 10-20MB,<100ms,可忽略
- 整体 release end-to-end: 估 15-20min

## Parallelization strategy

任务高度 sequential (build → sha → upload,各 target 之间 fail-fast),**无 parallel 优势**。
Writing-plans 按 sequential 5 phase 生成 (release.sh / install.sh / updater / Cross.toml / dev-dep fix)。
