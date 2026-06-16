#!/bin/sh
# Fake `kimi acp` binary: completes a prompt successfully, then exits
# non-zero when stdin closes. Regression test for the shutdown semantic
# that a Completed turn must not be flipped to Failed by the child's
# post-EOF exit code.
#
# This mirrors Kimi Code CLI behavior where the stdio server may return a
# non-zero status after the client closes stdin, even though the ACP turn
# itself finished normally.

while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentCapabilities":{},"authMethods":[]}}\n' "$id"
      ;;
    *'"method":"session/new"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"ses_nonzero_exit"}}\n' "$id"
      ;;
    *'"method":"session/prompt"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"text":"pong","stopReason":"end_turn"}}\n' "$id"
      ;;
  esac
done

# Exit non-zero on EOF to simulate Kimi Code CLI post-shutdown behavior.
exit 1
