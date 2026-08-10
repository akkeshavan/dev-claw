#!/usr/bin/env sh
# Install dev-claw — AI-powered SDLC automation
#
# Usage:
#   curl -fsSL https://akkeshavan.github.io/dev-claw/install.sh | sh
#
# Options (env vars):
#   INSTALL_DIR   Override install directory  (default: ~/.local/bin or /usr/local/bin)
#   VERSION       Pin a specific version tag  (default: latest)

set -e

REPO="akkeshavan/dev-claw"
BINARY="dclaw"

# ── Colours ────────────────────────────────────────────────────────────────────
if [ -t 1 ]; then
    RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
    CYAN='\033[0;36m'; BOLD='\033[1m'; RESET='\033[0m'
else
    RED=''; GREEN=''; YELLOW=''; CYAN=''; BOLD=''; RESET=''
fi

info()  { printf "${CYAN}info${RESET}  %s\n" "$1"; }
ok()    { printf "${GREEN}ok${RESET}    %s\n" "$1"; }
warn()  { printf "${YELLOW}warn${RESET}  %s\n" "$1"; }
die()   { printf "${RED}error${RESET} %s\n" "$1" >&2; exit 1; }

# ── Detect OS ─────────────────────────────────────────────────────────────────
OS="$(uname -s)"
case "$OS" in
    Darwin) OS_NAME="macos" ;;
    Linux)  OS_NAME="linux" ;;
    *)      die "Unsupported operating system: $OS. Download manually from https://github.com/${REPO}/releases" ;;
esac

# ── Detect architecture ───────────────────────────────────────────────────────
ARCH="$(uname -m)"
case "$ARCH" in
    x86_64 | amd64)  ARCH_NAME="x86_64" ;;
    aarch64 | arm64) ARCH_NAME="aarch64" ;;
    *)                die "Unsupported architecture: $ARCH. Download manually from https://github.com/${REPO}/releases" ;;
esac

# ── Map to release asset name ─────────────────────────────────────────────────
if [ "$OS_NAME" = "macos" ]; then
    TARGET="${ARCH_NAME}-apple-darwin"
else
    # Use musl (statically linked) on Linux for maximum compatibility
    TARGET="${ARCH_NAME}-unknown-linux-musl"
fi

# ── Resolve version ───────────────────────────────────────────────────────────
if [ -z "${VERSION:-}" ]; then
    info "Fetching latest release version..."
    VERSION="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
        | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": "\(.*\)".*/\1/')"
    [ -n "$VERSION" ] || die "Failed to fetch latest version. Set VERSION env var to pin a version."
fi

ok "Installing ${BINARY} ${VERSION} (${TARGET})"

# ── Build download URL ────────────────────────────────────────────────────────
ASSET_NAME="${BINARY}-${VERSION}-${TARGET}"
URL="https://github.com/${REPO}/releases/download/${VERSION}/${ASSET_NAME}.tar.gz"
CHECKSUM_URL="https://github.com/${REPO}/releases/download/${VERSION}/checksums.txt"

# ── Determine install directory ───────────────────────────────────────────────
if [ -n "${INSTALL_DIR:-}" ]; then
    BIN_DIR="$INSTALL_DIR"
elif [ -w "/usr/local/bin" ]; then
    BIN_DIR="/usr/local/bin"
elif [ -n "${HOME:-}" ]; then
    BIN_DIR="${HOME}/.local/bin"
    mkdir -p "$BIN_DIR"
else
    die "Cannot determine install directory. Set INSTALL_DIR env var."
fi

# ── Download ──────────────────────────────────────────────────────────────────
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

info "Downloading ${URL}..."
curl -fsSL --retry 3 -o "${TMP_DIR}/${ASSET_NAME}.tar.gz" "$URL" \
    || die "Download failed. Check https://github.com/${REPO}/releases for available assets."

# ── Verify checksum (best effort) ────────────────────────────────────────────
if command -v sha256sum >/dev/null 2>&1 || command -v shasum >/dev/null 2>&1; then
    info "Verifying checksum..."
    curl -fsSL --retry 3 -o "${TMP_DIR}/checksums.txt" "$CHECKSUM_URL" 2>/dev/null || {
        warn "Could not fetch checksums.txt — skipping verification"
    }
    if [ -f "${TMP_DIR}/checksums.txt" ]; then
        EXPECTED="$(grep "${ASSET_NAME}.tar.gz" "${TMP_DIR}/checksums.txt" | awk '{print $1}')"
        if [ -n "$EXPECTED" ]; then
            if command -v sha256sum >/dev/null 2>&1; then
                ACTUAL="$(sha256sum "${TMP_DIR}/${ASSET_NAME}.tar.gz" | awk '{print $1}')"
            else
                ACTUAL="$(shasum -a 256 "${TMP_DIR}/${ASSET_NAME}.tar.gz" | awk '{print $1}')"
            fi
            if [ "$EXPECTED" = "$ACTUAL" ]; then
                ok "Checksum verified"
            else
                die "Checksum mismatch! Expected: ${EXPECTED}, got: ${ACTUAL}"
            fi
        else
            warn "No checksum entry found for ${ASSET_NAME}.tar.gz — skipping"
        fi
    fi
fi

# ── Extract and install ───────────────────────────────────────────────────────
info "Extracting..."
tar -xzf "${TMP_DIR}/${ASSET_NAME}.tar.gz" -C "$TMP_DIR"

EXTRACTED_BINARY="${TMP_DIR}/${ASSET_NAME}/${BINARY}"
[ -f "$EXTRACTED_BINARY" ] || die "Binary not found in archive at ${EXTRACTED_BINARY}"

chmod +x "$EXTRACTED_BINARY"
cp "$EXTRACTED_BINARY" "${BIN_DIR}/${BINARY}"

ok "Installed ${BINARY} to ${BIN_DIR}/${BINARY}"

# ── PATH reminder ─────────────────────────────────────────────────────────────
case ":${PATH}:" in
    *":${BIN_DIR}:"*) ;;
    *)
        printf "\n${YELLOW}Note:${RESET} ${BIN_DIR} is not in your PATH.\n"
        printf "Add this to your shell profile:\n"
        printf "  ${BOLD}export PATH=\"${BIN_DIR}:\$PATH\"${RESET}\n\n"
        ;;
esac

# ── Verify installation ───────────────────────────────────────────────────────
if command -v "$BINARY" >/dev/null 2>&1; then
    INSTALLED_VERSION="$("$BINARY" --version 2>/dev/null || true)"
    ok "Verified: ${INSTALLED_VERSION}"
fi

printf "\n${GREEN}${BOLD}dev-claw is ready.${RESET}\n"
printf "Run ${BOLD}${BINARY} --help${RESET} to get started.\n"
printf "Docs: https://akkeshavan.github.io/dev-claw\n\n"
