// Makes the Paddock film from the stills record.mjs --film paddock took: draws
// every frame of stage.html on its own clock in headless Chrome, writes the score
// (score.js, in Chrome's offline Web Audio), mixes and masters it, and encodes
// both shapes of the film.
//
//   node docs/record-demo/paddock-film/film.mjs [--cut long|short|both] [--draft] [--sheet] [--only 4x5|1x1] [--out DIR]
//
// --cut picks the cut: long (25 s, opening on the abstract intro), short (15 s,
// opening on the app) or both, the default.
// --draft is quick: 30 frames a second at the frame's own size, no motion blur.
// --sheet makes only a contact sheet of each shape (six frames) and the sound.
// --out is where the films go (default: the Desktop).
//
// The stills are read from paddock-film/.shots, where record.mjs puts them. The
// fonts (Inter and JetBrains Mono, both under the SIL Open Font License) are
// fetched once from Google Fonts into paddock-film/.fonts. Both folders are
// ignored by git. Needs Google Chrome and ffmpeg with libx264; on macOS the AAC
// is Apple's own encoder (aac_at), elsewhere ffmpeg's.

import { spawn, spawnSync } from 'node:child_process';
import { once } from 'node:events';
import { createReadStream, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

import { timeline } from './timeline.js';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const SHOTS = path.join(HERE, '.shots');
const FONTS = path.join(HERE, '.fonts');
const CHROME = process.env.CHROME || '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const FFMPEG = process.env.FFMPEG || 'ffmpeg';
const SHAPES = { '4x5': { w: 1080, h: 1350 }, '1x1': { w: 1080, h: 1080 } };
// The loudness feeds play at: -14 LUFS integrated, true peaks under -1 dB.
const MASTER = { loudness: -14, peak: -1, rate: 48000 };
const log = (...words) => console.log(...words);
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function run(program, args, what) {
  const out = spawnSync(program, args, { encoding: 'utf8', maxBuffer: 1 << 26 });
  if (out.error) throw new Error(`${what}: ${out.error.message}`);
  if (out.status !== 0) throw new Error(`${what} failed:\n${out.stderr.slice(-2000)}`);
  return out;
}

// --------------------------------------------------------------------- fonts

const FONT_CSS = 'https://fonts.googleapis.com/css2?family=Inter:opsz,wght@14..32,100..900&family=JetBrains+Mono:wght@500&display=block';

async function fetchFonts() {
  const want = { inter: 'Inter', 'jetbrains-mono': 'JetBrains Mono' };
  if (Object.keys(want).every((name) => existsSync(path.join(FONTS, `${name}.woff2`)))) return;
  mkdirSync(FONTS, { recursive: true });
  // Google Fonts answers with woff2 for a browser it knows.
  const agent = 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36';
  const text = await (await fetch(FONT_CSS, { headers: { 'user-agent': agent } })).text();
  // Each face comes after a comment naming its script; the Latin ones are wanted.
  const faces = [...text.matchAll(/\/\* latin \*\/\s*@font-face\s*\{([^}]*)\}/g)].map((m) => m[1]);
  for (const [name, family] of Object.entries(want)) {
    const face = faces.find((f) => f.includes(`font-family: '${family}'`));
    const src = face && face.match(/url\((https:[^)]+\.woff2)\)/);
    if (!src) throw new Error(`Google Fonts did not list ${family}.`);
    writeFileSync(path.join(FONTS, `${name}.woff2`), Buffer.from(await (await fetch(src[1])).arrayBuffer()));
    log(`Fetched the ${family} font`);
  }
}

// ------------------------------------------------------------------ the page

// The stage, the stills and the fonts, from one small local server.
export function serve() {
  const types = { '.html': 'text/html', '.js': 'text/javascript', '.json': 'application/json', '.png': 'image/png', '.woff2': 'font/woff2' };
  const server = createServer((req, res) => {
    const url = decodeURIComponent(new URL(req.url, 'http://x').pathname);
    const [root, rest] = url.startsWith('/shots/') ? [SHOTS, url.slice(7)] : url.startsWith('/fonts/') ? [FONTS, url.slice(7)] : [HERE, url.slice(1)];
    const file = path.join(root, rest);
    if (!file.startsWith(root) || !existsSync(file) || !statSync(file).isFile()) { res.writeHead(404).end(); return; }
    res.writeHead(200, { 'content-type': types[path.extname(file)] || 'application/octet-stream', 'cache-control': 'no-store' });
    createReadStream(file).pipe(res);
  });
  return new Promise((resolve) => server.listen(0, '127.0.0.1', () => resolve(server)));
}

// A small Chrome DevTools Protocol client, as in record.mjs.
export async function openChrome(profile) {
  const chrome = spawn(CHROME, ['--headless=new', '--remote-debugging-port=0', `--user-data-dir=${profile}`, '--no-first-run',
    '--no-default-browser-check', '--hide-scrollbars', '--force-color-profile=srgb', '--autoplay-policy=no-user-gesture-required',
    'about:blank'], { stdio: 'ignore' });
  let port = 0;
  for (let i = 0; i < 100 && !port; i++) {
    await sleep(100);
    try { port = Number(readFileSync(path.join(profile, 'DevToolsActivePort'), 'utf8').split('\n')[0]); } catch { /* not up yet */ }
  }
  if (!port) { chrome.kill(); throw new Error('Chrome did not start.'); }
  const target = (await (await fetch(`http://127.0.0.1:${port}/json`)).json()).find((t) => t.type === 'page');
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { ws.addEventListener('open', resolve); ws.addEventListener('error', reject); });
  let nextId = 1;
  const pending = new Map();
  const problems = [];
  ws.addEventListener('message', (event) => {
    const msg = JSON.parse(event.data);
    if (msg.id && pending.has(msg.id)) {
      const { resolve, reject } = pending.get(msg.id);
      pending.delete(msg.id);
      if (msg.error) reject(new Error(msg.error.message)); else resolve(msg.result);
    } else if (msg.method === 'Runtime.exceptionThrown') {
      problems.push(msg.params.exceptionDetails.exception?.description || msg.params.exceptionDetails.text);
    } else if (msg.method === 'Runtime.consoleAPICalled' && msg.params.type === 'error') {
      problems.push(msg.params.args.map((a) => a.value ?? a.description).join(' '));
    }
  });
  const send = (method, params = {}) => new Promise((resolve, reject) => {
    const id = nextId++;
    pending.set(id, { resolve, reject });
    ws.send(JSON.stringify({ id, method, params }));
  });
  const evaluate = async (expression) => {
    const r = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
    return r.result.value;
  };
  await send('Page.enable');
  await send('Runtime.enable');
  return {
    send, evaluate, problems,
    async open(url, ready) {
      await send('Page.navigate', { url });
      for (let i = 0; i < 300; i++) {
        if (problems.length) throw new Error(`The page failed: ${problems.join('\n')}`);
        if (await evaluate(ready).catch(() => false)) return;
        await sleep(100);
      }
      throw new Error(`${url} did not get ready.`);
    },
    async close() {
      const gone = chrome.exitCode === null ? once(chrome, 'exit') : Promise.resolve();
      send('Browser.close').catch(() => {});
      const timer = setTimeout(() => chrome.kill('SIGKILL'), 5000);
      await gone;
      clearTimeout(timer);
    },
  };
}

// --------------------------------------------------------------- the picture

export async function openStage(page, base, shape, scale, cut = 'short') {
  await page.send('Emulation.setDeviceMetricsOverride', { width: shape.w, height: shape.h, deviceScaleFactor: scale, mobile: false });
  await page.open(`${base}/stage.html?w=${shape.w}&h=${shape.h}&cut=${cut}`, 'window.ready === true');
}

async function frame(page, t, format = 'png') {
  await page.evaluate(`render(${t})`);
  const shot = await page.send('Page.captureScreenshot', { format, ...(format === 'jpeg' ? { quality: 95 } : {}), optimizeForSpeed: format === 'jpeg' });
  return Buffer.from(shot.data, 'base64');
}

// The middle of each scene and the end card, in a grid; with the intro, four of its
// moments first (scattered, the type, gathering, the drop).
async function sheet(page, shape, file, work, T) {
  const { SCENES, IMPACT, INTRO } = T;
  const intro = INTRO ? [1.0, 2.3, INTRO - 0.9, INTRO - 0.08] : [];
  const times = [...intro, SCENES.both + 1.6, SCENES.all + 1.9, SCENES.search + 1.0, SCENES.search + 2.6, SCENES.usage + 1.8, IMPACT + 2.2];
  const files = [];
  for (const [n, t] of times.entries()) {
    const f = path.join(work, `sheet-${n}.png`);
    writeFileSync(f, await frame(page, t));
    files.push(f);
  }
  run(FFMPEG, ['-hide_banner', '-loglevel', 'error', '-y', '-i', path.join(work, 'sheet-%d.png'),
    '-vf', `scale=${Math.round(shape.w / 2.5)}:-2:flags=lanczos,tile=${times.length / 2}x2:padding=12:color=0x202024`, '-frames:v', '1', file], 'Making the contact sheet');
}

// Every frame, drawn and piped to ffmpeg. The finished film runs at 60 a second
// with each frame the blend of two drawn at 120: a 180° shutter's motion blur.
async function picture(page, shape, { draft, sound, target, T }) {
  const { LENGTH } = T;
  const fps = draft ? 30 : 120;
  const count = Math.round(LENGTH * fps);
  const blur = draft ? '' : 'tmix=frames=2:weights=1 1,fps=60,';
  const aac = process.platform === 'darwin' ? ['-c:a', 'aac_at', '-aac_at_mode', 'cbr', '-b:a', '320k'] : ['-c:a', 'aac', '-b:a', '320k'];
  const ffmpeg = spawn(FFMPEG, [
    '-hide_banner', '-loglevel', 'error', '-y',
    '-f', 'image2pipe', '-framerate', String(fps), '-c:v', draft ? 'mjpeg' : 'png', '-i', '-',
    '-i', sound,
    '-vf', `${blur}scale=${shape.w}:${shape.h}:flags=lanczos:in_range=pc:out_range=tv:out_color_matrix=bt709,format=yuv420p`,
    '-colorspace', 'bt709', '-color_primaries', 'bt709', '-color_trc', 'bt709', '-color_range', 'tv',
    '-c:v', 'libx264', '-profile:v', 'high', '-preset', draft ? 'veryfast' : 'slower', '-crf', draft ? '20' : '14',
    '-g', draft ? '60' : '120', '-r', draft ? '30' : '60',
    '-map', '0:v', '-map', '1:a', ...aac, '-shortest', '-movflags', '+faststart', target,
  ], { stdio: ['pipe', 'inherit', 'inherit'] });
  const done = new Promise((resolve, reject) => {
    ffmpeg.on('error', (err) => reject(new Error(`Could not run ${FFMPEG}: ${err.message}`)));
    ffmpeg.on('exit', (code) => (code === 0 ? resolve() : reject(new Error(`ffmpeg failed (exit ${code}).`))));
  });
  ffmpeg.stdin.on('error', () => {});
  const began = Date.now();
  for (let n = 0; n < count; n++) {
    const image = await frame(page, n / fps, draft ? 'jpeg' : 'png');
    if (!ffmpeg.stdin.write(image)) await once(ffmpeg.stdin, 'drain');
    if (n % (fps * 2) === 0 && n) {
      const left = ((Date.now() - began) / n) * (count - n) / 1000;
      log(`  ${(n / fps).toFixed(0)}s of ${LENGTH}s drawn, about ${Math.ceil(left / 60)} min left`);
    }
  }
  ffmpeg.stdin.end();
  await done;
}

// ------------------------------------------------------------------ the sound

// The score and the effects, rendered by score.js in the page, as two stems.
async function score(page, base, work, T) {
  await page.open(`${base}/score.html?cut=${T.cut}`, 'window.ready === true');
  const stems = await page.evaluate(`renderStems(${JSON.stringify(T.cues())})`);
  const files = {};
  for (const [name, data] of Object.entries(stems)) {
    files[name] = path.join(work, `${name}.wav`);
    writeFileSync(files[name], Buffer.from(data, 'base64'));
  }
  return files;
}

// The mix and the master. The music is cleaned up and glued, and ducked a little
// whenever an effect hits; the two are summed and compressed gently. Then it is
// brought to -14 LUFS by one steady gain and its peaks caught by a limiter run at
// four times the sample rate, so it holds true peaks (between the samples) too,
// under -1 dB. The limiter takes a little loudness, so the gain is measured and set
// twice. Then the result is measured, in stereo and folded to mono, and printed.
function master(stems, file, { LENGTH }) {
  const mix = '[0:a]highpass=f=28,acompressor=threshold=-20dB:ratio=2:attack=25:release=280:knee=4:makeup=1.5[music];'
    + '[1:a]acompressor=threshold=-16dB:ratio=2.5:attack=5:release=120:knee=3,asplit[fx][key];'
    + '[music][key]sidechaincompress=threshold=0.06:ratio=3:attack=8:release=320:knee=4[ducked];'
    + '[ducked][fx]amix=inputs=2:duration=longest:normalize=0,'
    + 'acompressor=threshold=-10dB:ratio=1.6:attack=30:release=300:knee=6';
  const inputs = ['-i', stems.music, '-i', stems.fx];
  // Under the target by enough to cover what going back to 48 kHz adds between samples.
  const ceiling = 10 ** ((MASTER.peak - 0.9) / 20);
  const chain = (gain) => `${mix},volume=${gain.toFixed(2)}dB,aresample=${MASTER.rate * 4},`
    + `alimiter=limit=${ceiling.toFixed(4)}:attack=1:release=60:level=disabled,`
    + `aresample=${MASTER.rate}:filter_size=64:phase_shift=10:cutoff=0.97,atrim=end=${LENGTH}`;
  const loudness = (filter) => {
    const out = run(FFMPEG, ['-hide_banner', '-nostats', ...inputs, '-filter_complex', `${filter},ebur128=framelog=quiet`, '-f', 'null', '-'], 'Measuring the mix');
    return Number(out.stderr.slice(out.stderr.lastIndexOf('Summary:')).match(/I:\s+(-?[\d.]+) LUFS/)[1]);
  };
  let gain = MASTER.loudness - loudness(mix);
  for (let pass = 0; pass < 2; pass++) gain += MASTER.loudness - loudness(chain(gain));
  run(FFMPEG, ['-hide_banner', '-nostats', '-y', ...inputs, '-filter_complex', chain(gain), '-c:a', 'pcm_s24le', file], 'Mastering');
  // The finished file, measured again: loudness and true peak, and how much is lost folded to mono.
  const meter = (filter) => {
    const out = run(FFMPEG, ['-hide_banner', '-nostats', '-i', file, '-af', `${filter}ebur128=peak=true:framelog=quiet`, '-f', 'null', '-'], 'Metering');
    const tail = out.stderr.slice(out.stderr.lastIndexOf('Summary:'));
    return { i: Number(tail.match(/I:\s+(-?[\d.]+) LUFS/)[1]), peak: Number(tail.match(/Peak:\s+(-?[\d.]+) dBFS/)[1]) };
  };
  const stereo = meter('');
  // Mono as a phone speaker plays it: the two sides summed, in both channels.
  const mono = meter('pan=stereo|c0=0.5*c0+0.5*c1|c1=0.5*c0+0.5*c1,');
  log(`Master: ${stereo.i} LUFS, true peak ${stereo.peak} dBTP (gain ${gain.toFixed(1)} dB); `
    + `folded to mono ${mono.i} LUFS, ${(stereo.i - mono.i).toFixed(1)} dB down.`);
  return { stereo, mono };
}

// ---------------------------------------------------------------------- main

export async function makeFilm({ cut = 'both', draft = false, sheetOnly = false, only = null, out = path.join(os.homedir(), 'Desktop') } = {}) {
  if (!existsSync(path.join(SHOTS, 'shots.json'))) throw new Error(`No stills in ${SHOTS}. Run: node docs/record-demo/record.mjs --film paddock`);
  if (!existsSync(CHROME)) throw new Error(`Google Chrome was not found at ${CHROME}. Set CHROME to its path.`);
  await fetchFonts();
  mkdirSync(out, { recursive: true });
  const work = mkdtempSync(path.join(os.tmpdir(), 'paddock-film-'));
  const server = await serve();
  const base = `http://127.0.0.1:${server.address().port}`;
  const page = await openChrome(path.join(work, 'chrome'));
  try {
    for (const which of cut === 'both' ? ['long', 'short'] : [cut]) {
      const T = timeline(which);
      const label = `${T.LENGTH}s`;
      const sound = path.join(out, `paddock-2.8-${label}-sound.wav`);
      master(await score(page, base, work, T), sound, T);
      for (const [name, shape] of Object.entries(SHAPES)) {
        if (only && only !== name) continue;
        await openStage(page, base, shape, sheetOnly || !draft ? 2 : 1, which);
        const stem = `paddock-2.8-${label}-${name}`;
        const sheetFile = path.join(out, `${stem}-sheet.png`);
        await sheet(page, shape, sheetFile, work, T);
        log(`Wrote ${sheetFile}`);
        // The thumbnail: the first app scene with its whole headline up.
        writeFileSync(path.join(out, `${stem}-thumbnail.png`), await frame(page, T.SCENES.both + 2.4));
        // And with the intro, its three lines all up: the post's own hook.
        if (T.INTRO) writeFileSync(path.join(out, `${stem}-thumbnail-intro.png`), await frame(page, T.SAID[2] + 1.1));
        if (sheetOnly) continue;
        const target = path.join(out, `${stem}${draft ? '-draft' : ''}.mp4`);
        log(`Drawing the ${label} ${name} film${draft ? ' (draft)' : ''}…`);
        await picture(page, shape, { draft, sound, target, T });
        log(`Wrote ${target} (${(statSync(target).size / 1e6).toFixed(1)} MB)`);
      }
    }
    if (page.problems.length) log(`Page errors:\n${page.problems.join('\n')}`);
  } finally {
    await page.close();
    server.close();
    rmSync(work, { recursive: true, force: true, maxRetries: 20, retryDelay: 250 });
  }
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  const option = (name) => { const i = process.argv.indexOf(name); return i < 0 ? null : process.argv[i + 1] || null; };
  makeFilm({
    cut: option('--cut') || 'both',
    draft: process.argv.includes('--draft'),
    sheetOnly: process.argv.includes('--sheet'),
    only: option('--only'),
    out: option('--out') ? path.resolve(option('--out')) : undefined,
  }).catch((err) => {
    console.error(`paddock-film: ${err.message}`);
    process.exit(1);
  });
}
