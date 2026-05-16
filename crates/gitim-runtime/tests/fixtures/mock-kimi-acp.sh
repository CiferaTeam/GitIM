#!/bin/sh
# Fake `kimi acp` for preflight integration tests.
#
# Treats argv[0] == "--version" as a fast `kimi --version` probe and
# exits 0 with a synthetic version line. Otherwise it speaks just
# enough ACP JSON-RPC to drive `preflight_kimi_with_config` through:
#   initialize → session/new → (optional session/set_model) →
#   session/prompt → one session/update notification with text
# and then exits, letting the preflight see a successful "say hi".
#
# Used by tests/preflight_kimi.rs. See preflight_hermes.rs for the
# matching pattern on hermes.

if [ "$1" = "--version" ]; then
  echo "kimi 0.99.0-mock"
  exit 0
fi

# ACP loop. Reads JSON-RPC frames one per line and writes canned
# responses on stdout. `sed` extracts the numeric request id so the
# same script handles any frame order.
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentCapabilities":{},"authMethods":[]}}\n' "$id"
      ;;
    *'"method":"session/new"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"ses_preflight"}}\n' "$id"
      ;;
    *'"method":"session/set_model"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
      ;;
    *'"method":"session/prompt"'*)
      # Emit a session/update notification carrying a text chunk —
      # `find_text_chunk` walks the tree, so any non-empty `text`
      # field is enough to satisfy the preflight.
      printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"ses_preflight","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hi"}}}}\n'
      # Then ack the prompt itself. After this the preflight will
      # take the success branch and kill the child.
      printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
      ;;
  esac
done
