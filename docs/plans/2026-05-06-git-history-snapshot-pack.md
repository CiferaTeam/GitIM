# Git 历史 Snapshot Pack 迁移方案

## 背景

GitIM 的消息写入会形成大量 Git commit。长期运行后，仓库历史可能累积到百万级提交，导致 `git log`、fetch、rebase、commit graph 遍历和 Web 端历史展示变慢。

Snapshot Pack 的目标是把某个切换点的业务文件状态固化为新 epoch 分支的首个提交，同时让仍在旧分支上的 agent 能通过普通 Git 同步发现切换指令，再迁移到新 epoch。

## 目标

1. 新 epoch 分支的历史从单个 snapshot commit 开始，工作树包含切换点的完整 GitIM 数据。
2. 旧分支保留一个 redirect commit，作为旧 agent 的自动切换入口。
3. 受 runtime 管理的 agent 能在无本地未推送变更时自动切换到新 epoch。
4. 有本地未推送变更的 agent 先进入 paused 状态，保存 replay 信息后再切换。
5. 旧历史通过 tag 和 bundle 归档，支持审计和人工恢复。

## 核心结构

切换前：

```text
main: A -- B -- C
```

切换后：

```text
main:          A -- B -- C -- R

main-epoch-2:  S
```

- `C`：旧 epoch 的切换点。
- `S`：新 epoch 分支的 orphan snapshot commit。
- `R`：旧分支上的 redirect commit。

`S` 的业务文件内容与 `C` 一致，并包含当前 epoch 的元数据。`R` 通过同一份元数据文件声明目标 epoch、目标分支和目标提交。

## 元数据文件

使用仓库根目录的 `gitim.epoch.yaml` 作为 Git 跟踪文件。

新 epoch 分支上的 active 文件：

```yaml
schema_version: 1
status: active
epoch: 2
branch: main-epoch-2
snapshot:
  source_branch: main
  source_commit: <C>
  commit: <S>
  created_at: "2026-05-06T00:00:00Z"
archive:
  tag: archive/epoch-1/<C-short>
  bundle_sha256: <sha256>
```

旧分支上的 redirect 文件：

```yaml
schema_version: 1
status: redirected
epoch: 1
branch: main
redirect:
  target_epoch: 2
  target_branch: main-epoch-2
  target_commit: <S>
  snapshot_of: <C>
  created_at: "2026-05-06T00:00:00Z"
archive:
  tag: archive/epoch-1/<C-short>
  bundle_sha256: <sha256>
```

daemon 把 `status: redirected` 视为只读信号。任何写入 API 在切换完成前返回 `epoch_redirected`，并带上目标分支和目标提交。

## Pack Coordinator 流程

Snapshot Pack 由一个协调进程执行，可以作为未来的 `gitim pack snapshot` 命令。

### 阶段 1：进入维护窗口

1. runtime 向受管理的 agent loop 发送 pause 请求。
2. daemon 停止接受新的写入请求，已有写入完成当前 commit 后进入只读状态。
3. coordinator 拉取远端最新状态，确认当前分支 HEAD 为切换点 `C`。
4. coordinator 检查受管理 clone 的状态：
   - `git status --porcelain` 为空。
   - `@{upstream}..HEAD` 提交数为 0。

### 阶段 2：归档旧历史

1. 创建归档 tag：`archive/epoch-1/<C-short>` 指向 `C`。
2. 生成 bundle：`git bundle create gitim-epoch-1-<C-short>.bundle --all`。
3. 计算 bundle sha256，写入 epoch 元数据。
4. 推送归档 tag。

### 阶段 3：创建新 epoch 分支

1. 基于 `C` 的 tree 生成临时工作树。
2. 写入 active 版 `gitim.epoch.yaml`。
3. 创建 orphan commit `S`。
4. 推送 `S` 到 `refs/heads/main-epoch-2`。

### 阶段 4：发布旧分支 redirect

1. 回到旧分支 `main`。
2. 写入 redirected 版 `gitim.epoch.yaml`。
3. 创建 redirect commit `R`。
4. 推送 `R` 到 `main`。

旧 agent 后续执行正常 fetch/pull 时会 fast-forward 到 `R`，从而发现切换指令。

### 阶段 5：迁移受管理 clone

对每个受管理 clone：

1. fetch `origin main main-epoch-2`。
2. 读取旧分支上的 redirect 元数据。
3. 检查本地未推送提交。
4. 无未推送提交时切换到 `main-epoch-2` 并跟踪 `origin/main-epoch-2`。
5. 重建本地 `.gitim/index.db`。
6. 恢复 daemon 和 agent loop。

## Agent 自动切换状态机

daemon 在以下时机检查 `gitim.epoch.yaml`：

- 启动后。
- sync loop fetch 后。
- push 被拒后。
- 写入 API 执行前。

状态机：

```text
active
  │ 发现 status=redirected
  ▼
redirect_detected
  │ 检查本地未推送提交
  ├─ 无未推送提交 → switch_epoch
  └─ 有未推送提交 → paused_for_replay

switch_epoch
  │ fetch + checkout target_branch
  ▼
reindex
  │ 重建本地索引
  ▼
active

paused_for_replay
  │ 保存 replay queue，等待 runtime 或人工处理
  ▼
replay_pending
```

`paused_for_replay` 状态下，daemon 继续提供 read/search/status，写入 API 返回 `epoch_replay_required`。

## Replay 规则

Replay 只处理本地未推送的 `.thread` 增量。

1. 计算本地 HEAD 与旧 upstream 的 merge-base。
2. 从 `merge-base..HEAD` 提取新增的 `.thread` entry。
3. 按 channel 和旧行号排序。
4. 切到目标 epoch 分支。
5. 逐条通过 daemon 写入路径重放。
6. 建立旧行号到新行号的映射。
7. 当 `P` 指向本地未推送 entry 时，使用映射后的新行号。
8. 当 `P` 指向 snapshot 中已有 entry 时，沿用原行号。

Replay 完成后，daemon 更新本地 agent state 的 cursor，重建索引，再恢复 agent loop。

## 本地状态保留

切换 epoch 时保留以下 Git 忽略状态：

- `.gitim/config.yaml`
- `.gitim/me.json`
- `.gitim/run/`
- `.gitim/agent-state.json`
- `.gitim/index.db`，切换后重建

runtime 侧保留 agent 配置、provider session token 和 workspace registry。切换完成后，agent loop 使用原 handler 和原 provider 配置继续工作。

## 远端分支策略

新 epoch 分支命名格式：

```text
main-epoch-<N>
```

旧分支保留 redirect commit。所有受管理 clone 完成切换后，远端默认分支可以改为最新 epoch 分支。

后续 epoch 继续递增：

```text
main-epoch-2
main-epoch-3
main-epoch-4
```

每次 pack 都在当前 active epoch 分支上创建下一代 snapshot 分支，并在当前分支末尾发布 redirect commit。

## 实现计划

### 任务 1：定义 epoch 元数据类型

文件：

- `crates/gitim-core/src/epoch.rs`
- `crates/gitim-core/src/lib.rs`

内容：

- `EpochStatus::{Active, Redirected}`
- `EpochFile`
- `SnapshotInfo`
- `RedirectInfo`
- YAML 读写 helper
- schema version 校验

### 任务 2：daemon 接入 redirect 检测

文件：

- `crates/gitim-daemon/src/state.rs`
- `crates/gitim-daemon/src/handlers/send.rs`
- `crates/gitim-daemon/src/handlers/channel.rs`
- `crates/gitim-daemon/src/card_handlers.rs`

内容：

- daemon 启动时读取 epoch 文件。
- 写入 API 前检查 redirect 状态。
- status API 返回当前 epoch 状态。
- sync loop fetch 后刷新 epoch 状态。

### 任务 3：实现 pack coordinator

文件：

- `crates/gitim-daemon/src/handlers/pack.rs`
- `cli/src/commands/pack.ts`

内容：

- 创建归档 tag。
- 创建 bundle 和 sha256。
- 生成 orphan snapshot commit。
- 推送新 epoch 分支。
- 在旧分支生成 redirect commit。
- 输出迁移结果 JSON。

### 任务 4：runtime 管理受控 agent 切换

文件：

- `crates/gitim-runtime/src/http.rs`
- `crates/gitim-runtime/src/agent_loop.rs`
- `crates/gitim-runtime/src/workspace.rs`

内容：

- pause/resume agent loop。
- 枚举 workspace 内受管理 clone。
- 调用 daemon status 获取 redirect。
- 执行 fetch 和 branch switch。
- 切换后重启 daemon 或触发 reindex。

### 任务 5：实现 replay queue

文件：

- `crates/gitim-daemon/src/replay.rs`
- `crates/gitim-sync/src/git.rs`

内容：

- 提取本地未推送 `.thread` 增量。
- 持久化 `.gitim/replay/epoch-<N>.json`。
- 切换后按顺序重放。
- 保存旧行号到新行号映射。

### 任务 6：测试

覆盖场景：

- 无本地未推送提交的 clone 自动切换。
- 旧分支 redirect commit 能被旧 clone fast-forward 获取。
- 新 epoch 分支只有 snapshot 起点和后续消息提交。
- daemon 在 redirected 状态拒绝写入。
- replay 能保持同一批本地消息的父子关系。
- 切换后搜索索引重建成功。

## 验证清单

Pack 完成后检查：

```bash
git fetch --all --tags
git log --oneline main -3
git log --oneline main-epoch-2 -3
git show main:gitim.epoch.yaml
git show main-epoch-2:gitim.epoch.yaml
```

期望：

- `main` 顶部是 redirect commit。
- `main-epoch-2` 顶部是 active snapshot commit 或后续正常消息 commit。
- `main-epoch-2` 的早期历史不包含旧 epoch 的百万级提交。
- 受管理 agent 的 daemon status 显示 active epoch 为 2。
- 新消息能在 `main-epoch-2` 上正常写入、提交、push。

## 回滚

归档 tag 和 bundle 是回滚入口。

1. 停止 runtime 和 daemon。
2. 将远端默认分支改回旧分支。
3. 让受管理 clone fetch 旧分支。
4. 根据 `archive/epoch-1/<C-short>` 定位旧切换点。
5. 从 bundle 恢复缺失对象。

回滚后，daemon 按旧分支上的 epoch 元数据重新进入 active 或 redirected 状态。
