# Card-Chat 联动优化设计

> **问题域**：AI 在 Channel 聊天中提到 Card 时，用户无法直观识别 Card 内容，也无法快速跳转和切回。
>
> **目标**：让 Card 在 Chat 中有可读标识、可点击跳转、可快速切回，消除割裂感。

## 问题诊断

### 1. Card 名字是乱码，不可索引

- Card 的 `card_id` 是机器生成的标识（如 `20260520-035646-7cf`），人类无法通过 ID 知道内容
- Card 详情页 breadcrumb 显示 `#{channel} / {cardId}`，以乱码 ID 为主标识
- AI 在消息中引用 Card 时只能用反引号包裹 ID（如 `card \`20260520-035646-7cf\``），纯文本不可点击

### 2. 无法快速跳转和切回

- **Chat → Card**：没有可点击的 Card 链接。AI 说"我在 card xxx 写了..."，用户只能手动打开 Card Drawer 逐个找
- **Card → Chat**：Card Detail 的 Back 按钮语义模糊（`navigate(-1)`），没有明确"回到 #{channel}"的指向
- 消息协议中**已定义** `<#channel/card-id>` 语法（见 `awesome-agents-team-0519.thread` L164），但前后端**均未实现**

### 3. 根本原因

| 层面 | 问题 |
|------|------|
| 协议 | GitIM 消息语法已定义 `<#channel/card-id>` 卡片链接，但后端 `LinkKind` 无 `Card` 变体，`extract_links()` 不解析该格式 |
| 前端 | `message-parser.ts` 只解析 `<#channel>` 和 `<#channel:LNNNNNN>`，无 `card-link` fragment 类型 |
| UI | Card Detail 未利用 `card.title` 作为 primary identifier，返回导航无 channel 语义 |

## 设计方案

### 原则

1. **最小改动优先**：先纯前端改动解决 80% 痛点，后端协议升级作为增强
2. **渐进增强**：不改动 Card ID 生成方式（避免数据迁移），通过 UI 和渲染层解决问题
3. **向后兼容**：新语法不影响旧消息解析

---

### Phase 1: 纯前端改动（高优先级，立即实施）

#### 1.1 消息中增加 Card 链接解析

在前端 `message-parser.ts` 中新增 `card-link` fragment 类型，支持两种格式：

- `<#channel/card-id>` — 裸 Card 链接（点击后跳转到 Card Detail）
- `<#channel/card-id|Card标题>` — 带显示标题的 Card 链接（更 human-readable）

**语法规则**：
```
<#channel-name/card-id>           → 显示 card-id，点击跳转
<#channel-name/card-id|显示文本>   → 显示"显示文本"，点击跳转
```

- `channel-name` 需通过 `isValidChannel` 校验
- `card-id` 格式：非空、不含 `/` 和 `>`、不含换行
- `|` 后的显示文本可选

**message-parser.ts 改动**：
1. `Fragment` 类型新增 `card-link` 变体：`{ type: "card-link"; channel: string; cardId: string; title?: string }`
2. `parseGitimLink` 中处理 `#` 前缀时，先检查 `channel/card-id` 格式（含 `/`），再回退到 `channel:LNNNNNN` 和 `channel`
3. `INLINE_RE` 无需改动，仍匹配 `<([#~!])([^>\n]+)>`

**message-body.tsx 改动**：
1. `FragmentRenderer` 新增 `card-link` case
2. 渲染为可点击的按钮/链接样式（类似 channel-link 但用不同图标，如 `LayoutGrid` 或 `Square`）
3. 点击后 `navigate(/cards/${channel}/${cardId})`

#### 1.2 自动识别消息中的 Card ID（防旧消息失效）

现有消息中大量 AI 用反引号包裹 Card ID（如 `card \`20260520-035646-7cf\``）。Phase 1 中，前端在渲染消息时**自动识别**这种模式，将其渲染为可点击的 Card 链接。

**识别规则**（宽松匹配）：
- 模式：`card \`{card-id}\`` 或 `card \`{channel}/{card-id}\`` 或 `\`{card-id}\``（当上下文在 channel 中时）
- 优先尝试在**当前 channel** 下查找该 card_id，如果存在则渲染为链接
- 如果 card_id 不在当前 channel，或 store 中无该 card，则保持原样（纯文本）

**实现方式**：在 `message-body.tsx` 中，对 `text` fragment 做二次扫描，识别 Card ID 模式后转换为可点击元素。这需要读取 `useCardStore` 中的卡片列表进行查找。

> **注意**：自动识别是 best-effort 的启发式，不保证 100% 准确。未来 AI 应尽量使用 `<#channel/card-id>` 语法。

#### 1.3 Card Detail 页 UI 优化

**Breadcrumb 改造**（`card-detail.tsx`）：
- 当前：`#{channel} / <font-mono>{cardId}</font-mono>`
- 改为：`#{channel} / {card.title}`（title 为主标识）
- cardId 降级到 meta bar 或更小字体显示：`card id {cardId}`（已有，保持不变）

**返回按钮语义化**（`card-detail.tsx`）：
- 当前：`<ArrowLeft /> Back`
- 改为：`<ArrowLeft /> #{channel}`（明确告诉用户回到哪个 channel）
- 保留 `navigate(-1)` 的行为逻辑，但 UI 标签显示 channel 名
- 如果 `navigate(-1)` 无法回到 channel（如从书签直接打开），则显式 `navigate(/chat/${channel})`

**新增"快速切回"按钮**（可选增强）：
- 在 Card Detail 顶部增加一个小按钮或面包屑链接：`#{channel}` 可点击直接跳转回 channel
- 位置：Back 按钮旁边或 breadcrumb 中

---

### Phase 2: 后端协议增强（中等优先级）

#### 2.1 后端新增 Card Link 类型

在 Rust 后端增加对 `<#channel/card-id>` 的解析，使链接在 API 响应中以结构化数据返回。

**改动点**：

1. `crates/gitim-core/src/types/link.rs`：
   ```rust
   pub enum LinkKind {
       Channel { name: String },
       Message { channel: String, line_number: u64 },
       Card { channel: String, card_id: String },  // ← 新增
       UserProfile { handler: Handler },
       Softlink { url: String, title: Option<String> },
   }
   ```

2. `crates/gitim-core/src/link.rs`：
   - 新增 `CARD_LINK_RE` 或修改 `parse_channel_or_message` 函数
   - 识别 `channel/card-id` 格式（含 `/` 但不含 `:L`）

3. `crates/gitim-daemon/src/handlers.rs`：
   - `link_to_json()` 新增 `Card` 分支，序列化为 `"kind": "card"`

**语法规则**：
```
<#channel/card-id>           → Card { channel, card_id }
<#channel/card-id|显示文本>  → Card { channel, card_id }（pipe 后的文本由前端渲染决定，后端不存储）
```

> 关于 pipe 语法：后端 `extract_links` 只提取结构化数据，pipe 后的显示文本可以忽略（前端自行从 title 字段查找），也可以作为 `title` 字段存储。考虑到简单性，后端仅解析 `channel/card-id` 部分，pipe 文本留给前端处理。

#### 2.2 前端适配后端 API

如果后端返回 `kind: "card"` 的 links，前端在 `MessageBody` 中渲染对应的 Card 链接。

---

### Phase 3: AI 输出规范（低优先级，文档+提示）

#### 3.1 更新 AGENTS.md 或系统提示

在 agent 的系统提示中增加：**引用 Card 时，请使用 `<#channel/card-id>` 格式，而非反引号**。例如：
- 不好：`我在 card \`20260520-035646-7cf\` 里写了...`
- 好：`我在 <#general/20260520-035646-7cf> 里写了...`
- 更好：`我在 <#general/20260520-035646-7cf|Token Rotation 规则> 里写了...`

#### 3.2 在 Card 创建时自动告知 AI 格式

当 agent 创建 Card 后，返回的 Card 信息应包含可点击链接格式，方便 agent 在后续消息中引用。

---

## 实施计划

| 阶段 | 任务 | 改动文件 | 预估复杂度 |
|------|------|----------|-----------|
| 1.1 | 前端 message-parser 增加 card-link | `lib/message-parser.ts` | 低 |
| 1.1 | 前端 message-body 渲染 card-link | `components/chat/message-body.tsx` | 低 |
| 1.2 | 自动识别反引号 Card ID（best-effort） | `components/chat/message-body.tsx` | 中 |
| 1.3 | Card Detail breadcrumb 优化 | `components/cards/card-detail.tsx` | 低 |
| 1.3 | Card Detail 返回按钮语义化 | `components/cards/card-detail.tsx` | 低 |
| 2.1 | 后端 LinkKind 增加 Card 变体 | `crates/gitim-core/src/types/link.rs` | 低 |
| 2.1 | 后端 extract_links 解析 card link | `crates/gitim-core/src/link.rs` | 低 |
| 2.1 | 后端 handlers.rs 序列化 card link | `crates/gitim-daemon/src/handlers.rs` | 低 |
| 2.2 | 前端适配后端 card link API | `components/chat/message-body.tsx` | 低 |

**推荐实施顺序**：
1. 先实施 Phase 1.3（UI 优化）— 立即可见的效果
2. 再实施 Phase 1.1（card-link 解析）— 让新消息可点击
3. 再实施 Phase 1.2（自动识别）— 补救旧消息
4. 后端协议（Phase 2）可延后，前端独立工作即可

## 预期效果

### Before
```
AI: 已记录。OpenCode PR card `20260520-035646-7cf` 已按这个规则更新...
```
- 用户看到 `20260520-035646-7cf` 完全不知道是什么
- 手动打开 Card Drawer → 滚动查找 → 点击进入 → 看完点 Back → 不知道回到哪

### After
```
AI: 已记录。OpenCode PR card <#general/20260520-035646-7cf|Token Rotation 规则> 已按这个规则更新...
```
- 用户看到"Token Rotation 规则"，一眼知道是什么
- 点击直接跳转到 Card Detail
- Card Detail 顶部显示：`← #general` / `Token Rotation 规则`
- 点击 `#general` 直接回到 channel，点击 `←` 也可以返回
- 旧消息中的 `card \`20260520-035646-7cf\`` 也被自动高亮为可点击链接

## 测试策略

1. **message-parser 单元测试**：测试 `<#general/abc123>`、`<#general/abc123|Title>`、边界 case（空 card_id、含 `>`、含换行）
2. **message-body 渲染测试**：确认 card-link 渲染为正确 DOM 元素，点击触发 navigate
3. **Card Detail 测试**：确认 breadcrumb 显示 title，返回按钮显示 channel 名
4. **端到端**：发送含 `<#channel/card-id>` 的消息，确认读取后 links 数组包含 card 类型（后端完成后）
