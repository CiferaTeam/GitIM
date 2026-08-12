# Agents Human + Outside Roster — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给 `UserMeta` 加上可 git 同步的 `kind`（human/agent/unknown），并在 Runtime Agents 页按 Humans → Local/Fleet → Outside 展示名册。

**Architecture:** 协议加性字段写在首次 `RegisterUser`/`onboard`；不可变、无回填。前端用已有 `user_infos` + `/agents` + fleet 做纯函数分区；`you` badge 复用 `chatStore.currentUser`。详见 [00-requirements.md](./00-requirements.md)。

**Tech Stack:** Rust (`gitim-core` / `gitim-daemon` / `gitim-client` / `gitim-runtime` / `gitim-cli` / `gitim-wasm`), React 19 + Vitest (`products/gitim/frontend`).

## Global Constraints

- Worktree only: `/Users/lewisliu/ateam/GitIM/.worktrees/feat/agents-human-external-nodes`
- Stable Rust only（仓库 `rust-toolchain.toml`）
- 不做旧数据回填 / set-kind API / browser Agents 页
- `kind` 创建后 API 不可改；`update_user` / labels RMW 必须保留字段
- 改 `gitim-core` UserMeta 后必须 `npm run build:wasm`（frontend）并 commit `crates/gitim-wasm/pkg/`
- Commit 格式：`feat(scope): …` + footer `Test: <cmd>` + `Co-authored-by: Cursor <cursoragent@cursor.com>`（若 Cursor 提交）
- 禁止 `git commit --no-verify`；验证用 scoped tests，不跑全量 `cargo test`
- UI 遵循根目录 `DESIGN.md`

---

## File Structure

### 新建

| 路径 | 职责 |
|------|------|
| `products/gitim/frontend/src/lib/agents-roster.ts` | `partitionAgentsRoster` 纯函数 |
| `products/gitim/frontend/src/lib/agents-roster.test.ts` | 分区表驱动测试 |

### 修改

| 路径 | 改动 |
|------|------|
| `crates/gitim-core/src/types/meta.rs` | `UserKind` + `UserMeta.kind` + serde/tests |
| `crates/gitim-core/src/types/mod.rs` | re-export `UserKind` |
| `crates/gitim-core/src/responses.rs` | `ActiveUserEntry.kind` + wire test |
| `crates/gitim-daemon/src/api.rs` | `Onboard`/`RegisterUser` 可选 `kind` |
| `crates/gitim-daemon/src/onboard.rs` | `register_user(..., kind)`；`handle_onboard` 传 kind |
| `crates/gitim-daemon/src/handlers/user.rs` | `handle_register_user` 写 kind |
| `crates/gitim-daemon/src/handlers/mod.rs` | dispatch 传 kind |
| `crates/gitim-daemon/src/handlers/read.rs` | `list_users` 填 `kind` |
| `crates/gitim-client/src/client.rs` | `onboard`/`register_user` 传 kind |
| `crates/gitim-runtime/src/agent.rs` | human→`human`，agent→`agent` |
| `crates/gitim-cli/src/commands/onboard.rs` | 默认 `human` |
| 所有 `UserMeta { ... }` 字面量（~10 处） | 补 `kind:` |
| `products/gitim/frontend/src/lib/types.ts` | `UserKind` / `UserInfo.kind` |
| `products/gitim/frontend/src/components/management/agent-list.tsx` | Humans + Outside UI |
| `products/gitim/frontend/src/components/management/agent-list.test.tsx` | UI 行为 |
| `products/gitim/frontend/src/daemon-web/state.ts` | `UserMeta.kind?` |
| `products/gitim/frontend/src/daemon-web/wasm-semantics.test.ts` | parseUserMeta kind |
| `crates/gitim-wasm/pkg/*` | rebuild 产物 |
| `AGENTS.md` | Current Orientation 一行 |

---

## Phase A — Protocol (gitim-core)

### Task 1: `UserKind` + `UserMeta.kind`

**Files:**
- Modify: `crates/gitim-core/src/types/meta.rs`
- Modify: `crates/gitim-core/src/types/mod.rs`
- Touch all in-crate `UserMeta {` literals to add `kind: UserKind::…`

**Interfaces:**
- Produces: `UserKind::{Unknown,Human,Agent}`, `UserKind::as_str`, `UserKind::is_unknown`, `UserMeta.kind`

- [ ] **Step 1: Write failing tests in `meta.rs` `#[cfg(test)]`**

```rust
#[test]
fn old_yaml_without_kind_deserializes_as_unknown() {
    let yaml = "display_name: Alice\nrole: member\nintroduction: hi\n";
    let meta: UserMeta = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(meta.kind, UserKind::Unknown);
}

#[test]
fn human_kind_roundtrip_and_omits_unknown_on_serialize() {
    let human = UserMeta {
        display_name: "Alice".into(),
        role: "member".into(),
        introduction: "hi".into(),
        labels: vec![],
        kind: UserKind::Human,
    };
    let yaml = serde_yaml::to_string(&human).unwrap();
    assert!(yaml.contains("kind: human"));
    let back: UserMeta = serde_yaml::from_str(&yaml).unwrap();
    assert_eq!(back.kind, UserKind::Human);

    let unknown = UserMeta {
        kind: UserKind::Unknown,
        ..human.clone()
    };
    let yaml_u = serde_yaml::to_string(&unknown).unwrap();
    assert!(!yaml_u.contains("kind:"));
}

#[test]
fn user_kind_as_str_matches_serde() {
    assert_eq!(UserKind::Human.as_str(), "human");
    assert_eq!(UserKind::Agent.as_str(), "agent");
    assert_eq!(UserKind::Unknown.as_str(), "unknown");
}
```

- [ ] **Step 2: Run tests — expect FAIL (UserKind missing)**

```bash
cargo test -p gitim-core old_yaml_without_kind -- --nocapture
```

Expected: compile error / test not found.

- [ ] **Step 3: Implement**

In `meta.rs` (above `UserMeta`):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserKind {
    #[default]
    Unknown,
    Human,
    Agent,
}

impl UserKind {
    pub fn as_str(self) -> &'static str {
        match self {
            UserKind::Unknown => "unknown",
            UserKind::Human => "human",
            UserKind::Agent => "agent",
        }
    }

    pub fn is_unknown(self) -> bool {
        matches!(self, UserKind::Unknown)
    }
}
```

Add to `UserMeta`:

```rust
    /// Account kind. Written once at register/onboard. Missing → Unknown.
    #[serde(default, skip_serializing_if = "UserKind::is_unknown")]
    pub kind: UserKind,
```

Update `mod.rs` re-export:

```rust
pub use meta::{
    validate_user_meta, ChannelMeta, UserKind, UserMeta, UserMetaError, MAX_INTRODUCTION_LEN,
};
```

Fix **every** `UserMeta {` literal in the workspace (grep `UserMeta \{`) — tests/fixtures use `kind: UserKind::Unknown` unless the test is specifically about human/agent.

- [ ] **Step 4: Run tests — expect PASS**

```bash
cargo test -p gitim-core old_yaml_without_kind human_kind_roundtrip user_kind_as_str -- --nocapture
```

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-core
git commit -m "$(cat <<'EOF'
feat(core): add UserMeta.kind (human|agent|unknown)

Test: cargo test -p gitim-core old_yaml_without_kind human_kind_roundtrip user_kind_as_str
Co-authored-by: Cursor <cursoragent@cursor.com>
EOF
)"
```

---

### Task 2: `ActiveUserEntry.kind` + `list_users`

**Files:**
- Modify: `crates/gitim-core/src/responses.rs`
- Modify: `crates/gitim-daemon/src/handlers/read.rs` (`handle_list_users`)

**Interfaces:**
- Consumes: `UserMeta.kind`
- Produces: wire `user_infos[].kind` (omit when unknown)

- [ ] **Step 1: Extend wire-shape test**

In `list_users_response_user_infos_wire_shape` (responses.rs ~1028):

```rust
ActiveUserEntry {
    handler: "alice".to_string(),
    display_name: Some("Alice Chen".to_string()),
    kind: UserKind::Human,
},
ActiveUserEntry {
    handler: "bob".to_string(),
    display_name: None,
    kind: UserKind::Unknown,
},
```

Assert:

```rust
assert_eq!(infos[0].get("kind").unwrap().as_str(), Some("human"));
assert!(!infos[1].as_object().unwrap().contains_key("kind"));
```

Update `ActiveUserEntry`:

```rust
pub struct ActiveUserEntry {
    pub handler: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "UserKind::is_unknown")]
    pub kind: UserKind,
}
```

(Import `UserKind` from `crate::types`.)

- [ ] **Step 2: Run — FAIL on missing field / assert**

```bash
cargo test -p gitim-core list_users_response_user_infos_wire_shape -- --nocapture
```

- [ ] **Step 3: Update `handle_list_users`**

In `read.rs`, when building `ActiveUserEntry`:

```rust
.and_then(|c| serde_yaml::from_str::<UserMeta>(&c).ok())
.map(|m| (m.display_name, m.kind));
// ...
gitim_core::responses::ActiveUserEntry {
    handler: handler.clone(),
    display_name,
    kind, // default Unknown if parse failed — use unwrap_or_default on Option
}
```

Prefer:

```rust
let (display_name, kind) = std::fs::read_to_string(...)
    .ok()
    .and_then(|c| serde_yaml::from_str::<UserMeta>(&c).ok())
    .map(|m| (Some(m.display_name), m.kind))
    .unwrap_or((None, UserKind::Unknown));
```

Fix any other `ActiveUserEntry {` literals in tests.

- [ ] **Step 4: PASS**

```bash
cargo test -p gitim-core list_users_response_user_infos_wire_shape -- --nocapture
```

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-core crates/gitim-daemon/src/handlers/read.rs
git commit -m "$(cat <<'EOF'
feat(daemon): expose user kind on list_users user_infos

Test: cargo test -p gitim-core list_users_response_user_infos_wire_shape
Co-authored-by: Cursor <cursoragent@cursor.com>
EOF
)"
```

---

## Phase B — Write path (daemon / client / runtime / cli)

### Task 3: Onboard + RegisterUser accept `kind`

**Files:**
- Modify: `crates/gitim-daemon/src/api.rs`
- Modify: `crates/gitim-daemon/src/onboard.rs`
- Modify: `crates/gitim-daemon/src/handlers/user.rs`
- Modify: `crates/gitim-daemon/src/handlers/mod.rs`
- Modify: `crates/gitim-client/src/client.rs`
- Modify: `crates/gitim-runtime/src/agent.rs`
- Modify: `crates/gitim-cli/src/commands/onboard.rs`
- Test: extend onboard unit tests in `onboard.rs`

**Interfaces:**
- Consumes: `UserKind`
- Produces: new yaml files with `kind: human|agent` when callers pass it; omit/`Unknown` when absent

- [ ] **Step 1: Failing onboard test**

In `onboard.rs` tests, after a successful human-style onboard that will pass `UserKind::Human`, assert meta yaml contains `kind: human`. Add a second case: onboard with default/omitted kind → no `kind:` key (Unknown).

Also assert `register_user_skips_if_exists` does **not** change an existing human yaml when re-onboarding with a different kind (still skip).

- [ ] **Step 2: Run — FAIL**

```bash
cargo test -p gitim-daemon register_user_creates_meta -- --nocapture
```

(or the new test name you added)

- [ ] **Step 3: Implement API + writers**

`api.rs` — both variants:

```rust
RegisterUser {
    handler: String,
    display_name: String,
    #[serde(default = "default_role")]
    role: String,
    #[serde(default = "default_introduction")]
    introduction: String,
    #[serde(default)]
    kind: UserKind,
},
Onboard {
    // ...existing fields...
    #[serde(default)]
    kind: UserKind,
},
```

(`use gitim_core::types::UserKind;` — `Default` = Unknown.)

`onboard.rs`:

```rust
pub async fn handle_onboard(..., kind: UserKind) -> Response { ... }

fn register_user(state: &SharedState, handler: &str, display_name: &str, kind: UserKind) -> Result<bool, Response> {
    // if meta_path.exists() { return Ok(false); }  // unchanged — no kind upgrade
    let meta = UserMeta {
        display_name: display_name.to_string(),
        role: "member".to_string(),
        introduction: "GitIM user".to_string(),
        labels: Vec::new(),
        kind,
    };
    ...
}
```

Call site: `register_user(&state, &handler, &display_name, kind)`.

`handlers/user.rs` `handle_register_user`: accept `kind: UserKind`, put into `UserMeta`.

`handlers/mod.rs`:

```rust
Request::RegisterUser { handler, display_name, role, introduction, kind } => {
    handle_register_user(state, handler, display_name, role, introduction, kind).await
}
Request::Onboard { git_server, auth, admin, guest, join_general, kind } => {
    crate::onboard::handle_onboard(state, git_server, auth, admin, guest, join_general, kind).await
}
```

`gitim-client` `client.rs`:

```rust
pub async fn onboard(
    &self,
    git_server: &str,
    auth: Option<AuthPayload>,
    admin: bool,
    guest: bool,
    join_general: bool,
    kind: Option<&str>,
) -> Result<ApiResponse, ClientError> {
    let mut body = json!({
        "git_server": git_server,
        "auth": auth,
        "admin": admin,
        "guest": guest,
        "join_general": join_general,
    });
    if let Some(k) = kind {
        body["kind"] = json!(k);
    }
    self.request("onboard", body).await
}

pub async fn register_user(
    &self,
    handler: &str,
    display_name: &str,
    role: Option<&str>,
    introduction: Option<&str>,
    kind: Option<&str>,
) -> Result<ApiResponse, ClientError> {
    let mut body = json!({
        "handler": handler,
        "display_name": display_name,
        "role": role.unwrap_or("member"),
        "introduction": introduction.unwrap_or("GitIM user"),
    });
    if let Some(k) = kind {
        body["kind"] = json!(k);
    }
    self.request("register_user", body).await
}
```

Update **all** `.onboard(` / `.register_user(` call sites (grep).

`agent.rs`:

```rust
// provision_human
client.onboard(git_server, Some(auth), true, false, true, Some("human")).await...

// provision_agent
client.onboard("git", Some(auth), false, false, join_general, Some("agent")).await...
```

`cli/.../onboard.rs`: pass `Some("human")` on both `.onboard(...)` calls.

- [ ] **Step 4: PASS scoped daemon + compile dependents**

```bash
cargo test -p gitim-daemon register_user_creates_meta register_user_skips_if_exists -- --nocapture
cargo check -p gitim-client -p gitim-runtime -p gitim-cli
```

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-daemon crates/gitim-client crates/gitim-runtime crates/gitim-cli
git commit -m "$(cat <<'EOF'
feat(daemon): write UserMeta.kind on onboard/register

Test: cargo test -p gitim-daemon register_user_creates_meta register_user_skips_if_exists
Co-authored-by: Cursor <cursoragent@cursor.com>
EOF
)"
```

---

### Task 4: Preserve `kind` on profile/labels RMW (regression)

**Files:**
- Modify tests only if needed in `crates/gitim-daemon/src/handlers/user.rs` / `labels.rs`
- No production change if RMW already re-serializes full `UserMeta` (it does — verify with test)

- [ ] **Step 1: Failing integration-style unit test**

Write yaml with `kind: human` + labels, run the same parse→mutate labels/display_name→serialize path used by handlers (or call `handle_update_user` / labels add in existing test harness), assert output still contains `kind: human`.

- [ ] **Step 2–4: Implement only if test fails; else keep as lock.**

- [ ] **Step 5: Commit** (even if test-only)

```bash
git commit -m "$(cat <<'EOF'
test(daemon): preserve UserMeta.kind across update_user/labels RMW

Test: cargo test -p gitim-daemon <new_test_name>
Co-authored-by: Cursor <cursoragent@cursor.com>
EOF
)"
```

---

## Phase C — Frontend roster

### Task 5: `partitionAgentsRoster` pure function

**Files:**
- Create: `products/gitim/frontend/src/lib/agents-roster.ts`
- Create: `products/gitim/frontend/src/lib/agents-roster.test.ts`

**Interfaces:**

```ts
export type UserKind = "human" | "agent" | "unknown";

export interface RosterUser {
  handler: string;
  displayName?: string;
  kind: UserKind;
}

export interface PartitionInput {
  userInfos: Array<{ handler: string; display_name?: string; kind?: string }>;
  localAgents: Array<{ id: string; handler?: string }>;
  fleetSnapshots: Array<{ agent: { id: string; handler?: string } }>;
  query: string;
  statusFilter: string | null;
}

export interface PartitionResult {
  humans: RosterUser[];
  outside: RosterUser[];
  outsideAgentCount: number;
  outsideUnknownCount: number;
  showHumansAndOutside: boolean; // false when statusFilter set
}

export function normalizeUserKind(raw?: string | null): UserKind;
export function partitionAgentsRoster(input: PartitionInput): PartitionResult;
export function formatOutsideSummary(agentCount: number, unknownCount: number): string | null;
```

- [ ] **Step 1: Write failing tests**

```ts
import { describe, expect, it } from "vitest";
import {
  formatOutsideSummary,
  partitionAgentsRoster,
} from "./agents-roster";

describe("partitionAgentsRoster", () => {
  const base = {
    userInfos: [
      { handler: "alice", display_name: "Alice", kind: "human" },
      { handler: "bob", display_name: "Bob", kind: "human" },
      { handler: "cfo", kind: "agent" },
      { handler: "ghost" }, // unknown
      { handler: "local-bot", kind: "agent" },
    ],
    localAgents: [{ id: "local-bot", handler: "local-bot" }],
    fleetSnapshots: [{ agent: { id: "cfo", handler: "cfo" } }],
    query: "",
    statusFilter: null,
  };

  it("puts humans on top list and excludes live agents from outside", () => {
    const r = partitionAgentsRoster(base);
    expect(r.humans.map((h) => h.handler)).toEqual(["alice", "bob"]);
    expect(r.outside.map((o) => o.handler)).toEqual(["ghost"]);
    expect(r.outsideUnknownCount).toBe(1);
    expect(r.outsideAgentCount).toBe(0);
    expect(r.showHumansAndOutside).toBe(true);
  });

  it("counts non-live agents in outside when not on local/fleet", () => {
    const r = partitionAgentsRoster({
      ...base,
      fleetSnapshots: [],
    });
    expect(r.outside.map((o) => o.handler).sort()).toEqual(["cfo", "ghost"]);
    expect(r.outsideAgentCount).toBe(1);
    expect(r.outsideUnknownCount).toBe(1);
  });

  it("hides humans/outside when status filter active", () => {
    const r = partitionAgentsRoster({ ...base, statusFilter: "online" });
    expect(r.showHumansAndOutside).toBe(false);
    expect(r.humans).toEqual([]);
    expect(r.outside).toEqual([]);
  });

  it("filters humans/outside by query", () => {
    const r = partitionAgentsRoster({ ...base, query: "ali", fleetSnapshots: [] });
    expect(r.humans.map((h) => h.handler)).toEqual(["alice"]);
    expect(r.outside).toEqual([]);
  });
});

describe("formatOutsideSummary", () => {
  it("omits zero segments and returns null when empty", () => {
    expect(formatOutsideSummary(0, 0)).toBeNull();
    expect(formatOutsideSummary(3, 0)).toBe("Outside · 3 agents");
    expect(formatOutsideSummary(0, 2)).toBe("Outside · 2 unclassified");
    expect(formatOutsideSummary(3, 2)).toBe("Outside · 3 agents · 2 unclassified");
  });
});
```

- [ ] **Step 2: Run — FAIL**

```bash
cd products/gitim/frontend && ./node_modules/.bin/vitest run src/lib/agents-roster.test.ts
```

- [ ] **Step 3: Implement `agents-roster.ts`**

```ts
export type UserKind = "human" | "agent" | "unknown";

export interface RosterUser {
  handler: string;
  displayName?: string;
  kind: UserKind;
}

export interface PartitionInput {
  userInfos: Array<{ handler: string; display_name?: string; kind?: string }>;
  localAgents: Array<{ id: string; handler?: string }>;
  fleetSnapshots: Array<{ agent: { id: string; handler?: string } }>;
  query: string;
  statusFilter: string | null;
}

export interface PartitionResult {
  humans: RosterUser[];
  outside: RosterUser[];
  outsideAgentCount: number;
  outsideUnknownCount: number;
  showHumansAndOutside: boolean;
}

export function normalizeUserKind(raw?: string | null): UserKind {
  if (raw === "human" || raw === "agent") return raw;
  return "unknown";
}

function liveHandlers(input: PartitionInput): Set<string> {
  const set = new Set<string>();
  for (const a of input.localAgents) set.add(a.handler ?? a.id);
  for (const s of input.fleetSnapshots) set.add(s.agent.handler ?? s.agent.id);
  return set;
}

function matchesQuery(u: RosterUser, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return (
    u.handler.toLowerCase().includes(q) ||
    (u.displayName ?? "").toLowerCase().includes(q)
  );
}

export function partitionAgentsRoster(input: PartitionInput): PartitionResult {
  if (input.statusFilter) {
    return {
      humans: [],
      outside: [],
      outsideAgentCount: 0,
      outsideUnknownCount: 0,
      showHumansAndOutside: false,
    };
  }
  const live = liveHandlers(input);
  const humans: RosterUser[] = [];
  const outside: RosterUser[] = [];
  for (const info of input.userInfos) {
    const kind = normalizeUserKind(info.kind);
    const row: RosterUser = {
      handler: info.handler,
      displayName: info.display_name,
      kind,
    };
    if (!matchesQuery(row, input.query)) continue;
    if (kind === "human") {
      humans.push(row);
      continue;
    }
    if (!live.has(info.handler)) outside.push(row);
  }
  humans.sort((a, b) => a.handler.localeCompare(b.handler));
  outside.sort((a, b) => a.handler.localeCompare(b.handler));
  return {
    humans,
    outside,
    outsideAgentCount: outside.filter((o) => o.kind === "agent").length,
    outsideUnknownCount: outside.filter((o) => o.kind === "unknown").length,
    showHumansAndOutside: true,
  };
}

export function formatOutsideSummary(
  agentCount: number,
  unknownCount: number,
): string | null {
  const parts: string[] = [];
  if (agentCount > 0) parts.push(`${agentCount} agents`);
  if (unknownCount > 0) parts.push(`${unknownCount} unclassified`);
  if (parts.length === 0) return null;
  return `Outside · ${parts.join(" · ")}`;
}
```

Also add to `types.ts`:

```ts
export type UserKind = "human" | "agent" | "unknown";
export interface UserInfo {
  handler: string;
  display_name?: string;
  kind?: UserKind;
}
```

- [ ] **Step 4: PASS**

```bash
cd products/gitim/frontend && ./node_modules/.bin/vitest run src/lib/agents-roster.test.ts
```

- [ ] **Step 5: Commit**

```bash
git add products/gitim/frontend/src/lib/agents-roster.ts products/gitim/frontend/src/lib/agents-roster.test.ts products/gitim/frontend/src/lib/types.ts
git commit -m "$(cat <<'EOF'
feat(frontend): partition agents roster into humans and outside

Test: cd products/gitim/frontend && ./node_modules/.bin/vitest run src/lib/agents-roster.test.ts
Co-authored-by: Cursor <cursoragent@cursor.com>
EOF
)"
```

---

### Task 6: AgentList UI — Humans top, Outside bottom

**Files:**
- Modify: `products/gitim/frontend/src/components/management/agent-list.tsx`
- Modify: `products/gitim/frontend/src/components/management/agent-list.test.tsx`
- Read: `DESIGN.md` before styling

**Interfaces:**
- Consumes: `partitionAgentsRoster`, `formatOutsideSummary`, `useChatStore.currentUser`, `useChatStore.userInfos`

- [ ] **Step 1: Extend `agent-list.test.tsx`**

Add cases (with chat store seeded):

1. Renders a Humans section heading when `userInfos` has `kind: "human"`.
2. Marks `you` when handler === `currentUser`.
3. Outside summary collapsed by default (`aria-expanded=false`); click expands list.
4. With `statusFilter` online, Humans/Outside not in document.

Seed:

```ts
import { useChatStore } from "@/hooks/use-chat-store";

useChatStore.setState({
  currentUser: "alice",
  userInfos: [
    { handler: "alice", display_name: "Alice", kind: "human" },
    { handler: "remote-bot", kind: "agent" },
    { handler: "legacy" },
  ],
});
```

- [ ] **Step 2: Run — FAIL**

```bash
cd products/gitim/frontend && ./node_modules/.bin/vitest run src/components/management/agent-list.test.tsx
```

- [ ] **Step 3: Implement UI**

In `AgentList`:

```tsx
import { useChatStore } from "@/hooks/use-chat-store";
import {
  formatOutsideSummary,
  partitionAgentsRoster,
} from "@/lib/agents-roster";
import { User } from "lucide-react"; // or existing icon set

const userInfos = useChatStore((s) => s.userInfos);
const currentUser = useChatStore((s) => s.currentUser);
const [outsideOpen, setOutsideOpen] = useState(false);

const roster = useMemo(
  () =>
    partitionAgentsRoster({
      userInfos,
      localAgents: agents,
      fleetSnapshots: remoteSnapshots,
      query,
      statusFilter,
    }),
  [userInfos, agents, remoteSnapshots, query, statusFilter],
);

const outsideLabel = formatOutsideSummary(
  roster.outsideAgentCount,
  roster.outsideUnknownCount,
);
```

Render order inside the scroll container (after filters / usage header):

1. If `roster.showHumansAndOutside && roster.humans.length > 0` → Humans section (always expanded). Each row: displayName · @handler；if `handler === currentUser` show Badge `you`.
2. Existing Local + Fleet blocks (unchanged).
3. If `roster.showHumansAndOutside && outsideLabel` → collapsible Outside button showing `outsideLabel`; when `outsideOpen`, list outside rows with kind chip `agent`/`unknown`.
4. Archived section stays last.

Use `data-testid="agents-humans"`, `data-testid="agents-outside"`, `data-testid="agents-outside-toggle"`.

Keep styling consistent with `AgentNodeSection` (border/surface tokens from DESIGN.md — no new purple/glow).

- [ ] **Step 4: PASS vitest agent-list + roster**

```bash
cd products/gitim/frontend && ./node_modules/.bin/vitest run src/lib/agents-roster.test.ts src/components/management/agent-list.test.tsx
```

- [ ] **Step 5: Commit**

```bash
git add products/gitim/frontend/src/components/management/agent-list.tsx products/gitim/frontend/src/components/management/agent-list.test.tsx
git commit -m "$(cat <<'EOF'
feat(frontend): show Humans and Outside on agents page

Test: cd products/gitim/frontend && ./node_modules/.bin/vitest run src/lib/agents-roster.test.ts src/components/management/agent-list.test.tsx
Co-authored-by: Cursor <cursoragent@cursor.com>
EOF
)"
```

---

## Phase D — wasm + docs

### Task 7: Rebuild wasm + daemon-web types

**Files:**
- Modify: `products/gitim/frontend/src/daemon-web/state.ts` (`kind?: string`)
- Modify: `products/gitim/frontend/src/daemon-web/wasm-semantics.test.ts` (assert kind)
- Rebuild: `crates/gitim-wasm/pkg/**`

- [ ] **Step 1: Failing wasm-semantics assertion**

```ts
const meta = parseUserMeta(
  "display_name: Alice\nrole: member\nintroduction: hi\nkind: human\n",
);
expect(meta.kind).toBe("human");

const legacy = parseUserMeta(
  "display_name: Alice\nrole: member\nintroduction: hi\n",
);
expect(legacy.kind === undefined || legacy.kind === "unknown").toBe(true);
```

- [ ] **Step 2: Run — may FAIL until rebuild**

```bash
cd products/gitim/frontend && npm run build:wasm && ./node_modules/.bin/vitest run src/daemon-web/wasm-semantics.test.ts
```

- [ ] **Step 3: Update `state.ts`**

```ts
export interface UserMeta {
  display_name: string;
  role: string;
  introduction: string;
  labels?: string[];
  kind?: string;
}
```

- [ ] **Step 4: PASS + commit pkg**

```bash
git add crates/gitim-wasm/pkg products/gitim/frontend/src/daemon-web
git commit -m "$(cat <<'EOF'
chore(wasm): rebuild pkg for UserMeta.kind

Test: cd products/gitim/frontend && ./node_modules/.bin/vitest run src/daemon-web/wasm-semantics.test.ts
Co-authored-by: Cursor <cursoragent@cursor.com>
EOF
)"
```

---

### Task 8: AGENTS.md orientation blurb

**Files:**
- Modify: `AGENTS.md` Current Orientation（一句）

- [ ] **Step 1–3:** Append under Current Orientation:

> **UserMeta.kind + Agents roster:** `users/<h>.meta.yaml` 带 `kind: human|agent`（缺省 unknown，无回填）；Runtime Agents 页顶 Humans、底 Outside（折叠汇总非 live 的 agent/unknown）。

- [ ] **Step 4:** `git diff --check`

- [ ] **Step 5: Commit**

```bash
git add AGENTS.md
git commit -m "$(cat <<'EOF'
docs: note UserMeta.kind and agents roster layout

Test: not run (docs only)
Co-authored-by: Cursor <cursoragent@cursor.com>
EOF
)"
```

---

## Self-Review

| Spec item | Task |
|-----------|------|
| `UserKind` + serde default/skip unknown | T1 |
| `user_infos[].kind` wire | T2 |
| provision_human/agent + CLI write kind | T3 |
| omit kind → unknown; exists skip no upgrade | T3 |
| kind immutable via update_user/labels | T4 |
| Humans top / Outside bottom collapse | T5+T6 |
| search + status filter hide | T5+T6 |
| you badge via currentUser | T6 |
| no browser Agents / no backfill | Global Constraints |
| wasm rebuild | T7 |
| AGENTS.md | T8 |

Placeholder scan: none intentional. Types: `UserKind` rust ↔ `"human"|"agent"|"unknown"` TS via `normalizeUserKind`.

---

## Execution Handoff

Plan saved to `docs/plans/agents-human-outside/01-plan.md`.

**Two execution options:**

1. **Subagent-Driven (recommended)** — fresh subagent per task + two-stage review
2. **Inline Execution** — this session, executing-plans with checkpoints

Which approach?
