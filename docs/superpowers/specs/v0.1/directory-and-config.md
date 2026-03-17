# 目录结构与全局配置

> GitIM v0.1 Schema

---

## 目录结构

```
<repo_root>/
├── .gitim/
│   ├── config.yaml                # 全局配置（必需）
│   ├── me.json                    # 当前用户身份（.gitignore，每个 clone 独立）
│   └── run/                       # 运行时文件（.gitignore）
│       ├── gitim.pid              # daemon 进程 ID
│       ├── gitim.sock             # Unix Domain Socket
│       ├── gitim.port             # HTTP 端口（仅调试模式）
│       └── gitim.lock             # 文件锁，防止重复启动
├── users/                         # 用户目录（必需）
│   └── <handler>.meta.json
├── channels/                      # 公共频道（必需）
│   ├── <channel_name>.thread
│   └── <channel_name>.meta.json
└── dm/                            # 私信（可选）
    ├── <handler1>--<handler2>.thread
    └── <handler1>--<handler2>.meta.json
```

### 必需结构

| 路径 | 类型 | 说明 |
|------|------|------|
| `.gitim/` | 目录 | 配置根目录 |
| `.gitim/config.yaml` | 文件 | 实例配置 |
| `users/` | 目录 | 用户文件目录 |
| `channels/` | 目录 | 公共频道目录（可为空） |

### 可选结构

| 路径 | 类型 | 说明 |
|------|------|------|
| `.gitim/run/` | 目录 | daemon 运行时文件 |
| `.gitim/me.json` | 文件 | 当前用户身份 |
| `dm/` | 目录 | 私信会话 |

---

## .gitignore

GitIM 仓库 MUST 包含一个 `.gitignore` 文件，至少包含：

```
.gitim/run/
.gitim/me.json
```

---

## 全局配置 config.yaml

### Schema

```yaml
version: 1
endpoint: github              # "github" 或 "gitea"
endpoint_url: ""              # 仅 gitea 时必填
daemon:
  sync_interval: 30           # git pull/push 间隔秒数，0 = 手动同步
  debug_http: false            # 是否开启 HTTP 调试端口
  debug_port: 3000             # HTTP 调试端口号
```

| 字段 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `version` | integer | MUST | — | Schema 版本，当前 MUST 为 1 |
| `endpoint` | string | MAY | `"github"` | 平台类型 |
| `endpoint_url` | string | MAY | `""` | 平台 URL（gitea 时必填） |
| `daemon.sync_interval` | integer | MAY | 30 | git pull/push 间隔秒数 |
| `daemon.debug_http` | boolean | MAY | false | 是否开启 HTTP 调试端口 |
| `daemon.debug_port` | integer | MAY | 3000 | HTTP 调试端口号 |

最小有效配置：

```yaml
version: 1
```

省略的字段 MUST 应用默认值。

---

## 设计决策

- **config.yaml 而非 JSON**：YAML 对人类可读性更好，适合手动编辑场景。
- **me.json 在 .gitignore 中**：每个 clone 副本有独立身份，不应提交到仓库。
- **run/ 目录集中运行时文件**：PID、socket、lock 放在一起，方便 cleanup 和 .gitignore。

## 涉及源文件

| 文件 | 职责 |
|------|------|
| `crates/gitim-core/src/types/config.rs` | Config / DaemonConfig 类型定义与默认值 |
| `crates/gitim-core/src/validator/mod.rs` | `validate_config()` 配置校验 |
| `crates/gitim-daemon/src/main.rs` | 启动时加载 config.yaml 和 me.json |
