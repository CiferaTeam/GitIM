#!/usr/bin/env bash
# Install GitIM repo git hooks into the shared git-common-dir (worktree-safe).
# Idempotent: re-run anytime to refresh. ln -sf means source updates flow through.
#
# IMPORTANT: Run from the main checkout, NOT from a git worktree. The symlink
# target points to wherever this script lives — if you install from a worktree,
# the hook will break when that worktree is deleted. Use --force to install
# anyway (e.g. for testing in a worktree).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GIT_COMMON_DIR="$(git -C "$ROOT" rev-parse --git-common-dir)"
# Resolve git-common-dir to an absolute path so we can compare with $ROOT/.git
case "$GIT_COMMON_DIR" in
  /*) GIT_COMMON_ABS="$GIT_COMMON_DIR" ;;
  *)  GIT_COMMON_ABS="$(cd "$ROOT" && cd "$GIT_COMMON_DIR" && pwd)" ;;
esac
HOOKS_DIR="$GIT_COMMON_ABS/hooks"

# Detect worktree: in the main checkout, git-common-dir is "<root>/.git";
# in a worktree, it's some other path (typically "<main>/.git").
if [ "$GIT_COMMON_ABS" != "$ROOT/.git" ]; then
  if [ "${1:-}" != "--force" ]; then
    echo "ERROR: This appears to be a git worktree (not the main checkout)."
    echo "  ROOT: $ROOT"
    echo "  git-common-dir: $GIT_COMMON_ABS"
    echo
    echo "Installing the hook from a worktree creates a symlink pointing into"
    echo "this worktree's path. When the worktree is deleted, the hook breaks."
    echo
    echo "Re-run from the main checkout, or pass --force to install anyway"
    echo "(only useful for testing the hook in this worktree)."
    exit 1
  fi
  echo "WARNING: Installing from worktree at $ROOT (--force given)."
fi

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
