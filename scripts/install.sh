#!/usr/bin/env bash
set -euo pipefail

REPO="user123cy/auger"
VERSION="${1:-latest}"

# --- detect platform -------------------------------------------------------
case "$(uname -s)" in
  Linux)  OS=linux ;;
  Darwin) OS=macos ;;
  MINGW* | MSYS* | CYGWIN*) OS=windows ;;
  *) echo "unsupported OS: $(uname -s)" >&2; exit 1 ;;
esac

case "$(uname -m)" in
  x86_64 | amd64) ARCH=x86_64 ;;
  aarch64 | arm64) ARCH=aarch64 ;;
  *) echo "unsupported arch: $(uname -m)" >&2; exit 1 ;;
esac

case "$OS-$ARCH" in
  linux-x86_64) TARGET="x86_64-unknown-linux-gnu" ;;
  linux-aarch64) TARGET="aarch64-unknown-linux-gnu" ;;
  macos-x86_64) TARGET="x86_64-apple-darwin" ;;
  macos-aarch64) TARGET="aarch64-apple-darwin" ;;
  windows-x86_64) TARGET="x86_64-pc-windows-msvc" ;;
  *) echo "no prebuilt binary for $OS-$ARCH" >&2; exit 1 ;;
esac

# --- resolve tag -----------------------------------------------------------
if [ "$VERSION" = "latest" ]; then
  VERSION="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
    | grep -o '"tag_name": *"[^"]*"' | sed 's/.*"\(.*\)"/\1/')"
fi
VERSION="${VERSION#v}"

# --- pick install dir -------------------------------------------------------
if [ -n "${CARGO_HOME:-}" ]; then
  DEST="$CARGO_HOME/bin"
elif [ -d "$HOME/.cargo/bin" ]; then
  DEST="$HOME/.cargo/bin"
else
  DEST="/usr/local/bin"
fi
mkdir -p "$DEST"

# --- download ----------------------------------------------------------------
BASE="https://github.com/$REPO/releases/download/v$VERSION/auger-$TARGET"
if [ "$OS" = "windows" ]; then
  tmp="$(mktemp -d)"
  curl -fsSL "$BASE.zip" -o "$tmp/auger.zip"
  tar -xf "$tmp/auger.zip" -C "$tmp"
  mv -f "$tmp/auger.exe" "$DEST/"
  rm -rf "$tmp"
else
  curl -fsSL "$BASE.tar.gz" | tar -xz -C "$DEST" auger
fi

echo "installed auger v$VERSION to $DEST"
echo "run 'auger --help' to get started"
