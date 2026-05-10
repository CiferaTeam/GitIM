# 06 — WebUI:agent burn 按钮 + DM archive + show-archived

> 对应 [01-plan.md](01-plan.md) Part E。三个独立 sub-part(P2.c),独立 PR 友好。

## E.1 — channel show-archived toggle:现状 verify

**目的**:确认 channel-archive 现役 WebUI 是否已有 show-archived toggle。如果有,本 sub-part 是 no-op。如果没有,补齐(规模小独立 PR)。

**verify 步骤**:
- 浏览 [products/gitim/frontend/src/](../../../products/gitim/frontend/src/) 找 channel 列表 component
- grep "archived" / "归档" / 看是否有过滤逻辑

**如有,本 sub-part 关闭**。如无,补 toggle:
- 默认 hide archived channels
- toggle 切换显示 archive/channels/ 内容(daemon 已支持 archived_channels API)

**验收**:channel 列表能切换显示 archived 频道(若需补齐),否则 sub-part 文档化 no-op 并 close

**依赖**:无

---

## E.2 — DM 列表 archive 操作

**文件**:`products/gitim/frontend/src/...`(具体 component path 实施时定)

**改动**:
- DM 列表 component:每条 DM 加"归档"按钮(右键菜单 / hover action),点击调 `archive_dm(peer)`
- DM 列表 add show-archived toggle(默认 hide,与 channel 列表对齐)
- show-archived 时数据 source 切到 `list_archived_dms` API
- archived DM 渲染:打 archive 标记 + 仍可点开 read-only(read fallback 已支持)

**API 调用**:通过 daemon-web 现有路径(WebUI → backend → daemon socket)

**验收**:
- archive 按钮触发后 DM 从 default 列表消失,toggle 后可见
- archived DM 列表项 read-only(send 拦截会拒,UI 应该禁用 input)
- unarchive button 在 archived 状态可见,点击恢复

**依赖**:A.3 + A.6(daemon archive_dm / list_archived_dms / DM read fallback)

---

## E.3 — agent burn 按钮 + agent 列表 + dual-source

**文件**:
- `products/gitim/frontend/src/.../agent-detail.tsx`
- `products/gitim/frontend/src/.../agent-list.tsx`

### E.3.a — agent detail 页 burn 按钮

**改动**:
- "Hard Delete" 按钮 → 替换为 **"Burn"**(标红 + 醒目区域)
- 二次确认 dialog 文案:
  > 确认要 burn agent **@<handler>**?
  > 此操作将:
  > - 在它发过言的所有频道写入 leave-workspace 事件
  > - 归档它的 user 档案与所有 DM(WebUI 默认看不到,可手动 unarchive)
  > - 清理它的 clone 目录(物理删除)
  > 
  > **handler 不能再被新 agent 复用**(handler 终身唯一)。
  > 操作可部分恢复(unarchive user / DM),但 agent runtime 需重新 add。
  > 
  > [取消]   [确认 Burn]
- 按钮触发 → `POST /workspaces/{slug}/agents/burn { id }`(B.1)
- 不再调用 `agents/remove`(B.2 标 deprecated)

### E.3.b — agent 列表 dual-source

**改动**:
- 默认数据 source:runtime `GET /agents`(`ctx.agents`)— 现状
- show-archived toggle:切到 daemon `list_archived_users`(P2.e)
- archived agents 列表渲染:基础信息(handler / display_name)+ archive 标记 + 不显示 metadata(provider / model / messages_processed 等运行时字段,因为 runtime 早已不持有)
- archived agents 不可 stop / start / edit;唯一可操作是 "Unarchive User"(内部命令,UI 暴露给 user 用 — 但仍不能 unburn 完整 agent runtime)

**验收**:
- Burn 按钮二次确认 → 触发 burn endpoint → SSE 收到 burned event → 列表刷新
- show-archived toggle 切换两个数据 source
- archived agent 行 disabled 大部分操作

**依赖**:B.1 + A.2(list_archived_users)

---

## 整体依赖

E.1 / E.2 / E.3 三个独立 sub-part。

- E.1:0 dependency(单独 PR)
- E.2:依赖 A.3 / A.6(daemon DM archive surface)
- E.3:依赖 B.1(runtime burn) + A.2(list_archived_users)

实施顺序建议:E.1 先(verify 工作,可能 no-op)→ E.2 / E.3 在 daemon / runtime PR 合入后开,各自独立 PR。

---

## 测试

WebUI 测试 v1 不上 playwright(可选,scope 外)。手工 smoke:
- E.2:WebUI 跑通 DM archive / unarchive 一遍
- E.3:WebUI 跑通 burn 一遍,看二次确认 + 列表刷新 + show-archived 切换
- 这部分 smoke 在 [01-plan.md](01-plan.md) 合并前校验清单第 7 条已涵盖
