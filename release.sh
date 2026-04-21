#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
RELEASES_REPO="CiferaTeam/gitim-releases"

# ---------- Argument parsing ----------
DRY_RUN=false
ONLY_TARGET=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run) DRY_RUN=true; shift ;;
    --target)  ONLY_TARGET="$2"; shift 2 ;;
    *) echo "Usage: $0 [--dry-run] [--target <slug>]"; exit 1 ;;
  esac
done

$DRY_RUN && echo "==> DRY RUN (will not publish)"

# Single-target builds must not publish. Uploading one tarball + a truncated
# SHA256SUMS would overwrite the full multi-target file via `gh upload --clobber`,
# silently deleting hashes for the other 3 platforms and breaking their installs.
if [ -n "$ONLY_TARGET" ] && ! $DRY_RUN; then
  echo "Error: --target <slug> is debug-only (requires --dry-run)."
  echo "  Single-target upload would truncate SHA256SUMS and break other platforms."
  echo "  For a real release run all 4 targets:     ./release.sh"
  echo "  For single-target debugging add --dry-run: ./release.sh --target $ONLY_TARGET --dry-run"
  exit 1
fi

# ---------- Read version from Cargo workspace ----------
VERSION=$(grep 'version = "' "$ROOT/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')
TAG="v${VERSION}"
echo "==> Version: $VERSION"

# ---------- Verify tag exists ----------
if ! git rev-parse "$TAG" &>/dev/null; then
  echo "Error: tag $TAG not found. Run ./bump.sh first."
  exit 1
fi

# ---------- Verify release notes ----------
NOTES_FILE="$ROOT/docs/releases/${TAG}.md"
if [ ! -f "$NOTES_FILE" ]; then
  echo "Warning: release notes not found at $NOTES_FILE"
  NOTES_FILE=""
fi

# ---------- Target matrix ----------
#
# Each entry: rust_target:slug:tool
#   rust_target  — value for `--target`
#   slug         — filename segment (must match gitim-updater::detect_platform())
#   tool         — `cargo` for native / `cross` for Docker-based cross
#
# IMPORTANT: slugs are CONTRACT with gitim-updater / install.sh. Do not change
# without updating both.
ALL_TARGETS=(
  "aarch64-apple-darwin:darwin-arm64:cargo"
  "x86_64-apple-darwin:darwin-x86_64:cargo"
  "aarch64-unknown-linux-musl:linux-arm64:cross"
  "x86_64-unknown-linux-musl:linux-x86_64:cross"
)

if [ -n "$ONLY_TARGET" ]; then
  FILTERED=()
  for t in "${ALL_TARGETS[@]}"; do
    IFS=: read -r _r slug _tool <<< "$t"
    [ "$slug" = "$ONLY_TARGET" ] && FILTERED+=("$t")
  done
  if [ ${#FILTERED[@]} -eq 0 ]; then
    echo "Error: unknown --target slug '$ONLY_TARGET'"
    echo "Valid slugs: darwin-arm64 / darwin-x86_64 / linux-arm64 / linux-x86_64"
    exit 1
  fi
  TARGETS=("${FILTERED[@]}")
  echo "==> Single-target build: $ONLY_TARGET"
else
  TARGETS=("${ALL_TARGETS[@]}")
  echo "==> Full matrix: 4 targets"
fi

# ---------- Prepare staging ----------
STAGING="$ROOT/target/release-dist"
rm -rf "$STAGING"
mkdir -p "$STAGING"

# ---------- build_target function ----------
#
# Args: rust_target, slug, tool
#
# Produces: $STAGING/gitim-${TAG}-${slug}.tar.gz
#
# For linux targets, smoke-tests the gitim binary via `docker run alpine`.
build_target() {
  local rust_target="$1" slug="$2" tool="$3"
  local archive_name="gitim-${TAG}-${slug}"
  local out_dir="$STAGING/$archive_name"
  echo ""
  echo "==> [$slug] building (tool=$tool, rust_target=$rust_target)"

  case "$tool" in
    cargo)
      # Ensure rustup target installed for macOS cross (x86_64 from arm64 host)
      rustup target add "$rust_target" >/dev/null 2>&1 || true
      cargo build --release --target "$rust_target" \
        -p gitim-cli -p gitim-daemon -p gitim-runtime
      ;;
    cross)
      # Requires Docker running + `cargo install cross`
      cross build --release --target "$rust_target" \
        -p gitim-cli -p gitim-daemon -p gitim-runtime
      ;;
    *) echo "Error: unknown build tool '$tool'"; exit 1 ;;
  esac

  mkdir -p "$out_dir"
  cp "$ROOT/target/$rust_target/release/gitim"         "$out_dir/"
  cp "$ROOT/target/$rust_target/release/gitim-daemon"  "$out_dir/"
  cp "$ROOT/target/$rust_target/release/gitim-runtime" "$out_dir/"
  chmod +x "$out_dir"/*

  # Smoke test: for Linux targets, verify the binary starts inside Alpine.
  # For macOS targets on Apple Silicon host, native arm64 runs directly;
  # x86_64-apple-darwin runs via Rosetta 2 if installed — skip to keep
  # release reproducible on hosts without Rosetta.
  case "$slug" in
    linux-*)
      local arch_tag
      case "$slug" in
        linux-x86_64) arch_tag="linux/amd64" ;;
        linux-arm64)  arch_tag="linux/arm64" ;;
      esac
      echo "==> [$slug] smoke test (docker alpine, $arch_tag)"
      # Smoke-test all three shipped binaries — a musl link error in
      # gitim-daemon or gitim-runtime would otherwise ship silently.
      docker run --rm --platform "$arch_tag" \
        -v "$out_dir:/bins:ro" \
        alpine:3 sh -c '/bins/gitim --version && /bins/gitim-daemon --version && /bins/gitim-runtime --version' >/dev/null
      ;;
  esac

  # Tar it up
  (cd "$STAGING" && tar czf "${archive_name}.tar.gz" "$archive_name")
  rm -rf "$out_dir"  # keep only the tarball
  echo "==> [$slug] packaged: ${archive_name}.tar.gz"
}

# ---------- Run matrix (fail-fast) ----------
for t in "${TARGETS[@]}"; do
  IFS=: read -r rust_target slug tool <<< "$t"
  build_target "$rust_target" "$slug" "$tool"
done

# ---------- SHA256SUMS ----------
(cd "$STAGING" && shasum -a 256 gitim-${TAG}-*.tar.gz > SHA256SUMS)
echo ""
echo "==> SHA256SUMS:"
cat "$STAGING/SHA256SUMS"

# ---------- Dry run exit ----------
if $DRY_RUN; then
  echo ""
  echo "==> Dry run complete. Would publish to $RELEASES_REPO:"
  (cd "$STAGING" && ls -la gitim-${TAG}-*.tar.gz SHA256SUMS)
  exit 0
fi

# ---------- gh auth ----------
if ! gh auth status >/dev/null 2>&1; then
  echo "Error: gh not authenticated. Run: gh auth login"
  exit 1
fi

# ---------- Create / upload release ----------
# Fail-fast invariant: if any build above failed, `set -e` killed the script
# before we reach here. So any upload at this point is atomic-ish — either
# all assets for this matrix run land in the Release, or none (we exited early).
echo ""
echo "==> Publishing ${TAG} to ${RELEASES_REPO}..."

NOTES_ARGS=()
if [ -n "$NOTES_FILE" ]; then
  NOTES_ARGS=(--notes-file "$NOTES_FILE")
else
  NOTES_ARGS=(--notes "GitIM ${TAG} release")
fi

cd "$STAGING"
ASSETS=(gitim-${TAG}-*.tar.gz SHA256SUMS)

if gh release view "$TAG" --repo "$RELEASES_REPO" >/dev/null 2>&1; then
  echo "    Release $TAG exists, re-uploading assets..."
  gh release upload "$TAG" "${ASSETS[@]}" --repo "$RELEASES_REPO" --clobber
else
  gh release create "$TAG" "${ASSETS[@]}" \
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
