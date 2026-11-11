#!/bin/sh
# One-time setup of the narration voice for record.mjs: Kokoro (the open-source
# Kokoro-82M neural voice) run locally through the kokoro-onnx package.
#
# Everything goes into docs/record-demo/.tts/ (about 350 MB, ignored by git):
# a Python environment made by uv, the model and the voices file. Nothing is
# installed system-wide. Run it again at any time: it skips what is already there.
# To remove the voice, delete that folder.
#
#   sh docs/record-demo/setup-voice.sh
#
# Needs uv (https://docs.astral.sh/uv/) and curl. UV names uv if it is not on PATH.
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
TTS="$HERE/.tts"
PYTHON_VERSION=3.12
PACKAGE='kokoro-onnx==0.6.1'
FILES=https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0
# The half-precision model: the same voice as the full one at half the size.
MODEL=kokoro-v1.0.fp16.onnx
MODEL_BYTES=177464787
VOICES=voices-v1.0.bin
VOICES_BYTES=28214398

UV=${UV:-}
if [ -z "$UV" ]; then
  if command -v uv >/dev/null 2>&1; then UV=uv
  elif [ -x "$HOME/.local/bin/uv" ]; then UV="$HOME/.local/bin/uv"
  else echo "setup-voice: uv was not found. Install it from https://docs.astral.sh/uv/ or set UV to its path." >&2; exit 1
  fi
fi

mkdir -p "$TTS"

if [ -x "$TTS/venv/bin/python" ]; then
  echo "Python environment: already there."
else
  echo "Python environment: creating $TTS/venv"
  "$UV" venv --quiet --python "$PYTHON_VERSION" "$TTS/venv"
fi

if "$TTS/venv/bin/python" -c 'import kokoro_onnx' >/dev/null 2>&1; then
  echo "kokoro-onnx: already installed."
else
  echo "kokoro-onnx: installing $PACKAGE"
  "$UV" pip install --quiet --python "$TTS/venv/bin/python" "$PACKAGE"
fi

# fetch NAME BYTES: downloads to a .part file first, so a broken download is never mistaken for the file.
fetch() {
  if [ -f "$TTS/$1" ] && [ "$(wc -c < "$TTS/$1" | tr -d ' ')" = "$2" ]; then
    echo "$1: already there."
    return
  fi
  echo "$1: downloading ($(($2 / 1000000)) MB)"
  curl --fail --location --silent --show-error --output "$TTS/$1.part" "$FILES/$1"
  got=$(wc -c < "$TTS/$1.part" | tr -d ' ')
  if [ "$got" != "$2" ]; then
    echo "setup-voice: $1 came out as $got bytes, expected $2." >&2
    exit 1
  fi
  mv "$TTS/$1.part" "$TTS/$1"
}
fetch "$MODEL" "$MODEL_BYTES"
fetch "$VOICES" "$VOICES_BYTES"

# A short line, spoken and thrown away, to show that the voice works here.
"$TTS/venv/bin/python" "$HERE/speak.py" 'The voice is ready.' "$TTS/check.wav"
rm -f "$TTS/check.wav"
echo "Voice ready in $TTS ($(du -sh "$TTS" | cut -f1))."
