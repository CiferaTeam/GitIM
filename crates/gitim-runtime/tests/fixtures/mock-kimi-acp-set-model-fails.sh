#!/bin/sh
# Fake `kimi acp` that rejects session/set_model — pins the preflight
# behaviour: a bad model should fail the preflight with error_kind=Other
# and an error message that names the upstream JSON-RPC complaint
# (rather than silently passing because initialize + session/new
# succeeded). Used by tests/preflight_kimi.rs.

if [ "$1" = "--version" ]; then
  echo "kimi 0.99.0-mock"
  exit 0
fi

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
      printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32602,"message":"model not available: bogus-model"}}\n' "$id"
      exit 0
      ;;
  esac
done
