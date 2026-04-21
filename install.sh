#!/usr/bin/env bash
set -euo pipefail

RELEASES_REPO="CiferaTeam/gitim-releases"
INSTALL_DIR="$HOME/.gitim/bin"

# ---------- Detect platform ----------
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"
case "$ARCH" in
  aarch64) ARCH="arm64" ;;
  x86_64)  ARCH="x86_64" ;;
esac
PLATFORM="${OS}-${ARCH}"

SUPPORTED_PLATFORMS="darwin-arm64 darwin-x86_64 linux-arm64 linux-x86_64"

if ! echo "$SUPPORTED_PLATFORMS" | grep -qw "$PLATFORM"; then
  echo "Error: unsupported platform: $PLATFORM"
  echo "Supported: $SUPPORTED_PLATFORMS"
  echo ""
  echo "You can build from source instead:"
  echo "  git clone <repo> && ./install-from-source.sh"
  exit 1
fi

# ---------- Resolve version ----------
VERSION="${GITIM_VERSION:-latest}"

if [ "$VERSION" = "latest" ]; then
  echo "==> Fetching latest release..."
  TAG=$(curl -sSf "https://api.github.com/repos/${RELEASES_REPO}/releases/latest" \
    | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')
  if [ -z "$TAG" ]; then
    echo "Error: cannot determine latest release"
    exit 1
  fi
else
  TAG="v${VERSION}"
fi

echo "==> Installing GitIM ${TAG} (${PLATFORM})"

# ---------- Download ----------
ARCHIVE_NAME="gitim-${TAG}-${PLATFORM}.tar.gz"
DOWNLOAD_URL="https://github.com/${RELEASES_REPO}/releases/download/${TAG}/${ARCHIVE_NAME}"

TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

echo "==> Downloading ${ARCHIVE_NAME}..."
HTTP_CODE=$(curl -sSL -w '%{http_code}' -o "$TMPDIR/$ARCHIVE_NAME" "$DOWNLOAD_URL")
if [ "$HTTP_CODE" != "200" ]; then
  echo "Error: download failed (HTTP $HTTP_CODE)"
  echo "URL: $DOWNLOAD_URL"
  exit 1
fi

# ---------- Verify SHA256 ----------
SHA_URL="https://github.com/${RELEASES_REPO}/releases/download/${TAG}/SHA256SUMS"
echo "==> Verifying SHA256..."
SHA_FILE="$TMPDIR/SHA256SUMS"
if ! curl -sSfL -o "$SHA_FILE" "$SHA_URL"; then
  echo "Error: SHA256SUMS not found at $SHA_URL"
  echo "This release predates SHA256 verification. Recovery:"
  echo "  - Install the latest release (which ships with SHA256SUMS), or"
  echo "  - Bypass verification for this install: SKIP_SHA=1 bash install.sh"
  # Allow explicit bypass for one-shot recovery; never default to off.
  if [ "${SKIP_SHA:-0}" != "1" ]; then
    exit 1
  fi
  echo "==> SKIP_SHA=1 — skipping SHA verification (unsafe)"
else
  # Literal match on last whitespace-separated field (the filename). Avoids the
  # pipefail trap of `grep | awk | head -1` — a non-matching grep exits the pipe
  # non-zero and the script dies before the empty-check below can fire.
  EXPECTED_SHA=$(awk -v name="$ARCHIVE_NAME" '$NF == name { print $1; exit }' "$SHA_FILE")
  if [ -z "$EXPECTED_SHA" ]; then
    echo "Error: SHA256SUMS has no line for $ARCHIVE_NAME"
    exit 1
  fi
  # Prefer coreutils sha256sum (default on Linux). Fall back to BSD shasum -a 256
  # (default on macOS). Alpine / minimal Debian frequently lack `shasum`.
  if command -v sha256sum >/dev/null 2>&1; then
    ACTUAL_SHA=$(sha256sum "$TMPDIR/$ARCHIVE_NAME" | awk '{print $1}')
  elif command -v shasum >/dev/null 2>&1; then
    ACTUAL_SHA=$(shasum -a 256 "$TMPDIR/$ARCHIVE_NAME" | awk '{print $1}')
  else
    echo "Error: neither sha256sum nor shasum found; cannot verify SHA256"
    exit 1
  fi
  if [ "$EXPECTED_SHA" != "$ACTUAL_SHA" ]; then
    echo "Error: SHA256 mismatch"
    echo "  expected: $EXPECTED_SHA"
    echo "  actual:   $ACTUAL_SHA"
    rm -f "$TMPDIR/$ARCHIVE_NAME"
    exit 1
  fi
  echo "==> SHA256 verified."
fi

# ---------- Extract ----------
echo "==> Extracting..."
tar xzf "$TMPDIR/$ARCHIVE_NAME" -C "$TMPDIR"

# ---------- Install ----------
mkdir -p "$INSTALL_DIR"

BINARIES="gitim gitim-daemon gitim-runtime"
for bin in $BINARIES; do
  # Find the binary inside the extracted directory
  src=$(find "$TMPDIR" -name "$bin" -type f | head -1)
  if [ -z "$src" ]; then
    echo "Warning: $bin not found in archive, skipping"
    continue
  fi
  cp "$src" "$INSTALL_DIR/$bin"
  chmod +x "$INSTALL_DIR/$bin"
done

echo "==> Installed to $INSTALL_DIR"
for bin in $BINARIES; do
  if [ -f "$INSTALL_DIR/$bin" ]; then
    echo "    $bin -> $INSTALL_DIR/$bin"
  fi
done

# ---------- PATH guidance ----------
echo ""
case ":$PATH:" in
  *":$INSTALL_DIR:"*)
    echo "==> $INSTALL_DIR is already in your PATH. You're all set!"
    ;;
  *)
    SHELL_NAME="$(basename "$SHELL" 2>/dev/null || echo "sh")"
    case "$SHELL_NAME" in
      zsh)  RC_FILE="~/.zshrc" ;;
      bash) RC_FILE="~/.bashrc" ;;
      fish) RC_FILE="~/.config/fish/config.fish" ;;
      *)    RC_FILE="your shell config" ;;
    esac

    echo "==> Add GitIM to your PATH:"
    echo ""
    if [ "$SHELL_NAME" = "fish" ]; then
      echo "    fish_add_path $INSTALL_DIR"
    else
      echo "    export PATH=\"$INSTALL_DIR:\$PATH\""
    fi
    echo ""
    echo "    To make it permanent, add the line above to $RC_FILE"
    echo "    Then restart your terminal or run: source $RC_FILE"
    ;;
esac

echo ""
echo "==> Done! Run 'gitim --help' to get started."
