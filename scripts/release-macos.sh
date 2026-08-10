#!/usr/bin/env bash
# Build macOS releases locally and push to the dclaw GitHub Release.
#
# Usage:  ./scripts/release-macos.sh v0.1.0
#
# Prerequisites:
#   - gh CLI installed and authenticated (https://cli.github.com)
#   - cargo / rustup in PATH
#   - gh must have write access to akkeshavan/dev-claw
#
# This script:
#   1. Builds aarch64-apple-darwin and x86_64-apple-darwin
#   2. Packages each as dclaw-vX.Y.Z-<target>.tar.gz
#   3. Uploads both to the dclaw GitHub Release
#   4. Downloads all CI-built assets, computes sha256 for every asset
#   5. Uploads combined checksums.txt

set -euo pipefail

VERSION="${1:?Usage: $0 <version tag, e.g. v0.1.0>}"
BINARY_NAME="dclaw"
RELEASES_REPO="akkeshavan/dev-claw"

# ── Prerequisites ─────────────────────────────────────────────────────────────
command -v gh    >/dev/null 2>&1 || { echo "ERROR: gh CLI not found. Install from https://cli.github.com"; exit 1; }
command -v cargo >/dev/null 2>&1 || { echo "ERROR: cargo not found"; exit 1; }

# ── Install rust targets ──────────────────────────────────────────────────────
TARGETS=("aarch64-apple-darwin" "x86_64-apple-darwin")
for T in "${TARGETS[@]}"; do
    rustup target add "$T" 2>/dev/null || true
done

# ── Workspace ─────────────────────────────────────────────────────────────────
DIST="dist-macos-${VERSION}"
rm -rf "$DIST"
mkdir -p "$DIST"

# ── Build, package, and upload each darwin target ─────────────────────────────
for TARGET in "${TARGETS[@]}"; do
    echo ""
    echo "==> Building ${TARGET}..."
    cargo build --release --target "$TARGET"

    ASSET="${BINARY_NAME}-${VERSION}-${TARGET}"
    mkdir -p "${DIST}/${ASSET}"
    cp "target/${TARGET}/release/${BINARY_NAME}" "${DIST}/${ASSET}/"
    cp README.md LICENSE "${DIST}/${ASSET}/" 2>/dev/null || true

    (cd "$DIST" && tar czf "${ASSET}.tar.gz" "${ASSET}")
    echo "==> Uploading ${ASSET}.tar.gz to ${RELEASES_REPO} @ ${VERSION}..."
    gh release upload "$VERSION" "${DIST}/${ASSET}.tar.gz" \
        --repo "$RELEASES_REPO" --clobber
done

# ── Download all release assets and compute combined checksums ────────────────
echo ""
echo "==> Downloading all release assets to compute checksums..."
mkdir -p "${DIST}/all-assets"
gh release download "$VERSION" \
    --repo "$RELEASES_REPO" \
    --dir "${DIST}/all-assets"

> "${DIST}/checksums.txt"
for f in "${DIST}/all-assets"/*; do
    fname="$(basename "$f")"
    [[ "$fname" == "checksums.txt" ]] && continue
    shasum -a 256 "$f" \
        | awk -v name="$fname" '{print $1 "  " name}' \
        >> "${DIST}/checksums.txt"
done
sort -o "${DIST}/checksums.txt" "${DIST}/checksums.txt"

echo ""
echo "==> Combined checksums:"
cat "${DIST}/checksums.txt"

echo ""
echo "==> Uploading checksums.txt..."
gh release upload "$VERSION" "${DIST}/checksums.txt" \
    --repo "$RELEASES_REPO" --clobber

# ── Done ──────────────────────────────────────────────────────────────────────
echo ""
echo "Release ${VERSION} complete:"
echo "  aarch64-apple-darwin  built and uploaded"
echo "  x86_64-apple-darwin   built and uploaded"
echo "  checksums.txt         uploaded"
echo ""
echo "Artifacts in: ${DIST}/"
