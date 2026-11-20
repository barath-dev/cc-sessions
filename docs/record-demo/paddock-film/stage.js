// The Paddock film's picture. `render(t)` draws the frame at `t` seconds and
// nothing else: no CSS transitions or animations, so film.mjs can draw any frame
// in any order and get the same picture every time.
//
// The page is the real app's stills (record.mjs --film paddock) in a window on a
// dark stage, moved by a camera, with headlines, labels, a pointer and the end
// card drawn over it. ?w=&h= is the frame's size: 1080×1350 (4:5) or 1080×1080.
// ?cut=long opens on the abstract intro (timeline.js) before the app.

import { seeded, timeline } from './timeline.js';

const params = new URLSearchParams(location.search);
const W = Number(params.get('w') || 1080);
const H = Number(params.get('h') || 1350);
const TALL = H / W > 1.1;
const CUT = params.get('cut') || 'short';
const LONG = CUT === 'long';
const {
  BEAT, BAR, LENGTH, INTRO, SCENES, RISE, IMPACT, GATHER, OUTLINES, SAID, WORD, typedAt, PICK, RESUME, LABELS, RING, barAt,
} = timeline(CUT);

// ------------------------------------------------------------------- easing

const clamp = (v, lo = 0, hi = 1) => Math.min(hi, Math.max(lo, v));
const lerp = (a, b, p) => a + (b - a) * p;
const progress = (t, from, length) => clamp((t - from) / length);
const outExpo = (p) => (p >= 1 ? 1 : 1 - 2 ** (-10 * p));
const outCubic = (p) => 1 - (1 - p) ** 3;
const inCubic = (p) => p ** 3;
const inOutSine = (p) => 0.5 - 0.5 * Math.cos(Math.PI * p);
const inOutCubic = (p) => (p < 0.5 ? 4 * p ** 3 : 1 - (-2 * p + 2) ** 3 / 2);
// A critically damped spring, reaching exactly 1 at p = 1.
const spring = (p) => {
  const w = 7.5;
  const f = (x) => 1 - (1 + w * x) * Math.exp(-w * x);
  return f(p) / f(1);
};
// An underdamped one, for things that pop: a little past 1 and back.
const pop = (p) => (p >= 1 ? 1 : 1 - Math.exp(-6 * p) * Math.cos(9 * p));

// --------------------------------------------------------------------- DOM

const css = (el, rules) => { for (const [k, v] of Object.entries(rules)) el.style.setProperty(k, typeof v === 'number' ? `${v}px` : v); return el; };
const make = (tag, attrs = {}, ...children) => {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === 'style') css(el, v); else if (k === 'class') el.className = v; else el.setAttribute(k, v);
  }
  el.append(...children);
  return el;
};
const $ = (id) => document.getElementById(id);

const shots = await (await fetch('/shots/shots.json')).json();
const VIEW = shots.view;
const shot = Object.fromEntries(shots.shots.map((s) => [s.name, s]));
const box = (name, key) => shot[name].boxes[key];
const mid = (b) => ({ x: b.x + b.w / 2, y: b.y + b.h / 2 });

// The window: as wide as the frame allows, below the headline.
const PAD = Math.round(W * 0.055);
const HEAD = { top: Math.round(H * (TALL ? 0.07 : 0.06)), size: TALL ? 78 : 66 };
const HEAD_BOTTOM = HEAD.top + HEAD.size * 2.1;
const WIN_W = W - 2 * PAD;
const SCALE = WIN_W / VIEW.width;
const TITLE = Math.round(WIN_W * 0.032);
const WIN_H = TITLE + VIEW.height * SCALE;
// Where the camera's focus sits on screen: the middle of the room under the headline.
const ANCHOR = { x: W / 2, y: HEAD_BOTTOM + (H - HEAD_BOTTOM) / 2 + (TALL ? 10 : 6) };

const stage = css($('stage'), { width: W, height: H });
const aurora = make('div', { class: 'layer', id: 'aurora' });
const warm = make('div', { class: 'glow' });
const cool = make('div', { class: 'glow' });
const deep = make('div', { class: 'glow' });
aurora.append(deep, warm, cool);

const content = make('div', { id: 'content', style: { width: WIN_W, height: VIEW.height * SCALE } });
const images = {};
for (const s of shots.shots) {
  const img = make('img', { src: `/shots/${s.file}`, alt: '' });
  images[s.name] = img;
  content.append(img);
}
const marks = make('div', { class: 'mark', style: { transform: `scale(${SCALE})`, width: VIEW.width, height: VIEW.height, left: 0, top: 0 } });
content.append(marks);
const lights = make('div', { id: 'lights', style: { left: TITLE * 0.55, gap: TITLE * 0.28 } },
  ...['#ff5f57', '#febc2e', '#28c840'].map((c) => make('i', { style: { width: TITLE * 0.4, height: TITLE * 0.4, background: c } })));
const titlebar = make('div', { id: 'titlebar', style: { height: TITLE, 'font-size': TITLE * 0.42 } }, lights, 'Paddock');
const win = make('div', { id: 'window', style: { width: WIN_W, height: WIN_H } }, titlebar, content);
const camera = make('div', { id: 'camera', style: { width: WIN_W, height: WIN_H } }, win);

// Marks drawn on the page, in its own pixels (they move and scale with it).
const ring = (b, color) => marks.appendChild(make('div', { class: 'ring', style: {
  left: b.x - 5, top: b.y - 5, width: b.w + 10, height: b.h + 10, border: `2.5px solid ${color}`,
  'box-shadow': `0 0 0 4px ${color}33, 0 0 28px 6px ${color}66`, opacity: 0 } }));
const codexRing = ring(box('all', 'codexChip'), '#2fd3bd');
const resumeRing = ring(box('picked', 'resume'), '#2fd3bd');
const covers = box('usage', 'bars').map((bar, n) => {
  const value = box('usage', 'values')[n];
  const right = value.x + value.w + 4;
  return marks.appendChild(make('div', { class: 'cover', style: { left: bar.x - 2, top: Math.min(bar.y, value.y) - 2, width: right - bar.x + 2, height: Math.max(bar.h, value.h) + 4 } }));
});

// Labels and the pointer: pinned to the page but kept one size on screen.
const toolPill = (text, color) => make('div', { class: 'pill', style: {
  'font-size': 25, gap: 10, padding: '9px 18px 9px 13px', background: `linear-gradient(180deg, ${color[0]}, ${color[1]})`,
  'box-shadow': `0 8px 26px ${color[1]}88, inset 0 1px 0 rgb(255 255 255 / 0.35)` } },
  make('b', { style: { width: 11, height: 11, background: 'rgb(255 255 255 / 0.95)' } }), text);
const claudePill = toolPill('Claude Code', ['#f7a15a', '#d9692f']);
const codexPill = toolPill('Codex', ['#3ad8c2', '#16998b']);
const pointer = make('div', { id: 'pointer' });
pointer.innerHTML = '<svg width="34" height="44" viewBox="0 0 26 34" style="overflow:visible;filter:drop-shadow(0 3px 6px rgb(0 0 0 / 0.35))">'
  + '<path d="M2 2 L2 26.5 L8.2 20.8 L12.6 30.6 L16.6 28.8 L12.3 19.2 L20.6 19.2 Z" fill="#111820" stroke="#fff" stroke-width="1.8" stroke-linejoin="round"/></svg>';
const ripple = make('div', { class: 'ring', style: { 'border-radius': '50%', border: '3px solid #ffffff', opacity: 0 } });
const pinned = make('div', { class: 'mark', style: { left: 0, top: 0 } }, claudePill, codexPill, ripple, pointer);
win.append(pinned);

// Headlines: words that rise into place, sharpen and settle, a line at a time
// (`starts` says when each line begins). The intro's are bigger, in the middle.
const headline = make('div', { class: 'headline', style: { top: HEAD.top, 'font-size': HEAD.size, 'line-height': '1.06' } });
const INTRO_SIZE = TALL ? 88 : 76;
const introType = make('div', { class: 'headline', style: { top: H / 2 - INTRO_SIZE * 1.65, 'font-size': INTRO_SIZE, 'line-height': '1.08' } });
const HEADLINES = [
  ...(LONG ? [{ parent: introType, from: SAID[0], to: GATHER - 0.25, starts: SAID,
    lines: [[['23'], ['sessions'], ['open.']], [['15', 'done'], ['already', 'done'], ['done.', 'done']], [['3', 'waiting'], ['waiting', 'waiting'], ['on'], ['me.']]] }] : []),
  { from: LONG ? INTRO + 0.05 : -0.32, to: SCENES.all, lines: [[['Claude Code', 'claude'], ['and'], ['Codex.', 'codex']], [['One'], ['place.']]], second: SCENES.both + BAR - 0.05 },
  { from: SCENES.all + 0.08, to: SCENES.search, lines: [[['Every'], ['session.']], [['Every'], ['account.', 'both']]], second: SCENES.all + BAR * 0.5 },
  { from: SCENES.search + 0.08, to: SCENES.usage, lines: [[['Search'], ['both.']], [['Resume'], ['either.', 'both']]], second: PICK - 0.05 },
  { from: SCENES.usage + 0.08, to: RISE + 0.1, lines: [[['Tokens'], ['from'], ['both,']], [['by'], ['model.', 'both']]], second: SCENES.usage + BAR * 0.5 },
].map((spec) => {
  const lines = spec.lines.map((words) => make('div', { class: 'line' }, ...words.flatMap(([text, kind], i) => [
    ...(i ? [' '] : []), make('span', { class: `word ${kind || ''}` }, text)])));
  const el = make('div', {}, ...lines);
  (spec.parent || headline).append(el);
  return { size: spec.parent ? INTRO_SIZE : HEAD.size, starts: spec.starts || [spec.from, spec.second], ...spec, el,
    words: lines.map((line) => [...line.querySelectorAll('.word')]) };
});

// The end card: the old name, which breaks up on the impact as the new one forms.
const letters = (text) => [...text].map((ch) => make('span', {}, ch));
const oldName = make('div', { id: 'old', style: { 'font-size': TALL ? 116 : 104, color: '#d8d8de' } }, ...letters('cc-sessions'));
const newName = make('div', { id: 'name', style: { 'font-size': TALL ? 168 : 150 } }, ...letters('Paddock'));
const names = make('div', { id: 'names' }, oldName, newName);
const sub = make('div', { id: 'sub', style: { 'font-size': TALL ? 40 : 36, 'margin-top': TALL ? 26 : 20 } }, 'Claude Code + Codex, in one place');
const small = make('div', { id: 'small', style: { 'font-size': TALL ? 26 : 24, 'margin-top': 18 } }, 'Formerly cc-sessions  ·  Free and open source  ·  macOS, Windows, Linux');
const url = make('div', { id: 'url', style: { 'font-size': TALL ? 27 : 25, 'margin-top': TALL ? 46 : 36, padding: '13px 24px' } }, 'github.com/barath-dev/paddock');
const bloom = make('div', { id: 'bloom', style: { width: W * 1.1, height: W * 0.7, left: -W * 0.05, top: H / 2 - W * 0.35 } });
const end = make('div', { id: 'end', class: 'layer' }, names, sub, small, url);
const endLayer = make('div', { class: 'layer' }, bloom, end);

// Film grain: a few tiles of noise, one per frame at 24 a second, like film.
const grain = make('div', { class: 'layer', id: 'grain' });
const GRAIN = Array.from({ length: 8 }, (_, n) => {
  const c = document.createElement('canvas');
  c.width = c.height = 256;
  const g = c.getContext('2d');
  const data = g.createImageData(256, 256);
  const random = seeded(n + 1);
  for (let i = 0; i < data.data.length; i += 4) {
    const v = 128 + (random() + random() + random() - 1.5) * 150;
    data.data[i] = data.data[i + 1] = data.data[i + 2] = v;
    data.data[i + 3] = 255;
  }
  g.putImageData(data, 0, 0);
  return `url(${c.toDataURL()})`;
});

// A dark fall-off at the top, so a headline reads however close the camera is.
const scrim = make('div', { class: 'layer', style: {
  background: `linear-gradient(180deg, rgb(6 7 10 / 0.97) 0px, rgb(6 7 10 / 0.9) ${HEAD_BOTTOM - 30}px, rgb(6 7 10 / 0) ${HEAD_BOTTOM + 70}px)` } });

// The intro: the open sessions as terminal outlines, each a title rule, a few lines
// of text and sometimes a cursor, in the colours Paddock gives their states (working
// blue, idle grey; done green and waiting amber once the type has said so); a pool
// of dark behind the type; and the flash the window arrives in.
const STATE = { neutral: [205, 210, 222], working: [96, 165, 250], idle: [140, 146, 160], done: [74, 222, 128], waiting: [245, 165, 36] };
const DEPTH = [{ scale: 1, blur: 0, alpha: 1, drift: 16 }, { scale: 0.74, blur: 1.4, alpha: 0.72, drift: 10 }, { scale: 0.54, blur: 3.2, alpha: 0.48, drift: 6 }];
const BASE = W * 0.17;
const outlines = OUTLINES.map((o) => {
  const random = seeded(o.seed);
  const bars = Array.from({ length: o.lines }, () => make('i', { style: { display: 'block', height: 3, 'border-radius': 2,
    width: `${35 + random() * 50}%`, margin: '0 0 7px 0' } }));
  const cursor = make('i', { style: { display: o.cursor || o.state === 'working' ? 'block' : 'none', width: 8, height: 12, 'border-radius': 1 } });
  const tick = make('div', { style: { position: 'absolute', right: 8, top: 6, opacity: 0 } });
  tick.innerHTML = '<svg width="20" height="20" viewBox="0 0 20 20"><circle cx="10" cy="10" r="9" fill="rgb(74 222 128)"/>'
    + '<path d="M5.5 10.4 L8.6 13.3 L14.5 7" fill="none" stroke="#06200f" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"/></svg>';
  const rule = make('i', { style: { display: 'block', height: 0, 'border-top': '1.5px solid', margin: '0 0 10px 0' } });
  const el = make('div', { style: { position: 'absolute', left: 0, top: 0, 'border-radius': 10, padding: '0 12px', overflow: 'hidden',
    'border-width': '1.5px', 'border-style': 'solid', 'transform-origin': '50% 50%' } }, rule, ...bars, cursor, tick);
  const angle = random() * Math.PI * 2;
  return { ...o, el, rule, bars, cursor, tick, dir: [Math.cos(angle), Math.sin(angle)], phase: random() * 10 };
});
const introLayer = make('div', { class: 'layer' }, ...outlines.map((o) => o.el));
const pool = make('div', { class: 'glow', style: { width: W * 1.05, height: INTRO_SIZE * 5.2, left: -W * 0.025, top: H / 2 - INTRO_SIZE * 2.6,
  background: 'radial-gradient(closest-side, rgb(6 7 10 / 0.88), rgb(6 7 10 / 0.6) 55%, transparent)' } });
const flash = make('div', { class: 'glow', style: { width: W * 1.6, height: W * 1.2, left: -W * 0.3, top: H / 2 - W * 0.6,
  background: 'radial-gradient(closest-side, rgb(255 250 240 / 0.95), rgb(255 220 180 / 0.35) 40%, transparent)', opacity: 0 } });
introLayer.append(pool, introType);

// The name in a corner once the app is on screen, for anyone who scrolls on before the end card.
const mark = make('div', { style: { position: 'absolute', left: PAD, bottom: Math.round(PAD * 0.75), display: 'flex', 'align-items': 'center', gap: 10,
  'font-size': 26, 'font-weight': 740, 'letter-spacing': '-0.03em', color: '#ffffff', padding: '9px 16px 9px 13px', 'border-radius': 14,
  background: 'rgb(10 12 16 / 0.78)', 'box-shadow': '0 0 0 1px rgb(255 255 255 / 0.12), 0 8px 24px rgb(0 0 0 / 0.35)', 'backdrop-filter': 'blur(10px)' } },
  make('b', { style: { display: 'block', width: 14, height: 14, 'border-radius': 4, background: 'linear-gradient(135deg, #f7a35c, #39d5c0)',
    'box-shadow': '0 0 14px rgb(120 200 190 / 0.6)' } }), 'Paddock');
stage.append(aurora, introLayer, camera, scrim, headline, flash, mark, endLayer, make('div', { class: 'layer', id: 'vignette' }), grain);

// ------------------------------------------------------------------ camera

// Where the camera looks (a point on the page, in its pixels), how close, and how
// the window is turned. A move eases from wherever the last one had got to; `cut`
// starts from its own `start` instead, as a cut to a new picture does.
const folderMid = { x: 680, y: (box('folder', 'claude').y + box('folder', 'codex').y + box('folder', 'codex').h) / 2 };
const search = mid(box('search', 'search'));
const picked = box('picked', 'panel');
const models = box('usage', 'byModel');
// After the intro the window arrives whole, where the outlines gathered, and the
// camera then moves in on the two sessions as it does from the start of the short cut.
const ARRIVE = { x: VIEW.width / 2, y: VIEW.height / 2, z: 1.0, rx: 0, ry: 0 };
const MOVES = [
  ...(LONG ? [{ t: 0, dur: 0.01, ...ARRIVE }, { t: INTRO + 0.05, dur: SCENES.all - INTRO - 0.1, ease: inOutCubic, x: folderMid.x, y: folderMid.y, z: 1.9, rx: 5, ry: -3 }]
    : [{ t: 0, dur: 0.01, x: folderMid.x - 40, y: folderMid.y + 6, z: 1.72, rx: 16, ry: -14 },
      { t: 0, dur: SCENES.all - 0.05, ease: inOutSine, x: folderMid.x, y: folderMid.y, z: 1.9, rx: 5, ry: -3 }]),
  { t: SCENES.all, dur: 0.95, x: VIEW.width / 2, y: VIEW.height / 2 + 10, z: 1.0, rx: 0, ry: 0 },
  { t: SCENES.all + 1.25, dur: 1.4, x: mid(box('all', 'chips')).x, y: box('all', 'chips').y + 120, z: 1.32, rx: 0, ry: 0 },
  { t: SCENES.search, dur: 0.6, x: search.x + 60, y: search.y + 40, z: 2.0, rx: 0, ry: 0 },
  { t: typedAt(WORD.length - 1) + 0.22, dur: 0.7, x: VIEW.width * 0.52, y: 380, z: 1.22, rx: 0, ry: 0 },
  { t: PICK + 0.1, dur: 0.75, x: picked.x + picked.w * 0.42, y: picked.y + 230, z: 1.75, rx: 0, ry: 0 },
  { t: SCENES.usage, dur: 0.95, cut: true, start: { x: mid(models).x + 60, y: mid(models).y + 30, z: 1.7, rx: 18, ry: 16 },
    x: mid(models).x, y: mid(models).y, z: 2.3, rx: 0, ry: 0 },
  { t: SCENES.usage + 1.1, dur: RISE - SCENES.usage - 1.1, ease: inOutSine, x: mid(models).x + 10, y: mid(models).y, z: 2.42, rx: 0, ry: 0 },
  { t: RISE, dur: IMPACT - RISE, ease: inCubic, x: mid(models).x, y: mid(models).y, z: 1.5, rx: -16, ry: 0 },
];
const KEYS = ['x', 'y', 'z', 'rx', 'ry'];
// Each move's starting point: where the moves before it had got to by then.
const starts = [];
for (let i = 0; i < MOVES.length; i++) {
  const m = MOVES[i];
  starts.push(m.cut || i === 0 ? { ...(m.start || m) } : moveAt(i - 1, m.t));
}
function moveAt(i, t) {
  const m = MOVES[i];
  const p = (m.ease || spring)(progress(t, m.t, m.dur));
  return Object.fromEntries(KEYS.map((k) => [k, lerp(starts[i][k], m[k], p)]));
}
function cameraAt(t) {
  let i = 0;
  while (i + 1 < MOVES.length && MOVES[i + 1].t <= t) i++;
  return moveAt(i, t);
}

// The picture on the window at `t`.
const PICTURES = [
  [0, 'folder'], [SCENES.all, 'all'],
  ...[...WORD].map((_, n) => [typedAt(n), `search-${n + 1}`]),
  [typedAt(WORD.length - 1) + 0.2, 'search'], [PICK, 'picked'], [SCENES.usage, 'usage'],
];
const pictureAt = (t) => PICTURES.filter(([at]) => at <= t).at(-1)[1];

// The pointer's path in the search scene: [time, x, y] on the page.
const flaky = mid(box('search', 'codex'));
const resume = mid(box('picked', 'resume'));
const PATH = [
  [PICK - 0.62, VIEW.width * 0.66, 640], [PICK - 0.05, flaky.x - 120, flaky.y + 4], [PICK + 0.32, flaky.x - 120, flaky.y + 4],
  [RESUME - 0.08, resume.x + 6, resume.y + 4], [SCENES.usage, resume.x + 6, resume.y + 4],
];
function pointerAt(t) {
  if (t < PATH[0][0] || t >= SCENES.usage) return null;
  let i = 0;
  while (i + 1 < PATH.length && PATH[i + 1][0] <= t) i++;
  const [t0, x0, y0] = PATH[i];
  const [t1, x1, y1] = PATH[Math.min(i + 1, PATH.length - 1)];
  const p = t1 > t0 ? inOutCubic(progress(t, t0, t1 - t0)) : 1;
  return { x: lerp(x0, x1, p), y: lerp(y0, y1, p), shown: outCubic(progress(t, PATH[0][0], 0.2)) };
}

// ------------------------------------------------------------------ render

function backdrop(t) {
  const lift = Math.exp(-((t - IMPACT - 0.2) ** 2) / 0.5);
  const endGlow = clamp((t - RISE) / (IMPACT - RISE));
  const size = Math.max(W, H);
  css(deep, { width: size * 1.4, height: size * 1.4, left: W / 2 - size * 0.7, top: H * 0.55 - size * 0.7,
    background: 'radial-gradient(closest-side, rgb(40 46 72 / 0.55), transparent)' });
  css(warm, { width: size * 0.95, height: size * 0.75,
    left: W * (0.02 + 0.05 * Math.sin(t * 0.35)) - size * 0.2, top: H * (0.1 + 0.04 * Math.cos(t * 0.3)) - size * 0.2,
    background: `radial-gradient(closest-side, rgb(245 140 60 / ${0.34 + 0.25 * lift + 0.1 * endGlow}), transparent)` });
  css(cool, { width: size * 0.95, height: size * 0.8,
    left: W * (0.62 + 0.05 * Math.cos(t * 0.3)) - size * 0.2, top: H * (0.62 + 0.05 * Math.sin(t * 0.33)) - size * 0.2,
    background: `radial-gradient(closest-side, rgb(40 205 185 / ${0.3 + 0.25 * lift + 0.1 * endGlow}), transparent)` });
  grain.style.backgroundImage = GRAIN[Math.floor(t * 24) % GRAIN.length];
}

function drawWindow(t) {
  const c = cameraAt(t);
  // On the rise the window falls back into the dark and goes out of focus.
  const away = inCubic(progress(t, RISE, IMPACT - RISE - 0.15));
  // After the intro it comes in under the flash, where the outlines gathered.
  const arrive = LONG ? outCubic(progress(t, INTRO - 0.12, 0.22)) : 1;
  camera.style.opacity = String(arrive * (1 - away));
  camera.style.visibility = arrive > 0 ? 'visible' : 'hidden';
  scrim.style.opacity = String(arrive);
  camera.style.filter = away > 0 ? `blur(${away * 18}px)` : 'none';
  const ex = c.x * SCALE;
  const ey = TITLE + c.y * SCALE;
  camera.style.transform = `translate(${ANCHOR.x - ex * c.z}px, ${ANCHOR.y - ey * c.z}px) scale(${c.z})`;
  win.style.transformOrigin = `${ex}px ${ey}px`;
  win.style.transform = `perspective(${WIN_W * 2.2}px) rotateX(${c.rx}deg) rotateY(${c.ry}deg)`;

  const now = pictureAt(t);
  for (const [name, img] of Object.entries(images)) img.style.visibility = name === now ? 'visible' : 'hidden';

  // Rings: the Codex account, then Resume, each drawn in and held.
  const ringShow = (el, at, until) => {
    const p = progress(t, at, 0.35);
    const shown = t < until ? p : 0;
    el.style.opacity = String(shown);
    el.style.transform = `scale(${lerp(1.25, 1, outCubic(p))})`;
  };
  ringShow(codexRing, RING, SCENES.search);
  ringShow(resumeRing, RESUME, SCENES.usage);

  // Usage's bars grow in: the cover over each one slides off to the right.
  covers.forEach((cover, n) => {
    const p = outCubic(progress(t, barAt(n), 0.5));
    cover.style.visibility = now === 'usage' ? 'visible' : 'hidden';
    cover.style.transform = `scaleX(${1 - p})`;
  });

  // Labels and the pointer stay one size on screen however close the camera is.
  const at = (el, x, y, scale = 1) => { el.style.transform = `translate(${x * SCALE}px, ${TITLE + y * SCALE}px) scale(${scale / c.z})`; };
  const label = (el, row, when) => {
    const b = box('folder', row);
    const p = progress(t, when, 0.55);
    el.style.opacity = String(t < SCENES.all ? clamp(p * 3) : 0);
    // Hung on the row's top edge, its right end just short of the account column,
    // where the row and the one above have nothing to cover.
    el.style.transformOrigin = '0 0';
    at(el, b.x + b.w * 0.56, b.y + 1, lerp(0.6, 1, pop(p)));
    el.style.translate = `${-el.offsetWidth}px ${-el.offsetHeight / 2}px`;
    el.style.transformOrigin = `${el.offsetWidth}px ${el.offsetHeight / 2}px`;
  };
  label(claudePill, 'claude', LABELS[0]);
  label(codexPill, 'codex', LABELS[1]);

  const p = pointerAt(t);
  pointer.style.opacity = p ? String(p.shown) : '0';
  if (p) at(pointer, p.x, p.y);
  const click = [PICK, RESUME].find((when) => t >= when && t < when + 0.45);
  ripple.style.opacity = '0';
  if (click !== undefined && p) {
    const q = progress(t, click, 0.45);
    const r = lerp(8, 34, outCubic(q));
    ripple.style.opacity = String(0.9 * (1 - q));
    ripple.style.width = ripple.style.height = `${2 * r}px`;
    ripple.style.left = ripple.style.top = `${-r}px`;
    at(ripple, p.x, p.y);
  }
}

function drawHeadline(t) {
  for (const h of HEADLINES) {
    const shown = t >= h.from - 0.01 && t < h.to;
    h.el.style.display = shown ? 'block' : 'none';
    if (!shown) continue;
    const leaving = outCubic(progress(t, h.to - 0.24, 0.24));
    h.words.forEach((words, line) => {
      const start = h.starts[line];
      words.forEach((word, i) => {
        const p = progress(t, start + i * 0.07, 0.62);
        const e = outExpo(p);
        const y = lerp(0.55, 0, e) * h.size - leaving * 0.35 * h.size;
        const blur = lerp(14, 0, outCubic(p)) + leaving * 10;
        word.style.opacity = String(clamp(p * 2.2) * (1 - leaving));
        word.style.transform = `translateY(${y}px) scale(${lerp(1.06, 1, e)})`;
        word.style.filter = blur > 0.05 ? `blur(${blur}px)` : 'none';
      });
    });
  }
}

const scatter = [...oldName.children].map((_, i) => {
  const random = seeded(i * 7 + 3);
  return { dx: (random() - 0.5) * 260, dy: (random() - 0.5) * 180, r: (random() - 0.5) * 50, delay: random() * 0.08 };
});

function drawEnd(t) {
  const on = t >= RISE;
  endLayer.style.display = on ? 'block' : 'none';
  if (!on) return;
  // The old name comes up out of the dark as the window goes, and breaks up on the impact.
  const rise = outCubic(progress(t, RISE + 0.25, 0.6));
  [...oldName.children].forEach((el, i) => {
    const s = scatter[i];
    const p = outExpo(progress(t, IMPACT + s.delay, 0.75));
    el.style.opacity = String(rise * (1 - clamp(p * 1.4)));
    el.style.transform = `translate(${s.dx * p}px, ${s.dy * p + lerp(18, 0, rise)}px) rotate(${s.r * p}deg) scale(${lerp(1, 1.3, p)})`;
    const blur = lerp(8, 0, rise) + p * 22;
    el.style.filter = blur > 0.05 ? `blur(${blur}px)` : 'none';
  });
  // The new one forms from the middle out, letter by letter.
  const count = newName.children.length;
  [...newName.children].forEach((el, i) => {
    const order = Math.abs(i - (count - 1) / 2);
    const p = progress(t, IMPACT + 0.04 + order * 0.045, 0.8);
    const e = spring(p);
    el.style.opacity = String(clamp(p * 2.5));
    el.style.transform = `translateY(${lerp(42, 0, e)}px) scale(${lerp(1.35, 1, e)})`;
    const blur = lerp(26, 0, outCubic(p));
    el.style.filter = blur > 0.05 ? `blur(${blur}px)` : 'none';
  });
  // A light runs across it once it has formed: one gradient laid across the whole
  // name, each letter showing its own part of it.
  // The gradient is three names wide with the light in its middle, so every letter
  // is covered wherever the light is.
  const w = newName.offsetWidth;
  const light = lerp(-0.35, 1.35, inOutCubic(progress(t, IMPACT + 0.55, 1.0))) * w;
  for (const el of newName.children) {
    el.style.backgroundSize = `${w * 3}px 100%`;
    el.style.backgroundPosition = `${light - el.offsetLeft - w * 1.5}px 0`;
  }
  const fade = (el, at) => {
    const p = progress(t, at, 0.7);
    el.style.opacity = String(outCubic(p));
    el.style.transform = `translateY(${lerp(16, 0, outExpo(p))}px)`;
    const blur = lerp(10, 0, outCubic(p));
    el.style.filter = blur > 0.05 ? `blur(${blur}px)` : 'none';
  };
  fade(sub, IMPACT + 0.55);
  fade(small, IMPACT + 0.8);
  fade(url, IMPACT + 1.05);
  // The flash of the impact, settling into a glow behind the name.
  const flash = Math.exp(-Math.max(0, t - IMPACT) / 0.35) * (t >= IMPACT ? 1 : 0);
  bloom.style.opacity = String(clamp(0.25 * outCubic(progress(t, IMPACT, 0.6)) + 0.75 * flash));
  bloom.style.transform = `scale(${lerp(0.7, 1.05, outCubic(progress(t, IMPACT, 1.2)))})`;
}

// Where the window is on screen when it arrives: the grid the outlines gather into.
function arrivalRect() {
  const ex = ARRIVE.x * SCALE;
  const ey = TITLE + ARRIVE.y * SCALE;
  return { x: ANCHOR.x - ex * ARRIVE.z, y: ANCHOR.y - ey * ARRIVE.z, w: WIN_W * ARRIVE.z, h: WIN_H * ARRIVE.z };
}
const GRID = { cols: 6, rows: 4, gap: 12 };
// Each outline's tile: outlines sorted top to bottom into rows, each row left to right,
// so every one travels to a tile near where it was and the paths do not cross much.
const tiles = (() => {
  const order = outlines.map((o, i) => i).sort((a, b) => outlines[a].y - outlines[b].y);
  const slot = new Array(outlines.length);
  for (let r = 0; r < GRID.rows; r++) {
    const row = order.slice(r * GRID.cols, (r + 1) * GRID.cols).sort((a, b) => outlines[a].x - outlines[b].x);
    row.forEach((i, c) => { slot[i] = { col: c, row: r }; });
  }
  return slot;
})();

function drawIntro(t) {
  const on = LONG && t < INTRO + 0.35;
  introLayer.style.display = on ? 'block' : 'none';
  const f = LONG ? Math.exp(-Math.abs(t - INTRO) / (t < INTRO ? 0.06 : 0.3)) : 0;
  flash.style.opacity = String(on || t < INTRO + 1.5 ? clamp(f * 0.85) : 0);
  if (!on) return;
  const rect = arrivalRect();
  const tw = (rect.w - GRID.gap * (GRID.cols + 1)) / GRID.cols;
  const th = (rect.h - GRID.gap * (GRID.rows + 1)) / GRID.rows;
  const fill = inCubic(progress(t, INTRO - 0.4, 0.4));
  const gone = outCubic(progress(t, INTRO - 0.05, 0.3));
  pool.style.opacity = String(1 - outCubic(progress(t, GATHER - 0.4, 0.5)));
  outlines.forEach((o, n) => {
    const d = DEPTH[o.depth];
    const shown = clamp(progress(t, o.at, 0.25) * 1.4);
    const popIn = pop(progress(t, o.at, 0.45));
    // Scattered: drifting, nearer ones faster, and flickering a little.
    const drift = d.drift * t;
    const sx = o.x * W + o.dir[0] * drift;
    const sy = o.y * H + o.dir[1] * drift;
    const sw = BASE * o.size * d.scale;
    const sh = sw * 0.66;
    // Gathered: each into its tile, the middle ones first.
    const tile = tiles[n];
    const tx = rect.x + GRID.gap + tile.col * (tw + GRID.gap) + tw / 2;
    const ty = rect.y + GRID.gap + tile.row * (th + GRID.gap) + th / 2;
    const far = Math.hypot(tx - W / 2, ty - H / 2) / Math.hypot(W / 2, H / 2);
    const g = spring(progress(t, GATHER + far * 0.35, 0.85));
    const x = lerp(sx, tx, g);
    const y = lerp(sy, ty, g);
    const w = lerp(sw, tw, g);
    const h = lerp(sh, th, g);
    const flicker = 0.88 + 0.12 * Math.sin(t * 17 + o.phase) * Math.sin(t * 5.3 + o.phase * 2);
    // Its state's colour, once the type has said it; done ones also dim a little and
    // get a tick, waiting ones pulse on the beat.
    const said = o.state === 'done' ? progress(t, SAID[1] + 0.1, 0.35) : o.state === 'waiting' ? progress(t, SAID[2] + 0.1, 0.35) : 1;
    const hue = STATE[o.state].map((c, k) => lerp(STATE.neutral[k], c, said));
    const [r, gg, b] = hue.map((c) => Math.round(lerp(c, 236, g * 0.7)));
    const color = (a) => `rgb(${r} ${gg} ${b} / ${a})`;
    const pulse = o.state === 'waiting' ? said * (0.5 + 0.5 * Math.cos((2 * Math.PI * t) / BEAT)) * (1 - g) : 0;
    const dim = o.state === 'done' ? lerp(1, 0.72, said * (1 - g)) : o.state === 'idle' ? 0.8 : 1;
    o.tick.style.opacity = String(o.state === 'done' ? outCubic(said) * (1 - g) : 0);
    o.tick.style.transform = `scale(${lerp(0.4, 1, pop(said))})`;
    const el = o.el;
    el.style.width = `${w}px`;
    el.style.height = `${h}px`;
    el.style.padding = `0 ${Math.max(4, w * 0.07)}px`;
    el.style.transform = `translate(${x - w / 2}px, ${y - h / 2}px) scale(${lerp(0.6, 1, popIn)})`;
    el.style.opacity = String(shown * lerp(d.alpha * flicker * dim, 1, g) * (1 - gone));
    const blur = lerp(d.blur, 0, g) + (1 - clamp(popIn)) * 4;
    el.style.filter = `${blur > 0.05 ? `blur(${blur}px) ` : ''}brightness(${lerp(1.9, 1, outCubic(progress(t, o.at, 0.4))) + fill * 0.8})`;
    el.style.borderColor = color(0.9);
    el.style.background = color(lerp(0.06, 0.85, fill));
    el.style.boxShadow = `0 0 ${18 + fill * 30 + pulse * 26}px ${color(0.35 + fill * 0.4 + pulse * 0.4)}, inset 0 0 14px ${color(0.15 + pulse * 0.2)}`;
    o.rule.style.borderColor = color(0.5);
    o.rule.style.marginTop = `${h * 0.2}px`;
    for (const bar of o.bars) bar.style.background = color(0.45);
    o.cursor.style.background = color(Math.floor(t / BEAT + o.phase) % 2 ? 0.95 : 0.1);
  });
}

function drawMark(t) {
  const shown = outCubic(progress(t, SCENES.both + 0.6, 0.6)) * (1 - outCubic(progress(t, RISE - 0.1, 0.4)));
  mark.style.opacity = String(0.9 * shown);
}

window.render = (t) => {
  t = clamp(t, 0, LENGTH);
  backdrop(t);
  drawMark(t);
  drawIntro(t);
  drawWindow(t);
  drawHeadline(t);
  drawEnd(t);
};

await document.fonts.load(`720 ${HEAD.size}px Inter`);
await document.fonts.load('500 20px "JetBrains Mono"');
await Promise.all(Object.values(images).map((img) => img.decode()));
window.render(0);
window.ready = true;
