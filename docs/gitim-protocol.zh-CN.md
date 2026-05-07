# GitIM 协议

> 底层 IM 协议怎么工作,以及怎么直接用。
>
> ← 返回 [README](../README.zh-CN.md)

---

GitIM 是一套把消息建模成"**纯文本行 + Git commit**"的 IM 协议。它的全部状态都是人类可读的文本文件,全部变更都通过 Git 传播和审计。没有数据库,没有自定义传输层,没有服务端状态机 —— server 就是你选的 git server。

## 核心概念

| 概念        | 是什么                                                                 |
| ----------- | ---------------------------------------------------------------------- |
| Workspace   | 一个 Git 仓库。所有消息、频道、用户都在里面。                           |
| Channel     | 仓库里一个 `.thread` 文件,如 `general.thread`、`eng.thread`。           |
| Message     | `.thread` 文件里的一行。                                                |
| Thread      | 通过"父消息指针"串起来的消息链。不需要独立的 thread ID。                  |
| Handler     | 用户身份,小写 `a-z0-9-`,1–39 字符(`system` 保留)。通常等于 GitHub handle。 |
| DM          | 两人私聊。文件名把两个 handler 按字典序用 `--` 拼起来(如 `alice--bob`)。 |

## 消息格式

每条消息的开头是结构化的 prefix:

```
[L<行号>][P<父行号>][@<handler>][<时间戳>] <正文>
```

例如:

```
[L1][P0][@alice][2026-04-21T10:00:00Z] 大家好,我是 Alice
[L2][P1][@bob][2026-04-21T10:01:30Z] 欢迎 @alice
[L3][P0][@alice][2026-04-21T10:02:00Z] 今天 PR review 谁来?
```

字段:

- `L<行号>` —— 消息在文件中的行号,**就是**消息的 ID。行号唯一、稳定、肉眼可定位。
- `P<父行号>` —— 回复指向的父消息行号。`P0` 表示顶层消息。
- `@<handler>` —— 作者。
- `<时间戳>` —— ISO-8601 UTC。

**续行规则**:下一行如果不以 `[L...]` 开头,就算作上一条消息的续行。方便写多段落消息、贴代码块。

## 文件结构

一个典型的 workspace 长这样:

```
my-workspace/
├── general.thread          # 频道
├── random.thread
├── alice--bob.thread       # alice ↔ bob 的 DM
├── users/
│   ├── alice.meta.yaml     # 用户元信息
│   └── bob.meta.yaml
└── .gitim/
    └── config.yaml         # 本地配置(被 .gitignore 忽略)
```

## 上手

一个前提:`gitim` 所有命令(除了 `onboard` / `update` / `stop`)都必须在 workspace 根目录下运行 —— 也就是含 `.gitim/` 那层目录。CLI 会自动探测并按需启动 daemon,你不用手动管 daemon 进程。

### 初始化 workspace

`gitim onboard` 是**一条命令搞定**,全部参数通过 flag 传,没有交互向导。按 git provider 不同,几种典型写法:

```sh
# GitHub —— 最常用
gitim onboard <repo> <org> --token <ghp_xxx>
gitim onboard <repo> <org> --handler alice --display-name "Alice"   # 或用 handler + display-name 替代 token

# 纯本地 (单人 / 离线 demo)
gitim onboard --git-server git --handler alice --display-name "Alice"

# Gitea / GitLab (自托管)
gitim onboard <repo> <org> --git-server gitea  --url https://git.example.com --token <tok>
gitim onboard <repo> <org> --git-server gitlab --url https://gitlab.example.com --token <tok>
```

它一次性做完:克隆(或初始化)Git 仓库 → 启动 daemon → 推断并注册你的身份 → 提交 `users/<handler>.meta.yaml`。仓库已经存在时 onboard 等价于"加入现有团队 workspace",daemon 会 clone 下来并把你的 user meta 加进去 push 给大家。

全部 flag(含 `--admin` / `--guest` / `--refresh` 等)见 `gitim onboard --help`。

### 发消息 / 读频道

```sh
gitim send <channel> "消息内容" [--reply-to <行号>]
gitim read <channel> [--limit <n>] [--since <行号>]
```

例子:

```sh
gitim send general "今晚 10 点维护 staging"
gitim send eng "@alice PR #42 漏了个 case"      # @handler 写在消息体里就是提及
gitim send eng "同意" --reply-to 42               # 回复 L42
gitim read eng --limit 20                         # 只看最近 20 条
gitim read eng --since 100                        # 只看 L100 之后的
```

消息体里的 `@handler` 会被解析为提及,被提及的人会看到高亮。一条消息可以 `@` 多人。

### 私聊(DM)

```sh
gitim dm send <handler> "消息内容"    # 发 DM
gitim dm read <handler>                # 读和某人的 DM
gitim dm list                          # 列出你参与的所有 DM 会话
```

第一次 DM 对方时,daemon 自动创建 `<a>--<b>.thread`(两个 handler 按字典序)。之后所有 DM 追加进同一个文件。DM 在读写语义上和公共频道完全对称,只是参与者被约束成两个人。

### 频道管理

```sh
gitim channels                                        # 列出所有频道
gitim create-channel <name> [--display-name ...] [--introduction ...]
gitim join-channel <channel> -t alice -t bob         # 邀请 alice、bob 到这个频道
gitim archive-channel <name>                          # 归档
gitim unarchive-channel <name>                        # 取消归档
gitim archived-channels                               # 列出已归档频道
```

> 注意:`join-channel` 的语义是"**邀请他人**加入",不是"我自己加入"(在 workspace 里你对所有公共频道默认都可发言)。

### 看板(card)

每个频道可以挂若干张卡片,作为轻量级任务/工单看板。卡片自带讨论线。

```sh
gitim card create <channel> "实现 rate limiter" --label backend --assignee alice --status todo
gitim card ls --channel eng --status doing           # 筛选
gitim card read <channel> <card-id>                   # 看卡片及其讨论
gitim card comment <channel> <card-id> "已实现,待 review"
gitim card update <channel> <card-id> --status done --assignee bob
gitim card archive <channel> <card-id>
gitim card archived --channel eng                     # 列出归档卡片
```

### 搜索

```sh
gitim search "rate limit"                                      # 全文搜索
gitim search "rate limit" --author alice --channel eng          # 按作者 / 频道过滤
gitim search "..." --type dm                                    # 只在 DM 里搜
gitim search "..." --include-cards                              # 把卡片讨论也纳入
```

### 多端同步

在多台机器上对**同一个**远端 Git 仓库 `gitim onboard`,daemon 就在后台做增量同步,你看到的始终是所有机器合并后一致的视图。

**离线体验**:断网时消息照样能发出(落到本地 commit),联网后 daemon 自动 push 本地变更、拉取别人的变更。并发冲突由 daemon 内部处理,用户无感。

### 审计与回溯

消息就是 git commit,任何 git 工具都是你的审计工具:

```sh
git log general.thread                # 这个频道的所有变更
git blame general.thread               # 每行消息的 commit / 作者
git log --all --author=alice           # alice 的所有发言
git show <commit>                      # 看某次具体变更的上下文
```

整个讨论历史可以被打包、镜像、归档、离线审计,而且是**不可篡改**的:改了就改了,git 看得一清二楚。

### 管理与维护

```sh
gitim status       # daemon 状态 / 当前 workspace 信息
gitim users        # 列出 workspace 里所有用户
gitim reindex      # 重建全文索引
gitim stop         # 停止 daemon
gitim update       # 自升级到最新版(或指定版本)
gitim --help       # 完整命令列表
```

## 为什么这样设计

### 为什么消息是"一行文本"?

- `cat general.thread` 就是打开频道
- `grep` 就是搜索
- 任意 diff 工具都能 review 消息变更
- 任意文本编辑器都能读(写入由 daemon 校验合规)

### 为什么用行号当消息 ID?

行号天然唯一、肉眼可定位(`L42` 就是跳到第 42 行)、不需要 UUID / snowflake 之类额外设施。并发和合并带来的一致性问题由 daemon 内部的一致性层负责,协议用户不需要关心实现细节。

### 为什么用 Git 做传输?

- **无状态 server** —— 任何 git hosting(GitHub / GitLab / Gitea / self-host / 甚至共享硬盘)都能当 server
- **自带审计** —— 每条消息都是 commit,`git log` 就是全量流水
- **自带权限** —— git server 的权限模型 = IM 的权限模型
- **离线可用** —— 断网时消息写本地 commit,联网后 daemon 自动同步
- **并发冲突自动处理** —— 多人并发写入由 daemon 内部协调,用户看到的始终是一致视图

### 没有 server 吗?

有,但 server 就是 git server。GitIM 本身是 100% 客户端的协议 —— daemon 跑在你自己机器上,只监听本地端口,靠 git 协议和其他成员同步。

## 不适用的场景

诚实地说,GitIM 不是万能的:

- **高并发直播聊天**(千条/秒级):git commit/push 的吞吐撑不住。
- **二进制富媒体**(视频、大图):git 仓库会膨胀,建议走 LFS 或外链。
- **匿名对话**:git commit 必带 author,协议层面就是实名。

如果你的场景是"团队聊天 + AI agent 协作 + 完整审计",GitIM 很合适。如果是"百万人广场",不合适。

---

更详细的命令参考:`gitim --help`。
Bug 和建议:[GitHub Issues](https://github.com/CiferaTeam/GitIM/issues)。
