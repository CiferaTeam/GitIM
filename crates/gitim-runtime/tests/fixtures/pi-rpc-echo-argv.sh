#!/bin/sh
# Fixture for pi override tests: reads the RPC prompt line (and discards it),
# then emits a message_update whose delta contains the joined argv plus the
# literal GITIM_OK marker (so the parent's success path treats it as a valid
# response and captures it in `output_preview`), followed by agent_end.
#
# Used by the `model_override_is_ignored` test to confirm pi's CLI argv
# stays at the hardcoded `--mode rpc --no-session --no-tools` regardless
# of the caller's `model_override` value.
IFS= read -r _line

ARGV="$*"
printf '%s\n' "{\"type\":\"message_update\",\"assistantMessageEvent\":{\"type\":\"text_delta\",\"delta\":\"GITIM_OK ARGV=$ARGV\"}}"
printf '%s\n' '{"type":"agent_end"}'
exit 0
