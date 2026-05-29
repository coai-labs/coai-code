#!/usr/bin/env sh
# install.sh — one-line installer for coai
# Usage: curl -fsSL https://raw.githubusercontent.com/coai-labs/coai-code/main/install.sh | sh

set -e

REPO="coai-labs/coai-code"
BINARY="coai"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
err() {
    printf 'error: %s\n' "$1" >&2
    exit 1
}

need_cmd() {
    if ! command -v "$1" > /dev/null 2>&1; then
        err "required command '$1' not found"
    fi
}

# ---------------------------------------------------------------------------
# Detect OS
# ---------------------------------------------------------------------------
OS="$(uname -s)"
case "$OS" in
    Linux)  os="linux" ;;
    Darwin) os="darwin" ;;
    MINGW*|MSYS*|CYGWIN*|Windows_NT)
        err "Windows detected. Please download the .zip from https://github.com/${REPO}/releases" ;;
    *)
        err "unsupported OS: $OS" ;;
esac

# ---------------------------------------------------------------------------
# Detect architecture
# ---------------------------------------------------------------------------
ARCH="$(uname -m)"
case "$ARCH" in
    x86_64)          arch="x86_64" ;;
    aarch64|arm64)   arch="aarch64" ;;
    *)
        err "unsupported architecture: $ARCH" ;;
esac

# ---------------------------------------------------------------------------
# Build target triple
# ---------------------------------------------------------------------------
case "${os}-${arch}" in
    linux-x86_64)   target="x86_64-unknown-linux-gnu" ;;
    linux-aarch64)  target="aarch64-unknown-linux-gnu" ;;
    darwin-x86_64)  target="x86_64-apple-darwin" ;;
    darwin-aarch64) target="aarch64-apple-darwin" ;;
    *)
        err "unsupported platform: ${os}-${arch}" ;;
esac

# ---------------------------------------------------------------------------
# Resolve latest release tag
# ---------------------------------------------------------------------------
need_cmd curl

printf 'Fetching latest release tag from GitHub...\n'

TAG="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep '"tag_name"' \
    | head -1 \
    | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')"

if [ -z "$TAG" ]; then
    err "could not determine latest release tag. Check https://github.com/${REPO}/releases"
fi

printf 'Latest release: %s\n' "$TAG"

# ---------------------------------------------------------------------------
# Download and extract
# ---------------------------------------------------------------------------
ARCHIVE="${BINARY}-${TAG}-${target}.tar.gz"
URL="https://github.com/${REPO}/releases/download/${TAG}/${ARCHIVE}"

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

printf 'Downloading %s...\n' "$ARCHIVE"
if ! curl -fsSL --output "${TMPDIR}/${ARCHIVE}" "$URL"; then
    err "download failed: $URL\nCheck that the release exists: https://github.com/${REPO}/releases"
fi

printf 'Extracting...\n'
tar -xzf "${TMPDIR}/${ARCHIVE}" -C "$TMPDIR"

# ---------------------------------------------------------------------------
# Install
# ---------------------------------------------------------------------------
INSTALL_DIR="${COAI_INSTALL_DIR:-$HOME/.local/bin}"

if [ ! -d "$INSTALL_DIR" ]; then
    mkdir -p "$INSTALL_DIR"
fi

# Binary may be at the top level or inside a subdirectory
BIN_SRC="$(find "$TMPDIR" -type f -name "$BINARY" | head -1)"
if [ -z "$BIN_SRC" ]; then
    err "binary '${BINARY}' not found in archive"
fi

DEST="${INSTALL_DIR}/${BINARY}"
cp "$BIN_SRC" "$DEST"
chmod +x "$DEST"

printf 'Installed %s to %s\n' "$BINARY" "$DEST"

# ---------------------------------------------------------------------------
# PATH check
# ---------------------------------------------------------------------------
case ":${PATH}:" in
    *":${INSTALL_DIR}:"*)
        # already in PATH
        ;;
    *)
        printf '\nNote: %s is not in your PATH.\n' "$INSTALL_DIR"
        printf 'Add the following to your shell profile (~/.bashrc, ~/.zshrc, etc.):\n\n'
        printf '  export PATH="%s:$PATH"\n\n' "$INSTALL_DIR"
        printf 'Then restart your shell or run: source ~/.bashrc\n'
        ;;
esac

printf '\nDone! Run `%s --help` to get started.\n' "$BINARY"
