#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"

echo "==> Building and installing GitIM from source..."
echo ""

cargo install --path "$ROOT/crates/gitim-cli"
cargo install --path "$ROOT/crates/gitim-daemon"
cargo install --path "$ROOT/crates/gitim-runtime"

echo ""
echo "==> Installed to ~/.cargo/bin/"
echo "    gitim"
echo "    gitim-daemon"
echo "    gitim-runtime"
