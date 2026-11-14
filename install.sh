#!/bin/sh
# cc-sessions installer for macOS and Linux.
#
#   curl -fsSL https://raw.githubusercontent.com/barath-dev/cc-sessions/main/install.sh | sh
#
# Downloads the one program for this system, checks it against the release's
# SHA-256 list, puts it in ~/.local/share/cc-sessions, and runs its setup.
# Running it again later updates the program and keeps all settings.
# To pass setup options:  curl ... | sh -s -- --yes --command cs
set -eu

REPO="barath-dev/cc-sessions"
BASE="${CC_SESSIONS_INSTALL_BASE:-https://github.com/$REPO/releases/latest/download}"

case "$(uname -s)" in
  Darwin) os=macos ;;
  Linux) os=linux ;;
  *) echo "cc-sessions: this installer is for macOS and Linux. On Windows use install.ps1." >&2; exit 1 ;;
esac
case "$(uname -m)" in
  arm64 | aarch64) arch=arm64 ;;
  x86_64 | amd64) arch=x86_64 ;;
  *) echo "cc-sessions: no build for processor type $(uname -m)." >&2; exit 1 ;;
esac
asset="cc-sessions-$os-$arch"

fetch() {
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$1" -o "$2"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$2" "$1"
  else
    echo "cc-sessions: curl or wget is needed to download." >&2
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
  echo "cc-sessions: the download does not match the release checksum. Nothing was installed." >&2
  exit 1
fi

dir="$HOME/.local/share/cc-sessions"
mkdir -p "$dir"
chmod +x "$tmp/$asset"
mv "$tmp/$asset" "$dir/cc-sessions"
echo "Installed $("$dir/cc-sessions" --version) in $dir"
echo

# Ask the setup questions on the terminal even when this script was piped in.
if [ -t 0 ]; then
  "$dir/cc-sessions" setup "$@"
elif (exec </dev/tty) 2>/dev/null; then
  "$dir/cc-sessions" setup "$@" </dev/tty
else
  "$dir/cc-sessions" setup --yes "$@"
fi

echo
echo "If Claude Sessions was already running, restart it to use the new version."
echo "From now on it keeps itself up to date (Settings > General > Updates to change that)."
