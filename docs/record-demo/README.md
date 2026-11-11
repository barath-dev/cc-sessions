# Recording the demo

`record.mjs` makes `docs/demo.mp4` and `docs/demo.png`, the narrated tour of the page
linked from the top-level README. To record them again, from the repo root:

```sh
cargo build --release
node docs/record-demo/record.mjs
```

It needs Node 22 or later, Google Chrome, and ffmpeg with ffprobe (and libx264). Set
`CHROME`, `FFMPEG` or `FFPROBE` if they are not in the usual place. It was written on macOS.

The narration is spoken by macOS's built-in `say` command (the Samantha voice, or Daniel
if that is not installed) and mixed into the video by the same run. On a system without
`say` the script says so and makes the video without sound.

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
