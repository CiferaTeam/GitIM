# Hermes Multi-LLM QA 检查清单

> 覆盖 T0–T15 所有实现。标注 **[自动化]** 的条目有对应单元或集成测试覆盖,无需每次手动验证;标注 **[手动]** 的条目需人工操作确认。

---

## 1. LLM section 出现/消失逻辑

**条件:** WebUI AddAgentDialog 中,provider 选 hermes 时 LLM 选择区域出现;切换到 claude / codex 时消失。

- 验证方法:打开 AddAgentDialog → 选 hermes → 确认 "LLM Provider" 和 "Model" 字段渲染 → 切换到 claude → 确认字段消失。**[手动]**

---

## 2. 已配 provider 在 dropdown 中出现

**条件:** 用户 `~/.hermes/.env` 中设置了 `KIMI_API_KEY`,`GET /hermes/llm/providers` 应返回包含 `kimi-coding` 的列表。

- 验证方法:在 `.env` 中加 `KIMI_API_KEY=sk-xxx`,重启 runtime,打开 AddAgentDialog 选 hermes,确认 kimi-coding 出现在 provider dropdown。**[手动]**

---

## 3. 未配 provider 不在 dropdown

**条件:** 某 provider 未在 `.env` 中设置 API key,该 provider 不应出现在列表中。

- 验证方法:清空 `~/.hermes/.env`(或备份后置空),重启 runtime,确认 provider dropdown 仅返回 anthropic(内置无需 key)或空列表。**[手动]**

---

## 4. 选 provider 后 model 列表 live fetch 及 loading 提示

**条件:** 在 provider dropdown 中选择一个 provider,前端发起 `GET /hermes/llm/providers/<name>/models`,期间显示 loading 状态。

- 验证方法:选择 provider → 确认 Model 字段出现 loading 指示(spinner 或 "Loading..." 文本) → loading 结束后模型列表填充。**[手动]**

---

## 5. Live fetch 失败显示 error + Custom 输入框

**条件:** `GET /hermes/llm/providers/<name>/models` 返回错误或空列表时,UI 显示错误提示并呈现 Custom 文本输入框让用户手动填写 model 名。

- 验证方法:拔掉网络或将 base_url 改为无效地址 → 选该 provider → 确认 error 提示出现 + Custom 输入框可用。**[手动]**

---

## 6. Custom 输入提交后 me.json 落字段

**条件:** 用户手动输入 model 名并提交 Add,`<agent-clone>/.gitim/me.json` 中应写入 `llm_provider` 和 `llm_model` 字段。

- 验证方法:在 Custom 框输入 `my-custom-model` → 提交 → `cat <agent-clone>/.gitim/me.json | grep llm`。**[手动]**
- 也被 **[自动化]** 单元测试覆盖:`http.rs` add_agent 路径的 `llm_provider`/`llm_model` 字段落盘测试。

---

## 7. Detect 按钮带 llm 参数验证真实 handshake

**条件:** Detect 按钮点击时,后端以 `llm_provider` + `llm_model` 作为 override 对目标 hermes profile 做 preflight handshake,而非 default profile 的当前 LLM。

- 验证方法:选择与 default profile 不同的 provider/model → 点 Detect → 检查 runtime 日志确认 preflight 使用了指定的 `llm_provider`/`llm_model`。**[手动]**

---

## 8. Detect 失败 → Add 按钮 disabled

**条件:** Detect 失败(网络错误、key 无效、model 不存在)时,Add 按钮保持 disabled 状态,不允许提交。

- 验证方法:填写无效 model 名 → 点 Detect → 确认返回失败 → Add 按钮仍为 disabled。**[手动]**
- **[自动化]** T8 测试覆盖后端 detect 端点错误路径。

---

## 9. Add 成功后 hermes profile config 写入

**条件:** `POST /agents/add` 成功后,`~/.hermes/profiles/gitim-<handler>/config.yaml` 中 `model` 子树包含正确的 `provider`、`default`、`base_url`。

- 验证方法:Add 成功后 `cat ~/.hermes/profiles/gitim-<handler>/config.yaml | grep -A3 model`。**[手动]**
- **[自动化]** `http.rs` 集成测试覆盖 `apply_model_config` 三次 config-set 的 happy path(httpmock)。

---

## 10. apply_model_config 中途失败 → profile 被删 + 错误提示

**条件:** `hermes config set` 序列中途失败(如 binary 被 rename 或 PATH 中断),后端应回滚:delete_profile + cleanup_agent_dir,返回 500,前端显示错误。

- 验证方法(手动破坏法):
  1. 准备一个 wrapper script 替换 `hermes`,前两次调用成功,第三次返回非零 exit code。
  2. 提交 Add → 确认返回 500 + 错误提示。
  3. 确认 `~/.hermes/profiles/gitim-<handler>/` 不存在(已回滚)。
  4. 恢复真实 `hermes` binary。**[手动]**
- **[自动化]** `apply_model_config` 单元测试覆盖中途失败的 cleanup 路径。

---

## 11. 已有 claude/codex agent 不受影响

**条件:** 新增或修改 hermes multi-LLM 功能后,已有的 claude/codex agent 正常 poll、收消息、回复,无报错。

- 验证方法:检查已有 claude/codex agent 的 agent_loop 日志,确认无新增错误;发送测试消息确认正常响应。**[手动]**
- **[自动化]** 全量 `cargo test --workspace` 覆盖 claude/codex 路径的 unit tests。

---

## 12. Dialog close 后再开 state 重置

**条件:** 关闭 AddAgentDialog 后重新打开,LLM provider/model 选择、Detect 状态、error 消息均重置为初始态。

- 验证方法:选 hermes → 选 provider → 点 Detect → 关闭 dialog → 重新打开 → 确认 LLM section 为空/初始状态。**[手动]**

---

## 13. POST /agents/add provider=hermes 缺 llm_provider → 400

**条件:** `POST /agents/add` body 中 `provider` 为 `hermes` 但缺少 `llm_provider` 字段时,后端返回 400 而非 500 或静默成功。

- 验证方法(curl):
  ```bash
  curl -s -X POST http://localhost:<port>/workspaces/<slug>/agents/add \
    -H 'Content-Type: application/json' \
    -d '{"handler":"test-llm","provider":"hermes","system_prompt":"x"}' \
    | jq .
  ```
  期望: `{"error": "...", "error_code": "missing_llm_provider"}` + HTTP 400。**[手动]**
- **[自动化]** `http.rs` 参数校验单元测试覆盖此路径。

---

## 14. GET /hermes/llm/providers hermes_home 缺失时返 200 空列表

**条件:** 当 hermes 未安装或 `HERMES_HOME` 目录不存在时,`GET /hermes/llm/providers` 返回 HTTP 200 + 空数组,不返回 500。

- 验证方法:在测试环境设置 `HERMES_HOME=/nonexistent` 并重启 runtime → `curl http://localhost:<port>/hermes/llm/providers` → 期望 `[]` + 200。**[手动]**
- **[自动化]** T14 e2e 测试覆盖此 fallback 路径(`introspect.rs` + `http.rs` 单元测试)。

---

## 15. GET /hermes/llm/providers/<unknown>/models → 400

**条件:** 请求一个不存在 provider 名的 models 端点,后端返回 400(不是 404,因为路由已匹配,是参数不合法)。

- 验证方法(curl):
  ```bash
  curl -s http://localhost:<port>/hermes/llm/providers/nonexistent-provider/models | jq .
  ```
  期望: HTTP 400 + `{"error_code": "unknown_provider"}` 或等价错误。**[手动]**
- **[自动化]** `models.rs` + `http.rs` 单元测试覆盖 unknown provider 路径。

---

## 16. kimi-coding base_url 前缀自动解析

**条件:** 用户 `KIMI_API_KEY` 以 `sk-kimi-` 开头时,introspect 返回的 `base_url` 为 `https://api.kimi.com/coding/v1` 而非默认的 `https://api.moonshot.ai/v1`。

- 验证方法:`GET /hermes/llm/providers` 确认 kimi-coding 的 `base_url` 字段根据 key 前缀正确分支。**[手动]**
- **[自动化]** `introspect.rs` 单元测试覆盖 kimi prefix detection 两条分支。

---

## 17. minimax / minimax-cn 无 /models 时返回静态 fallback

**条件:** minimax 系列 provider 使用 Anthropic protocol(base_url 含 `/anthropic` suffix),`/models` 返回 404;后端应返回预定义静态模型列表或空列表,而非将 404 透传为错误。

- 验证方法:选择 minimax provider → 确认 Model dropdown 显示静态 fallback 列表(如 `abab6.5s-chat`)或提示"请手动输入"。**[手动]**
- **[自动化]** `models.rs` 单元测试覆盖 Anthropic-protocol provider 的静态 fallback 路径。

---

## 18. custom_providers 在 config.yaml 中被正确 introspect

**条件:** 用户在 hermes `config.yaml` 中配置了 `custom_providers`,这些 provider 应出现在 `GET /hermes/llm/providers` 结果中。

- 验证方法:向 `~/.hermes/config.yaml` 添加一条 custom_provider → 重启 runtime → 确认出现在 provider 列表。**[手动]**
- **[自动化]** `introspect.rs` 单元测试覆盖 `custom_providers` 解析。

---

## 覆盖率摘要

| 条目 | 类型 | 对应任务 |
|------|------|----------|
| 1–5 | 主要手动 | T12–T15(前端) |
| 6 | 手动 + 自动 | T9(me.json 落盘) |
| 7–8 | 手动 + 自动 | T8(detect 端点) |
| 9 | 手动 + 自动 | T10(apply_model_config) |
| 10 | 手动 + 自动 | T10(回滚路径) |
| 11 | 手动 + 自动 | T1–T15 全量测试 |
| 12 | 手动 | T12–T15(前端状态重置) |
| 13 | 手动 + 自动 | T9(参数校验) |
| 14 | 手动 + 自动 | T4(introspect fallback) |
| 15 | 手动 + 自动 | T6(models endpoint) |
| 16 | 手动 + 自动 | T3(kimi prefix detection) |
| 17 | 手动 + 自动 | T5–T6(Anthropic-protocol fallback) |
| 18 | 手动 + 自动 | T4(custom_providers introspect) |
