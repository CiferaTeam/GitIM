#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"

# ---------- Parse bump type ----------
BUMP="${1:-patch}"
case "$BUMP" in
  major|minor|patch) ;;
  *)
    echo "Usage: $0 [major|minor|patch]"
    echo "  major: 0.4.0 -> 1.0.0"
    echo "  minor: 0.4.0 -> 0.5.0"
    echo "  patch: 0.4.0 -> 0.4.1 (default)"
    exit 1
    ;;
esac

# ---------- Read current version from latest git tag ----------
CURRENT_TAG=$(git tag -l 'v*' --sort=-v:refname | head -1)
if [ -z "$CURRENT_TAG" ]; then
  echo "Error: no version tags found"
  exit 1
fi

CURRENT="${CURRENT_TAG#v}"
IFS='.' read -r MAJOR MINOR PATCH <<< "$CURRENT"

case "$BUMP" in
  major) MAJOR=$((MAJOR + 1)); MINOR=0; PATCH=0 ;;
  minor) MINOR=$((MINOR + 1)); PATCH=0 ;;
  patch) PATCH=$((PATCH + 1)) ;;
esac

NEXT="${MAJOR}.${MINOR}.${PATCH}"
NEXT_TAG="v${NEXT}"

# ---------- Confirm ----------
echo "Current: ${CURRENT_TAG}"
echo "Next:    ${NEXT_TAG} (${BUMP})"
echo ""
printf "Proceed? [y/N] "
read -r REPLY
case "$REPLY" in
  y|Y|yes|YES) ;;
  *) echo "Aborted."; exit 0 ;;
esac

# ---------- Generate release notes ----------
NOTES_DIR="$ROOT/docs/releases"
NOTES_FILE="$NOTES_DIR/${NEXT_TAG}.md"
mkdir -p "$NOTES_DIR"

echo ""
echo "==> Generating release notes (${CURRENT_TAG}..HEAD)..."

COMMIT_LOG=$(git log --oneline "${CURRENT_TAG}..HEAD")

if [ -z "$COMMIT_LOG" ]; then
  echo "Warning: no commits since ${CURRENT_TAG}"
  COMMIT_LOG="(no changes)"
fi

if ! command -v claude &>/dev/null; then
  echo "Warning: claude CLI not found, using raw commit log"
  {
    echo "# GitIM ${NEXT_TAG}"
    echo ""
    echo "## Changes"
    echo ""
    echo "$COMMIT_LOG" | sed 's/^/- /'
  } > "$NOTES_FILE"
else
  claude -p "你是一个 release notes 生成器。根据以下 git commit log，生成简洁的中文 release notes。

要求：
- 标题：# GitIM ${NEXT_TAG}
- 按类别分组：新功能、修复、改进、其他（没有的类别不写）
- 每条一行，简洁明了
- 忽略纯重构、文档、CI 类 commit，除非有实质影响
- 不要写开头寒暄，直接输出 markdown

Commits:
${COMMIT_LOG}" > "$NOTES_FILE"
fi

echo "==> Release notes:"
echo ""
cat "$NOTES_FILE"
echo ""

# ---------- Bump Cargo.toml ----------
echo "==> Bumping workspace version: ${CURRENT} -> ${NEXT}"
sed -i '' "s/^version = \".*\"/version = \"${NEXT}\"/" "$ROOT/Cargo.toml"

# ---------- Update Cargo.lock ----------
echo "==> Updating Cargo.lock..."
cargo generate-lockfile --quiet

# ---------- Commit & tag ----------
git add "$NOTES_FILE" "$ROOT/Cargo.toml" Cargo.lock
git commit -m "chore: bump version to ${NEXT}"
git tag "$NEXT_TAG"

echo ""
echo "==> Done: ${NEXT_TAG}"
echo "    Cargo.toml version: ${NEXT}"
echo "    Tag: ${NEXT_TAG}"
echo "    Release notes: ${NOTES_FILE}"
echo ""
echo "    Next step: ./release.sh"
