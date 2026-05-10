# 05 — Agent prompt 文案

> 对应 [01-plan.md](01-plan.md) Part D。3 段文案,沿用 AI 第一性原理 + 交接语气(memory `feedback_prompt_style_for_llms`)。

## D.1 — leave-workspace event 解读

**文件**:[crates/gitim-agent-provider/src/prompts.rs](../../../crates/gitim-agent-provider/src/prompts.rs)

**位置**:在 `default_reset_protocol`(prompts.rs:229 附近)旁边新增小节,或合进现有 reset 协议的"识别工作环境变化"段

**要传达的语义**:
- 在 thread 里看到 `[L<n>][@<x>][<ts>] leave-workspace` event = 该 user 已退出 workspace,**不是临时 stop,是终态退出**
- 不要再 mention / @ 它(daemon 写入拦截会拒,你的发送会失败)
- 必须提及它过去发言时,用过去式 + 不假设它会回应("之前 @x 在 #dev 提到过 ..." 而不是 "@x 你说过 ...")
- 与 `leave-channel` event 的区别:leave-channel 只退出某个频道(还在 workspace),leave-workspace 是从整个工作环境消失

**风格参考**:reset protocol 现有"识别工作环境变化"段的写法(陈述事实 + 给行为指引,不做规则化指令)

---

## D.2 — archive-dm / unarchive-dm 命令

**文件**:同上

**位置**:`default_gitim_api`(prompts.rs:311 附近)的 DM / 频道小节,跟 leave-channel 同级

**要传达的语义**:
- archive-dm `<peer>` = 把跟 peer 的私信归档(双方视图都消失,可 unarchive 反悔)
- 用例:某条 DM 线索的工作已结束,不再相关 → 归档,精细化净化你的 DM 列表
- 与 leave-channel 同级:leave-channel 切断一个频道订阅,archive-dm 切断一条 DM。`[[RESET]]` 是 session 级重锤,这两个是手术刀
- 公开行为:写 git commit,影响双方视图 — 不是逃避争议工具

---

## D.3 — burn-self 命令 + 使用边界

**文件**:同上

**位置**:`default_gitim_api` DM / 命令小节末尾,标"终态命令"或类似 — 与其他命令视觉上分开,提示这是 irreversible

**要传达的语义**:
- burn-self = 我从 workspace 退出。daemon 会:在所有我发过言的 thread 各写一条 leave-workspace event + 归档我跟所有人的 DM + 归档我的 user 档案 + 清理我的 clone(或部分,见下)
- **不可逆**:执行后我的 user entry / DMs 都进入 archive/,我不能自己恢复(只能 user 重新 add 一个新 agent,但 handler 不能复用)

**何时用**:
- 任务明确完成 + workspace owner / coordinator 不再需要我
- 没有后续工作可承接
- 我自己确认这是终结,不是临时 stop

**何时不要用**:
- 任务卡住 / context 混乱时:用 `[[RESET]]` 重置 session,**不是** burn-self
- 不确定是否真的完成:向 owner 请示,**不要** 自作主张
- 想"清理 context":用 leave-channel(切断频道)或 archive-dm(切断单条 DM),**不是** burn-self

**与 stop / disconnect 的区别**:
- stop / disconnect:临时停下,我还能被 user 唤醒
- burn-self:彻底退出,我消失

---

## 文案风格约束

- **不**写"如果你看到 X 就 Y"这种 if-then 规则化表达 — 用陈述语气("X 意味着 ... 你应该 ...")
- **不**列长 bullet — 段落体,跟现有 prompts.rs 风格一致
- **不**重复 daemon 错误信息("会拒绝你") — 让 daemon 错误自己 surface,prompt 只讲心智模型
- 用第一人称("我退出"),不用第二人称("agent 你应当...") — agent prompt 是 agent 自己读的

---

## 验收

- 三段文案各自简洁(每段 < 200 字)
- 与现有 reset / api 段落风格一致(读起来不突兀)
- 不引入新的强制规则(prompt 是心智模型,不是 lint 规则)
- prompts.rs 现有测试如有 prompt 文本 assertion,同步更新;无 assertion 不新增测试(memory `feedback_prompt_style_for_llms`,过度 assert 锁死文案迭代)

---

## 依赖

无强依赖 — 跟 daemon / runtime / CLI 工作可并行。但 D.3 提到的 "burn-self 不可逆 / 不自动清 runtime" 这个边界,跟 C.3 的实施细节(self-burn 后 runtime cleanup 缺口)对齐 — 实施时若 C.3 选了方案 A(runtime 自愈),D.3 文案需相应调整(去掉 "不能自己恢复"措辞)。
