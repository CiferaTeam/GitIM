#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
RELEASES_REPO="CiferaTeam/gitim-releases"
DRY_RUN=false

if [ "${1:-}" = "--dry-run" ]; then
  DRY_RUN=true
  echo "==> DRY RUN (will not publish)"
fi

# ---------- Read version from Cargo workspace ----------
VERSION=$(grep 'version = "' "$ROOT/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')
TAG="v${VERSION}"
echo "==> Version: $VERSION"

# ---------- Detect platform ----------
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"
case "$ARCH" in
  aarch64) ARCH="arm64" ;;
  x86_64)  ARCH="x86_64" ;;
esac
PLATFORM="${OS}-${ARCH}"
echo "==> Platform: $PLATFORM"

# ---------- Generate release notes ----------
NOTES_DIR="$ROOT/docs/releases"
NOTES_FILE="$NOTES_DIR/${TAG}.md"
mkdir -p "$NOTES_DIR"

# Find the previous version tag (by semver order, not commit distance)
PREV_TAG=$(git tag -l 'v*' --sort=-v:refname | awk -v tag="$TAG" 'found{print;exit} $0==tag{found=1}')

if [ -n "$PREV_TAG" ]; then
  echo "==> Generating release notes (${PREV_TAG}..HEAD)..."
  COMMIT_LOG=$(git log --oneline "${PREV_TAG}..HEAD")
else
  echo "==> Generating release notes (all commits, first release)..."
  COMMIT_LOG=$(git log --oneline)
fi

if ! command -v claude &>/dev/null; then
  echo "Warning: claude CLI not found, using raw commit log as release notes"
  {
    echo "# GitIM ${TAG}"
    echo ""
    echo "## Changes"
    echo ""
    echo "$COMMIT_LOG" | sed 's/^/- /'
  } > "$NOTES_FILE"
else
  claude -p "你是一个 release notes 生成器。根据以下 git commit log，生成简洁的中文 release notes。

要求：
- 标题：# GitIM ${TAG}
- 按类别分组：新功能、修复、改进、其他（没有的类别不写）
- 每条一行，简洁明了
- 忽略纯重构、文档、CI 类 commit，除非有实质影响
- 不要写开头寒暄，直接输出 markdown

Commits:
${COMMIT_LOG}" > "$NOTES_FILE"
fi

echo "==> Release notes saved to $NOTES_FILE"
echo ""
cat "$NOTES_FILE"
echo ""

if $DRY_RUN; then
  echo "==> Dry run: skipping commit, tag, and publish."
  echo "    Would commit: $NOTES_FILE"
  echo "    Would tag:    $TAG"
  echo "    Run without --dry-run to publish."
  exit 0
fi

# ---------- Commit release notes & tag source repo ----------
git add "$NOTES_FILE"
git commit -m "docs: release notes for ${TAG}"
git tag "$TAG"
echo "==> Tagged source repo: $TAG"

# ---------- Build all binaries ----------
echo "==> Building binaries (release)..."
cargo build --release -p gitim-cli -p gitim-daemon -p gitim-runtime

# ---------- Package tarball ----------
ARCHIVE_NAME="gitim-${TAG}-${PLATFORM}"
STAGING="$ROOT/target/release/dist"
rm -rf "$STAGING"
mkdir -p "$STAGING/$ARCHIVE_NAME"

cp "$ROOT/target/release/gitim"         "$STAGING/$ARCHIVE_NAME/"
cp "$ROOT/target/release/gitim-daemon"  "$STAGING/$ARCHIVE_NAME/"
cp "$ROOT/target/release/gitim-runtime" "$STAGING/$ARCHIVE_NAME/"
chmod +x "$STAGING/$ARCHIVE_NAME"/*

cd "$STAGING"
tar czf "${ARCHIVE_NAME}.tar.gz" "$ARCHIVE_NAME"
echo "==> Packaged: $STAGING/${ARCHIVE_NAME}.tar.gz"

# ---------- Check gh auth ----------
if ! gh auth status > /dev/null 2>&1; then
  echo "Error: gh not authenticated. Run: gh auth login"
  exit 1
fi

# ---------- Create release and upload ----------
echo "==> Publishing ${TAG} to ${RELEASES_REPO}..."

if gh release view "$TAG" --repo "$RELEASES_REPO" > /dev/null 2>&1; then
  echo "    Release $TAG exists, uploading asset..."
  gh release upload "$TAG" "${ARCHIVE_NAME}.tar.gz" \
    --repo "$RELEASES_REPO" --clobber
else
  gh release create "$TAG" "${ARCHIVE_NAME}.tar.gz" \
    --repo "$RELEASES_REPO" \
    --title "GitIM ${TAG}" \
    --notes-file "$ROOT/$NOTES_FILE"
fi

echo ""
echo "==> Published ${TAG}"
echo "    https://github.com/${RELEASES_REPO}/releases/tag/${TAG}"
echo ""
echo "    Install:"
echo "    curl -sSf https://raw.githubusercontent.com/${RELEASES_REPO}/main/install.sh | sh"
echo ""
echo "    Update:"
echo "    gitim update"
