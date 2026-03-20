# TODOS

## TODO: .gitignore 管理 — 移入 repo 初始化模板

**What:** 设计 init 流程，把 `.gitim/me.json` 和 `.gitim/run/` 的 gitignore 规则纳入 repo 初始化模板，从 `onboard.ts` 中移除 `.gitignore` 修改逻辑。

**Why:** 多人并发 onboard 时 `.gitignore` 冲突，这些规则属于 repo 基础设施而非个人设置。

**Depends on:** GitStorage 职责分离重构完成后再做。

**Context:** 当前 `onboard.ts:77-85` 每次 onboard 都检查并追加 `.gitignore` 条目。如果 5 个用户同时 onboard，各自追加同样的行然后提交，合并时会冲突。正确做法是在 repo 创建时就把这些规则写入 `.gitignore` 并提交。

---

## TODO: onboard bootstrap git 操作收敛到 daemon

**What:** `onboard.ts` 里的 `git add -A` / `git commit` / `git push` 应在 repo 初始化后通过 daemon API 完成，而不是 CLI 直接提交。

**Why:** 符合「git 提交权收敛到 daemon」的架构原则。当前 onboard 是唯一绕过 daemon 直接做 git commit 的 CLI 路径。

**Depends on:** GitStorage 重构 + daemon 能处理用户注册和 repo 配置的完整流程。

**Context:** `onboard.ts:110-113` 直接执行 `git add -A && git commit && git push`。重构后流程应为：CLI 做 bootstrap（clone/init 不可避免）→ 启动 daemon → 通过 daemon API 注册用户 → daemon 内部通过 GitStorage 完成 commit/push。
