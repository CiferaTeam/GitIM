#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if ! command -v shellcheck >/dev/null 2>&1; then
  echo "Error: shellcheck not found. Install it with: brew install shellcheck"
  exit 127
fi

mapfile -t ROOT_SCRIPTS < <(find "$ROOT" -maxdepth 1 -type f -name '*.sh' -print | sort)

if [ "${#ROOT_SCRIPTS[@]}" -eq 0 ]; then
  echo "==> No root shell scripts found."
  exit 0
fi

echo "==> shellcheck root shell scripts"
for script in "${ROOT_SCRIPTS[@]}"; do
  echo "    ${script#"$ROOT/"}"
done

shellcheck "${ROOT_SCRIPTS[@]}"
