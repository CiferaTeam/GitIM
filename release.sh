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

# ---------- Verify tag exists (should be created by bump.sh) ----------
if ! git rev-parse "$TAG" &>/dev/null; then
  echo "Error: tag $TAG not found. Run ./bump.sh first."
  exit 1
fi

# ---------- Verify release notes exist ----------
NOTES_FILE="$ROOT/docs/releases/${TAG}.md"
if [ ! -f "$NOTES_FILE" ]; then
  echo "Warning: release notes not found at $NOTES_FILE"
  NOTES_FILE=""
fi

# ---------- Detect platform ----------
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"
case "$ARCH" in
  aarch64) ARCH="arm64" ;;
  x86_64)  ARCH="x86_64" ;;
esac
PLATFORM="${OS}-${ARCH}"
echo "==> Platform: $PLATFORM"

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

if $DRY_RUN; then
  echo ""
  echo "==> Dry run complete. Would publish:"
  echo "    Tag:     $TAG"
  echo "    Repo:    $RELEASES_REPO"
  echo "    Archive: ${ARCHIVE_NAME}.tar.gz"
  [ -n "$NOTES_FILE" ] && echo "    Notes:   $NOTES_FILE"
  echo ""
  echo "    Contents:"
  tar tzf "${ARCHIVE_NAME}.tar.gz"
  echo ""
  echo "    Run without --dry-run to publish."
  exit 0
fi

# ---------- Check gh auth ----------
if ! gh auth status > /dev/null 2>&1; then
  echo "Error: gh not authenticated. Run: gh auth login"
  exit 1
fi

# ---------- Create release and upload ----------
echo "==> Publishing ${TAG} to ${RELEASES_REPO}..."

NOTES_ARGS=()
if [ -n "$NOTES_FILE" ]; then
  NOTES_ARGS=(--notes-file "$NOTES_FILE")
else
  NOTES_ARGS=(--notes "GitIM ${TAG} release for ${PLATFORM}")
fi

if gh release view "$TAG" --repo "$RELEASES_REPO" > /dev/null 2>&1; then
  echo "    Release $TAG exists, uploading asset..."
  gh release upload "$TAG" "${ARCHIVE_NAME}.tar.gz" \
    --repo "$RELEASES_REPO" --clobber
else
  gh release create "$TAG" "${ARCHIVE_NAME}.tar.gz" \
    --repo "$RELEASES_REPO" \
    --title "GitIM ${TAG}" \
    "${NOTES_ARGS[@]}"
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
