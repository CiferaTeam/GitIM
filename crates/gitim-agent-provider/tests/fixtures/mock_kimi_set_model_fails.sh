#!/bin/sh
# Fake `kimi acp` binary: acks initialize / session/new, then rejects
# session/set_model with a JSON-RPC error. Used by the kimi integration
# test that pins the plan §1333 contract: set_session_model failure
# must produce ExecResult { status: Failed, session_token: Some(sid) }
# so the user can retry with a corrected model and resume the same
# conversation (vs. silently losing the session).
#
# Reads one JSON-RPC request per line from stdin, matches on the
# method name, and writes back a canned response. Exits after the
# set_model error so the driver's cleanup path can run.

while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentCapabilities":{},"authMethods":[]}}\n' "$id"
      ;;
    *'"method":"session/new"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"ses_fake_kimi"}}\n' "$id"
      ;;
    *'"method":"session/set_model"'*)
      printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32602,"message":"model not available: bogus-model"}}\n' "$id"
      exit 0
      ;;
  esac
done
