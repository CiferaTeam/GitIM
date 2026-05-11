#!/usr/bin/env bash
set -euo pipefail

# rustup's cargo/rustc shims MUST win over Homebrew rust, otherwise cross-target
# builds fail with "can't find crate for core" — Homebrew rustc doesn't know
# about targets installed by rustup (they live in ~/.rustup/toolchains/...).
export PATH="$HOME/.cargo/bin:$PATH"

# cross 0.2.5 only publishes amd64 manifests for its images. On Apple Silicon,
# Docker defaults to linux/arm64 pull and fails with 'no matching manifest for
# linux/arm64/v8'. Force amd64 here so Docker Desktop runs the amd64 image via
# Rosetta 2. (Smoke-test docker-run calls pass --platform explicitly, so they
# are not affected by this default.)
export DOCKER_DEFAULT_PLATFORM=linux/amd64

# Enable sccache when present — caches rustc output across targets, so the
# second release cycle only rebuilds changed code (most deps identical across
# the 4-target matrix). No-op when not installed; `cargo install sccache` or
# `brew install sccache` to activate. Only benefits the two cargo-host
# (macOS) targets; cross-container rustc isn't wrapped.
if command -v sccache >/dev/null 2>&1; then
  export RUSTC_WRAPPER=sccache
  echo "==> sccache enabled (host targets only)"
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RELEASES_REPO="CiferaTeam/GitIM"

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
  echo "  For a real release run all 4 targets:     ./scripts/release.sh"
  echo "  For single-target debugging add --dry-run: ./scripts/release.sh --target $ONLY_TARGET --dry-run"
  exit 1
fi

# ---------- Read version from Cargo workspace ----------
VERSION=$(grep 'version = "' "$ROOT/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')
TAG="v${VERSION}"
echo "==> Version: $VERSION"

# ---------- Verify tag exists ----------
if ! git rev-parse "$TAG" &>/dev/null; then
  echo "Error: tag $TAG not found. Run ./scripts/bump.sh first."
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

# ---------- Pre-flight: cross needs Linux-host toolchains pre-installed ----------
#
# cross 0.2.5 invokes `rustup toolchain add stable-<linux-host>` inside its
# container, but rustup 1.28+ refuses to install non-host toolchains unless
# `--force-non-host` is passed — cross doesn't pass it, so the whole matrix
# fails on the first linux target. Pre-install with --force-non-host here;
# cross sees them already present and skips its own add.
#
# Idempotent. First-run cost: ~1 GB disk total for the two toolchains.
# Skip when only building macOS targets (no cross involved).
if [ -z "$ONLY_TARGET" ] || [[ "$ONLY_TARGET" == linux-* ]]; then
  for cross_toolchain in stable-x86_64-unknown-linux-gnu stable-aarch64-unknown-linux-gnu; do
    if ! rustup toolchain list 2>/dev/null | grep -q "^$cross_toolchain"; then
      echo "==> Installing cross-container toolchain $cross_toolchain (first-time, ~500MB)..."
      rustup toolchain install "$cross_toolchain" --force-non-host --profile minimal
    fi
  done
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

  # Force stable toolchain for all release builds. The maintainer may use nightly
  # for daily dev, but release must be reproducible: nightly versions drift, and
  # cross (running Linux containers) cannot install matching nightly toolchains
  # across host archs — e.g. a nightly-aarch64-apple-darwin host cannot provision
  # nightly-x86_64-unknown-linux-gnu inside the cross container.
  case "$tool" in
    cargo)
      # Idempotent target install under stable (let rustup errors surface — the
      # old `|| true` silently swallowed add failures that cascaded into
      # "can't find crate for core").
      if ! rustup +stable target list --installed 2>/dev/null | grep -qx "$rust_target"; then
        echo "==> [$slug] installing rustup target $rust_target for stable toolchain..."
        rustup +stable target add "$rust_target"
      fi
      cargo +stable build --release --target "$rust_target" \
        -p gitim-cli -p gitim-daemon -p gitim-runtime
      ;;
    cross)
      # Requires Docker running + `cargo install cross`.
      cross +stable build --release --target "$rust_target" \
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
      # Pre-pull the correct-arch alpine. Without this, a previous Linux
      # target's smoke test may leave the wrong arch cached locally, and
      # `docker run --platform linux/amd64` warns "image ... does not match
      # the specified platform: wanted linux/amd64, actual: linux/arm64/v8"
      # then silently runs the wrong arch under Rosetta — false-positive smoke.
      docker pull --platform "$arch_tag" alpine:3 >/dev/null
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
