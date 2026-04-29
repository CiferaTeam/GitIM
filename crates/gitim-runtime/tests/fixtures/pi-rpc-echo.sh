#!/bin/sh
IFS= read -r line

case "$line" in
  *'"type":"prompt"'*'"message":"Reply with exactly: GITIM_OK"'*)
    printf '%s\n' '{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"GITIM_OK"}}'
    printf '%s\n' '{"type":"agent_end"}'
    exit 0
    ;;
  *)
    printf 'unexpected stdin: %s\n' "$line" >&2
    exit 2
    ;;
esac
