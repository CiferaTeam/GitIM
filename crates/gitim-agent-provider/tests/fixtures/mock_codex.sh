#!/usr/bin/env bash
# Mock codex CLI for testing gitim-agent-provider.
# Simulates `codex exec --json` and `codex exec resume <thread_id> --json`.

set -euo pipefail

if [[ "${1:-}" == "--version" ]]; then
    echo "codex-cli mock"
    exit 0
fi

if [[ "${MOCK_CODEX_FAIL_WITH_STDERR:-}" == "1" ]]; then
    echo '{"type":"thread.started","thread_id":"mock-codex-thread"}'
    echo "mock codex stderr diagnostic" >&2
    exit 1
fi

MODE="exec"
THREAD_ID="mock-codex-thread"
PROMPT=""
SAW_CD_FLAG="false"
SAW_MAX_EFFORT="false"

while [[ $# -gt 0 ]]; do
    case "$1" in
        exec)
            MODE="exec"
            shift
            ;;
        resume)
            MODE="resume"
            THREAD_ID="$2"
            shift 2
            ;;
        --json|--model|-C|--cd|--sandbox|--color|--output-last-message|--output-schema|--profile|--config|-c|--add-dir|--image)
            if [[ "$1" == "-C" || "$1" == "--cd" ]]; then
                SAW_CD_FLAG="true"
            fi
            if [[ "$1" == "-c" || "$1" == "--config" ]]; then
                if [[ "${2:-}" == 'model_reasoning_effort="xhigh"' ]]; then
                    SAW_MAX_EFFORT="true"
                fi
            fi
            if [[ "$1" == "--model" || "$1" == "-C" || "$1" == "--cd" || "$1" == "--color" || "$1" == "--output-last-message" || "$1" == "--output-schema" || "$1" == "--profile" || "$1" == "--config" || "$1" == "-c" || "$1" == "--add-dir" || "$1" == "--image" ]]; then
                shift 2
            else
                shift
            fi
            ;;
        --full-auto|--dangerously-bypass-approvals-and-sandbox|--skip-git-repo-check|--ephemeral|--oss)
            shift
            ;;
        *)
            PROMPT="$1"
            shift
            ;;
    esac
done

if [[ "${MOCK_CODEX_REQUIRE_MAX_EFFORT:-}" == "1" && "$SAW_MAX_EFFORT" != "true" ]]; then
    echo "missing model_reasoning_effort xhigh config" >&2
    exit 2
fi

echo '{"type":"thread.started","thread_id":"'"$THREAD_ID"'"}'
echo '{"type":"turn.started"}'

if [[ "$MODE" == "resume" ]]; then
    if [[ "$SAW_CD_FLAG" == "true" ]]; then
        echo "resume does not accept -C/--cd" >&2
        exit 2
    fi
    echo '{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"Resumed mock codex thread"}}'
else
    echo '{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"Hello from mock codex!"}}'
fi

echo '{"type":"turn.completed","usage":{"input_tokens":1,"cached_input_tokens":0,"output_tokens":1}}'
