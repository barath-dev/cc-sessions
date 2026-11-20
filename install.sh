#!/bin/sh
# Paddock installer for macOS and Linux.
#
#   curl -fsSL https://raw.githubusercontent.com/barath-dev/paddock/main/install.sh | sh
#
# Downloads the one program for this system, checks it against the release's
# SHA-256 list, puts it in ~/.local/share/paddock, and runs its setup.
# Running it again later updates the program and keeps all settings. An install
# from before the rename (cc-sessions) is updated where it is.
# To pass setup options:  curl ... | sh -s -- --yes --command cs
set -eu

REPO="barath-dev/paddock"
BASE="${PADDOCK_INSTALL_BASE:-${CC_SESSIONS_INSTALL_BASE:-https://github.com/$REPO/releases/latest/download}}"

case "$(uname -s)" in
  Darwin) os=macos ;;
  Linux) os=linux ;;
  *) echo "paddock: this installer is for macOS and Linux. On Windows use install.ps1." >&2; exit 1 ;;
esac
case "$(uname -m)" in
  arm64 | aarch64) arch=arm64 ;;
  x86_64 | amd64) arch=x86_64 ;;
  *) echo "paddock: no build for processor type $(uname -m)." >&2; exit 1 ;;
esac
asset="paddock-$os-$arch"

fetch() {
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$1" -o "$2"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$2" "$1"
  else
    echo "paddock: curl or wget is needed to download." >&2
    exit 1
  fi
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

echo "Downloading $asset..."
fetch "$BASE/$asset" "$tmp/$asset"
fetch "$BASE/checksums.txt" "$tmp/checksums.txt"

expected=$(awk -v name="$asset" '$2 == name || $2 == "*" name { print $1 }' "$tmp/checksums.txt")
if command -v sha256sum >/dev/null 2>&1; then
  actual=$(sha256sum "$tmp/$asset" | awk '{ print $1 }')
else
  actual=$(shasum -a 256 "$tmp/$asset" | awk '{ print $1 }')
fi
if [ -z "$expected" ] || [ "$expected" != "$actual" ]; then
  echo "paddock: the download does not match the release checksum. Nothing was installed." >&2
  exit 1
fi

dir="$HOME/.local/share/paddock"
program="$dir/paddock"
if [ ! -e "$program" ] && [ -e "$HOME/.local/share/cc-sessions/cc-sessions" ]; then
  # Installed before the rename: update it in place, where its commands point.
  dir="$HOME/.local/share/cc-sessions"
  program="$dir/cc-sessions"
fi
mkdir -p "$dir"
chmod +x "$tmp/$asset"
mv "$tmp/$asset" "$program"
echo "Installed $("$program" --version) in $dir"
echo

# Ask the setup questions on the terminal even when this script was piped in.
if [ -t 0 ]; then
  "$program" setup "$@"
elif (exec </dev/tty) 2>/dev/null; then
  "$program" setup "$@" </dev/tty
else
  "$program" setup --yes "$@"
fi

echo
echo "If Paddock was already running, restart it to use the new version."
echo "From now on it keeps itself up to date (Settings > General > Updates to change that)."
