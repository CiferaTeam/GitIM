#!/bin/sh
set -eu

# Consume the prompt command. The mock output mirrors Pi RPC mode: one GitIM
# prompt can produce multiple assistant turns when the model uses tools.
IFS= read -r _prompt

printf '%s\n' '{"type":"agent_start"}'
printf '%s\n' '{"type":"turn_end","message":{"role":"assistant","stopReason":"toolUse","usage":{"input":100,"output":10,"cacheRead":1000,"cacheWrite":1,"totalTokens":1111,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}}},"toolResults":[]}'
printf '%s\n' '{"type":"turn_end","message":{"role":"assistant","stopReason":"toolUse","usage":{"input":200,"output":20,"cacheRead":2000,"cacheWrite":2,"totalTokens":2222,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}}},"toolResults":[]}'
printf '%s\n' '{"type":"turn_end","message":{"role":"assistant","stopReason":"stop","usage":{"input":300,"output":30,"cacheRead":3000,"cacheWrite":3,"totalTokens":3333,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}}},"toolResults":[]}'
printf '%s\n' '{"type":"agent_end"}'
printf '%s\n' '{"type":"response","command":"get_state","success":true,"data":{"sessionId":"pi-session-1"}}'
