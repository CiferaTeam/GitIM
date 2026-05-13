# Archive DMs Lazy Load + Prefix Search — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** archive DM 列表改成「按需 + 翻页 + peer handler 前缀搜索」,WebUI 在用户展开
sidebar section 时才拉首页 5 条,搜索结果走服务端。

**Architecture:** daemon `handle_list_archived_dms` 加 `prefix/offset/limit` 参数与
`has_more` peek;runtime `/im/dm/archived` 透传 query string;前端 store 把
`archivedDms: Channel[]` 换成 `archivedDmsView: ArchivedDmsView | null`,sidebar 上
按 expand 触发首页 + Load more button,搜索框 onChange debounce 后 reset 重新拉。

**Tech Stack:** Rust workspace (`gitim-core` / `gitim-daemon` / `gitim-client` /
`gitim-runtime`)、Vite + React 19 + TypeScript + Zustand、Vitest、Playwright。

**Spec:** `docs/plans/archive-dms-lazy-load/01-design.md`(commit `16cb59b`)。

**Testing cadence**(遵循 CLAUDE.md):
- 任务开头跑一次 `cargo test`(全量)+ `cd products/gitim/frontend && npm test`,建立 baseline
- 中间 scoped:`cargo test -p <crate>` / `npm test -- <file>`,不要每改一次就全量
- 末尾跑全量 + Playwright E2E

---

## File Map

| 文件 | 角色 |
|------|------|
| `crates/gitim-core/src/responses.rs` | `ListArchivedDmsResponse` 加 `has_more` |
| `crates/gitim-daemon/src/api.rs` | `Request::ListArchivedDms` 加 prefix/offset/limit |
| `crates/gitim-daemon/src/handlers/mod.rs` | dispatch 透传新字段 |
| `crates/gitim-daemon/src/handlers/read.rs` | 真正的过滤 + 分页 + peek 逻辑 |
| `crates/gitim-client/src/client.rs` | IPC client 方法签名扩展 |
| `crates/gitim-runtime/src/http.rs` | `im_list_archived_dms` 解析 query string |
| `products/gitim/frontend/src/lib/client.ts` | frontend HTTP wrapper 改签名 |
| `products/gitim/frontend/src/daemon-web/handlers.ts` | 浏览器 fallback 适配新参数 |
| `products/gitim/frontend/src/daemon-web/worker.ts` | worker route 透传 |
| `products/gitim/frontend/src/hooks/use-chat-store.ts` | `archivedDms` → `archivedDmsView` |
| `products/gitim/frontend/src/hooks/use-chat-store.test.ts` | store actions 测试 |
| `products/gitim/frontend/src/app.tsx` | 删 eager fetch 与所有相关 wiring |
| `products/gitim/frontend/src/components/chat/sidebar.tsx` | section UI:input + Load more + lazy |
| `products/gitim/frontend/e2e/sidebar-layout.spec.ts` | E2E 覆盖新交互(若已有相关场景) |

---

## Task 0: Baseline

**Files:** 无改动。

- [ ] **Step 1: 跑全量后端测试,确认 baseline 绿**

  Run: `cargo test`
  Expected: 全量通过(忽略 `#[ignore]` 的 claude CLI 测试)。若已有红测试,先记下,后续不混淆。

- [ ] **Step 2: 跑前端单元测试**

  Run: `cd products/gitim/frontend && npm test`
  Expected: 全部 vitest 套件通过。

- [ ] **Step 3: 不 commit。**

---

## Task 1: 扩展 `ListArchivedDmsResponse` 的 `has_more` 字段

**Files:**
- Modify: `crates/gitim-core/src/responses.rs:213-228`
- Modify: `crates/gitim-core/src/responses.rs:870-895`(wire-shape 测试)

- [ ] **Step 1: 写失败 test**

  在 responses.rs 的 `#[cfg(test)]` 块里(`list_archived_dms_response_wire_shape` 附近)
  补充测试覆盖 `has_more` 的默认值与序列化:

  ```rust
  #[test]
  fn list_archived_dms_response_has_more_field() {
      let r = ListArchivedDmsResponse {
          dms: vec![],
          has_more: true,
      };
      let v = serde_json::to_value(&r).unwrap();
      assert_eq!(v["has_more"], serde_json::json!(true));
      // Backward compatible: missing has_more deserializes as false (default).
      let r2: ListArchivedDmsResponse =
          serde_json::from_str(r#"{"dms":[]}"#).unwrap();
      assert!(!r2.has_more);
  }
  ```

- [ ] **Step 2: 跑测试确认 FAIL**

  Run: `cargo test -p gitim-core list_archived_dms_response_has_more_field`
  Expected: FAIL,因为字段未定义。

- [ ] **Step 3: 修改 `ListArchivedDmsResponse`**

  在 `responses.rs:226` 的 struct 加 `#[serde(default)] pub has_more: bool` 字段。
  现有 `dms` 字段顺序不变。

- [ ] **Step 4: 更新现有的 `list_archived_dms_response_wire_shape` 测试**

  把 `ListArchivedDmsResponse { dms: ... }` 改成显式传 `has_more: false`,或加
  `..Default::default()`(若 struct derive Default)。保持 wire shape 测试覆盖原行为。

- [ ] **Step 5: 跑测试**

  Run: `cargo test -p gitim-core list_archived_dms`
  Expected: PASS。

- [ ] **Step 6: Commit**

  ```
  feat(core): add has_more to ListArchivedDmsResponse

  Default false so existing daemon callers stay wire-compatible.
  Lazy-loaded sidebar will set has_more=true while pages remain.
  ```

---

## Task 2: daemon `Request::ListArchivedDms` 加 prefix/offset/limit

**Files:**
- Modify: `crates/gitim-daemon/src/api.rs:342-348`
- Modify: `crates/gitim-daemon/src/api.rs:780-795`(api 测试)
- Modify: `crates/gitim-daemon/src/handlers/mod.rs:442-448`(dispatch)

- [ ] **Step 1: 写失败 test**

  在 `api.rs` 的现有 round-trip 测试附近补充:

  ```rust
  #[test]
  fn list_archived_dms_request_carries_pagination() {
      let json = r#"{"method":"list_archived_dms","author":"alice","prefix":"bo","offset":5,"limit":10}"#;
      let req: Request = serde_json::from_str(json).unwrap();
      match req {
          Request::ListArchivedDms { author, prefix, offset, limit } => {
              assert_eq!(author.as_deref(), Some("alice"));
              assert_eq!(prefix.as_deref(), Some("bo"));
              assert_eq!(offset, 5);
              assert_eq!(limit, 10);
          }
          _ => panic!("wrong variant"),
      }
  }

  #[test]
  fn list_archived_dms_request_defaults() {
      let json = r#"{"method":"list_archived_dms"}"#;
      let req: Request = serde_json::from_str(json).unwrap();
      match req {
          Request::ListArchivedDms { author, prefix, offset, limit } => {
              assert!(author.is_none());
              assert!(prefix.is_none());
              assert_eq!(offset, 0);
              assert_eq!(limit, 5);
          }
          _ => panic!("wrong variant"),
      }
  }
  ```

- [ ] **Step 2: 跑测试确认 FAIL**

  Run: `cargo test -p gitim-daemon list_archived_dms_request`
  Expected: FAIL(字段未定义)。

- [ ] **Step 3: 改 `Request::ListArchivedDms` 变体**

  在 `api.rs:344` 的 enum 变体里加三个字段:

  - `#[serde(default)] prefix: Option<String>`
  - `#[serde(default)] offset: usize`
  - `#[serde(default = "default_archived_dms_limit")] limit: usize`(default fn 返回 5)

  在文件顶部(或 enum 同模块)加:
  ```rust
  fn default_archived_dms_limit() -> usize { 5 }
  ```

  注:`limit` 用 default fn 而不是 `Option<usize>`,让 daemon handler 拿到具体 usize
  不用再 unwrap_or。0 在 daemon 端 reject(见 Task 3 step 3)。

- [ ] **Step 4: 改 dispatch (`handlers/mod.rs:442-448`)**

  把 dispatch 从 `Request::ListArchivedDms { author } => ...` 改成接收
  `{ author, prefix, offset, limit }`,然后调:

  ```rust
  handle_list_archived_dms(state, resolved_author, prefix, offset, limit).await
  ```

  函数签名变化在 Task 3 完成。本 step 暂时让 dispatch 引用新签名,daemon 编译会断,Task 3
  接上。**所以这两 task 合成一个 commit**,但分两步 review。

- [ ] **Step 5: 跑测试**

  Run: `cargo test -p gitim-daemon list_archived_dms_request`
  Expected: PASS(round-trip 测试)。其它 daemon test 暂时编译失败,等 Task 3 修。

- [ ] **Step 6: 不 commit,继续 Task 3。**

---

## Task 3: daemon `handle_list_archived_dms` 实现过滤 + 分页 + peek

**Files:**
- Modify: `crates/gitim-daemon/src/handlers/read.rs:275-311`
- Add tests: `crates/gitim-daemon/src/handlers/read.rs`(或对应 `#[cfg(test)]` 模块)

- [ ] **Step 1: 写失败 tests**

  目标行为:
  1. 空 `archive/dm/` → `dms=[], has_more=false`
  2. 5 条 DM、limit=5 → 全部返回、`has_more=false`
  3. 6 条 DM、limit=5、offset=0 → 前 5 条、`has_more=true`
  4. 6 条 DM、limit=5、offset=5 → 第 6 条、`has_more=false`
  5. prefix `"bo"` (case-insensitive,handler 已全小写) → 只返 peer 以 bo 开头的
  6. prefix 含大写(`"BO"`) → 也匹配 `bob` `boss`(insensitive)
  7. limit=0 → daemon 拒绝(返回 error 或 fallback 到 1?约定:reject 并回 invalid request)

  测试用 `tempfile::TempDir` 构造 `archive/dm/<a>--<b>.thread` 文件,然后调
  `handle_list_archived_dms(state, author, prefix, offset, limit).await`。
  parse response JSON 验证 `dms` 顺序与 `has_more`。

  注意单元测试要构造 SharedState — 看 `handlers/read.rs` 其它 test 找现有 helper
  (例如 `setup_test_state()` 之类)复用。

- [ ] **Step 2: 跑测试确认 FAIL**

  Run: `cargo test -p gitim-daemon handle_list_archived_dms`
  Expected: 编译失败(签名未改)。

- [ ] **Step 3: 改 `handle_list_archived_dms` 签名 + 实现**

  签名:
  ```rust
  pub async fn handle_list_archived_dms(
      state: SharedState,
      author: String,
      prefix: Option<String>,
      offset: usize,
      limit: usize,
  ) -> Response
  ```

  实现:
  1. `if limit == 0 || limit > 100 { return Response::error(...) }` — 守护上下界
  2. 已有的 readdir → strip suffix → parse_dm_filename → peer 计算 不变
  3. peer 算出来后,若 `prefix` 非空且非 None:
     - `let needle = prefix.as_deref().unwrap_or("").to_ascii_lowercase();`
     - `if !needle.is_empty() && !peer.to_ascii_lowercase().starts_with(&needle) { continue; }`
  4. 收集进 `entries: Vec<ArchivedDmEntry>`
  5. `entries.sort_by(|a, b| a.peer.cmp(&b.peer))`
  6. `let total_after_filter = entries.len();`  // 不需要这个,删掉
  7. peek 法:
     ```rust
     let window: Vec<_> = entries.into_iter().skip(offset).take(limit + 1).collect();
     let has_more = window.len() > limit;
     let dms = window.into_iter().take(limit).collect();
     ```
  8. 返回 `ListArchivedDmsResponse { dms, has_more }`

  注意:`peer.to_ascii_lowercase()` 每条做一次,百万级时浪费,但 handler 文档说
  handler 已 normalized 为 lowercase(`onboard::register_user`),所以 `peer` 本身
  已是 lowercase — `prefix` 也走 `to_ascii_lowercase()` 后直接比 OK。可以省略
  `peer.to_ascii_lowercase()`,改成 `peer.starts_with(&needle)`。**保留 peer 这边的
  to_ascii_lowercase 作为防御性**(handler invariant 可能 drift)。

- [ ] **Step 4: 跑 scoped tests**

  Run: `cargo test -p gitim-daemon handle_list_archived_dms`
  Expected: 全部 PASS。

  Run: `cargo test -p gitim-daemon` (整个 crate)
  Expected: PASS,确认没把别的 handler 改坏。

- [ ] **Step 5: Commit (合并 Task 2 + Task 3)**

  ```
  feat(daemon): paginate + prefix-filter list_archived_dms

  Request gains prefix/offset/limit (limit default 5, max 100).
  Handler peeks limit+1 to compute has_more without counting the
  whole archive directory. Filter is case-insensitive prefix match
  against peer handler.
  ```

---

## Task 4: gitim-client `list_archived_dms` 改签名

**Files:**
- Modify: `crates/gitim-client/src/client.rs:272-273`
- Modify: `crates/gitim-client/src/client.rs:840-870`(round-trip 测试区域)

- [ ] **Step 1: 写失败 test**

  在 client.rs 的 #[cfg(test)] 块加:

  ```rust
  #[test]
  fn list_archived_dms_request_serializes_pagination() {
      let req = build_request(
          "list_archived_dms",
          json!({"prefix": "al", "offset": 0, "limit": 5}),
      );
      assert_eq!(
          req,
          json!({
              "method": "list_archived_dms",
              "prefix": "al",
              "offset": 0,
              "limit": 5,
          })
      );
  }
  ```

- [ ] **Step 2: 跑测试确认 FAIL**

  Run: `cargo test -p gitim-client list_archived_dms_request_serializes_pagination`
  Expected: PASS(因为 `build_request` 是通用 helper,只是 round-trip。新签名才是 fail 的目标)。
  如果 PASS 了说明 helper 通用,改用更精确的测试:

  ```rust
  #[tokio::test]
  async fn list_archived_dms_passes_params() {
      // mock daemon socket(若 test infra 支持),or just assert build_request shape
      // via the new public method:
      let _ = build_request("list_archived_dms", json!({
          "prefix": "al", "offset": 0, "limit": 5
      }));
      // existing tests already cover round-trip; verify the method call
      // signature compiles by exercising it via a higher-level test.
  }
  ```

  若现有 test infra 无法直接验证 `client.list_archived_dms(...)` 的 wire 形状,
  改用 `cargo build` 通过 + 在 runtime integration test 兜底。这种情况下本 Task 不写
  独立 unit test,而是把 client 改动放在 Task 5 的 runtime test 一起覆盖。

- [ ] **Step 3: 改 method 签名**

  改 `client.rs:272`:

  ```rust
  pub async fn list_archived_dms(
      &self,
      prefix: Option<&str>,
      offset: usize,
      limit: usize,
  ) -> Result<ApiResponse, ClientError> {
      let mut body = serde_json::Map::new();
      if let Some(p) = prefix {
          body.insert("prefix".into(), json!(p));
      }
      body.insert("offset".into(), json!(offset));
      body.insert("limit".into(), json!(limit));
      self.request("list_archived_dms", serde_json::Value::Object(body)).await
  }
  ```

- [ ] **Step 4: 修复其它调用方编译**

  `cargo build -p gitim-runtime` — runtime `http.rs:1180` 调旧签名会断,Task 5 修。
  CLI / 其它调用方(若有)需要相应改动。先 `cargo build` 检视 callers。

  Run: `grep -rn "list_archived_dms" crates/ products/`
  Expected:列出所有 callers,逐个 fix(或在对应 Task 里 fix)。本 Task 只改 client 方法
  本身 + 它的 unit test(若有)。

- [ ] **Step 5: 跑 scoped tests**

  Run: `cargo test -p gitim-client`
  Expected: PASS。`gitim-runtime` 暂时编译失败,等 Task 5。

- [ ] **Step 6: 不 commit,继续 Task 5。**

---

## Task 5: runtime `im_list_archived_dms` 解析 query string

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs:1172-1181`
- Add test: `crates/gitim-runtime/tests/` 下找现有 integration test 文件加 case,或就近 inline `#[cfg(test)]`

- [ ] **Step 1: 写失败 integration test**

  目标:request `GET /workspaces/<slug>/im/dm/archived?prefix=bo&offset=0&limit=5`
  返回 `{ dms: [...], has_more: bool }`。

  在 `gitim-runtime/tests/` 找已有 archived-dm http test(用 `grep -rn "im_list_archived_dms\|/im/dm/archived" crates/gitim-runtime/tests/`),复用其 setup。
  如果没有,加最小 integration test:启 fake daemon → 通过 runtime HTTP 调用 → 断言 response shape。

  若该 crate 集成测试启动成本太高,**降级为单元测试**:测 query string 解析(就近写
  `extract_archived_dms_params(uri) -> (Option<String>, usize, usize)` 之类的 helper 并测它),
  端到端走手工 smoke。

- [ ] **Step 2: 跑测试确认 FAIL**

  Run: `cargo test -p gitim-runtime im_list_archived_dms` (或对应测试名)
  Expected: FAIL。

- [ ] **Step 3: 改 `im_list_archived_dms`**

  ```rust
  use axum::extract::Query;
  #[derive(serde::Deserialize)]
  struct ArchivedDmsQuery {
      #[serde(default)] prefix: Option<String>,
      #[serde(default)] offset: Option<usize>,
      #[serde(default)] limit: Option<usize>,
  }

  async fn im_list_archived_dms(
      State(state): State<SharedRuntimeState>,
      WorkspaceSlug(slug): WorkspaceSlug,
      Query(q): Query<ArchivedDmsQuery>,
  ) -> axum::response::Response {
      let client = match human_client(&state, &slug) {
          Ok(c) => c,
          Err(j) => return j,
      };
      let prefix = q.prefix.as_deref();
      let offset = q.offset.unwrap_or(0);
      let limit = q.limit.unwrap_or(5).min(100).max(1);
      api_response_to_json(client.list_archived_dms(prefix, offset, limit).await)
  }
  ```

  注:`limit` clamp 在 runtime 也做一道,防止 daemon 因 0 / >100 抛 error 暴露给前端 500。

- [ ] **Step 4: 跑 scoped tests**

  Run: `cargo test -p gitim-runtime`
  Expected: PASS。

- [ ] **Step 5: Commit(合并 Task 4 + Task 5)**

  ```
  feat(client+runtime): pass prefix/offset/limit to list_archived_dms

  gitim-client method now takes (prefix, offset, limit). runtime
  HTTP handler parses query string with sane clamps so daemon never
  sees out-of-range limit values.
  ```

---

## Task 6: frontend `lib/client.ts::listArchivedDms` 改签名

**Files:**
- Modify: `products/gitim/frontend/src/lib/client.ts:1118-1134`

- [ ] **Step 1: 改 `ArchivedDmEntry` 周边类型 + 改 listArchivedDms 签名**

  ```ts
  export interface ArchivedDmsPage {
    dms: ArchivedDmEntry[];
    hasMore: boolean;
  }

  export interface ListArchivedDmsOptions {
    prefix?: string;
    offset?: number;
    limit?: number;
  }

  export async function listArchivedDms(
    slug: string,
    opts?: ListArchivedDmsOptions,
  ): Promise<ApiResponse<ArchivedDmsPage>> {
    const prefix = opts?.prefix ?? "";
    const offset = opts?.offset ?? 0;
    const limit = opts?.limit ?? 5;
    if (isLocalMode()) {
      void slug;
      const res = (await localDmArchiveBackend().listArchivedDms({
        prefix, offset, limit,
      })) as ApiResponse<{ dms: ArchivedDmEntry[]; has_more: boolean }>;
      if (!res.ok) return res;
      return { ok: true, data: { dms: res.data.dms, hasMore: res.data.has_more } };
    }
    const params = new URLSearchParams();
    if (prefix) params.set("prefix", prefix);
    params.set("offset", String(offset));
    params.set("limit", String(limit));
    const res = await fetch(`${wsBase(slug)}/im/dm/archived?${params}`);
    const json = (await res.json()) as ApiResponse<{
      dms: ArchivedDmEntry[]; has_more: boolean;
    }>;
    if (!json.ok) return json;
    return { ok: true, data: { dms: json.data.dms, hasMore: json.data.has_more } };
  }
  ```

  注:把后端 `has_more` 映射成前端常用的 `hasMore`,避免 store 层混 snake/camel。

- [ ] **Step 2: 编译验证**

  Run: `cd products/gitim/frontend && npx tsc -b --noEmit`
  Expected: app.tsx / sidebar.tsx 引用旧形状的地方报错,这些 Task 8-10 修。
  本 step 接受 type error,只确认 client.ts 本身 self-consistent。

- [ ] **Step 3: 不 commit,继续 Task 7。**

---

## Task 7: frontend `daemon-web/handlers.ts::listArchivedDms` 适配

**Files:**
- Modify: `products/gitim/frontend/src/daemon-web/handlers.ts:903-936`
- Modify: `products/gitim/frontend/src/daemon-web/worker.ts:131`

- [ ] **Step 1: 写失败 test**

  在 `products/gitim/frontend/src/daemon-web/handlers.test.ts` 现有套件里加:

  ```ts
  describe("listArchivedDms pagination", () => {
    it("returns has_more=true when more entries exist after limit", async () => {
      // setup: write 6 .thread files into archive/dm/ via the in-memory FS
      // call listArchivedDms({ prefix: "", offset: 0, limit: 5 })
      // expect data.dms.length === 5 && data.has_more === true
    });
    it("filters by lowercase prefix", async () => {
      // setup: write alice--me, bob--me, me--carol
      // call listArchivedDms({ prefix: "bo", offset: 0, limit: 5 })
      // expect dms.length === 1 && dms[0].peer === "bob"
    });
    it("limit=0 falls back to 1", async () => {
      // call with limit=0 → handler clamps to 1
    });
  });
  ```

  Setup helper 看 `handlers.test.ts` 现有 archive-dm 测试(grep `archive/dm` 找),复用
  in-memory fs 构造法。

- [ ] **Step 2: 跑测试确认 FAIL**

  Run: `cd products/gitim/frontend && npm test -- handlers`
  Expected: FAIL。

- [ ] **Step 3: 改 `listArchivedDms` 签名 + 实现**

  ```ts
  export async function listArchivedDms(opts?: {
    prefix?: string;
    offset?: number;
    limit?: number;
  }): Promise<ApiResponse> {
    try {
      const prefix = (opts?.prefix ?? "").toLowerCase();
      const offset = Math.max(0, opts?.offset ?? 0);
      const limit = Math.min(100, Math.max(1, opts?.limit ?? 5));

      const s = getState();
      const archiveDmDir = `${s.repoDir}/archive/dm`;
      if (!(await exists(archiveDmDir))) {
        return ok({ dms: [], has_more: false });
      }

      const items = await readdir(archiveDmDir);
      const me = s.me.handler;
      const entries: Array<{ peer: string; dm_pair_stem: string }> = [];
      for (const item of items) {
        if (!item.endsWith(".thread")) continue;
        const stem = item.slice(0, -".thread".length);
        const parts = stem.split("--");
        if (parts.length !== 2) continue;
        const [a, b] = parts;
        let peer: string;
        if (a === me) peer = b;
        else if (b === me) peer = a;
        else continue;
        if (prefix && !peer.toLowerCase().startsWith(prefix)) continue;
        entries.push({ peer, dm_pair_stem: stem });
      }
      entries.sort((x, y) => x.peer.localeCompare(y.peer));
      const window = entries.slice(offset, offset + limit + 1);
      const has_more = window.length > limit;
      return ok({ dms: window.slice(0, limit), has_more });
    } catch (e) {
      return err(String((e as Error).message ?? e));
    }
  }
  ```

- [ ] **Step 4: 改 worker route**

  `worker.ts:131` 当前:`listArchivedDms: () => handlers.listArchivedDms(),`
  改成接受 payload:
  ```ts
  listArchivedDms: (payload: unknown) =>
    handlers.listArchivedDms(payload as {
      prefix?: string; offset?: number; limit?: number;
    } | undefined),
  ```
  如果 worker dispatch 已经统一传 payload(其它 handler 怎么传),沿用同样模式。

- [ ] **Step 5: 改 `lib/client.ts` 中 `localDmArchiveBackend().listArchivedDms(...)` 调用**

  Task 6 step 1 已经写成传 `{prefix, offset, limit}`,确认这跟 worker 路由对得上(worker
  把 payload 透传给 handler)。

- [ ] **Step 6: 跑 scoped tests**

  Run: `npm test -- handlers`
  Expected: PASS。

- [ ] **Step 7: Commit(合并 Task 6 + Task 7)**

  ```
  feat(frontend): listArchivedDms supports pagination + prefix filter

  HTTP wrapper builds query string and maps has_more → hasMore.
  Browser-mode daemon-web handler implements the same semantics
  against the in-memory FS so local mode keeps parity.
  ```

---

## Task 8: store — `archivedDms` → `archivedDmsView`

**Files:**
- Modify: `products/gitim/frontend/src/hooks/use-chat-store.ts:14-21, 42, 111`(及其它涉及 archivedDms 的行)
- Modify: `products/gitim/frontend/src/hooks/use-chat-store.test.ts`

- [ ] **Step 1: 写失败 test**

  在 `use-chat-store.test.ts` 加测试:

  ```ts
  describe("archivedDmsView", () => {
    it("starts as null (uninitialized)", () => {
      const s = useChatStore.getState();
      expect(s.archivedDmsView).toBeNull();
    });

    it("resetArchivedDmsView clears items and stores query", () => {
      const store = useChatStore.getState();
      store.resetArchivedDmsView("alice");
      const v = useChatStore.getState().archivedDmsView!;
      expect(v.items).toEqual([]);
      expect(v.offset).toBe(0);
      expect(v.query).toBe("alice");
      expect(v.loading).toBe(false);
      expect(v.error).toBeNull();
      expect(v.hasMore).toBe(true);
    });

    it("appendArchivedDmsPage extends items + advances offset", () => {
      useChatStore.getState().resetArchivedDmsView("");
      useChatStore.getState().appendArchivedDmsPage({
        items: [{ peer: "alice", dm_pair_stem: "alice--me" }],
        hasMore: false,
      });
      const v = useChatStore.getState().archivedDmsView!;
      expect(v.items.length).toBe(1);
      expect(v.offset).toBe(1);
      expect(v.hasMore).toBe(false);
    });

    it("setArchivedDmsLoading / setArchivedDmsError set flags", () => {
      useChatStore.getState().resetArchivedDmsView("");
      useChatStore.getState().setArchivedDmsLoading(true);
      expect(useChatStore.getState().archivedDmsView!.loading).toBe(true);
      useChatStore.getState().setArchivedDmsError("boom");
      const v = useChatStore.getState().archivedDmsView!;
      expect(v.error).toBe("boom");
      expect(v.loading).toBe(false);
    });
  });
  ```

- [ ] **Step 2: 跑测试确认 FAIL**

  Run: `npm test -- use-chat-store`
  Expected: FAIL。

- [ ] **Step 3: 改 `use-chat-store.ts`**

  - 删除 `archivedDms: Channel[]`(line 21)与 `setArchivedDms`(line 42)
  - 加 `ArchivedDmsView` type 与 `archivedDmsView: ArchivedDmsView | null`
  - 加 actions:`resetArchivedDmsView(query)`、`appendArchivedDmsPage({items, hasMore})`、
    `setArchivedDmsLoading(b)`、`setArchivedDmsError(e)`
  - 初始值 `archivedDmsView: null`

  Type 定义:

  ```ts
  import type { ArchivedDmEntry } from "@/lib/client";

  export interface ArchivedDmsView {
    items: ArchivedDmEntry[];
    offset: number;
    hasMore: boolean;
    query: string;
    loading: boolean;
    error: string | null;
  }
  ```

  Actions 实现(关键不变量):
  - `resetArchivedDmsView(query)` — 写 `{ items: [], offset: 0, hasMore: true, query, loading: false, error: null }`
  - `appendArchivedDmsPage({items, hasMore})` — 若 view 为 null,no-op(说明已被 reset 为
    null,新页面无所归属);否则 items 拼接 + offset += items.length + hasMore 覆写
  - `setArchivedDmsLoading(loading)` — view 非 null 时写,且置 `error = null`(若 loading=true)
  - `setArchivedDmsError(error)` — view 非 null 时写,且置 `loading = false`

- [ ] **Step 4: 跑 scoped test**

  Run: `npm test -- use-chat-store`
  Expected: PASS。

- [ ] **Step 5: 全局类型检查**

  Run: `npx tsc -b --noEmit`
  Expected: 一系列 sidebar.tsx / app.tsx 错误(预期,后续 Task 修)。

- [ ] **Step 6: 不 commit,继续 Task 9。**

---

## Task 9: 移除 `app.tsx` eager fetch + 所有 archivedDms 引用

**Files:**
- Modify: `products/gitim/frontend/src/app.tsx:152, 290, 302, 316, 341-359, 426, 475, 488`

- [ ] **Step 1: 全文搜索定位**

  Run: `grep -n "archivedDms\|setArchivedDms\|listArchivedDms" products/gitim/frontend/src/app.tsx`
  Expected:列出所有引用,逐行处理。

- [ ] **Step 2: 移除**

  - 删 `client.listArchivedDms(slug)` 调用(line 302)与 `Promise.all` 数组里的位置
  - 删对应的 `archivedDmsRes` 解构(line 316)、`archivedDms` 构造(line 341-359)
  - 删 `if (archivedDmsRes.ok && archivedDmsRes.data) setArchivedDms(archivedDms);`(line 426)
  - 删 `archivedDmsRes.ok && ...`(line 475)的 boolean 条件分支
  - 删 `setArchivedDms,`(line 488)
  - 删 `const setArchivedDms = useChatStore(...)`(line 152)

  整个 `Promise.all` 数组的 index 会变,小心解构后续访问。最稳:
  把数组写法和解构都按新长度对齐。

- [ ] **Step 3: 编译**

  Run: `npx tsc -b --noEmit`
  Expected: app.tsx 无 archivedDms 相关错误。sidebar.tsx 错误仍在,Task 10 修。

- [ ] **Step 4: 不 commit,继续 Task 10。**

---

## Task 10: sidebar UI — input + Load more + lazy on expand

**Files:**
- Modify: `products/gitim/frontend/src/components/chat/sidebar.tsx:161, 179, 773-842`

- [ ] **Step 1: 写失败 E2E (Playwright)**

  在 `products/gitim/frontend/e2e/sidebar-layout.spec.ts` 或新建
  `e2e/archived-dms.spec.ts` 加场景:

  - case A:展开 ARCHIVED DMS section,看到 first 5 个 archive DM(若仓库 fixture 不足 5,
    fixture 里多 seed 几个)
  - case B:输入 "bo" 到搜索框,wait debounce,只显示 peer 以 bo 开头的 DM
  - case C:点 "Load more",列表多 5 条;若不足 5,Load more 消失

  E2E fixture seeding 看现有 spec 的模式(`browser_navigate` + setup script)。
  fixture 不容易准备的场景 → 改为单元测试 sidebar 渲染(用 `@testing-library/react` 若已装,
  否则手动调 actions 验证 view state)。**如果加 E2E 成本大,本步降级为手工 smoke,
  在 Task 12 完成。**

- [ ] **Step 2: 改 sidebar.tsx archive DM section**

  关键改动点:

  a. **store hook**(line 161):
  把 `const archivedDms = useChatStore((s) => s.archivedDms);` 替换:
  ```ts
  const archivedDmsView = useChatStore((s) => s.archivedDmsView);
  const resetArchivedDmsView = useChatStore((s) => s.resetArchivedDmsView);
  const appendArchivedDmsPage = useChatStore((s) => s.appendArchivedDmsPage);
  const setArchivedDmsLoading = useChatStore((s) => s.setArchivedDmsLoading);
  const setArchivedDmsError = useChatStore((s) => s.setArchivedDmsError);
  ```
  ⚠️ 注意 memory `project_zustand_selector_pitfalls.md`:每个 selector 单独取
  primitive / 引用,不要返 object literal / `.filter(...)` 派生数组,避免循环渲染。

  b. **lazy 触发**:`archivedDmsOpen` 切到 true 时,若 view===null,触发首页拉。
  用 useEffect 监听 `archivedDmsOpen + slug`:
  ```ts
  useEffect(() => {
    if (!archivedDmsOpen) return;
    if (archivedDmsView !== null) return;
    void fetchArchivedDmsPage(slug, "", 0);
  }, [archivedDmsOpen, slug]);
  ```
  `fetchArchivedDmsPage(slug, query, offset)` 是新的本地 helper(也可以提到 sidebar
  外 hook):
  - 若 view===null 且 offset===0 → `resetArchivedDmsView(query)`
  - `setArchivedDmsLoading(true)`
  - `await client.listArchivedDms(slug, { prefix: query, offset, limit: 5 })`
  - response.ok → `appendArchivedDmsPage({ items: data.dms, hasMore: data.hasMore })` +
    `setArchivedDmsLoading(false)`
  - response 失败 → `setArchivedDmsError(error)`
  - race guard:发请求前 snapshot `query`,response 回来时若 `useChatStore.getState().archivedDmsView?.query !== snapshotQuery` → 丢弃

  c. **section render**(line 773-842):
  - title 删 count badge(line 788-792 那块,直接去掉)
  - 展开后,在列表之前加 `<input>`:
    ```tsx
    <input
      type="text"
      placeholder="Filter by handle..."
      value={archivedDmsView?.query ?? ""}
      onChange={(e) => onPrefixChange(e.target.value)}
      // styling 沿用 sidebar 现有 input(channel search 那个),保持 design 一致
    />
    ```
    `onPrefixChange(newQuery)` debounce 300ms 后调
    `fetchArchivedDmsPage(slug, newQuery, 0)`。debounce 用 `useRef<number | null>` +
    `setTimeout`(无第三方依赖)。
  - 列表渲染 `archivedDmsView?.items` 不再渲染 `archivedDms`(Channel[] 形状)。
    现有的 row 渲染需要把 `ArchivedDmEntry { peer, dm_pair_stem }` 映射成 link target
    /click handler。看现有 `archivedDms.map((c) => ...)` 怎么取 `c.id` `c.name` 之类,
    替换成 `entry.peer` / `entry.dm_pair_stem`。如果 row component 需要 Channel 形状,
    inline 构造一个 minimal channel-shaped 对象,只供 click target 使用(与 §app.tsx 老
    路径一致,见原 line 343-353)。
  - loading 状态:`archivedDmsView?.loading` → 显示 "Loading...";error → 红色 + Retry 按钮
  - 空 + query 空:"No archived DMs"
  - 空 + query 非空:"No matches"
  - 底部:`archivedDmsView?.hasMore && !loading` → `<button>Load more</button>`,
    onClick 调 `fetchArchivedDmsPage(slug, view.query, view.offset)`

- [ ] **Step 3: 跑前端 typecheck**

  Run: `npx tsc -b --noEmit`
  Expected: PASS。

- [ ] **Step 4: 跑前端 vitest**

  Run: `npm test`
  Expected: PASS。

- [ ] **Step 5: 跑 Playwright E2E**(若 Task 10 step 1 写了 spec)

  Run: `npm run test:e2e -- archived-dms.spec.ts`(或对应 spec)
  Expected: PASS。

- [ ] **Step 6: Commit (Task 8 + 9 + 10 合并)**

  ```
  feat(frontend): lazy-load archived DMs with prefix search + Load more

  Sidebar section starts collapsed; first page (5 entries) loads on
  expand. Top input debounce-filters by peer-handle prefix via the
  server. Load more button paginates with offset. Eager fetch in
  app.tsx removed.
  ```

---

## Task 11: archive / unarchive 钩子 invalidate view

**Files:**
- 调研:`grep -rn "archive_dm\|unarchive_dm\|setArchivedDms" products/gitim/frontend/src` 找
  mutation success callback 和 SSE event handler 位置

- [ ] **Step 1: 找钩子点**

  Run: `grep -rn "archive_dm\|unarchive_dm\|archiveDm\|unarchiveDm" products/gitim/frontend/src`

  预期发现:
  - mutation API 调用点(可能在 store action 或 component event handler)
  - SSE event 在 `daemon-web/` 或 chat store 的 onMessage handler

  把找到的所有 archive/unarchive success path 列出来。

- [ ] **Step 2: 在每个 success path 加 reset 钩子**

  pattern:
  ```ts
  // after archive/unarchive succeeds:
  const view = useChatStore.getState().archivedDmsView;
  if (view !== null) {
    // section 当时被展开过(否则 view===null)
    // 重拉首页,保留当前 query
    void fetchArchivedDmsPage(slug, view.query, 0);
  }
  ```

  `fetchArchivedDmsPage` 此时需要从 sidebar 提到一个共享位置(例如 `hooks/use-archived-dms.ts`)
  以便 chat 别处也能调。

  如果改动太大,降级为「重置 view 为 null」一行:
  ```ts
  useChatStore.setState({ archivedDmsView: null });
  ```
  下次用户展开 section 时会重新触发首页拉(因为 useEffect 监听 `archivedDmsView===null`)。
  但若 section 当时已经展开,用户看到列表瞬间空白 → bad UX。**所以优先用上面的「保留
  query 重拉」路径,如果实现复杂再降级。**

- [ ] **Step 3: 跑前端 typecheck + vitest**

  Run: `npx tsc -b --noEmit && npm test`
  Expected: PASS。

- [ ] **Step 4: Commit**

  ```
  feat(frontend): invalidate archived DMs view on archive/unarchive

  Refetch first page (current query preserved) after archive or
  unarchive succeeds, so the sidebar reflects the new state without
  a full reload.
  ```

---

## Task 12: 全量验证 + 手动 smoke

- [ ] **Step 1: 全量 cargo test**

  Run: `cargo test`
  Expected: 全部 PASS(忽略 `#[ignore]`)。

- [ ] **Step 2: 全量 frontend test**

  Run: `cd products/gitim/frontend && npm test`
  Expected: PASS。

- [ ] **Step 3: Lint + typecheck**

  Run: `cd products/gitim/frontend && npm run lint && npx tsc -b --noEmit`
  Expected: PASS。

- [ ] **Step 4: Playwright E2E**

  Run: `cd products/gitim/frontend && npm run test:e2e`
  Expected: PASS。注意:这跑的是 `sidebar-layout.spec.ts` + `mobile-layout.spec.ts`,
  并不包含本 plan 新加的 archive 场景。若 Task 10 step 1 加了 spec,把它加进
  `package.json` 的 `test:e2e` 脚本里或单独 `npm run test:e2e -- archived-dms.spec.ts`。

- [ ] **Step 5: 手动 smoke**(若启动 webui 方便)

  - 启 runtime + 把 fixture workspace 灌入 ≥ 10 个 archive DM
  - 展开 sidebar ARCHIVED DMS section
  - 看到首页 5 条,Load more 出现
  - 点 Load more → 5+5 条
  - 在 input 输入 peer handler 前缀 → 列表收敛到匹配项
  - 清空 input → 回到首页 5 条
  - archive 一个新 DM → 列表自动刷新,新 DM 出现在正确位置
  - unarchive → 列表自动刷新,该 DM 消失

- [ ] **Step 6: Commit 若 Step 1-5 触发额外修复;否则结束。**

---

## Self-review

走完 Task 1-12 应覆盖:
- ✅ §API:Task 1 + Task 2 + Task 3(后端);Task 5(runtime); Task 6 + Task 7(前端)
- ✅ §Daemon:Task 3
- ✅ §Runtime:Task 5
- ✅ §Frontend Store:Task 8
- ✅ §Frontend Sidebar:Task 10
- ✅ §Eager Fetch 移除:Task 9
- ✅ §Client:Task 6 + Task 7
- ✅ §Archive / Unarchive 钩子:Task 11
- ✅ §错误与边界:Task 10(loading / error / empty / race);Task 3(daemon 守护)
- ✅ §不在 Scope:本 plan 也不涉及

类型一致性:
- daemon `ListArchivedDmsResponse.has_more`(snake)
- 前端 store `archivedDmsView.hasMore`(camel)
- 转换在 `lib/client.ts` listArchivedDms 完成(snake → camel)
- 测试名 `appendArchivedDmsPage` 与 store action 名一致 ✓

风险:
- Task 11 的 success path 可能比预期多(SSE 推送 + 本地 mutation 两条路径);step 1 调研
  先,有遗漏路径再回头补
- E2E 加新 spec 成本大;允许 fallback 到手工 smoke 不阻塞 merge

---

## Execution Handoff

Plan saved to `docs/plans/archive-dms-lazy-load/02-plan.md`。

执行选项:

1. **Subagent-Driven** — 每个 Task dispatch 新 subagent,任务间 review,快速迭代
2. **Inline Execution** — 当前 session 用 `executing-plans` 跑,批量执行 + checkpoint

由 user 选定。
