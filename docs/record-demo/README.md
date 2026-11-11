# Recording the demo

`record.mjs` makes `docs/demo.mp4` and `docs/demo.png`, the tour of the page linked from
the top-level README. To record them again, from the repo root:

```sh
cargo build --release
node docs/record-demo/record.mjs
```

It needs Node 22 or later, Google Chrome and ffmpeg (with libx264). Set `CHROME` or
`FFMPEG` if they are not in the usual place. It was written on macOS.

The script builds a made-up home folder in `/tmp/alex` (three accounts, ten sessions,
fifteen saved conversations), starts cc-sessions on it on port 47999 with the tests' fake
`claude`, drives the page in headless Chrome, and removes everything again when it is
done. Your own accounts, settings and running server are not read or touched. Pass
`--keep` to leave the demo home and the captured frames in place.

The data, the captions and the order of the tour are all near the top of the script.
