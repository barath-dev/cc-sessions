// Records the README demo: docs/demo.mp4 (a tour of the page) and docs/demo.png.
//
// It builds a made-up home folder with three accounts, starts cc-sessions on it
// with the tests' fake `claude`, drives the page in headless Chrome, and encodes
// what Chrome drew with ffmpeg. The real install is never touched: the tool runs
// with its own HOME, CC_SESSIONS_HOME and port.
//
//   cargo build --release
//   node docs/record-demo/record.mjs [--keep]
//
// --keep leaves the demo home and the captured frames in place for a look.
// CHROME and FFMPEG name the two programs if they are not in the usual place.
// DEMO_WORK_DIR is where the frames go while recording (about 500 MB; default
// the system's temporary folder).

import { spawn, spawnSync } from 'node:child_process';
import { once } from 'node:events';
import { createHash } from 'node:crypto';
import {
  copyFileSync, chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, utimesSync, writeFileSync,
} from 'node:fs';
import { writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const REPO = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
const EXE = path.join(REPO, 'target', 'release', 'cc-sessions');
const FAKE_CLAUDE = path.join(REPO, 'target', 'release', 'fake-claude');
const CHROME = process.env.CHROME || '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const FFMPEG = process.env.FFMPEG || 'ffmpeg';
const KEEP = process.argv.includes('--keep');

// Short on purpose: the resume command in a session's details prints this path.
const HOME = '/tmp/alex';
const MARKER = path.join(HOME, '.cc-sessions-demo');
const STATE = path.join(HOME, '.config', 'cc-sessions');
const BIN = path.join(HOME, 'bin');
const PORT = 47999;

// The page is laid out at 1280x800 and drawn at twice that, then scaled to the
// video size, which keeps small text sharp.
const VIEW = { width: 1280, height: 800 };
const SCALE = 2;
const VIDEO = { width: 1600, height: 1000, fps: 30, crf: 20 };

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const log = (...words) => console.log(...words);

// ------------------------------------------------------------------ demo data

const MIN = 60 * 1000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;

// Ids come from the name, so every run produces the same ones.
function uuid(seed) {
  const hex = createHash('sha1').update(seed).digest('hex');
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-4${hex.slice(13, 16)}-a${hex.slice(17, 20)}-${hex.slice(20, 32)}`;
}

const ACCOUNTS = {
  work: { loggedIn: true, email: 'alex@northwind.example', orgName: 'Northwind', subscriptionType: 'team' },
  personal: { loggedIn: true, email: 'alex@mailbox.example', subscriptionType: 'max' },
  client: { loggedIn: true, email: 'alex@harborlight.example', orgName: 'Harborlight Studio', subscriptionType: 'enterprise' },
};

const FOLDERS = {
  storefront: `${HOME}/code/storefront`,
  gateway: `${HOME}/code/api-gateway`,
  design: `${HOME}/code/design-system`,
  notes: `${HOME}/notes`,
};

// Times are "how long ago". Interactive sessions get a stand-in process each.
const SESSIONS = [
  { account: 'work', folder: 'storefront', kind: 'background', state: 'working', name: 'Migrate checkout to the new payments API',
    started: 38 * MIN, updated: 15 * 1000, detail: 'Updating the checkout form to the new client: 14 of 22 tests pass',
    timeline: [
      [36 * MIN, 'Reading the current checkout flow and its tests'],
      [21 * MIN, 'Replaced the payment client and its error handling'],
      [9 * MIN, 'Running the checkout tests: 9 of 22 pass'],
      [3 * MIN, 'Fixing the address validation step'],
      [15 * 1000, 'Updating the checkout form to the new client: 14 of 22 tests pass'],
    ] },
  { account: 'work', folder: 'storefront', kind: 'interactive', status: 'idle', name: 'Fix cart badge count on mobile',
    started: 2 * HOUR, updated: 6 * MIN },
  { account: 'client', folder: 'storefront', kind: 'background', state: 'blocked', name: 'Add product image zoom',
    started: 64 * MIN, updated: 12 * MIN, detail: 'Waiting for you: approve adding the image-zoom package',
    timeline: [
      [58 * MIN, 'Comparing three zoom libraries for size and touch support'],
      [12 * MIN, 'Waiting for you: approve adding the image-zoom package'],
    ] },
  { account: 'personal', folder: 'storefront', kind: 'background', state: 'done', name: 'Write tests for the discount codes',
    started: 95 * MIN, updated: 47 * MIN, detail: 'Done: 18 tests added, all passing',
    timeline: [[80 * MIN, 'Listing the discount rules to cover'], [47 * MIN, 'Done: 18 tests added, all passing']] },
  { account: 'work', folder: 'storefront', kind: 'background', state: 'stopped', name: 'Upgrade to React 19',
    started: 27 * HOUR, updated: 26 * HOUR, detail: 'Stopped after updating the dependencies',
    timeline: [[26 * HOUR, 'Stopped after updating the dependencies']] },
  { account: 'work', folder: 'gateway', kind: 'interactive', status: 'busy', name: 'Rate limiting for the public API',
    started: 25 * MIN, updated: 1 * MIN },
  { account: 'client', folder: 'gateway', kind: 'background', state: 'blocked', name: 'Rotate the staging API keys',
    started: 22 * MIN, updated: 4 * MIN, detail: 'Waiting for you: confirm which keys to revoke',
    timeline: [[20 * MIN, 'Found 6 keys in use on staging'], [4 * MIN, 'Waiting for you: confirm which keys to revoke']] },
  { account: 'work', folder: 'gateway', kind: 'background', state: 'done', name: 'Document the orders endpoints',
    started: 4 * HOUR, updated: 3 * HOUR, detail: 'Done: docs/orders.md covers all 9 endpoints',
    timeline: [[3 * HOUR, 'Done: docs/orders.md covers all 9 endpoints']] },
  { account: 'personal', folder: 'notes', kind: 'background', state: 'working', name: "Summarise this week's reading list",
    started: 11 * MIN, updated: 2 * MIN, detail: 'Summarising article 5 of 8',
    timeline: [[10 * MIN, 'Collecting the 8 saved articles'], [2 * MIN, 'Summarising article 5 of 8']] },
  { account: 'personal', folder: 'notes', kind: 'interactive', status: 'idle', name: 'Plan the weekend hiking trip',
    started: 70 * MIN, updated: 31 * MIN },
];

// Saved conversations: [account, folder, title, last activity ago, prompts].
// A title that matches a session above is that session's conversation.
const HISTORY = [
  ['work', 'storefront', 'Migrate checkout to the new payments API', 1 * MIN, [
    'Move the checkout flow from the old payments client to the new payments API. Keep the tests passing as you go.',
    'Keep the old client behind a feature flag until we have shipped']],
  ['work', 'gateway', 'Rate limiting for the public API', 1 * MIN, [
    'Add per-key rate limiting to the public API, 600 requests a minute by default.',
    'Return a Retry-After header when a key is over its limit',
    'Where should the limits be configured?']],
  ['work', 'storefront', 'Fix cart badge count on mobile', 6 * MIN, [
    'The cart badge in the header shows the wrong count on mobile after an item is removed. Find out why and fix it.',
    'Does the same bug affect the mini-cart drawer?',
    'Add a test that removes an item and checks the badge',
    'Run the mobile viewport tests again and summarise the result']],
  ['personal', 'notes', 'Plan the weekend hiking trip', 31 * MIN, [
    'Help me plan a two-day hike for four people, with a packing list.',
    'Shorten day two to under 12 km']],
  ['work', 'storefront', 'Upgrade to React 19', 26 * HOUR, [
    'Upgrade the storefront to React 19 and list what breaks.',
    'Start with the dependencies only']],
  ['work', 'storefront', 'Cart totals rounding bug', 2 * DAY, [
    'Cart totals are off by a cent when a percentage discount is applied. Find the rounding error.',
    'Use integer cents everywhere in the totals code',
    'Add test cases for 10%, 15% and 33% discounts']],
  ['work', 'gateway', 'Trace slow requests on the orders endpoint', 3 * DAY, [
    'GET /orders takes over two seconds for large accounts. Find out where the time goes.',
    'Add an index for the query you found and measure again']],
  ['client', 'storefront', 'Speed up the product listing page', 4 * DAY, [
    'The product listing page is slow to load on phones. Measure it and suggest the three biggest wins.',
    'Lazy-load the images below the fold',
    'What is the bundle size now?']],
  ['personal', 'notes', 'Draft a blog post about container queries', 5 * DAY, [
    'Draft a short blog post explaining CSS container queries with one worked example.',
    'Make the intro shorter and add a browser support note']],
  ['work', 'design', 'Dark mode tokens for buttons and inputs', 6 * DAY, [
    'Add dark mode colour tokens for buttons and inputs, and check their contrast.',
    'Show me the contrast ratios as a table']],
  ['client', 'gateway', 'Webhook retries with exponential backoff', 8 * DAY, [
    'Failed webhooks are retried straight away and flood the receiver. Add exponential backoff with jitter.',
    'Cap it at six attempts and log the final failure']],
  ['client', 'storefront', 'Abandoned cart reminder email', 9 * DAY, [
    'Write the template for the abandoned cart reminder email, with the cart items and a link back.',
    'Make it readable with images turned off']],
  ['client', 'design', 'Accessible date picker component', 12 * DAY, [
    'Build a date picker that works with the keyboard and a screen reader.',
    'Add arrow-key navigation between days',
    'Write the usage notes for the docs site']],
  ['personal', 'notes', 'Reading notes on performance budgets', 15 * DAY, [
    'Turn my rough notes on web performance budgets into a tidy summary.']],
  ['personal', 'design', 'Try a fluid type scale', 20 * DAY, [
    'Show me a fluid type scale using clamp() for headings and body text.',
    'Use a 1.2 ratio on small screens and 1.25 on large ones']],
];

function writeJson(file, value) {
  mkdirSync(path.dirname(file), { recursive: true });
  writeFileSync(file, JSON.stringify(value, null, 2));
}

function resetHome() {
  // Only a folder this script made is ever removed.
  if (existsSync(HOME)) {
    if (!existsSync(MARKER)) throw new Error(`${HOME} exists and was not made by this script. Move it away first.`);
    rmSync(HOME, { recursive: true, force: true });
  }
  mkdirSync(HOME, { recursive: true });
  writeFileSync(MARKER, 'Made by docs/record-demo/record.mjs. Safe to delete.\n');
}

function removeHome() {
  if (existsSync(MARKER)) rmSync(HOME, { recursive: true, force: true });
}

// A process's start time, in the form Claude Code records it. Closing checks it against `ps`.
function procStart(pid) {
  const out = spawnSync('ps', ['-o', 'lstart=', '-p', String(pid)], { env: { LC_ALL: 'C', TZ: 'UTC', PATH: '/usr/bin:/bin' } });
  return String(out.stdout).trim().replace(/\s+/g, ' ');
}

// The processes that play the interactive sessions, ended when the script finishes.
const standIns = [];

function buildDemo() {
  const now = Date.now();
  const iso = (ago) => new Date(now - ago).toISOString();

  mkdirSync(BIN, { recursive: true });
  copyFileSync(FAKE_CLAUDE, path.join(BIN, 'claude'));
  chmodSync(path.join(BIN, 'claude'), 0o755);
  for (const folder of Object.values(FOLDERS)) mkdirSync(folder, { recursive: true });

  for (const [name, auth] of Object.entries(ACCOUNTS)) {
    const dir = path.join(HOME, `.claude-${name}`);
    mkdirSync(path.join(dir, 'projects'), { recursive: true });
    writeJson(path.join(dir, 'fake-auth.json'), auth);
    // One account keeps history longer, as a real settings.json might.
    if (name === 'client') writeJson(path.join(dir, 'settings.json'), { cleanupPeriodDays: 90 });

    const agents = [];
    for (const s of SESSIONS.filter((s) => s.account === name)) {
      const sessionId = uuid(s.name);
      const cwd = FOLDERS[s.folder];
      const startedAt = now - s.started;
      if (s.kind === 'interactive') {
        // `fake-claude sleep` lives for two minutes, so these start right before the recording.
        const child = spawn(path.join(BIN, 'claude'), ['sleep'], { stdio: 'ignore', env: { PATH: '/usr/bin:/bin' } });
        standIns.push(child);
        const record = { pid: child.pid, sessionId, cwd, startedAt, procStart: procStart(child.pid), kind: 'interactive',
          name: s.name, status: s.status, updatedAt: now - s.updated, statusUpdatedAt: now - s.updated };
        writeJson(path.join(dir, 'sessions', `${child.pid}.json`), record);
        agents.push({ pid: child.pid, kind: 'interactive', status: s.status, name: s.name, cwd, sessionId, startedAt });
      } else {
        const id = sessionId.slice(0, 8);
        const job = path.join(dir, 'jobs', id);
        writeJson(path.join(job, 'state.json'), { detail: s.detail, updatedAt: iso(s.updated) });
        writeFileSync(path.join(job, 'timeline.jsonl'), s.timeline
          .map(([ago, detail]) => JSON.stringify({ at: iso(ago), state: 'working', detail })).join('\n') + '\n');
        agents.push({ id, kind: 'background', state: s.state, name: s.name, cwd, sessionId, startedAt });
      }
    }
    writeJson(path.join(dir, 'fake-agents.json'), agents);
  }

  for (const [account, folder, title, ago, prompts] of HISTORY) {
    const cwd = FOLDERS[folder];
    const sessionId = uuid(title);
    const started = ago + (prompts.length * 9 + 4) * MIN;
    const lines = prompts.map((content, i) => ({
      type: 'user', cwd, sessionId, timestamp: iso(started - i * 9 * MIN), message: { role: 'user', content },
    }));
    lines.splice(1, 0, { type: 'ai-title', aiTitle: title, sessionId });
    const file = path.join(HOME, `.claude-${account}`, 'projects', cwd.replaceAll('/', '-'), `${sessionId}.jsonl`);
    mkdirSync(path.dirname(file), { recursive: true });
    writeFileSync(file, lines.map((line) => JSON.stringify(line)).join('\n') + '\n');
    // The page shows the file's modification time as the conversation's date.
    utimesSync(file, new Date(now - ago), new Date(now - ago));
  }
}

// ----------------------------------------------------------------- the tool

// A clean environment: nothing from this shell (or from a Claude Code session
// this may be run inside, which sets CLAUDE_* variables) reaches the tool.
const TOOL_ENV = {
  HOME, CC_SESSIONS_HOME: STATE, CC_SESSIONS_PORT: String(PORT), SHELL: '/bin/zsh', PATH: `${BIN}:/usr/bin:/bin`,
};

function tool(...args) {
  const out = spawnSync(EXE, args, { env: TOOL_ENV, cwd: HOME, encoding: 'utf8' });
  if (out.status !== 0) throw new Error(`cc-sessions ${args.join(' ')} failed: ${out.stderr || out.stdout}`);
  return out.stdout.trim();
}

function stopTool() {
  if (!existsSync(path.join(STATE, 'ui-token'))) return;
  try { tool('stop'); } catch (err) { console.error(String(err.message)); }
}

// -------------------------------------------------------------------- Chrome

// A small Chrome DevTools Protocol client over Node's built-in WebSocket.
async function openChrome(profile) {
  const chrome = spawn(CHROME, [
    '--headless=new', '--remote-debugging-port=0', `--user-data-dir=${profile}`, '--no-first-run',
    '--no-default-browser-check', '--hide-scrollbars', '--lang=en-US', '--force-color-profile=srgb',
    // Screencast frames follow the window's own scale factor, not the emulated one set later.
    `--force-device-scale-factor=${SCALE}`, `--window-size=${VIEW.width},${VIEW.height}`, 'about:blank',
  ], { stdio: 'ignore' });

  // Port 0 lets Chrome pick a free one; it writes the choice into the profile.
  let debugPort = 0;
  for (let i = 0; i < 100 && !debugPort; i++) {
    await sleep(100);
    try { debugPort = Number(readFileSync(path.join(profile, 'DevToolsActivePort'), 'utf8').split('\n')[0]); } catch { /* not up yet */ }
  }
  if (!debugPort) { chrome.kill(); throw new Error('Chrome did not start.'); }
  let target;
  for (let i = 0; i < 50 && !target; i++) {
    const list = await (await fetch(`http://127.0.0.1:${debugPort}/json`)).json();
    target = list.find((t) => t.type === 'page');
    if (!target) await sleep(100);
  }
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { ws.addEventListener('open', resolve); ws.addEventListener('error', reject); });

  let nextId = 1;
  const pending = new Map();
  const handlers = new Map();
  const problems = [];
  ws.addEventListener('message', (event) => {
    const msg = JSON.parse(event.data);
    if (msg.id && pending.has(msg.id)) {
      const { resolve, reject } = pending.get(msg.id);
      pending.delete(msg.id);
      if (msg.error) reject(new Error(msg.error.message)); else resolve(msg.result);
    } else if (msg.method === 'Runtime.exceptionThrown') {
      problems.push(msg.params.exceptionDetails.exception?.description || msg.params.exceptionDetails.text);
    } else if (handlers.has(msg.method)) {
      handlers.get(msg.method)(msg.params);
    }
  });
  const send = (method, params = {}) => new Promise((resolve, reject) => {
    const id = nextId++;
    pending.set(id, { resolve, reject });
    ws.send(JSON.stringify({ id, method, params }));
  });
  async function evaluate(expression) {
    const r = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
    return r.result.value;
  }
  await send('Page.enable');
  await send('Runtime.enable');
  return {
    send, evaluate, problems,
    on: (method, handler) => handlers.set(method, handler),
    // Asked to quit and waited for, so it is done writing its profile before that is removed.
    async close() {
      const gone = chrome.exitCode === null ? once(chrome, 'exit') : Promise.resolve();
      send('Browser.close').catch(() => {});
      const timer = setTimeout(() => chrome.kill('SIGKILL'), 5000);
      await gone;
      clearTimeout(timer);
    },
  };
}

// ------------------------------------------------------- pointer and captions

// Runs in the page. The page's Content-Security-Policy blocks <style> tags and
// style attributes, so everything is built with DOM calls and el.style.
// The overlay is a popover: that puts it in the top layer, where it can be
// lifted above the page's modal dialogs.
function installOverlay() {
  const css = (el, rules) => { for (const [name, value] of Object.entries(rules)) el.style.setProperty(name, value); return el; };
  const SVG = 'http://www.w3.org/2000/svg';

  const layer = css(document.createElement('div'), {
    position: 'fixed', inset: '0', width: '100vw', height: '100vh', margin: '0', padding: '0', border: '0',
    background: 'transparent', overflow: 'hidden', 'pointer-events': 'none',
  });
  layer.setAttribute('popover', 'manual');

  const ring = css(document.createElement('div'), {
    position: 'absolute', left: '-22px', top: '-22px', width: '44px', height: '44px', 'border-radius': '50%',
    background: 'rgb(47 95 208 / 0.35)', border: '2px solid rgb(47 95 208 / 0.8)', opacity: '0',
  });

  const pointer = css(document.createElementNS(SVG, 'svg'), {
    position: 'absolute', left: '0', top: '0', overflow: 'visible', filter: 'drop-shadow(0 2px 3px rgb(0 0 0 / 0.35))',
  });
  pointer.setAttribute('width', '26');
  pointer.setAttribute('height', '34');
  pointer.setAttribute('viewBox', '0 0 26 34');
  const arrow = document.createElementNS(SVG, 'path');
  arrow.setAttribute('d', 'M2 2 L2 26.5 L8.2 20.8 L12.6 30.6 L16.6 28.8 L12.3 19.2 L20.6 19.2 Z');
  arrow.setAttribute('fill', '#111820');
  arrow.setAttribute('stroke', '#ffffff');
  arrow.setAttribute('stroke-width', '1.8');
  arrow.setAttribute('stroke-linejoin', 'round');
  pointer.append(arrow);

  const caption = css(document.createElement('div'), {
    position: 'absolute', left: '50%', bottom: '26px', transform: 'translateX(-50%)', padding: '9px 18px',
    'border-radius': '10px', background: 'rgb(18 26 36 / 0.94)', border: '1px solid rgb(255 255 255 / 0.22)',
    color: '#ffffff', font: '600 16px/1.35 "Avenir Next", "Segoe UI", system-ui, sans-serif', 'white-space': 'nowrap',
    'box-shadow': '0 6px 20px rgb(10 20 35 / 0.3)', opacity: '0', transition: 'opacity 180ms ease',
  });

  layer.append(caption, ring, pointer);
  document.body.append(layer);
  layer.showPopover();

  let raisedFor = null;
  const place = (el, x, y, extra = '') => { el.style.transform = `translate(${x}px, ${y}px) ${extra}`; };
  globalThis.__demo = {
    // The arrow's tip sits 2px inside its box.
    point(x, y, ms) {
      pointer.style.transition = ms ? `transform ${ms}ms cubic-bezier(0.45, 0, 0.25, 1)` : 'none';
      place(pointer, x - 2, y - 2);
    },
    press(x, y) {
      ring.style.transition = 'none';
      ring.style.opacity = '0.9';
      place(ring, x, y, 'scale(0.3)');
      ring.getBoundingClientRect();   // settle the start state so the change below animates
      ring.style.transition = 'transform 420ms ease-out, opacity 420ms ease-out';
      ring.style.opacity = '0';
      place(ring, x, y, 'scale(1.15)');
    },
    caption(text) {
      caption.style.opacity = '0';
      if (!text) return;
      setTimeout(() => { caption.textContent = text; caption.style.opacity = '1'; }, caption.textContent ? 180 : 0);
    },
    // A modal dialog opened after the overlay would cover it; showing the overlay again puts it back on top.
    raise() {
      const dialog = document.querySelector('dialog[open]');
      const current = dialog && dialog.firstElementChild;
      if (current && current !== raisedFor) { layer.hidePopover(); layer.showPopover(); }
      raisedFor = current;
    },
    // Where to aim in the first `selector` element containing `text` (or in `inside`, a selector within it).
    box(selector, text, inside) {
      let el = [...document.querySelectorAll(selector)].find((e) => !text || e.textContent.includes(text));
      if (el && inside) el = el.querySelector(inside);
      if (!el) return null;
      el.scrollIntoView({ block: 'nearest' });
      const r = el.getBoundingClientRect();
      return { x: r.x, y: r.y, w: r.width, h: r.height };
    },
  };
}

// The moves a person would make, drawn by the overlay and sent as real mouse
// events too, so hover and focus states show up.
function actor(page) {
  // Starts below the list, clear of any row.
  let at = { x: 1010, y: 748 };
  const js = (strings, ...values) => page.evaluate(String.raw({ raw: strings }, ...values.map((v) => JSON.stringify(v))));
  const mouse = (type, extra = {}) => page.send('Input.dispatchMouseEvent', { type, x: at.x, y: at.y, ...extra });

  const me = {
    hold: sleep,
    caption: (text) => js`__demo.caption(${text})`,
    async start() {
      await page.evaluate(`(${installOverlay})()`);
      await js`__demo.point(${at.x}, ${at.y}, 0)`;
      await mouse('mouseMoved');
    },
    async waitFor(expression, what) {
      for (let i = 0; i < 100; i++) {
        if (await page.evaluate(expression)) return;
        await sleep(100);
      }
      throw new Error(`Timed out waiting for ${what}.`);
    },
    // Glide to a spot in an element. `fx` and `fy` say where in it, as fractions; `dx` is a fixed offset from its left edge instead.
    async moveTo(selector, { text, inside, fx = 0.5, fy = 0.5, dx } = {}) {
      const box = await js`__demo.box(${selector}, ${text || ''}, ${inside || ''})`;
      if (!box) throw new Error(`Nothing on the page for ${selector} ${text || ''} ${inside || ''}`);
      const to = { x: Math.round(box.x + (dx ?? box.w * fx)), y: Math.round(box.y + box.h * fy) };
      const from = at;
      const ms = Math.round(Math.min(640, Math.max(300, 200 + Math.hypot(to.x - from.x, to.y - from.y) * 0.6)));
      await js`__demo.point(${to.x}, ${to.y}, ${ms})`;
      const began = Date.now();
      const steps = Math.ceil(ms / 40);
      for (let i = 1; i <= steps; i++) {
        const t = i / steps;
        const eased = t < 0.5 ? 4 * t * t * t : 1 - ((-2 * t + 2) ** 3) / 2;
        at = { x: from.x + (to.x - from.x) * eased, y: from.y + (to.y - from.y) * eased };
        await mouse('mouseMoved');
        await sleep(Math.max(0, began + ms * t - Date.now()));
      }
      at = to;
      await sleep(90);
    },
    async click(selector, where = {}) {
      await me.moveTo(selector, where);
      await js`__demo.press(${at.x}, ${at.y})`;
      await mouse('mousePressed', { button: 'left', clickCount: 1 });
      await sleep(70);
      await mouse('mouseReleased', { button: 'left', clickCount: 1 });
      await js`__demo.raise()`;
      await sleep(140);
    },
    // A native <select> menu is not drawn in headless Chrome, so point at it and show the result of choosing.
    async choose(selector, value) {
      await me.moveTo(selector);
      await js`__demo.press(${at.x}, ${at.y})`;
      await sleep(350);
      await js`(() => { const el = document.querySelector(${selector}); el.value = ${value}; el.dispatchEvent(new Event('change', { bubbles: true })); })()`;
    },
    async type(text, perKey = 85) {
      for (const char of text) {
        await page.send('Input.insertText', { text: char });
        await sleep(perKey);
      }
    },
    async erase(selector) {
      const length = await js`document.querySelector(${selector}).value.length`;
      for (let n = length - 1; n >= 0; n--) {
        await js`(() => { const el = document.querySelector(${selector}); el.value = el.value.slice(0, ${n}); el.dispatchEvent(new Event('input', { bubbles: true })); })()`;
        await sleep(45);
      }
    },
  };
  return me;
}

// --------------------------------------------------------------------- tour

const CLOSE_ME = 'Fix cart badge count on mobile';
const LOOK_AT = 'Migrate checkout to the new payments API';

async function tour(a, mark) {
  mark('1 sessions');
  await a.caption('Claude Code sessions from every account, in one place.');
  await a.hold(3000);

  mark('2 filter and search');
  await a.caption('Filter by status, or search by folder or name.');
  await a.click('#chips .chip', { text: 'Blocked' });
  await a.hold(1000);
  await a.click('#chips .chip', { text: 'Open' });
  await a.hold(300);
  await a.click('.search', { dx: 150 });
  await a.type('store');
  await a.hold(1000);
  await a.erase('.search');
  await a.hold(300);

  mark('3 details');
  await a.caption('Open a session to see what it is doing.');
  await a.click('.row', { text: LOOK_AT, inside: '.row-name', dx: 150 });
  await a.hold(2500);
  await a.click('.row', { text: LOOK_AT, inside: '.row-name', dx: 150 });

  mark('4 close');
  await a.caption('Close a session. Its conversation is kept.');
  await a.click('.row', { text: CLOSE_ME, inside: '.row-actions .btn' });
  await a.hold(1100);
  await a.click('#dialog .btn-danger');
  await a.waitFor(`document.querySelectorAll('#toasts .toast').length > 0`, 'the Closed toast');
  await a.hold(700);
  await a.click('#chips .chip', { text: 'Closed' });
  await a.caption('Closed sessions can be resumed later.');
  // Only pointed at: Resume would open a real terminal window.
  await a.moveTo('.row', { text: CLOSE_ME, inside: '.row-actions .btn' });
  await a.hold(1100);
  await a.click('#chips .chip', { text: 'Open' });

  mark('5 history');
  await a.caption('History keeps every saved conversation.');
  await a.click('.tab', { text: 'History' });
  await a.waitFor(`document.querySelectorAll('.row-history').length > 0`, 'the history list');
  await a.hold(900);
  await a.click('.search', { dx: 150 });
  await a.type('cart');
  await a.hold(500);
  await a.click('.row', { text: CLOSE_ME, inside: '.row-name', dx: 150 });
  await a.hold(2100);

  mark('6 accounts');
  await a.caption('Each account, with its login and plan.');
  await a.click('.tab', { text: 'Accounts' });
  await a.hold(1200);
  await a.caption('Add another account.');
  await a.click('.toolbar .btn-primary');
  await a.hold(250);
  await a.type('side-project', 70);
  await a.hold(500);
  await a.click('#dialog .btn-primary');
  await a.waitFor(`document.querySelectorAll('.account').length === 4`, 'the new account card');
  // Beside "Not logged in", clear of the text and of the Log in button below it.
  await a.moveTo('.account', { text: 'side-project', inside: '.warn', fx: 1.65, fy: 0.55 });
  await a.hold(1400);

  mark('7 dark theme');
  await a.caption('Light or dark.');
  await a.choose('#theme', 'dark');
  await a.hold(700);
  await a.click('.tab', { text: 'Sessions' });
  await a.hold(1200);

  mark('8 disconnect');
  await a.caption('Disconnect when you are done. Your sessions keep running.');
  await a.click('#disconnect');
  await a.hold(1000);
  await a.click('#dialog .btn-primary');
  await a.waitFor(`!document.querySelector('#offline').hidden`, 'the Disconnected screen');
  await a.hold(2100);
}

// ------------------------------------------------------------ capture, encode

// Chrome sends a frame whenever the page changes, with the time it was drawn.
// Returns the frames of the tour, each with its time in seconds from the start.
async function record(page, framesDir, run) {
  mkdirSync(framesDir, { recursive: true });
  const frames = [];
  const writes = [];
  page.on('Page.screencastFrame', ({ data, metadata, sessionId }) => {
    page.send('Page.screencastFrameAck', { sessionId }).catch(() => {});
    const file = path.join(framesDir, `f${String(frames.length).padStart(5, '0')}.jpg`);
    frames.push({ file, at: metadata.timestamp });
    writes.push(writeFile(file, Buffer.from(data, 'base64')));
  });
  await page.send('Page.startScreencast', {
    format: 'jpeg', quality: 92, maxWidth: VIEW.width * SCALE, maxHeight: VIEW.height * SCALE, everyNthFrame: 1,
  });
  for (let i = 0; i < 50 && !frames.length; i++) await sleep(100);
  if (!frames.length) throw new Error('Chrome sent no frames.');

  const began = Date.now() / 1000;
  await run();
  const ended = Date.now() / 1000;
  await page.send('Page.stopScreencast');
  await Promise.all(writes);

  // Start with the frame that was on screen when the tour began.
  const first = Math.max(0, frames.findLastIndex((f) => f.at <= began));
  const kept = frames.slice(first).filter((f) => f.at < ended).map((f) => ({ file: f.file, at: Math.max(0, f.at - began) }));
  return { frames: kept, seconds: ended - began };
}

// Frames arrive unevenly (none while nothing moves), so each video frame is the
// latest one drawn by its time. ffmpeg's concat list would do the same, but it
// rounds image durations to 1/25 s, which adds up over two thousand frames.
async function encode({ frames, seconds }, target) {
  const ffmpeg = spawn(FFMPEG, [
    '-hide_banner', '-loglevel', 'error', '-y',
    '-f', 'image2pipe', '-framerate', String(VIDEO.fps), '-c:v', 'mjpeg', '-i', '-',
    // Chrome's JPEGs are full-range BT.601; video players expect limited-range BT.709.
    '-vf', `scale=${VIDEO.width}:${VIDEO.height}:flags=lanczos:in_range=pc:out_range=tv:in_color_matrix=bt601:out_color_matrix=bt709,format=yuv420p`,
    '-colorspace', 'bt709', '-color_primaries', 'bt709', '-color_trc', 'bt709', '-color_range', 'tv',
    '-c:v', 'libx264', '-preset', 'slow', '-crf', String(VIDEO.crf), '-movflags', '+faststart', '-an', target,
  ], { stdio: ['pipe', 'inherit', 'inherit'] });
  const done = new Promise((resolve, reject) => {
    ffmpeg.on('error', (err) => reject(new Error(`Could not run ${FFMPEG}: ${err.message}`)));
    ffmpeg.on('exit', (code) => (code === 0 ? resolve() : reject(new Error(`ffmpeg failed (exit ${code}).`))));
  });
  ffmpeg.stdin.on('error', () => {});   // a failed ffmpeg is reported by its exit code

  let index = 0;
  let image = readFileSync(frames[0].file);
  for (let n = 0; n < Math.round(seconds * VIDEO.fps); n++) {
    let latest = index;
    while (latest + 1 < frames.length && frames[latest + 1].at <= n / VIDEO.fps) latest++;
    if (latest !== index) image = readFileSync(frames[index = latest].file);
    if (!ffmpeg.stdin.write(image)) await once(ffmpeg.stdin, 'drain');
  }
  ffmpeg.stdin.end();
  await done;
}

// ---------------------------------------------------------------------- main

async function main() {
  for (const file of [EXE, FAKE_CLAUDE]) {
    if (!existsSync(file)) throw new Error(`${path.relative(REPO, file)} is missing. Run: cargo build --release`);
  }
  if (!existsSync(CHROME)) throw new Error(`Google Chrome was not found at ${CHROME}. Set CHROME to its path.`);

  const docs = path.join(REPO, 'docs');
  const work = mkdtempSync(path.join(process.env.DEMO_WORK_DIR || os.tmpdir(), 'cc-sessions-demo-'));
  let page;
  try {
    // A server left over from an earlier run would hold the port and the old data.
    if (existsSync(MARKER)) stopTool();
    resetHome();
    buildDemo();
    log(tool('--no-open'));
    const token = readFileSync(path.join(STATE, 'ui-token'), 'utf8').trim();

    page = await openChrome(path.join(work, 'chrome'));
    // The page writes dates the way the browser's language does; pin it so every machine records the same.
    await page.send('Emulation.setLocaleOverride', { locale: 'en-US' });
    await page.send('Page.addScriptToEvaluateOnNewDocument', {
      source: `try { localStorage.setItem('cc-tab', 'sessions'); localStorage.setItem('cc-theme', 'light'); } catch {}`,
    });
    const metrics = (scale) => page.send('Emulation.setDeviceMetricsOverride', { ...VIEW, deviceScaleFactor: scale, mobile: false });
    // Drawn at exactly the still's size, so it needs no scaling. Reduced motion stops the Working dots mid-pulse.
    await metrics(VIDEO.width / VIEW.width);
    await page.send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-reduced-motion', value: 'reduce' }] });
    await page.send('Page.navigate', { url: `http://127.0.0.1:${PORT}/?t=${token}` });
    const a = actor(page);
    await a.waitFor(`document.querySelectorAll('.row').length > 0 && document.querySelector('#conn-state').dataset.state === 'online'`, 'the sessions list');
    await sleep(500);
    const still = await page.send('Page.captureScreenshot', { format: 'png' });
    writeFileSync(path.join(docs, 'demo.png'), Buffer.from(still.data, 'base64'));
    log('Wrote docs/demo.png');

    await page.send('Emulation.setEmulatedMedia', { features: [] });
    await metrics(SCALE);
    await a.start();
    await sleep(400);
    const marks = [];
    let began = 0;
    const take = await record(page, path.join(work, 'frames'), () => {
      began = Date.now();
      return tour(a, (name) => marks.push(`${((Date.now() - began) / 1000).toFixed(1).padStart(5)}s  ${name}`));
    });
    log(`Recorded ${take.seconds.toFixed(1)}s in ${take.frames.length} frames. Scenes start at:\n${marks.join('\n')}`);
    if (page.problems.length) log(`Page errors:\n${page.problems.join('\n')}`);

    await encode(take, path.join(docs, 'demo.mp4'));
    log(`Wrote docs/demo.mp4 (${(statSync(path.join(docs, 'demo.mp4')).size / 1e6).toFixed(2)} MB)`);
  } finally {
    if (page) await page.close();
    stopTool();
    for (const child of standIns) child.kill();
    if (KEEP) {
      log(`Kept ${HOME} and ${work}`);
    } else {
      removeHome();
      // Chrome's helper processes can take a moment to let go of the profile.
      rmSync(work, { recursive: true, force: true, maxRetries: 20, retryDelay: 250 });
    }
  }
}

main().catch((err) => {
  console.error(`record-demo: ${err.message}`);
  process.exit(1);
});
