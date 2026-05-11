#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
INSTALL_DIR="$HOME/.gitim/bin"
CARGO_BIN="${CARGO_HOME:-$HOME/.cargo}/bin"
BINARIES="gitim gitim-daemon gitim-runtime"

echo "==> Building and installing GitIM from source..."
echo ""

cargo install --path "$ROOT/crates/gitim-cli"
cargo install --path "$ROOT/crates/gitim-daemon"
cargo install --path "$ROOT/crates/gitim-runtime"

# Relocate into ~/.gitim/bin so dev installs live at the same path as
# install.sh release installs. The WebUI auto-update endpoint requires this
# (strict check in gitim-runtime::update::strict_install_dir_check).
echo ""
echo "==> Relocating binaries to $INSTALL_DIR..."
mkdir -p "$INSTALL_DIR"
for bin in $BINARIES; do
  # Replace any existing regular file or stale symlink from a prior install.
  rm -f "$INSTALL_DIR/$bin"
  mv "$CARGO_BIN/$bin" "$INSTALL_DIR/$bin"
done

echo ""
echo "==> Installed to $INSTALL_DIR"
for bin in $BINARIES; do
  echo "    $bin"
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
