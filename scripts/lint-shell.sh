#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if ! command -v shellcheck >/dev/null 2>&1; then
  echo "Error: shellcheck not found. Install it with: brew install shellcheck"
  exit 127
fi

mapfile -t SHELL_SCRIPTS < <({
  find "$ROOT"         -maxdepth 1 -type f -name '*.sh' -print
  find "$ROOT/scripts" -maxdepth 1 -type f -name '*.sh' -print
} | sort -u)

if [ "${#SHELL_SCRIPTS[@]}" -eq 0 ]; then
  echo "==> No shell scripts found."
  exit 0
fi

echo "==> shellcheck shell scripts"
for script in "${SHELL_SCRIPTS[@]}"; do
  echo "    ${script#"$ROOT/"}"
done

shellcheck "${SHELL_SCRIPTS[@]}"
