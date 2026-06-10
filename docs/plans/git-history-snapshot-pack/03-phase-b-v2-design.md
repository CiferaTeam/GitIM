# Snapshot Pack — Phase B v2 设计（竞态修订版）

> **Lineage**: 总体 spec 见 [`../2026-05-06-git-history-snapshot-pack.md`](../2026-05-06-git-history-snapshot-pack.md)；
> 前版 plan 见 [`02-phase-b-auto-rotate.md`](02-phase-b-auto-rotate.md)（实现停在 Task 3/10，
> 成果在 `worktree-pr3-auto-rotate` 分支）。本文档修订 02 的竞态缺口，是 Phase B 重启的 design 基线。

## 背景：02 版断在哪

02 版的 atomic-push 仲裁解决了 **fire vs fire**（多节点同时过阈值）：`git push --atomic`
同推两个 ref，服务端串行处理，单 winner，loser 整体 reject。这部分设计正确，保留。

02 版没解决的是 **fire vs 普通写入**：`commit_lock` 是进程内锁，只保护 fire 节点自己。
Winner seal 老分支后，未感知 rotation 的节点 B 写消息 → push 冲突 → `pull_rebase` 把消息
rebase 到 redirect commit 之上 → push fast-forward 成功 → 消息永远留在 sealed branch，
新 epoch 不可见——**无声丢失**。02 砍掉 replay 的理由（"commit_lock 全程持有，无 in-flight
unpushed commits"）只对 fire 节点成立，对 follower 不成立。

## 用户已裁决决策

| 决策 | 裁决 |
|---|---|
| 切换窗口内消息丢失容忍度 | **零丢失是硬约束** |
| migrate 机制 | **方案 A：`rebase --onto`**（git 层搬运，复用现有 renumber 冲突机制） |
| browser 端（daemon-web） | **v1 只做只读拦截**（检测 redirect → 停写 + 提示），不实现 fire/follow/migrate |
| 阈值 | 1,000,000 commits，`GITIM_ROTATION_THRESHOLD` env override（沿用 02 裁决） |
| 触发点 | `on_pushed` 后（沿用 02 裁决） |
| 写入 API enforcement gate | 不做（沿用 Phase A 裁决；fence 在 push 层完备，handler 层无需 gate） |
| Bundle | 本地落地不上传；winner 负责；失败 warn 不阻塞（沿用 02） |
| 旧 epoch | 全留不 prune（沿用 02） |

## 协议不变量

1. **Sealed branch 的 tip 永远是 redirect commit `R`。**
   由 push-fence 保证：任何 push 前检查 `HEAD:gitim.epoch.yaml`，是 `redirected` 即拒推。
2. **Rotation 的唯一仲裁者是 git 服务端的 atomic push。**
   fire = `git push --atomic` 两个 ref（老分支 R + 新分支 S），全成或全败，零额外协调。
3. **判定一律以 origin 为准，不信本地残留。**
   follow / fire 的决策依据是 fetch 后 `origin/<branch>:gitim.epoch.yaml` 的状态。
   这是对 02 版的关键修正：02 的 `follow_redirect` 读本地工作树 epoch.yaml，loser 自己
   写的 redirect 残留会污染判定——在"fire 输给普通写入"场景下会切去不存在的分支。

### Fence 的完备性论证

Redirect commit `R` 携带 redirected 版 `gitim.epoch.yaml`，且后续消息 commit 永不修改该文件，
所以 **"R 在本地链上" ⇔ "HEAD tree 的 epoch.yaml 是 redirected"**——O(1) 判定（`git show
HEAD:gitim.epoch.yaml`），无遗漏。

Fence 有两个检查点，共享一段判定逻辑：

- **(i) fetch 后、rebase 前**：检查 `origin/<branch>:gitim.epoch.yaml`。redirected → 不把
  本地消息 rebase 到 R 上，直接进 migrate。作用：减少 migrate 的 base 复杂度。
- **(ii) push 前**：检查 `HEAD:gitim.epoch.yaml`。redirected → 拒推 → migrate。
  作用：不变量 1 的最终兜底，封死消息发布到 sealed branch 的唯一出口。

### 零丢失论证

消息丢失的唯一路径是"消息 commit 被发布到 sealed branch 上 R 之后"。发布只有 push 一个
出口，fence (ii) 封死它；被拦的消息要么 migrate 成功上新分支，要么留在本地未推送状态等
下轮 cycle 重试——任何失败模式只造成延迟，不造成丢失。

## 竞态收敛矩阵

| # | 场景 | 收敛路径 |
|---|------|---------|
| 1 | N 节点同时 fire | atomic push 仲裁出 1 winner；loser reject → **清理**（reset 老分支回 origin、删本地废 orphan ref）→ follow |
| 2 | fire 输给普通消息 push | 无 winner——fire 节点 reject 后清理，fetch 发现 origin 无 redirect → 回到 active，下次 push 后再 fire（自愈） |
| 3 | 普通消息 push 输给 fire | push reject → fetch → fence (i) 发现 origin tip redirected → 不 rebase 到 R → migrate：`rebase --onto origin/<new> <merge-base>` → checkout 新分支 → push 新分支，消息保住 |
| 4 | R 已被 pull 进本地后 handler 才写消息 | 消息 commit 落在 R 之上 → fence (ii) 拦截 → migrate（base 为 R） |
| 5 | follow 与 handler 写入抢锁 | 同进程 `commit_lock` 互斥：follow 先则写到新分支；写入先则归入场景 4 |
| 6 | 节点沉睡跨多个 epoch | follow 内部多跳 loop（main → epoch-2 → epoch-3 …，max-hop 32 防环），解析出最终 active 分支后**一次** migrate + checkout，不逐跳搬 |
| 7 | fire 中途 crash | atomic push 前 crash：本地残留 redirect commit，boot 时"origin 非 redirected 而本地 HEAD 是"→ 识别为半成品 fire → reset 清理。push 后 checkout 前 crash：boot 时 follow 检查直接切换。checkout 后 bundle 前 crash：bundle 丢失，best-effort 可接受 |
| 8 | migrate 的 rebase 冲突 | 复用现有 sync_loop 冲突恢复模式（rebase 前已捕获 `.thread` 增量 → reset → daemon 重放/renumber），把 reset 目标从 `origin/old` 换成 `origin/new`，机制零新建 |

## 组件改动

### gitim-sync `git.rs`

从 `worktree-pr3-auto-rotate` cherry-pick（已实现，带测试）：
- `count_commits_on_branch` / `create_orphan_commit` / `write_redirect_commit`

新增原语：
- `atomic_push_two_refs(old_branch, new_branch)` — `git push --atomic origin <new>:refs/heads/<new> <old>:refs/heads/<old>`，reject 与其他错误分型
- `show_file_at_ref(reference, path) -> Option<String>` — `git show <ref>:<path>`，fence 与 origin 判定共用
- `rebase_onto(new_base, old_base) -> Result` — migrate 用
- `reset_branch_to_origin(branch)` / `delete_local_branch(branch)` — Lost / crash 清理
- `checkout_branch(branch)`（`checkout -f`）、`tag_archive(tag, sha)`、`push_tag(tag)`、`bundle_to_path(path, ref)` — 02 版 Task 4/5 原样

### gitim-sync `rotate.rs`（新模块）

- `RotationOutcome { NotReady, Won {...}, Lost }`
- `try_fire_rotation(...)` — 02 版逻辑 + 两处修正：
  1. fetch 后以 `origin/<branch>:gitim.epoch.yaml` 判定是否已被 rotate（不信本地）
  2. Lost 分支调 `cleanup_failed_fire`（reset 老分支 + 删废 orphan ref）后再返回
- `resolve_active_branch(storage, start_branch) -> (branch, hops)` — 多跳解析：沿
  `origin/<b>:gitim.epoch.yaml` 的 redirect 链走到 active，max-hop 32
- `follow_redirect(...)` — 重写：fetch → resolve_active_branch → 若有未推送消息先
  migrate → checkout 最终 active 分支
- `check_push_fence(storage) -> bool` — `HEAD:gitim.epoch.yaml` 是否 redirected
- `migrate_unpushed(storage, target_branch)` — `rebase --onto`；失败返回错误，
  调用方走捕获增量重放兜底
- `cleanup_failed_fire(storage, old_branch, orphan_branch)`

### gitim-sync `sync_loop.rs`

`sync_with_push` 接入 fence：
- fetch 成功后：fence (i)（读 `origin/<branch>:gitim.epoch.yaml`）→ redirected →
  进入 migrate 路径（跳过对 R 的 rebase）
- push 前：fence (ii)（`check_push_fence`）→ redirected → 拒推 → migrate
- migrate 失败 → 复用现有"捕获增量 → reset → 重放"兜底（reset 目标换成新分支）

### gitim-daemon `state.rs`

- `ROTATION_THRESHOLD_DEFAULT: u64 = 1_000_000` + `GITIM_ROTATION_THRESHOLD` env override
- `try_rotate_inner()` — 持 `commit_lock`，调 `rotate::try_fire_rotation`；
  Won → `refresh_epoch_status`；Lost → `follow_redirect` + refresh
- **count throttle**：`AppState` 缓存上次 count 时刻，`on_pushed` 距上次 < 60s 跳过
  rotation 检查（1M 软阈值，过冲几百 commit 无影响；避免每次 push 都在百万 commit
  仓库上跑 `rev-list --count`）
- `on_pushed` → `spawn_blocking(try_rotate_inner)`（fire-and-forget，错误 warn）
- `on_synced` → `refresh_epoch_status` 后若 redirected → 持锁 `follow_redirect`
- daemon boot → 半成品 fire 清理检查 + follow 检查（同一段代码）

### gitim-runtime `http.rs`

- `/runtime/health` 加 `epoch_count` + `total_commit_count`（02 版 Task 9 原样）

### daemon-web（`products/gitim/frontend/src/daemon-web/`）

只读拦截（v1 范围）：
- sync 路径检测 `gitim.epoch.yaml` 状态，redirected → 写入 API 返回 `epoch_redirected`
  错误 + UI banner 提示刷新/切换
- 不实现 fire / follow / migrate

## 测试策略

| 层 | 场景 |
|---|---|
| `rotate_test.rs`（gitim-sync） | solo fire 胜出并切分支；under threshold NotReady；双 daemon race 单 winner 收敛（02 版 Task 5-7 保留） |
| 同上（v2 新增） | **fire-vs-write 双向**：消息 push 先赢 → fire 自愈（场景 2）；fire 先赢 → 消息 migrate 不丢（场景 3）；fence (ii) 拦截 R 之上的消息（场景 4）；多跳 follow（场景 6）；Lost 清理断言（本地老分支 == origin、无废 ref）；半成品 fire boot 清理（场景 7） |
| migrate 冲突 | 双方在新 epoch 同文件追加 → renumber 介入收敛（场景 8） |
| `epoch_rotation.rs`（gitim-daemon 集成） | threshold override 下 daemon 自动 rotate；on_synced follow |
| daemon-web | redirected 状态写入被拒 + banner（前端测试惯例从轻） |

验证基线：`cargo test -p gitim-sync`、`cargo test -p gitim-daemon --test epoch_rotation`，
不跑全量（遵循 CLAUDE.md 测试节奏）。

## 实施顺序

1. **资产抢救**：cherry-pick `worktree-pr3-auto-rotate` 的 Task 1-3 commits
   （`e8979778`、`cc8a092a`、`18507afc`、`8517cb1f`、`134c4691`、`7e2bc033`）到新分支，适配 main 漂移
2. git.rs 新原语（atomic push / show_file_at_ref / rebase_onto / 清理类 / bundle / tag）
3. rotate.rs（fire / resolve / follow / fence / migrate / cleanup）+ rotate_test 全场景
4. sync_loop fence 接入 + migrate 路径
5. daemon 接线（on_pushed / on_synced / boot + count throttle）+ 集成测试
6. runtime health + daemon-web 只读拦截
7. CLAUDE.md orientation 更新

## 非目标（沿用 02 + 新增）

- Bundle 上传外部 store；auto-prune 老 epoch；WebUI 手动 rotate 入口
- browser 端完整 fire/follow/migrate（v1 只读拦截）
- 新 clone 的历史下载优化（`--single-branch` / partial clone / 旧分支移出默认 fetch
  refspec）——rotation 不减新 clone 下载量的问题**留 v2**，设计时已知
- `snapshot.commit` 字段精确化（02 版已注明 v1 填 sealed SHA，后续 patch）
- 多 epoch 跨索引搜索（gitim-index 当前不跨 epoch）
