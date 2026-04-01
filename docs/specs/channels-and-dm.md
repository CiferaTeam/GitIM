# 频道与私信模块

> GitIM 当前实现（频道/DM 语义）

---

## 频道

### 文件

```text
channels/<channel_name>.meta.yaml
channels/<channel_name>.thread
```

### 频道命名规则

| 属性 | 值 |
|------|------|
| 字符集 | 小写字母 `a-z`、数字 `0-9`、连字符 `-` |
| 长度 | 1–32 个字符 |
| 模式 | `^[a-z0-9]+(-[a-z0-9]+)*$` |
| 限制 | MUST NOT 以连字符开头或结尾；MUST NOT 包含连续连字符 |

### 频道 Meta Schema

```yaml
display_name: 综合频道
created_by: nexus
created_at: 20250316T120000Z
introduction: 团队日常沟通频道
members:
  - nexus
  - lewis
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `display_name` | string | MUST | 显示名称，1-64 字符 |
| `created_by` | string | MUST | 创建者 handler |
| `created_at` | string | MUST | 创建时间，UTC `YYYYMMDDTHHmmssZ` |
| `introduction` | string | MUST | 频道简介，1-500 字符 |
| `members` | string[] | MAY | 成员列表；为空时表示 open channel |

---

## 私信（DM）

### 文件

```text
dm/<handler1>--<handler2>.thread
```

当前实现**不单独创建或消费 DM `.meta.yaml`**。DM 参与者由文件名直接推导。

两个 handler 按字典序（逐字符 ASCII 值比较）排列，以 `--` 连接。由于 handler 本身不允许连续连字符，`--` 作为 DM 分隔符无歧义。

### 排序示例

| handler A | handler B | 文件名 |
|-----------|-----------|--------|
| `lewis` | `nexus` | `dm/lewis--nexus.thread` |
| `cifera-nexus` | `lewis` | `dm/cifera-nexus--lewis.thread` |
| `alice` | `alice2` | `dm/alice--alice2.thread` |

### API 表示

- daemon API 内部使用 `dm:handler1,handler2` 形式，例如 `dm:alice,god`
- CLI `gitim dm send/read <handler>` 会自动处理该转换

---

## 设计决策

- **频道有显式 meta，DM 没有**：频道需要成员列表和显示名；DM 参与者可由文件名唯一确定。
- **DM 用 `--` 分隔**：无需额外目录层级或独立索引文件。
- **成员列表放在频道 meta**：便于发送校验、`poll` 过滤和成员变更广播。

## 涉及源文件

| 文件 | 职责 |
|------|------|
| `crates/gitim-core/src/types/meta.rs` | `ChannelMeta` 类型 |
| `crates/gitim-core/src/dm.rs` | `dm_filename()` / `parse_dm_filename()` |
| `crates/gitim-core/src/validator/mod.rs` | `validate_channel_meta()` / `validate_channel_name()` |
| `crates/gitim-daemon/src/handlers.rs` | channel / DM 路由与成员校验 |
