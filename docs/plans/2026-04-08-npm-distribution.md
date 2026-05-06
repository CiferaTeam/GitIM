# GitIM npm Distribution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `npm install -g @gitim/cli` installs both the TypeScript CLI and the precompiled Rust daemon binary in one command.

**Architecture:** Two scoped npm packages — `@gitim/cli` (main, contains CLI JS) declares `@gitim/daemon-darwin-arm64` (platform binary) as an optionalDependency. npm's `os`/`cpu` fields handle platform filtering. A local `release.sh` script orchestrates build + version sync + publish. Versions are always locked in step.

**Tech Stack:** npm (packaging/distribution), shell script (release automation)

**Prerequisite:** Create the `@gitim` npm organization at https://www.npmjs.com/org/create before publishing. This is a one-time manual step.

---

### File Map

| Action | Path | Purpose |
|--------|------|---------|
| Rename | `publish.sh` → `install-from-source.sh` | Preserve source-build path |
| Create | `packages/daemon-darwin-arm64/package.json` | Platform binary npm package config |
| Create | `packages/daemon-darwin-arm64/.gitignore` | Keep binary out of git |
| Modify | `cli/package.json` | Rename to `@gitim/cli`, add optionalDependencies + files |
| Create | `release.sh` | Build + version sync + publish automation |

---

### Task 1: Rename publish.sh

**Files:**
- Rename: `publish.sh` → `install-from-source.sh`

- [ ] **Step 1: Rename the file**

```bash
cd /Users/lewisliu/ateam/GitIM
git mv publish.sh install-from-source.sh
```

- [ ] **Step 2: Verify**

```bash
ls install-from-source.sh
```

Expected: file exists.

- [ ] **Step 3: Commit**

```bash
git add install-from-source.sh
git commit -m "chore: rename publish.sh to install-from-source.sh"
```

---

### Task 2: Create daemon platform package

**Files:**
- Create: `packages/daemon-darwin-arm64/package.json`
- Create: `packages/daemon-darwin-arm64/.gitignore`

- [ ] **Step 1: Create directory**

```bash
mkdir -p packages/daemon-darwin-arm64
```

- [ ] **Step 2: Create package.json**

Write `packages/daemon-darwin-arm64/package.json`:

```json
{
  "name": "@gitim/daemon-darwin-arm64",
  "version": "0.1.0",
  "description": "GitIM daemon binary for macOS arm64",
  "license": "Apache-2.0",
  "os": ["darwin"],
  "cpu": ["arm64"],
  "bin": {
    "gitim-daemon": "./gitim-daemon"
  },
  "files": ["gitim-daemon"]
}
```

Key fields:
- `os` + `cpu`: npm only installs this package on matching platforms
- `bin`: npm links `gitim-daemon` into the same bin directory as `gitim`
- `files`: only the binary gets published (no junk)

- [ ] **Step 3: Create .gitignore to exclude binary from git**

Write `packages/daemon-darwin-arm64/.gitignore`:

```
gitim-daemon
```

- [ ] **Step 4: Verify package.json is valid**

```bash
cd /Users/lewisliu/ateam/GitIM/packages/daemon-darwin-arm64
node -e "const p = require('./package.json'); console.log(p.name, p.bin, p.os, p.cpu)"
```

Expected: `@gitim/daemon-darwin-arm64 { 'gitim-daemon': './gitim-daemon' } [ 'darwin' ] [ 'arm64' ]`

- [ ] **Step 5: Commit**

```bash
cd /Users/lewisliu/ateam/GitIM
git add packages/daemon-darwin-arm64/package.json packages/daemon-darwin-arm64/.gitignore
git commit -m "feat: add @gitim/daemon-darwin-arm64 platform package"
```

---

### Task 3: Update CLI package.json

**Files:**
- Modify: `cli/package.json`

- [ ] **Step 1: Update package.json**

Edit `cli/package.json` — three changes:
1. Rename `"name"` from `"gitim-cli"` to `"@gitim/cli"`
2. Add `"files": ["dist"]` so only compiled JS gets published
3. Add `"optionalDependencies"` pointing to daemon package with exact version

Full resulting file:

```json
{
  "name": "@gitim/cli",
  "version": "0.1.0",
  "type": "module",
  "bin": {
    "gitim": "./dist/index.js"
  },
  "files": ["dist"],
  "scripts": {
    "build": "tsc",
    "dev": "tsx src/index.ts"
  },
  "optionalDependencies": {
    "@gitim/daemon-darwin-arm64": "0.1.0"
  },
  "dependencies": {
    "@mariozechner/pi-tui": "^0.60.0",
    "chalk": "^5.6.2",
    "commander": "^13.0.0"
  },
  "devDependencies": {
    "@types/node": "^22.0.0",
    "tsx": "^4.19.0",
    "typescript": "^5.7.0"
  }
}
```

- [ ] **Step 2: Verify CLI still builds**

```bash
cd /Users/lewisliu/ateam/GitIM/cli
npm run build
```

Expected: compiles without error.

- [ ] **Step 3: Commit**

```bash
cd /Users/lewisliu/ateam/GitIM
git add cli/package.json
git commit -m "feat: rename CLI to @gitim/cli, add daemon optionalDependency"
```

---

### Task 4: Create release script

**Files:**
- Create: `release.sh`

- [ ] **Step 1: Write release.sh**

Write `release.sh`:

```bash
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
  echo "    @gitim/daemon-darwin-arm64@$VERSION"
  echo "    @gitim/cli@$VERSION"
  echo ""
  echo "    Run without --dry-run to publish."
  exit 0
fi

# ---------- Publish daemon package first (CLI depends on it) ----------
echo "==> Publishing @gitim/daemon-darwin-arm64@$VERSION..."
cd "$DAEMON_PKG"
npm publish --access public

# ---------- Publish CLI package ----------
echo "==> Publishing @gitim/cli@$VERSION..."
cd "$ROOT/cli"
npm publish --access public

echo ""
echo "==> Published v$VERSION"
echo "    npm install -g @gitim/cli"
```

- [ ] **Step 2: Make executable**

```bash
chmod +x release.sh
```

- [ ] **Step 3: Verify dry run**

```bash
cd /Users/lewisliu/ateam/GitIM
./release.sh --dry-run
```

Expected: builds daemon and CLI, prints "Dry run complete. Would publish: ..." without actually publishing. Verify:
- `packages/daemon-darwin-arm64/gitim-daemon` binary exists and is executable
- Version in both package.json files matches Cargo.toml workspace version

- [ ] **Step 4: Commit**

```bash
cd /Users/lewisliu/ateam/GitIM
git add release.sh
git commit -m "feat: add release.sh for npm publish workflow"
```

---

### Task 5: End-to-end verification

- [ ] **Step 1: Run full dry-run from clean state**

```bash
cd /Users/lewisliu/ateam/GitIM
# Clean any artifacts from previous run
rm -f packages/daemon-darwin-arm64/gitim-daemon
./release.sh --dry-run
```

Expected output includes:
- `Building gitim-daemon (release)...` succeeds
- `Copied binary to .../gitim-daemon`
- `Syncing version 0.1.0 to package.json files...`
- `Building CLI...` succeeds
- `Dry run complete. Would publish:`

- [ ] **Step 2: Verify binary works**

```bash
./packages/daemon-darwin-arm64/gitim-daemon --help 2>&1 || true
```

Expected: binary runs (output may vary — just confirm it's a valid executable, not a crash).

- [ ] **Step 3: Verify package contents**

```bash
cd /Users/lewisliu/ateam/GitIM/packages/daemon-darwin-arm64
npm pack --dry-run 2>&1
```

Expected: lists `package.json` and `gitim-daemon` — nothing else.

```bash
cd /Users/lewisliu/ateam/GitIM/cli
npm pack --dry-run 2>&1
```

Expected: lists `package.json`, `dist/` files — no `src/`, no `node_modules/`.
