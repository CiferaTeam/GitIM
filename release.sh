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

# ---------- Build all binaries ----------
echo "==> Building binaries (release)..."
cargo build --release -p gitim-cli -p gitim-daemon -p gitim-runtime

# ---------- Package tarball ----------
TAG="v${VERSION}"
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

# Create release (or reuse existing tag)
if gh release view "$TAG" --repo "$RELEASES_REPO" > /dev/null 2>&1; then
  echo "    Release $TAG exists, uploading asset..."
  gh release upload "$TAG" "${ARCHIVE_NAME}.tar.gz" \
    --repo "$RELEASES_REPO" --clobber
else
  gh release create "$TAG" "${ARCHIVE_NAME}.tar.gz" \
    --repo "$RELEASES_REPO" \
    --title "GitIM ${TAG}" \
    --notes "GitIM ${TAG} release for ${PLATFORM}"
fi

echo ""
echo "==> Published ${TAG}"
echo "    https://github.com/${RELEASES_REPO}/releases/tag/${TAG}"
echo ""
echo "    Install:"
echo "    curl -sSf https://raw.githubusercontent.com/${RELEASES_REPO}/main/install.sh | sh"
