# Recording the demo

`record.mjs` makes `docs/demo.mp4`, a one-minute launch film for 2.5, and `docs/demo.png`,
the classic layout as a still; both are linked from the top-level README. The film is told
the way a keynote tells it: black title cards with large type, short lines, quick reveals
of the Finder layout, Usage and Settings, a soft music bed, and a "one more thing" close.
To record them again, from the repo root:

```sh
cargo build --release
node docs/record-demo/record.mjs
```

For the screenshot alone, add `--still`: it skips the narration and the recording, so
it needs neither the voice nor ffmpeg. `--no-music` leaves the music bed out.

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
fifteen saved conversations with a few weeks of token usage, one pinned folder), starts
cc-sessions on it on port 47999 with the tests' fake `claude`, drives the page in headless
Chrome, and removes everything again when it is done. Your own accounts, settings and
running server are not read or touched. Pass `--keep` to leave the demo home and the
captured frames in place.

The data is near the top of the script. The film is `tour()`: it runs in the Finder layout
(the still uses the classic one), and shows title cards with `a.card({ title, subtitle,
line, glow })` and `a.uncard()`. The narration is the `NARRATION` list just above it: one
line per scene, in order. Each line is spoken before the recording starts and measured,
and its scene is held until the line has been said, so changing a line changes the length
of the video. If the length changes, update the "See it" line in the top-level README.

## The music

`compose()` writes the music bed on this machine, so it is the project's own: an ambient
pad of four slow chords, soft sine tones with two quiet overtones, a little out of tune
left and right, a gentle low-pass and a small room of feedback delays. It swells at each
title card (scenes whose name starts with `card`). `mixSound()` lays the narration on top,
pushes the music down while the voice speaks (ffmpeg's `sidechaincompress`), takes the
tops off the loudest syllables with a limiter, and levels the result to about -16 LUFS.
The script prints how far under the voice the bed sits; `MUSIC.level` near the top moves
it.

## The what's new film

`--film whats-new` makes a 15-second square clip (1080×1080) of what is new in 2.6 and
2.7, for a social feed rather than the README. It is made to be watched without sound:
no voice, short headlines on the picture (`a.headline()`), zooms on what matters
(`a.zoom()`), a cut every two or three seconds and a beat written by `beat()`. It opens on
its point: what each prompt of one session cost. It makes no still, and writes
`docs/whats-new.mp4`, or wherever `--out` says:

```sh
node docs/record-demo/record.mjs --film whats-new --out ~/Desktop/whats-new.mp4
```

Its demo home adds three sessions with real work in them (`WORK`, written by
`workLines()`), since the loop check only judges sessions with edits, commands and enough
tokens: one goes round in circles, the same test failing after every fix, and two healthy
ones give "your usual" something to go on. It also says the tool was just updated from
2.5.1, so the "Updated to" window can be shown. Its scenes are `tourWhatsNew()`.

## The Paddock film

`--film paddock` makes the film of 2.8 (`docs/paddock.mp4` in the top-level README, and
the launch post). It is made differently from the others: `record.mjs` only takes stills
of the real page, and `paddock-film/` makes the film from them.

```sh
cargo build --release
node docs/record-demo/record.mjs --film paddock [--cut long|short|both] [--draft] [--out DIR]
```

- `record.mjs --film paddock` adds a Codex account to the demo home (`CODEX`: rollouts as
  Codex writes them, two of them held open by a stand-in `codex` so they show as working),
  takes stills of the page at four times its size with where things are on them
  (`capturePaddock()`), and writes them to `paddock-film/.shots`. `--stills` stops there.
- `paddock-film/film.mjs` draws every frame of `stage.html` on its own clock in headless
  Chrome: the stills in a window on a dark stage, a camera on springs, headlines, labels, a
  pointer and the end card. Frames are drawn at twice the size and 120 a second, and each
  frame of the film is the blend of two, so moves get motion blur. It can be run on its own
  once the stills are there: `node docs/record-demo/paddock-film/film.mjs --cut long --draft`.
- `paddock-film/score.js` writes the sound in Chrome's offline Web Audio, with no samples:
  a pad, drums, bass, a riser and an impact, and effects (whooshes, ticks, pops) on the
  picture's cues. `film.mjs` mixes and masters it to -14 LUFS with true peaks under -1 dB,
  and prints the loudness, the peak and how much is lost folded to mono.
- `paddock-film/timeline.js` holds every time both use, in beats. There are two cuts:
  `short` (15 s, 160 beats a minute) opens on the app; `long` (25 s, 128) opens on an
  abstract intro of the sessions the author had open (`OPEN`) gathering into one window.

Each cut and shape (4:5 and 1:1) gets an mp4, a contact sheet and thumbnails; the films
go to the Desktop unless `--out` says otherwise. The fonts (Inter and JetBrains Mono, under
the SIL Open Font License) are fetched once into `paddock-film/.fonts`. `--draft` takes
about a minute; a finished cut takes about 30 minutes for each shape.
