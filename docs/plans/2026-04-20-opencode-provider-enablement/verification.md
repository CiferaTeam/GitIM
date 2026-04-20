# OpenCode Provider Enablement — Manual Verification

Date: 2026-04-20
Tester: Claude + Lewis

## Automated Test Results

| Suite | Result |
|---|---|
| `cargo test -p gitim-agent-provider` | ✅ 18 passed |
| `cargo test -p gitim-runtime --lib preflight` | ✅ 15 passed |
| `cargo test -p gitim-runtime --test preflight_opencode` | ✅ 4 passed, 1 ignored |
| `cargo check --workspace` | ✅ no error |
| `cd webui-v2 && tsc -b` | ✅ no error |

## Step 5.2 — CLI injection of system prompt

**Command:**
```bash
OPENCODE_CONFIG_CONTENT='{"agent":{"gitim":{"prompt":"You are a grumpy cat. Reply with meow only.","mode":"primary"}}}' \
  opencode run --format json --dangerously-skip-permissions --agent gitim -- "Say hi"
```

**Output (trimmed):**
```ndjson
{"type":"step_start","sessionID":"ses_25731d299...","part":{...}}
{"type":"text","sessionID":"ses_25731d299...","part":{"text":"Meow.",...}}
{"type":"step_finish","sessionID":"ses_25731d299...","part":{...,"tokens":{...},"cost":0}}
```

✅ System prompt 约束生效 — "Say hi" → "Meow."
✅ NDJSON shape matches what `opencode/mod.rs::parse_line` expects (`step_start` / `text` / `step_finish` with top-level `sessionID`)

## Step 5.3 — default model when `--model` omitted

**Command:**
```bash
opencode run --format json --dangerously-skip-permissions -- "Reply with exactly: hi"
```

**Output:**
```
{"type":"text", "part":{"text":"hi", ...}}
```

✅ Uses user's `opencode auth login` default (no `--model` needed).

## Step 5.1 — Runtime + WebUI end-to-end (manual)

Not executed in this session. Recipe for a human to run:

```bash
# 1. Start runtime
cargo run -p gitim-runtime -- serve

# 2. In another terminal, start webui
cd webui-v2 && npm run dev

# 3. In browser:
#    - Create local workspace
#    - Click "Add Agent"
#    - Provider dropdown: select "OpenCode"
#    - Click "Detect" → should see green checkmark + ms timing
#    - Model section should say "OpenCode uses the default model from opencode auth login. No selection needed."
#    - Enter agent name (e.g. "cat")
#    - System Prompt: "You are a grumpy cat. Reply with meow only."
#    - Submit
# 4. Send message to the agent in the IM channel; expect meow-style reply.
```

## Known gotchas

- `OPENCODE_CONFIG_CONTENT` agent name collision: we use `"gitim"`. If the user's own `opencode.json` declares an agent named `gitim`, our injection will be *merged* with theirs (opencode config merge is deep). Risk low but non-zero.
- Cost: opencode preflight runs the user's authed model (unlike claude/codex which force haiku/mini). A handful of cents per provision. Document this in the UI if it becomes user-facing pain.
- First-time users without `opencode auth login` will see the preflight fail with opencode's own error message (passthrough).
