#!/bin/sh
# Fake `hermes acp` binary: completes session/prompt, then keeps stdout
# open long enough to catch drivers that wait forever for the ACP server to
# exit after a completed turn.

while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentCapabilities":{},"authMethods":[]}}\n' "$id"
      ;;
    *'"method":"session/new"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"ses_fake_hermes"}}\n' "$id"
      ;;
    *'"method":"session/prompt"'*)
      printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"ses_fake_hermes","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"fake hermes ok"}}}}\n'
      printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn","usage":{"inputTokens":12,"outputTokens":3,"totalTokens":120,"cachedReadTokens":100,"thoughtTokens":5}}}\n' "$id"
      sleep 30
      exit 0
      ;;
  esac
done
