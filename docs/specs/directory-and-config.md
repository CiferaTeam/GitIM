# 目录结构与全局配置

> GitIM 当前实现（工作副本结构）

---

## 目录结构

当前实现区分两类数据：

- **协议数据**：提交到 Git 仓库，供所有副本共享。
- **本地控制数据**：放在 `.gitim/`，每个工作副本独立维护，默认整体忽略。

```text
<working_copy>/
├── .gitim/                      # 本地控制目录，onboard 创建，默认整体 git ignore
│   ├── config.yaml              # 本地 daemon 配置
│   ├── me.json                  # 当前工作副本身份
│   ├── index.db                 # 本地 SQLite 搜索索引
│   └── run/                     # 运行时文件
│       ├── gitim.pid
│       ├── gitim.sock
│       ├── gitim.port
│       └── gitim.lock
├── users/
│   └── <handler>.meta.yaml
├── channels/
│   ├── <channel_name>.thread
│   └── <channel_name>.meta.yaml
└── dm/
    └── <handler1>--<handler2>.thread
```

### 协议数据

| 路径 | 类型 | 说明 |
|------|------|------|
| `users/` | 目录 | 用户元信息 |
| `channels/` | 目录 | 频道消息和频道元信息 |
| `dm/` | 目录 | 私信线程文件（按需出现） |

### 本地控制数据

| 路径 | 类型 | 说明 |
|------|------|------|
| `.gitim/` | 目录 | 本地控制根目录 |
| `.gitim/config.yaml` | 文件 | 本地 daemon 配置 |
| `.gitim/me.json` | 文件 | 当前工作副本身份 |
| `.gitim/index.db` | 文件 | 本地搜索索引 |
| `.gitim/run/` | 目录 | daemon 运行时文件 |

---

## .gitignore

当前实现会确保工作副本的 `.gitignore` 至少包含：

```text
.gitim/
```

这意味着 `config.yaml`、`me.json`、索引文件和运行时文件都不提交到远端仓库。

---

## 本地配置 `config.yaml`

### Schema

```yaml
version: 1
endpoint: github
endpoint_url: ""
daemon:
  sync_interval: 1
  debug_http: false
  debug_port: 3000
```

| 字段 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `version` | integer | MUST | — | 当前 MUST 为 `1` |
| `endpoint` | string | MAY | `"github"` | 预留的平台类型字段 |
| `endpoint_url` | string | MAY | `""` | 预留的平台 URL 字段 |
| `daemon.sync_interval` | integer | MAY | `1` | 后台 sync loop 间隔秒数；`0` 表示禁用定时同步 |
| `daemon.debug_http` | boolean | MAY | `false` | 是否开启 HTTP 调试端口 |
| `daemon.debug_port` | integer | MAY | `3000` | HTTP 调试端口 |

最小有效配置：

```yaml
version: 1
```

省略字段时使用默认值。

### 说明

- `endpoint` / `endpoint_url` 当前主要是保留字段；运行期身份来源是 `.gitim/me.json`。
- `.gitim/config.yaml` 由 daemon 在本地创建或加载，不要求提交到仓库。

---

## 设计决策

- **`.gitim/` 整体本地化**：身份、索引、运行时状态都与单个工作副本绑定。
- **协议数据保持纯文本**：共享数据仍然是 `users/`、`channels/`、`dm/` 下的纯文本文件。
- **配置用 YAML**：便于人工阅读和后续扩展。

## 涉及源文件

| 文件 | 职责 |
|------|------|
| `crates/gitim-core/src/types/config.rs` | Config / DaemonConfig 类型定义与默认值 |
| `crates/gitim-core/src/validator/mod.rs` | `validate_config()` 配置校验 |
| `crates/gitim-daemon/src/main.rs` | 启动时加载或创建 `.gitim/config.yaml` |
| `crates/gitim-daemon/src/onboard.rs` | 写入 `.gitim/me.json` |
| `crates/gitim-daemon/src/state.rs` | 初始化本地搜索索引 |
