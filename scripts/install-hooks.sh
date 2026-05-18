#!/usr/bin/env bash
# Install GitIM repo git hooks into the shared git-common-dir (worktree-safe).
# Idempotent: re-run anytime to refresh. ln -sf means source updates flow through.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HOOKS_DIR="$(git -C "$ROOT" rev-parse --git-common-dir)/hooks"

if [ ! -d "$HOOKS_DIR" ]; then
  mkdir -p "$HOOKS_DIR"
fi

SRC="$ROOT/scripts/hooks/pre-commit"
if [ ! -x "$SRC" ]; then
  echo "ERROR: $SRC not found or not executable."
  exit 1
fi

DST="$HOOKS_DIR/pre-commit"
ln -sf "$SRC" "$DST"

echo "Installed: $DST -> $SRC"
echo "Bypass an emergency commit with: git commit --no-verify"
