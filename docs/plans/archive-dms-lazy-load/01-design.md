# Archive DMs Lazy Load + Prefix Search — Design

Status: Approved, awaiting implementation plan.
Date: 2026-05-13.

## Why

WebUI sidebar 目前在 app 启动时 eager 拉全量 archive DM (`app.tsx:302` →
`GET /im/dm/archived`)。daemon (`read.rs:275-311`) 扫整个 `archive/dm/` 目录全量返回。
当一个 workspace 累积到几万、几十万归档 DM 时,启动延迟和 payload 都不可接受。

目标:展开 section 时才按页拉 5 条,顶部提供按 peer handler 的前缀搜索,
搜索结果走服务端,不依赖前端已加载到第几页。

## API

`GET /im/dm/archived?prefix=&offset=0&limit=5`

| 参数     | 类型   | Default | 说明                                          |
|---------|--------|---------|----------------------------------------------|
| prefix  | string | `""`    | 大小写不敏感前缀,匹配 **peer handler**;空 = 不过滤 |
| offset  | usize  | `0`     | 跳过前 N 条                                    |
| limit   | usize  | `5`     | 取多少条,上限 100                              |

Response:

```json
{ "dms": [{"peer": "alice", "dm_pair_stem": "alice--bob"}, ...], "has_more": true }
```

`has_more` 用 peek-N+1:daemon 实际 `take(limit + 1)`,若 N+1 条存在则
`has_more = true`,response 只返 N 条。不暴露 `total`(数全量目录代价大且没人需要)。

排序:peer handler 字典序升序。stable,readable。

## Daemon (`crates/gitim-daemon/src/handlers/read.rs:275-311`)

`handle_list_archived_dms` 改造步骤:

1. 接 `ListArchivedDmsRequest { prefix: Option<String>, offset: usize, limit: usize }`
2. readdir `archive/dm/`,filter `*.thread` 且 `parse_dm_filename` 成功
3. filter self 参与的对,计算 peer
4. 若 `prefix.is_some()` 且非空:`peer.to_ascii_lowercase().starts_with(&prefix.to_ascii_lowercase())`
5. `sort_unstable_by` peer 升序
6. `skip(offset).take(limit + 1)` → `has_more = collected.len() > limit`,截到 limit
7. 返回 `ListArchivedDmsResponse { dms, has_more }`

`gitim-core` 端:
- `ListArchivedDmsResponse` 新增 `has_more: bool` 字段
- 请求类型新增 `ListArchivedDmsRequest`,IPC 协议同步更新
- 旧 `ArchivedDmEntry` shape 不变

## Runtime (`crates/gitim-runtime/src/http.rs:1172-1181`)

`im_list_archived_dms` 透传新参数到 daemon。route 仍是 `GET /im/dm/archived`,
新增 query string 解析(`prefix` / `offset` / `limit`)。

## Frontend Store (`products/gitim/frontend/src/hooks/use-chat-store.ts`)

```ts
type ArchivedDmsView = {
  items: ArchivedDmEntry[];
  offset: number;       // 已加载到的边界(= items.length)
  hasMore: boolean;
  query: string;        // 当前生效的搜索串(经过 debounce 后)
  loading: boolean;
  error: string | null;
};

archivedDmsView: ArchivedDmsView | null  // null = 从未初始化
```

Actions:

- `resetArchivedDmsView(query: string)` — 清空 items / offset=0 / hasMore=true,query 写入
- `appendArchivedDmsPage({items, hasMore})` — items 追加,offset 增加,hasMore 覆写
- `setArchivedDmsLoading(boolean)` / `setArchivedDmsError(string | null)`

旧的 `archivedDms: Channel[]` 字段及 `setArchivedDms()` 删除。

## Frontend Sidebar (`products/gitim/frontend/src/components/chat/sidebar.tsx:773-842`)

- section 标题去掉 count badge,只显示 "ARCHIVED DMS"
- 展开切换(`archivedDmsOpen` false → true)时:若 view===null,触发拉首页
  `(prefix="", offset=0, limit=5)`
- section 顶部加 `<input>`(placeholder "Filter by handle..."),onChange →
  debounce 300ms → `resetArchivedDmsView(newQuery)` + 拉首页
- 列表底部:`hasMore` 时显示 "Load more" button → 用当前 query + 新 offset 追加
- collapse 时不清空 view(下次展开复用缓存)
- 空结果:items=[] + hasMore=false → 渲染 "No archived DMs"(query 为空时)
  或 "No matches"(query 非空时)

## Eager Fetch 移除

删除 `products/gitim/frontend/src/app.tsx:302` 处的 `client.listArchivedDms(slug)`
及 `:426` 处对应的 `setArchivedDms` 调用。

## Client (`products/gitim/frontend/src/lib/client.ts:1123-1133`)

`listArchivedDms` 改签名:

```ts
listArchivedDms(slug: string, opts?: {
  prefix?: string;
  offset?: number;
  limit?: number;
}): Promise<{ dms: ArchivedDmEntry[]; hasMore: boolean }>
```

构造 query string,GET 同一 endpoint,解析 `has_more` → `hasMore`。

## Archive / Unarchive 钩子

当前架构:archive / unarchive DM 走 mutation API,SSE 广播 event。

新架构下两条路径都触发 view 失效:

- 显式 mutation 成功 → 若 view 已初始化 → `resetArchivedDmsView(currentQuery)`;
  若 section 当时展开 → 自动拉首页;若 collapsed → view 设回 null,下次展开再拉。
- SSE archive/unarchive event handler 走同一 reset path,保证多端一致。

## 错误与边界

- empty 结果:见上(sidebar 文案)
- network error:展示一行 error + "Retry" 按钮,onClick 重发同一请求
- query 切换 race:debounce 触发新请求前,用 query 字段对比 stale response(收到的
  response 若 query 与 store.query 不一致 → 丢弃)
- readdir 失败:daemon 500;前端走 error path

## 不在 Scope

- 普通(active)DM 列表 lazy 化
- archived **频道** section 的 lazy 化(同 §API 模式可后续套用)
- DM 内容全文搜索(由 FTS5 处理,与本改动无关)
- 索引化 / FTS 升级(YAGNI;真到百万级再做)

## Risk

- readdir + sort 在百万级目录单次几百 ms 到秒级:本设计 cap 在"几十万级 acceptable",
  与用户的 scope 边界(eager 全量已经撑不住才改) 对齐。百万级触发 → 升级到 Approach B
  (in-memory BTreeMap 索引,sync_loop 维护)。
- archive/unarchive 频繁(SSE 推送密集时)→ view reset 风暴。若 section 展开,首页
  请求会被频繁触发。debounce reset (例如 500ms) 可以缓解,但 v1 不做,先观察。
