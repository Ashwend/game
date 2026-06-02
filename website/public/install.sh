#!/bin/sh
# Ashwend macOS installer.
#
#   curl -fsSL https://ashwend.game/install.sh | sh
#
# Downloads the latest macOS build and installs it to /Applications. Because the
# download happens via curl (not a browser), macOS does NOT attach the
# `com.apple.quarantine` flag, so Ashwend.app opens normally, with no "damaged"
# or Gatekeeper prompt. (The app is ad-hoc signed but not yet notarized; this
# installer is the friction-free path until then.)
set -eu

REPO="Ashwend/game"
ASSET="ashwend-aarch64-apple-darwin.zip"
APP="Ashwend.app"
DEST="${ASHWEND_INSTALL_DIR:-/Applications}"
URL="https://github.com/${REPO}/releases/latest/download/${ASSET}"

err() { printf 'install: %s\n' "$1" >&2; exit 1; }

[ "$(uname -s)" = "Darwin" ] || err "this installer is for macOS."
arch="$(uname -m)"
[ "$arch" = "arm64" ] || err "Ashwend ships only an Apple Silicon (arm64) build; detected '$arch'."
command -v curl >/dev/null 2>&1 || err "curl is required."
command -v ditto >/dev/null 2>&1 || err "ditto is required (ships with macOS)."

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

printf 'Downloading the latest Ashwend…\n'
curl -fSL --progress-bar "$URL" -o "$tmp/ashwend.zip" || err "download failed from $URL"

printf 'Unpacking…\n'
ditto -x -k "$tmp/ashwend.zip" "$tmp/unpacked" || err "could not unpack the download."

src="$tmp/unpacked/$APP"
[ -d "$src" ] || src="$(find "$tmp/unpacked" -maxdepth 3 -name "$APP" -type d 2>/dev/null | head -n1)"
[ -n "${src:-}" ] && [ -d "$src" ] || err "could not find $APP inside the download."

# Defensive: a curl download shouldn't be quarantined, but strip it just in case
# the user piped a pre-downloaded copy through some quarantining tool.
xattr -dr com.apple.quarantine "$src" 2>/dev/null || true

target="$DEST/$APP"
printf 'Installing to %s…\n' "$DEST"
if [ -d "$target" ] || [ -e "$target" ]; then
  rm -rf "$target" 2>/dev/null || sudo rm -rf "$target"
fi
if ! cp -R "$src" "$DEST/" 2>/dev/null; then
  printf 'Elevated permissions needed to write to %s…\n' "$DEST"
  sudo cp -R "$src" "$DEST/"
fi

printf 'Done. Launching Ashwend. Find it in %s.\n' "$DEST"
open "$target" 2>/dev/null || printf 'Launch it any time with: open "%s"\n' "$target"
