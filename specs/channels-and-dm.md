# 频道与私信模块

> GitIM v0.1 Schema

---

## 频道

### 文件

```
channels/<channel_name>.meta.yaml    # 元信息
channels/<channel_name>.thread       # 消息文件
```

### 频道命名规则

| 属性 | 值 |
|------|------|
| 字符集 | 小写字母 `a-z`、数字 `0-9`、连字符 `-` |
| 长度 | 1–32 个字符 |
| 模式 | `^[a-z0-9]+(-[a-z0-9]+)*$` |
| 限制 | MUST NOT 以连字符开头或结尾；MUST NOT 包含连续连字符 |

---

## 私信（DM）

### 文件

```
dm/<handler1>--<handler2>.meta.yaml
dm/<handler1>--<handler2>.thread
```

两个 handler 按字典序（逐字符 ASCII 值比较）排列，以 `--`（双连字符）连接。

使用双连字符是因为 handler 本身可以包含单连字符（如 `cifera-nexus`），但 MUST NOT 包含连续连字符，因此 `--` 作为分隔符不会产生歧义。

### 排序示例

| handler A | handler B | 文件名 |
|-----------|-----------|--------|
| `lewis` | `nexus` | `dm/lewis--nexus` |
| `cifera-nexus` | `lewis` | `dm/cifera-nexus--lewis` |
| `alice` | `alice2` | `dm/alice--alice2` |

---

## 共用 Meta Schema

频道和私信共用同一 schema：

```json
{
  "display_name": "综合频道",
  "created_by": "nexus",
  "created_at": "20250316T120000Z",
  "introduction": "团队日常沟通频道"
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `display_name` | string | MUST | 显示名称，1-64 字符 |
| `created_by` | string | MUST | 创建者 handler |
| `created_at` | string | MUST | 创建时间，UTC `YYYYMMDDTHHmmssZ` |
| `introduction` | string | MUST | 频道/会话简介，1-500 字符 |

---

## 设计决策

- **频道和 DM 共用 meta schema**：结构一致，简化实现。两者的区别仅在文件路径和命名规则。
- **DM 用 `--` 分隔而非目录**：扁平结构更简单，handler 命名规则保证 `--` 无歧义。
- **字典序排列 DM 文件名**：确保同一对用户的 DM 始终指向同一文件，无需查找。
- **频道目录必需、DM 目录可选**：最小化初始结构，DM 按需创建。

## 涉及源文件

| 文件 | 职责 |
|------|------|
| `crates/gitim-core/src/types/meta.rs` | ChannelMeta 类型（频道和 DM 共用） |
| `crates/gitim-core/src/dm.rs` | `dm_filename()` / `parse_dm_filename()` |
| `crates/gitim-core/src/validator/mod.rs` | `validate_channel_meta()` / `validate_channel_name()` |
| `crates/gitim-daemon/src/handlers.rs` | DM 路由（`dm:handler1,handler2` 格式） |
