#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
DRY_RUN=false

if [ "${1:-}" = "--dry-run" ]; then
  DRY_RUN=true
  echo "==> DRY RUN (will not publish)"
fi

# ---------- Read version from Cargo workspace ----------
VERSION=$(grep 'version = "' "$ROOT/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')
echo "==> Version: $VERSION"

# ---------- Build daemon ----------
echo "==> Building gitim-daemon (release)..."
cargo build --release -p gitim-daemon

# ---------- Copy binary to platform package ----------
DAEMON_PKG="$ROOT/packages/daemon-darwin-arm64"
cp "$ROOT/target/release/gitim-daemon" "$DAEMON_PKG/gitim-daemon"
chmod +x "$DAEMON_PKG/gitim-daemon"
echo "    Copied binary to $DAEMON_PKG/gitim-daemon"

# ---------- Sync versions in all package.json ----------
echo "==> Syncing version $VERSION to package.json files..."
node -e "
const fs = require('fs');
const version = '$VERSION';
const files = [
  '$ROOT/cli/package.json',
  '$DAEMON_PKG/package.json'
];
for (const f of files) {
  const pkg = JSON.parse(fs.readFileSync(f, 'utf8'));
  pkg.version = version;
  if (pkg.optionalDependencies) {
    for (const k of Object.keys(pkg.optionalDependencies)) {
      pkg.optionalDependencies[k] = version;
    }
  }
  fs.writeFileSync(f, JSON.stringify(pkg, null, 2) + '\n');
  console.log('    ' + f.replace('$ROOT/', ''));
}
"

# ---------- Build CLI ----------
echo "==> Building CLI..."
cd "$ROOT/cli"
npm run build

if $DRY_RUN; then
  echo ""
  echo "==> Dry run complete. Would publish:"
  echo "    @gitim-runtime/daemon-darwin-arm64@$VERSION"
  echo "    @gitim-runtime/cli@$VERSION"
  echo ""
  echo "    Run without --dry-run to publish."
  exit 0
fi

# ---------- Publish daemon package first (CLI depends on it) ----------
echo "==> Publishing @gitim-runtime/daemon-darwin-arm64@$VERSION..."
cd "$DAEMON_PKG"
npm publish --access public --registry=https://registry.npmjs.org

# ---------- Publish CLI package ----------
echo "==> Publishing @gitim-runtime/cli@$VERSION..."
cd "$ROOT/cli"
npm publish --access public --registry=https://registry.npmjs.org

echo ""
echo "==> Published v$VERSION"
echo "    npm install -g @gitim-runtime/cli"
