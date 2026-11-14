# Recording the demo

`record.mjs` makes `docs/demo.mp4` and `docs/demo.png`, the narrated tour of the page
linked from the top-level README. To record them again, from the repo root:

```sh
cargo build --release
node docs/record-demo/record.mjs
```

For the screenshot alone, add `--still`: it skips the narration and the recording, so
it needs neither the voice nor ffmpeg.

It needs Node 22 or later, Google Chrome, and ffmpeg with ffprobe (and libx264). Set
`CHROME`, `FFMPEG` or `FFPROBE` if they are not in the usual place. It was written on macOS.

## The voice

The narration is spoken by Kokoro (the open-source Kokoro-82M neural voice, through the
`kokoro-onnx` package), which runs on your own machine. Set it up once, from the repo root:

```sh
sh docs/record-demo/setup-voice.sh
```

It needs [uv](https://docs.astral.sh/uv/) and puts everything in `docs/record-demo/.tts/`,
which git ignores: a Python environment, the half-precision model and the voices file,
about 330 MB in all. Nothing is installed system-wide, running it again skips what is
already there, and deleting that folder removes the voice. The voice is `af_heart` at its
normal speed (`KOKORO` near the top of `record.mjs`). `speak.py` does the speaking and can be
tried on its own: `docs/record-demo/.tts/venv/bin/python docs/record-demo/speak.py "Hello." hello.wav`.

The script uses the first of these that is there, and says which:

1. Kokoro, once `setup-voice.sh` has been run;
2. macOS's built-in `say` command (the Samantha voice, or Daniel);
3. no voice: the video is made without sound.

Each spoken line has the silence around it trimmed and a short fade at both ends, and the
finished track is levelled to about -16 LUFS with its true peak under -1.5 dB.

## What it does

The script builds a made-up home folder in `/tmp/alex` (three accounts, ten sessions,
fifteen saved conversations), starts cc-sessions on it on port 47999 with the tests' fake
`claude`, drives the page in headless Chrome, and removes everything again when it is
done. Your own accounts, settings and running server are not read or touched. Pass
`--keep` to leave the demo home and the captured frames in place.

The data is near the top of the script. The captions and the order of the tour are in
`tour()`, and the narration is the `NARRATION` list just above it: one line per scene, in
order. Each line is spoken before the recording starts and measured, and its scene is held
until the line has been said, so changing a line changes the length of the video. If the
length changes, update the "See it" line in the top-level README.
