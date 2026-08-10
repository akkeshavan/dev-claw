#!/usr/bin/env bash
# Smoke-test dclaw on multiple Linux distros via Docker.
# Requires Docker Desktop running and a published GitHub release.
#
# Usage:
#   bash scripts/smoke-test-linux.sh [VERSION]
#
# VERSION defaults to the latest release. Pass e.g. "v0.1.2" to pin.
set -euo pipefail

VERSION="${1:-}"
SMOKE="$(mktemp /tmp/smoke.XXXXXX.sh)"

cat > "$SMOKE" << 'SMOKEEOF'
#!/bin/sh
set -e
PASS=0; FAIL=0

check() {
    local desc="$1"; shift
    if "$@" > /tmp/out.txt 2>&1; then
        echo "  PASS  $desc"
        PASS=$((PASS+1))
    else
        echo "  FAIL  $desc"
        sed 's/^/        /' /tmp/out.txt | head -4
        FAIL=$((FAIL+1))
    fi
}

# Minimal git repo so git subcommands work
git config --global user.email "test@example.com"
git config --global user.name "Smoke Test"
mkdir -p /repo && cd /repo && git init -q

check "--version exits 0"      dclaw --version
check "--help exits 0"         dclaw --help
check "git --help exits 0"     dclaw git --help
check "git check exits 0"      dclaw git check
check "review --help exits 0"  dclaw review --help
check "deps --help exits 0"    dclaw deps --help
check "release --help exits 0" dclaw release --help
check "env --help exits 0"     dclaw env --help
check "memory --help exits 0"  dclaw memory --help
check "config --help exits 0"  dclaw config --help

VER="$(dclaw --version 2>&1)"
VTAG="${DCLAW_VERSION:-0.1}"
case "$VER" in
    *"$VTAG"*) echo "  PASS  version string contains $VTAG"; PASS=$((PASS+1)) ;;
    *)          echo "  FAIL  version string: $VER";          FAIL=$((FAIL+1)) ;;
esac

echo ""
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
SMOKEEOF

chmod +x "$SMOKE"
trap 'rm -f "$SMOKE"' EXIT

INSTALL_CMD='curl -fsSL https://akkeshavan.github.io/dev-claw/install.sh | sh'
if [ -n "$VERSION" ]; then
    INSTALL_CMD="VERSION=$VERSION $INSTALL_CMD"
fi

# Version tag without leading 'v' for the version-string check
VTAG="${VERSION#v}"
VTAG="${VTAG:-0.1}"

OVERALL_PASS=0
OVERALL_FAIL=0

run_distro() {
    local image="$1"
    local label="$2"
    local setup="$3"

    printf "\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n"
    printf "▶ %s\n" "$label"
    printf "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n"

    if docker run --rm \
        -e DCLAW_VERSION="$VTAG" \
        -v "$SMOKE:/smoke.sh:ro" \
        "$image" \
        sh -c "
            set -e
            $setup
            $INSTALL_CMD
            export PATH=\"\$HOME/.local/bin:/usr/local/bin:\$PATH\"
            sh /smoke.sh
        "; then
        printf "✓ %s: all checks passed\n" "$label"
        OVERALL_PASS=$((OVERALL_PASS+1))
    else
        printf "✗ %s: one or more checks failed\n" "$label"
        OVERALL_FAIL=$((OVERALL_FAIL+1))
    fi
}

run_distro "ubuntu:24.04"    "Ubuntu 24.04 — glibc" \
    "apt-get update -qq && apt-get install -y -q curl git"

run_distro "debian:12-slim"  "Debian 12 — glibc" \
    "apt-get update -qq && apt-get install -y -q curl git"

run_distro "alpine:3.20"     "Alpine 3.20 — musl" \
    "apk add --quiet curl git"

run_distro "fedora:40"       "Fedora 40 — glibc" \
    "dnf install -y -q curl git"

# Rocky 9 ships curl-minimal which conflicts; --allowerasing swaps it out
run_distro "rockylinux:9"    "Rocky Linux 9 — glibc" \
    "dnf install -y --allowerasing curl git"

printf "\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n"
printf "Distros: %d passed, %d failed\n" "$OVERALL_PASS" "$OVERALL_FAIL"
printf "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n"
[ "$OVERALL_FAIL" -eq 0 ]
