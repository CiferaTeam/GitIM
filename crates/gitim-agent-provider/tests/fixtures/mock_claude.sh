#!/usr/bin/env bash
# Mock claude CLI for testing gitim-agent-provider.
# Simulates --output-format stream-json behavior.
#
# Scenarios are controlled by the prompt text (last arg after -p):
#   "happy"   — normal execution with text output
#   "error"   — execution that returns an error result
#   "crash"   — exit with non-zero status
#   "slow"    — sleep 10 seconds (for timeout testing)
#   "tools"   — includes tool_use and tool_result events
#   "control" — sends a control_request and reads response

PROMPT=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        -p) PROMPT="$2"; shift 2 ;;
        *) shift ;;
    esac
done

SESSION_ID="mock-session-$(date +%s)"

case "$PROMPT" in
    *happy*)
        echo '{"type":"system","subtype":"init","session_id":"'"$SESSION_ID"'"}'
        echo '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Hello from mock claude!"}]}}'
        echo '{"type":"result","session_id":"'"$SESSION_ID"'","result":"Hello from mock claude!","is_error":false,"duration_ms":100.0,"num_turns":1}'
        ;;
    *error*)
        echo '{"type":"system","subtype":"init","session_id":"'"$SESSION_ID"'"}'
        echo '{"type":"result","session_id":"'"$SESSION_ID"'","result":"Something went wrong","is_error":true,"duration_ms":50.0,"num_turns":1}'
        ;;
    *crash*)
        echo '{"type":"system","subtype":"init","session_id":"'"$SESSION_ID"'"}'
        exit 1
        ;;
    *slow*)
        echo '{"type":"system","subtype":"init","session_id":"'"$SESSION_ID"'"}'
        sleep 10
        echo '{"type":"result","session_id":"'"$SESSION_ID"'","result":"Done","is_error":false}'
        ;;
    *tools*)
        echo '{"type":"system","subtype":"init","session_id":"'"$SESSION_ID"'"}'
        echo '{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"call-1","name":"Bash","input":{"command":"echo hi"}}]}}'
        echo '{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"call-1","content":"hi"}]}}'
        echo '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Tool returned: hi"}]}}'
        echo '{"type":"result","session_id":"'"$SESSION_ID"'","result":"Tool returned: hi","is_error":false,"duration_ms":200.0,"num_turns":2}'
        ;;
    *control*)
        echo '{"type":"system","subtype":"init","session_id":"'"$SESSION_ID"'"}'
        echo '{"type":"control_request","request_id":"req-1","request":{"subtype":"tool_use","tool_name":"Bash","input":{"command":"ls"}}}'
        read -r RESPONSE
        echo '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Approved and done"}]}}'
        echo '{"type":"result","session_id":"'"$SESSION_ID"'","result":"Approved and done","is_error":false,"duration_ms":150.0,"num_turns":1}'
        ;;
    *)
        echo '{"type":"system","subtype":"init","session_id":"'"$SESSION_ID"'"}'
        echo '{"type":"result","session_id":"'"$SESSION_ID"'","result":"Unknown scenario","is_error":false}'
        ;;
esac
