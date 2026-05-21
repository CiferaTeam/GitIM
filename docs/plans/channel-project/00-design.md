# Channel Project Grouping (v1) — Design

Status: draft, ready for plan-eng-review
Slug: `channel-project`
Last brainstorm: 2026-05-21

## 问题

Channel 数量长大后,sidebar 列表平铺难以管理。需要在 channel 上面加一层"项目"(project)的归属,让相关 channel 能聚到一个 project 文件夹下。本特性增量上线 —— 已经存在的 channel 默认不在任何 project 里,后续可选择性加入。

## 心智模型

- **Channel** = 一次对话/一个话题
- **Project** = channel 的集合,**只承担管理(grouping)语义**,不参与 routing、不参与 permission gating、不影响 search/index/flows/cards lifecycle
- Project 是 workspace-scoped 的一等公民,但功能面薄到极致 —— v1 只能 create + 把 channel 装进/拿出

## 1. Data model (git-native)

### 1.1 `ChannelMeta` 加 optional `project` 字段

```rust
// crates/gitim-core/src/types/meta.rs
pub struct ChannelMeta {
    pub display_name: String,
    pub created_by: String,
    pub created_at: String,
    pub introduction: String,
    #[serde(default)]
    pub members: Vec<String>,
    /// 所属 project slug。None = 不在任何 project 下。
    /// 旧 channel meta 缺省 → None,backward-compat。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}
```

### 1.2 `ProjectMeta` 新类型

```rust
// crates/gitim-core/src/types/meta.rs
pub struct ProjectMeta {
    pub display_name: String,
    pub created_by: String,
    pub created_at: String,  // RFC 3339
    pub introduction: String,
}
```

### 1.3 Filesystem layout

```
<repo>/
├── channels/
│   └── <ch>/
│       ├── meta.yaml         # 加 project: <slug> 字段(optional)
│       └── ...
└── projects/                 # 新顶层目录
    └── <slug>/
        └── meta.yaml         # ProjectMeta
```

### 1.4 Slug 规则

跟 channel slug 对齐:
- 小写 `a-z 0-9 -`,1–39 字符
- 不能纯数字
- 含大写字母 → 拒绝(不做 normalize,直接 reject)
- Reserved: 取现有 channel reserved 列表(`gitim-core::validator` 已定义)+ 追加 `projects` 一项,作为 project slug 共用 reserved set
- 校验函数复用 channel slug 校验路径(`gitim-core::validator`),实施时实测它是 channel-only 还是 generic slug;若 channel-only 则抽出 `is_valid_slug(s, reserved: &[&str])` 通用版,channel 校验改走通用版 + channel-specific reserved set

## 2. Mutation surface (v1)

- `CreateProject { slug, display_name, introduction }` —— 写 `projects/<slug>/meta.yaml` + 单次 commit
- `SetChannelProject { channel, project: Option<String> }` —— 改 `channels/<ch>/meta.yaml` 的 `project` 字段 + 单次 commit;`None` / `Some("X")` 走同一接口,assign / unassign / reassign 都靠这一个

v1 **不做**:
- project rename(display_name 改动)
- project archive / unarchive
- project edit introduction
- project hard delete
- per-channel "project label only" (没有就是没有)

## 3. Validation

- `SetChannelProject { project: Some("X"), .. }` 校验:`projects/X/meta.yaml` 必须存在,否则 daemon 返 `error_code: project_not_found`
- `CreateProject` 校验:slug 合法 + slug 不冲突(已存在 → `error_code: project_exists`)
- channel.project 字段反向跟踪 unused project:**不做** —— project 可以是空的(只是 sidebar 不显示而已)
- channel archive 时:`project` 字段原样跟 meta 进 `archive/channels/<ch>/`,**不做 special handling**
- channel unarchive:`project` 字段原样回来。如果指向的 project 已被(未来的 v2)archive,这是 v2 关心的问题,v1 不会有

## 4. Permission (workspace-flat)

跟现有 channel mutation 一致:
- 任何 workspace member(`users/<handler>.meta.yaml` 存在)都能 `CreateProject`
- 任何 workspace member 都能 `SetChannelProject`(无 channel 群主 / project 群主 gate)
- daemon 的 write-guard:check `author handler ∈ users/`(已有路径,不动)

理由:project 是管理工具,过度 gating 反而降低使用率;workspace 已经是 trust boundary,内部不再 gate。

## 5. Routing / Archive / Flows / Cards / Index 边界确认

- **Routing v1 (recipients)**:不受 project 影响。`gitim-core::recipients` 的 input/output 不动。
- **Cards-follow-channel-archive**:不受影响。cards 跟 channel 走,channel 跟 project 走是 orthogonal。
- **Flows**:不受影响。flow 是 channel-orthogonal,不绑 project。
- **gitim-index (FTS5)**:不动 schema。v1 不加 by-project filter (YAGNI)。
- **Agent routing v1 recipients**:不动。
- **Agent provision / preflight**:不动。
- **Runtime CLI** (`gitim-runtime` subcommand):不动 (project mutation 走 daemon,不是 runtime workspace 概念)。

## 6. UI: Sidebar

### 6.1 视觉模型

- ⭐ asterisk = channel
- 📁 folder = project
- Channel 和 project 在同一层 mixed sort (用户的核心偏好)
- Project 文件夹 collapsible,展开后是它的成员 channel
- **空 project (无成员 channel) 隐式不显示** —— 渲染时 reduce `channels.filter(c => c.project === slug).length > 0`

### 6.2 排序

- 平级排序:每一项要么是 unassigned channel,要么是 non-empty project
- 默认排序:`pinned 在前(localStorage)` → 字典序
- Project 折叠状态 by default = collapsed (减少视觉噪音);用户展开后 localStorage 持久化

### 6.3 Pinned

- **Pinned 沿用现有 `gitim-pinned-conversations:<workspace>` localStorage**,新增 entry 类型
  - 当前 schema:`{ channels: string[], dms: string[] }`
  - 新增:`projects: string[]`
- **不写 git** —— 跟 channel pinned 一致,是 personal preference
- Project pin/unpin 操作通过 sidebar 上的 hover icon 或 context menu

## 7. UI: Cards

### 7.1 Filter bar 加 project filter

- Filter bar 加单选 dropdown:`All | Unassigned | <project A> | <project B> | ...`
- 选 project 后,channel filter 自动 derived(scope 到该 project 下的 channel)
- Kanban 列保持 todo / doing / done 不变

### 7.2 URL 参数

- 加 `project=<slug>` query param,跟现有 `channel=` / `label=` / `assignee=__me__` 并列
- `project=__unassigned__` 表示 "no project"
- `writeFilterToURL` / `readFilterFromURL` 同步更新

### 7.3 v1 不做

- per-project kanban section(像 sidebar 那样的视觉划分)
- project breadcrumb / header banner
- cards 排序按 project 分组(简单 filter 已经能达到"划分"效果)

## 8. CLI surface

新增子命令:
- `gitim projects list` —— 列 workspace 所有 project,展示 slug / display_name / channel 计数
- `gitim projects create <slug> --name "..." --intro "..."` —— 创建 project
- `gitim channel set-project <ch> <project_slug>` —— 把 channel 划进 project
- `gitim channel set-project <ch> --clear` —— 从 project 拿出来

不做:
- `gitim projects archive / rename / edit` (v2+)
- `gitim projects show <slug>` (`list` 已经足够)

## 9. Daemon API surface

### 9.1 新增 IPC methods

- `ListProjects` → `Vec<{ slug, meta: ProjectMeta, channel_count: usize }>`
- `CreateProject { slug, display_name, introduction }` → `()`
- `SetChannelProject { channel, project: Option<String> }` → `()`

### 9.2 现有 method response 扩展

- `ListChannels` / `ReadChannelMeta` response 的 `ChannelMeta` 暴露 `project: Option<String>` (新字段,backward-compat 通过 `Option<String>` + `skip_serializing_if`)

### 9.3 HTTP gateway (SSE / Runtime / WebUI)

- `GET /im/projects` → 同 ListProjects
- `POST /im/projects` → 同 CreateProject
- `PATCH /im/channels/{ch}/project` body `{ project: Option<String> }` → 同 SetChannelProject

### 9.4 SSE / push events

实施时对齐现有 SSE event 命名 convention(待 plan-eng-review 中确认现有 convention):
- `project_created { slug, meta }` 或 `projects_changed { added: [...], removed: [] }` —— Watcher 检测到 `projects/<slug>/meta.yaml` 新增时推
- channel.project 变更:复用现有 channel meta update 通道 (`channel_meta_updated` 或等价 event),不另开新 event 类型

## 10. Migration & backward-compat

- 旧 channel meta 没有 `project` 字段 → serde `#[serde(default)]` → `None`
- 旧 daemon 读到 channel meta 的 `project` 字段:serde unknown field 在 strict mode 会报错,**但 gitim 现在的 ChannelMeta serde 未开 deny_unknown_fields,所以会被 silently ignore** —— ✓ 安全
- 老客户端连新 daemon:`ListChannels` response 多了 `project` 字段,老 client 忽略它 → ✓
- `projects/` 目录在首次 `CreateProject` 时由 daemon mkdir + commit(`system@gitim` author)

## 11. Test plan

按 TDD,以下 unit + 集成测试:

### 11.1 `gitim-core`
- `ChannelMeta` 序列化:有/无 `project` 字段都能 round-trip
- `ProjectMeta` 序列化:基本 round-trip + 缺字段时反序列化失败 (`display_name` 必填等)
- Slug 校验复用 channel slug 路径

### 11.2 `gitim-daemon`
- `CreateProject` happy path:写 `projects/<slug>/meta.yaml` + 1 commit
- `CreateProject` 重复:返 `project_exists`
- `CreateProject` 非法 slug / reserved slug:返 `invalid_slug` / `reserved_slug`
- `SetChannelProject(Some)` 指向不存在 project:返 `project_not_found`,channel meta 不变
- `SetChannelProject(Some)` happy path:channel.meta.project 变成 Some(X) + 1 commit
- `SetChannelProject(None)` happy path:从 Some(X) 变 None + 1 commit
- `SetChannelProject` reassign:从 Some(X) 变 Some(Y) + 1 commit
- `ListProjects` 返回 channel_count 正确(空 project = 0)
- Channel archive 时 `project` 字段保留
- Channel unarchive 时 `project` 字段保留

### 11.3 `gitim-cli`
- `gitim projects list` / `create` argv 解析 + happy path 通 daemon IPC
- `gitim channel set-project <ch> <slug>` / `--clear` argv 解析 + happy path

### 11.4 `gitim-runtime` HTTP gateway
- `GET /im/projects` / `POST /im/projects` / `PATCH /im/channels/{ch}/project`
- write-guard 触发 (departed user 不能 mutate)

### 11.5 `gitim-frontend`
- Sidebar 平级 sort 算法 (mixed channel + project)
- 空 project 隐式不显示
- Pin/unpin project (localStorage)
- Cards filter bar project dropdown
- URL param `project=` round-trip

## 12. v2+ out-of-scope (锁定不做)

- Project rename / archive / edit introduction
- Project owner / members 概念 (permission 更细粒度)
- Per-project kanban section (cards 视图视觉划分)
- Search by project filter (gitim-index)
- Project pinned 写 git (跨设备同步偏好)
- Project 嵌套 (project 包 project) —— 永远不做
- 多归属 (channel.projects: Vec) —— 永远不做 (这次锁的是 0..1)
- Project 自己有 thread/messages —— 永远不做

## 13. Open question

无 —— 全部锁定后进 plan-eng-review。

---

## Appendix A: 关键决策溯源

| 决策 | 选择 | 理由 |
|------|------|------|
| Project 载体 | channel.meta 加 `project` 字段 + `projects/<slug>/meta.yaml` 独立 | 增量最干净;旧 channel 不动 |
| 归属基数 | 0..1 | 简单清晰;UI sidebar 干净;v2 想加 0..N 可以加字段 backward-compat |
| ProjectMeta 字段 | display_name / created_by / created_at / introduction | 对齐 ChannelMeta 心智;color/icon/members YAGNI |
| Mutation v1 | create project + set channel.project | 锁到"用户最低能用起来"的 set |
| Permission | workspace-flat (任何 member) | 跟现有 channel mutation 一致;trust boundary 是 workspace |
| Sidebar | 平级 mixed sort,空 project 隐式不显示 | 用户偏好;符合"project 跟 channel 平级"的心智 |
| Pinned | 沿用现有 localStorage | 对齐现有 channel pin,personal preference 不入 git |
| Cards 视图 | filter bar 加 project filter | 最小复用现有 kanban,YAGNI |
| Routing | project 不影响 recipients | project 是管理层,不动行为 |
