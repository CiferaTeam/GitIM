# 已有 hermes agent 迁移指引

新版的 add_agent 会自动给每个 hermes agent 创建独立 profile,但**已有的 hermes agent** 在升级前没有这个 profile。升级后,它们的 `agent_loop` 启动时会注入 `HERMES_HOME=~/.hermes/profiles/gitim-<handler>` —— 如果该目录不存在,hermes 会按空 profile 行为运行(无 .env、无 model 配置),agent 第一次回复时会失败。

## 一行命令补建所有 profile

在你的 workspace 根目录(包含 `.gitim-runtime/agents/<handler>/` 子目录的那个)跑:

```bash
for d in .gitim-runtime/agents/*/; do
  handler=$(basename "$d")
  hermes profile create "gitim-$handler" --clone --no-alias 2>/dev/null || true
done
```

逻辑:
- 遍历每个已 provisioned 的 agent 目录
- handler = 子目录名
- 调 `hermes profile create gitim-<handler> --clone --no-alias`
- `2>/dev/null || true` 吞掉 "already exists" 报错(幂等)

## 不需要做的事

- **不需要重启 runtime / daemon** — 下次该 agent 跑 `agent_loop` 时,`build_provider_config` 自动注入 HERMES_HOME 指向新建的 profile,hermes 直接读
- **不需要改 `me.json`** — me.json 没新字段;profile 名由 handler 后端推导
- **不需要碰 `.env`** — `--clone` 会从 user 的 default profile 拷 .env / config.yaml / SOUL.md 进新 profile

## 验证迁移成功

跑完后,任选一个 agent 验证:

```bash
ls ~/.hermes/profiles/gitim-<handler>/
# 应该有: config.yaml, .env, memories/, sessions/, ...
```

之后在 WebUI 给该 agent 发消息,应正常回复。

## 多机器场景

profile 是 per-machine 的(`~/.hermes` 在每台机器各一份)。如果该 user 在多台机器跑 daemon,**每台机器各自跑一次上面的命令**。WorkspaceConfig 不会同步 hermes profile 状态。
