#!/usr/bin/env bash
# Mock claude CLI for testing gitim-agent-provider.
# Simulates --input-format stream-json + --output-format stream-json:
# the process stays resident, reads one JSON user message per stdin line,
# and emits one turn's worth of stream-json on stdout per message.
#
# Scenarios are controlled by the user message's `content` string:
#   "happy"   — normal execution with text output
#   "error"   — execution that returns an error result
#   "crash"   — exit with non-zero status mid-turn
#   "slow"    — sleep 10 seconds (for timeout testing)
#   "tools"   — includes tool_use and tool_result events
#   "control" — sends a control_request and reads response
#   "multi-usage" — assistant usage for context, result usage for billing

SESSION_ID="mock-session-$$-$(date +%s)"

# Emit init once at process boot — real CLI does the same.
printf '{"type":"system","subtype":"init","session_id":"%s"}\n' "$SESSION_ID"

while IFS= read -r LINE; do
    # control_response replies use the same stdin pipe; skip them as turn triggers.
    if printf '%s' "$LINE" | grep -q '"type":"control_response"'; then
        continue
    fi

    # Extract a plain-string scenario tag from the user message's `content` field.
    # Sufficient for fixture use: tests only ever send string-content messages.
    CONTENT=$(printf '%s' "$LINE" | sed -n 's/.*"content":"\([^"]*\)".*/\1/p')

    case "$CONTENT" in
        *happy*)
            echo '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Hello from mock claude!"}]}}'
            printf '{"type":"result","session_id":"%s","result":"Hello from mock claude!","is_error":false,"duration_ms":100.0,"num_turns":1}\n' "$SESSION_ID"
            ;;
        *error*)
            printf '{"type":"result","session_id":"%s","result":"Something went wrong","is_error":true,"duration_ms":50.0,"num_turns":1}\n' "$SESSION_ID"
            ;;
        *crash*)
            exit 1
            ;;
        *slow*)
            sleep 10
            printf '{"type":"result","session_id":"%s","result":"Done","is_error":false}\n' "$SESSION_ID"
            ;;
        *tools*)
            echo '{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"call-1","name":"Bash","input":{"command":"echo hi"}}]}}'
            echo '{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"call-1","content":"hi"}]}}'
            echo '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Tool returned: hi"}]}}'
            printf '{"type":"result","session_id":"%s","result":"Tool returned: hi","is_error":false,"duration_ms":200.0,"num_turns":2}\n' "$SESSION_ID"
            ;;
        *multi-usage*)
            echo '{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"call-1","name":"Bash","input":{"command":"echo one"}}],"usage":{"input_tokens":1,"output_tokens":5,"cache_read_input_tokens":1000,"cache_creation_input_tokens":10}}}'
            echo '{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"call-1","content":"one"}]}}'
            echo '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"done"}],"usage":{"input_tokens":2,"output_tokens":6,"cache_read_input_tokens":2000,"cache_creation_input_tokens":20}}}'
            printf '{"type":"result","session_id":"%s","result":"done","is_error":false,"duration_ms":200.0,"num_turns":2,"usage":{"input_tokens":30,"output_tokens":80,"cache_read_input_tokens":30000,"cache_creation_input_tokens":300}}\n' "$SESSION_ID"
            ;;
        *control*)
            echo '{"type":"control_request","request_id":"req-1","request":{"subtype":"tool_use","tool_name":"Bash","input":{"command":"ls"}}}'
            read -r _RESPONSE
            echo '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Approved and done"}]}}'
            printf '{"type":"result","session_id":"%s","result":"Approved and done","is_error":false,"duration_ms":150.0,"num_turns":1}\n' "$SESSION_ID"
            ;;
        *)
            printf '{"type":"result","session_id":"%s","result":"Unknown scenario","is_error":false}\n' "$SESSION_ID"
            ;;
    esac
done
