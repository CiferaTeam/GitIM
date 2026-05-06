# 多 Provider 支持 — 手动 QA 检查清单

本清单覆盖 `feature/multi-provider` 分支端到端行为。逐条执行，每条独立验证。

## 前置准备

- **Runtime**: `cargo build -p gitim-runtime && ./target/debug/gitim-runtime`（默认监听 `http://127.0.0.1:16868`）
- **WebUI**: `cd webui-v2 && pnpm dev`（默认 `http://127.0.0.1:5173`）
- **Workspace**: 至少一个 provisioned agent workspace，`<workspace>/<handler>/.gitim/me.json` 可写
- **CLI 可用性**：`claude --version` 和 `codex --version` 能返回版本号（有登录态），否则只能部分验证
- 下文凡 `<port>` 指 runtime 端口（默认 `16868`）；凡 `<handler>` 指被测 agent handler

---

## 1. 新建 Claude agent 正流程

**Steps:**
1. WebUI 打开 Management / Agents 页面，点 "Add Agent"
2. Name 填 `claude-bot-qa`
3. Provider 下拉选 `Claude`
4. Model 下拉选 `Claude Haiku 4.5`
5. 点 "Detect" 按钮

**Expected:**
- Detect 期间按钮显示 loading
- 绿勾 + `OK — <N> ms` 出现（N ≈ 几千 ms）
- "Add" 按钮由 disabled 变为 enabled
- 点 Add → Dialog 关闭
- 列表里出现 `claude-bot-qa`，状态为 `idle` 或 `running`（非 `error`）

---

## 2. 新建 Codex agent 正流程

**Steps:**
1. 点 "Add Agent"
2. Name 填 `codex-bot-qa`
3. Provider 选 `Codex`
4. Model 选 `GPT-5.4`
5. 点 Detect → 等绿勾 → 点 Add

**Expected:**
- Detect 绿勾（本地登录了 codex 的情况下）
- Dialog 关闭，`codex-bot-qa` 出现在列表，状态 idle/running

---

## 3. 切换 provider 后 Detect 状态重置

**Steps:**
1. 打开 Add Agent Dialog
2. Provider 选 `Claude`，Model 选 Haiku 4.5
3. 点 Detect → 等绿勾
4. Provider 下拉切到 `Codex`

**Expected:**
- Detect 旁的绿勾消失（`detectResult` 被清空）
- Model 下拉重新变为空 / 只展示 Codex 的两个 model
- "Add" 按钮恢复 disabled
- Detect 按钮可重新点击（disabled 仅在 provider 为空或 detecting 时触发）

---

## 4. 未 Detect 点 Add 无效

**Steps:**
1. 打开 Dialog，填 Name，Provider 选 `Claude`，Model 选 Haiku 4.5
2. **不**点 Detect
3. 观察 Add 按钮

**Expected:**
- "Add" 按钮保持 disabled（hover 不触发 submit）
- 尝试按 Enter 键提交 → 表单不提交（handleSubmit 提前 return）

---

## 5. Detect 失败时 Add disabled + 错误文案

**Steps:**
1. 临时隐藏 claude binary：`mv $(which claude) /tmp/claude-backup`
2. 打开 Add Dialog，Provider 选 `Claude`，Model 选 Haiku 4.5
3. 点 Detect
4. 验证完成后恢复：`mv /tmp/claude-backup $(which claude)`

**Expected:**
- 红叉 + 文案 `CLI not found. Install claude/codex and retry.`（对应 `error_kind: "not_installed"`）
- Add 按钮保持 disabled
- 恢复 binary 后再点 Detect → 绿勾
- 如 binary 存在但 `claude login` 未登录 → 文案应展示原始 error（`error_kind: "other"`），Add 同样 disabled

---

## 6. Detect 状态在 Dialog 关闭后重置

**Steps:**
1. 打开 Add Dialog，选 Claude + Haiku，Detect 成功绿勾
2. 点 "Cancel" 或关闭 Dialog
3. 再点 "Add Agent" 重新打开

**Expected:**
- Name 输入框为空
- Provider 下拉回到 "— Select provider —"（空值）
- Model 下拉 disabled 或空
- Detect 旁无绿勾、无红叉（`detectResult=null`）
- Add 按钮 disabled

---

## 7. me.json 缺 provider → recover 登记为 Error

**Steps:**
1. 选一个已创建 agent，定位其 workspace：`<workspace>/<handler>/.gitim/me.json`
2. 编辑 me.json，**删除** `"provider"` 字段，保存
3. Kill runtime 进程：`pkill -f gitim-runtime` 或 `Ctrl-C`
4. 重启 runtime：`./target/debug/gitim-runtime`
5. WebUI 刷新 Management 页面

**Expected:**
- 该 agent 卡片显示红色 `Error` badge
- 卡片下方小字错误提示类似：`Missing "provider" in <path>/.gitim/me.json. Add "provider": "claude" or "provider": "codex" to the file and restart the runtime.`
- 点卡片不会进入聊天 / 不会尝试启动 agent loop
- Runtime 日志里有 `agent @<handler> recovered in error state: Missing "provider" ...` warn 日志

---

## 8. me.json provider="gemini" → recover 登记 Error

**Steps:**
1. 编辑同一 agent 的 me.json，把 `"provider"` 设成 `"gemini"`
2. 重启 runtime
3. WebUI 刷新

**Expected:**
- Agent 卡片 `Error` badge
- 错误文案：`Unsupported provider "gemini" in <path>/.gitim/me.json. Expected "claude" or "codex".`
- Agent loop 未启动
- 修复：恢复 `"provider": "claude"` 后重启，agent 回到 idle

---

## 9. POST /agents/add 缺 provider → 400

**Steps:**
```bash
curl -i -X POST -H 'Content-Type: application/json' \
  -d '{"handler":"qa-nobody","display_name":"QA"}' \
  http://127.0.0.1:<port>/agents/add
```

**Expected:**
- HTTP 400 Bad Request
- 响应 body 为 JSON 反序列化错误（axum 默认包装的 `missing field provider`）
- 没有 workspace 文件被创建

---

## 10. POST /agents/add provider="gemini" → 400

**Steps:**
```bash
curl -i -X POST -H 'Content-Type: application/json' \
  -d '{"handler":"qa-gemini","display_name":"QA","provider":"gemini"}' \
  http://127.0.0.1:<port>/agents/add
```

**Expected:**
- HTTP 400
- body JSON 含 `"error": "unsupported provider: gemini"`
- workspace 未创建

---

## 11. GET /preflight/unknown → 400

**Steps:**
```bash
curl -i http://127.0.0.1:<port>/preflight/unknown
```

**Expected:**
- HTTP 400 Bad Request
- body：`{"ok":false,"error":"unknown provider"}`

---

## 12. GET /preflight/claude 返回 PreflightResult shape

**Steps:**
```bash
curl -s http://127.0.0.1:<port>/preflight/claude | jq
```

**Expected:**
- HTTP 200（即便 binary 不存在也返回 200，`available: false` 承载错误）
- JSON 键覆盖：`available`（bool）、`provider` (`"claude"`)、`version`、`model_used`、`duration_ms` (number)、`output_preview`、`error`、`error_kind`
- CLI 存在并已登录 → `available: true`，`model_used: "claude-haiku-4-5"`，`output_preview` 含 `GITIM_OK`
- CLI 不存在 → `available: false`，`error_kind: "not_installed"`，`error` 含 `CLI not found`

---

## 13. GET /preflight/codex 返回 PreflightResult shape

**Steps:**
```bash
curl -s http://127.0.0.1:<port>/preflight/codex | jq
```

**Expected:**
- HTTP 200
- 结构同 12，`provider: "codex"`
- CLI 可用时 `model_used: "gpt-5.4-mini"`，`output_preview` 含 `GITIM_OK`
- CLI 不可用 → `available: false`，`error_kind: "not_installed"`

---

## 14. SSE /agents/events 推送 recover error event

**Steps:**
1. 启动 runtime（确认至少一个 agent 已 provisioned）
2. 另开终端订阅 SSE：
   ```bash
   curl -N http://127.0.0.1:<port>/agents/events
   ```
3. 编辑某 agent 的 me.json，删除 `"provider"`
4. Kill runtime，重启 runtime
5. 观察订阅流

**Expected:**
- 重启后流里能看到一条 `event_type: "error"` 的 event（`detail` 字段含 "Missing \"provider\"" 提示）
- `agent_id` 与被破坏的 agent handler 一致
- 正常 agent 不产生 error event

---

## 结尾回归

- [ ] `cargo test -p gitim-runtime` 全绿（不含 `--ignored`）
- [ ] `cd webui-v2 && pnpm exec tsc --noEmit` 无错误
- [ ] 老路径 `/preflight/claude` 的旧响应语义已被新 path-param 路由替换（14 条中 12/13 已覆盖，本条为提醒不要漏回归）
- [ ] 此清单 14 条全部通过 → 可以签字收尾
