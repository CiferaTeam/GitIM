#!/bin/sh
# Fixture for pi override tests: reads the RPC prompt line (and discards it),
# then emits a message_update whose delta contains the value of
# MY_TEST_KEY (so the parent's success path captures it in
# `output_preview`), followed by agent_end.
#
# Used by tests asserting that `env_override` actually reached the spawned
# subprocess. If MY_TEST_KEY isn't set, we emit "<unset>" so the test
# distinguishes "override missing" from "override present".
IFS= read -r _line

VAL="${MY_TEST_KEY:-<unset>}"
printf '%s\n' "{\"type\":\"message_update\",\"assistantMessageEvent\":{\"type\":\"text_delta\",\"delta\":\"$VAL\"}}"
printf '%s\n' '{"type":"agent_end"}'
exit 0
