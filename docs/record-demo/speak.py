"""Speaks text into a WAV file with Kokoro, the voice set up by setup-voice.sh.

    .tts/venv/bin/python speak.py [--voice af_heart] [--speed 1.0] TEXT OUT.wav [TEXT OUT.wav ...]
    .tts/venv/bin/python speak.py --list-voices

Several TEXT OUT pairs are spoken with the model loaded once, which is how
record.mjs calls it. The WAV is what Kokoro makes: 24 kHz, one channel, 16-bit.
"""

import argparse
import sys
import wave
from pathlib import Path

TTS = Path(__file__).resolve().parent / ".tts"
MODEL = TTS / "kokoro-v1.0.fp16.onnx"
VOICES = TTS / "voices-v1.0.bin"


def main() -> int:
    parser = argparse.ArgumentParser(description="Speak text into a WAV file with Kokoro.")
    parser.add_argument("--voice", default="af_heart", help="a voice in the voices file (default: af_heart)")
    parser.add_argument("--speed", type=float, default=1.0, help="1.0 is the voice's normal pace")
    parser.add_argument("--list-voices", action="store_true", help="print the voices in the voices file and stop")
    parser.add_argument("pairs", nargs="*", metavar="TEXT OUT", help="text to speak, then the WAV file to write")
    args = parser.parse_args()

    for file in (MODEL, VOICES):
        if not file.exists():
            print(f"speak: {file} is missing. Run docs/record-demo/setup-voice.sh first.", file=sys.stderr)
            return 1

    import numpy as np
    from kokoro_onnx import Kokoro

    kokoro = Kokoro(str(MODEL), str(VOICES))
    voices = sorted(kokoro.get_voices())
    if args.list_voices:
        print("\n".join(voices))
        return 0
    if not args.pairs or len(args.pairs) % 2:
        parser.error("give TEXT and OUT in pairs")
    if args.voice not in voices:
        print(f"speak: no voice named {args.voice}. The voices file has: {', '.join(voices)}", file=sys.stderr)
        return 1

    for text, out in zip(args.pairs[::2], args.pairs[1::2]):
        samples, rate = kokoro.create(text, voice=args.voice, speed=args.speed, lang="en-us")
        pcm = (np.clip(samples, -1.0, 1.0) * 32767.0).astype("<i2")
        with wave.open(out, "wb") as wav:
            wav.setnchannels(1)
            wav.setsampwidth(2)
            wav.setframerate(rate)
            wav.writeframes(pcm.tobytes())
    return 0


if __name__ == "__main__":
    sys.exit(main())
