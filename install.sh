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

SUPPORTED_PLATFORMS="darwin-arm64"

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
