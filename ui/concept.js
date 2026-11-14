'use strict';

// A concept: the sessions as a Finder-style browser. Folders open like folders,
// sessions are the files in them, accounts are tags. It reads and acts through
// the same server calls as the main page.

const COLORS = ['#0f9488', '#7c56d8', '#d0661c', '#c2417c', '#2f6fd0', '#4a8f2f', '#b08412', '#64768a'];
const STATUS = { working: 'Working', blocked: 'Blocked', idle: 'Idle', done: 'Done', closed: 'Closed' };
const DAY = 24 * 60 * 60 * 1000;
const $ = (selector) => document.querySelector(selector);

// ------------------------------------------------------------------ utilities

function h(tag, props = {}, ...children) {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    if (value == null || value === false) continue;
    if (key === 'class') el.className = value;
    else if (key === 'style') for (const [name, v] of Object.entries(value)) el.style.setProperty(name, v);
    else if (key.startsWith('on')) el.addEventListener(key.slice(2), value);
    else if (key in el) el[key] = value;
    else el.setAttribute(key, value);
  }
  el.append(...children.flat(Infinity).filter((child) => child != null && child !== false));
  return el;
}

function store(key, value) {
  try {
    if (value === undefined) return localStorage.getItem(key);
    localStorage.setItem(key, value);
  } catch { /* private window */ }
  return null;
}

// Fixed drawings, nothing from data.
function draw(markup) {
  const t = document.createElement('template');
  t.innerHTML = markup.trim();
  return t.content.firstChild;
}

const ART = {
  folder: '<svg class="ic ic-folder" viewBox="0 0 64 52" aria-hidden="true"><path class="tab" d="M4 9a4 4 0 0 1 4-4h15l5 5h28a4 4 0 0 1 4 4v4H4z"/><rect class="body" x="4" y="13" width="56" height="35" rx="4"/><rect class="shine" x="4" y="13" width="56" height="2.5" rx="1"/></svg>',
  session: '<svg class="ic ic-term" viewBox="0 0 64 52" aria-hidden="true"><rect class="frame" x="5" y="5" width="54" height="42" rx="6"/><path class="bar" d="M5 11a6 6 0 0 1 6-6h42a6 6 0 0 1 6 6v3H5z"/><circle class="dot" cx="11" cy="9.5" r="1.4"/><circle class="dot" cx="15.5" cy="9.5" r="1.4"/><circle class="dot" cx="20" cy="9.5" r="1.4"/><path class="prompt" d="M15 24l7 5-7 5" fill="none" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"/><path class="cursor" d="M26 35h12" fill="none" stroke-width="2.6" stroke-linecap="round"/></svg>',
};
// Sidebar and toolbar icons, from icons.js, by what they stand for here.
const GLYPHS = {
  recents: 'clock', needs: 'bell', open: 'circle-play', usage: 'chart-column', home: 'house', all: 'folders', folder: 'folder',
  icons: 'layout-grid', list: 'list', columns: 'columns-3', back: 'chevron-left', forward: 'chevron-right', search: 'search', settings: 'settings',
};
const glyph = (name) => {
  const svg = window.ccIcon(GLYPHS[name] || name);
  svg.classList.add('glyph');
  return svg;
};

function ago(ms) {
  if (!ms) return '';
  const s = Math.max(0, (Date.now() - ms) / 1000);
  if (s < 60) return 'just now';
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  if (s < 7 * 86400) return `${Math.floor(s / 86400)}d ago`;
  return new Date(ms).toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' });
}
const fullDate = (ms) => (ms ? new Date(ms).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' }) : '');
const plural = (n, word) => `${n} ${word}${n === 1 ? '' : 's'}`;

// 1234 -> 1.2K, 3456789 -> 3.5M.
function tokens(n) {
  if (n < 1000) return String(n);
  for (const [size, unit] of [[1e9, 'B'], [1e6, 'M'], [1e3, 'K']]) {
    if (n >= size) return `${(n / size).toFixed(n >= size * 100 ? 0 : 1).replace(/\.0$/, '')}${unit}`;
  }
  return String(n);
}

function toast(message, kind = 'ok') {
  const el = h('div', { class: `toast toast-${kind}` }, message);
  $('#toasts').append(el);
  setTimeout(() => el.remove(), kind === 'error' ? 9000 : 4500);
}

// ----------------------------------------------------------------------- API

let token = new URLSearchParams(location.search).get('t');
if (token) {
  store('cc-token', token);
  history.replaceState(null, '', location.pathname);
} else {
  token = store('cc-token') || '';
}
window.ccToken = token;   // for Settings, which talks to the server too

async function api(path, body) {
  const response = await fetch(path, {
    method: body ? 'POST' : 'GET',
    headers: { 'X-CC-Token': token, ...(body ? { 'Content-Type': 'application/json' } : {}) },
    body: body ? JSON.stringify(body) : undefined,
  });
  const data = await response.json().catch(() => ({}));
  if (response.status === 401) throw new Error('Open Claude Sessions from the terminal once, so this page has its key.');
  if (!response.ok) throw new Error(data.error || `Request failed (${response.status}).`);
  return data;
}

// --------------------------------------------------------------------- state

const S = {
  data: null,
  history: null,
  place: { kind: 'folder', id: '' },
  back: [],
  forward: [],
  view: ['icons', 'list', 'columns'].includes(store('cc-concept-view')) ? store('cc-concept-view') : 'icons',
  sort: store('cc-concept-sort') === 'name' ? 'name' : 'active',
  scale: Number(store('cc-concept-scale')) || 1,
  selected: null,      // an item key: 'f:<folder id>' or a session's key
  expanded: new Set(), // folders opened in the list view
  q: '',
  renaming: null,      // the key of the session whose name is being typed
  quickLook: false,
  usage: null,         // { accounts: { name: { hours, models } } } from /api/usage
  usageAt: 0,
  usageAccount: '',    // '' for every account
  metric: ['all', 'fresh', 'output'].includes(store('cc-concept-metric')) ? store('cc-concept-metric') : 'all',
  usageTable: false,
  stopped: false,      // set once the server is stopped from Settings
};

let tree = null;     // { root, byId }
let entries = [];

// One entry per conversation: what Claude Code reports, the saved conversation, or both.
function allEntries() {
  const saved = new Map((S.history || []).map((item) => [`${item.account}|${item.sessionId}`, item]));
  const list = S.data.sessions.map((s) => {
    const id = `${s.account}|${s.sessionId}`;
    const item = s.sessionId ? saved.get(id) : undefined;
    saved.delete(id);
    return entry(s, item);
  });
  for (const item of saved.values()) list.push(entry(null, item));
  // Opened and never used: not worth a file of its own.
  return list.filter((e) => e.session || e.item.prompts > 0);
}

function entry(s, item) {
  const { account, sessionId } = s || item;
  const cwd = (s && s.cwd) || (item && item.cwd) || '';
  return {
    key: sessionId ? `${account}|${sessionId}` : `s:${s.key}`,
    kind: 'session',
    session: s,
    item,
    account,
    sessionId,
    name: s ? s.name : item.title,
    renamed: (s || item).renamed,
    pinned: Boolean((s && s.pinned) || (item && item.pinned)),
    cwd,
    folder: folderId(cwd),
    status: s ? s.status : 'closed',
    open: Boolean(s && s.open),
    live: Boolean(item && item.live),
    at: Math.max((s && (s.closedAt || s.updatedAt)) || 0, (item && item.lastActivity) || 0),
    artifacts: (s && s.artifacts) || (item && item.artifacts) || [],
    command: (s && s.resumeCommand) || (item && item.resumeCommand) || '',
    tokens: (item && item.tokens) || null,
  };
}

function folderId(cwd) {
  const parts = cwd.split(/[\\/]+/).filter(Boolean);
  return parts.length ? `/${parts.join('/')}` : '?';
}

function folderPath(id) {
  if (!id || id === '?') return '';
  return /^\/(~|[A-Za-z]:)/.test(id) ? id.slice(1) : id;
}

const needsYou = (e) => e.open && (e.status === 'blocked' || (e.status === 'done' && Date.now() - e.at < DAY));

function accountColor(name) {
  const accounts = S.data ? S.data.accounts : [];
  const index = accounts.findIndex((a) => a.name === name);
  return (index >= 0 && accounts[index].color) || COLORS[(index < 0 ? 0 : index) % COLORS.length];
}

// Folders as a tree, with a chain of folders that each hold only the next one
// shown as one ("Programming/Org/KT"), the way the paths actually part.
function buildTree(list) {
  const root = { id: '', label: 'All folders', children: new Map(), own: [] };
  const unknown = [];
  for (const e of list) {
    if (e.folder === '?') { unknown.push(e); continue; }
    let node = root;
    for (const part of e.folder.slice(1).split('/')) {
      if (!node.children.has(part)) node.children.set(part, { id: `${node.id}/${part}`, label: part, children: new Map(), own: [] });
      node = node.children.get(part);
    }
    node.own.push(e);
  }
  const fold = (node) => {
    const kids = [...node.children.values()].map(fold).sort((a, b) => a.label.localeCompare(b.label, undefined, { sensitivity: 'base' }));
    if (node.id && !node.own.length && kids.length === 1) return { ...kids[0], label: `${node.label}/${kids[0].label}` };
    return { id: node.id, label: node.label, own: node.own, kids, all: node.own.concat(...kids.map((k) => k.all)) };
  };
  let top = fold(root).kids;
  while (top.length === 1 && !top[0].own.length && top[0].kids.length) top = top[0].kids;
  for (const node of top) node.label = folderPath(node.id);
  if (unknown.length) top.push({ id: '?', label: 'Unknown folder', own: unknown, kids: [], all: unknown });
  const top_ = { id: '', label: 'All folders', own: [], kids: top, all: list };
  const byId = new Map();
  const link = (node, parent) => {
    node.parent = parent;
    byId.set(node.id, node);
    node.kids.forEach((kid) => link(kid, node));
  };
  link(top_, null);
  return { root: top_, byId };
}

// ------------------------------------------------------------------ the place

function placeName(place = S.place) {
  if (place.kind === 'recents') return 'Recents';
  if (place.kind === 'needs') return 'Needs you';
  if (place.kind === 'open') return 'Open now';
  if (place.kind === 'usage') return 'Usage';
  if (place.kind === 'tag') return place.account;
  const node = tree.byId.get(place.id);
  return node ? node.label : 'All folders';
}

function samePlace(a, b) {
  return a.kind === b.kind && a.id === b.id && a.account === b.account;
}

function go(place, { remember = true } = {}) {
  if (samePlace(place, S.place)) return;
  if (remember) {
    S.back.push(S.place);
    S.forward = [];
  }
  S.place = place;
  S.selected = null;
  S.q = '';
  const search = $('#search');
  if (search) search.value = '';
  render();
}

function goBack() {
  if (!S.back.length) return;
  const from = S.place;
  S.forward.push(from);
  S.place = S.back.pop();
  S.selected = from.kind === 'folder' ? `f:${from.id}` : null;
  render();
}

function goForward() {
  if (!S.forward.length) return;
  S.back.push(S.place);
  S.place = S.forward.pop();
  S.selected = null;
  render();
}

function goUp() {
  if (S.place.kind !== 'folder') return;
  const node = tree.byId.get(S.place.id);
  if (!node || !node.parent) return;
  const from = node.id;
  go({ kind: 'folder', id: node.parent.id });
  S.selected = `f:${from}`;
  render();
}

const folderItem = (node) => ({ key: `f:${node.id}`, kind: 'folder', node, name: node.label });

const byActivity = (a, b) => b.at - a.at;
const byName = (a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: 'base' });

// What the current place holds: folders first, then sessions, sorted as chosen.
function itemsIn(place = S.place, { search = S.q } = {}) {
  let folders = [];
  let sessions;
  const needle = search.trim().toLowerCase();
  if (place.kind === 'folder') {
    const node = tree.byId.get(place.id) || tree.root;
    if (needle) {
      sessions = node.all;
    } else {
      folders = node.kids.map(folderItem);
      sessions = node.own;
    }
  } else if (place.kind === 'recents') {
    sessions = entries;
  } else if (place.kind === 'needs') {
    sessions = entries.filter(needsYou);
  } else if (place.kind === 'open') {
    sessions = entries.filter((e) => e.open);
  } else if (place.kind === 'usage') {
    return [];
  } else {
    sessions = entries.filter((e) => e.account === place.account);
  }
  if (needle) {
    sessions = sessions.filter((e) => `${e.name} ${e.cwd} ${e.account} ${(e.item && e.item.promptText) || ''} ${e.artifacts.map((a) => a.title).join(' ')}`
      .toLowerCase().includes(needle));
  }
  const order = S.sort === 'name' ? byName : byActivity;
  if (place.kind === 'recents' && !needle) return [...sessions].sort(byActivity).slice(0, 60);
  return [...folders.sort(S.sort === 'name' ? byName : (a, b) => latest(b.node) - latest(a.node)), ...[...sessions].sort(order)];
}

const latest = (node) => Math.max(0, ...node.all.map((e) => e.at));

function findItem(key) {
  if (!key) return null;
  if (key.startsWith('f:')) {
    const node = tree.byId.get(key.slice(2));
    return node ? folderItem(node) : null;
  }
  return entries.find((e) => e.key === key) || null;
}

// ------------------------------------------------------------------- drawing

function render() {
  if (!S.data || S.stopped) return;
  entries = allEntries();
  tree = buildTree(entries);
  if (S.place.kind === 'folder' && !tree.byId.has(S.place.id) && S.history) S.place = { kind: 'folder', id: '' };
  const waiting = S.data.sessions.filter((s) => s.open && (s.status === 'blocked' || (s.status === 'done' && Date.now() - (s.updatedAt || 0) < DAY))).length;
  document.title = `${waiting ? `(${waiting}) ` : ''}${placeName()} — Claude Sessions`;
  if (S.renaming) return;
  renderSidebar();
  renderToolbar();
  renderContent();
  renderPath();
  renderStatus();
  if (S.quickLook) renderQuickLook();
}

function sbItem({ place, icon, label, count, hot, color }) {
  const current = samePlace(place, S.place);
  return h('button', { type: 'button', class: 'sb-item', 'aria-current': current ? 'true' : null, onclick: () => go(place) },
    color ? h('span', { class: 'sb-tag', style: { '--acct': color } }) : glyph(icon),
    h('span', { class: 'sb-name' }, label),
    count != null && h('span', { class: `sb-count${hot && count ? ' is-hot' : ''}` }, String(count)));
}

function renderSidebar() {
  const pinned = (S.data.pinnedFolders || []).map(folderId).filter((id) => tree.byId.has(id));
  const home = tree.byId.has('/~') ? '/~' : null;
  $('#sb').replaceChildren(...[
    sbItem({ place: { kind: 'recents' }, icon: 'recents', label: 'Recents' }),
    sbItem({ place: { kind: 'needs' }, icon: 'needs', label: 'Needs you', count: entries.filter(needsYou).length, hot: true }),
    sbItem({ place: { kind: 'open' }, icon: 'open', label: 'Open now', count: entries.filter((e) => e.open).length }),
    sbItem({ place: { kind: 'usage' }, icon: 'usage', label: 'Usage' }),
    h('div', { class: 'sb-head' }, 'Favourites'),
    home && sbItem({ place: { kind: 'folder', id: home }, icon: 'home', label: 'Home' }),
    sbItem({ place: { kind: 'folder', id: '' }, icon: 'all', label: 'All folders' }),
    pinned.map((id) => sbItem({ place: { kind: 'folder', id }, icon: 'folder', label: id.slice(id.lastIndexOf('/') + 1) })),
    h('div', { class: 'sb-head' }, 'Accounts'),
    S.data.accounts.filter((a) => !a.removed).map((a) => sbItem({ place: { kind: 'tag', account: a.name }, label: a.name,
      color: accountColor(a.name), count: entries.filter((e) => e.account === a.name).length })),
    h('div', { class: 'sb-foot' }, h('button', { type: 'button', class: 'sb-item', onclick: () => window.ccSettings.open() }, glyph('settings'), h('span', { class: 'sb-name' }, 'Settings'))),
  ].flat().filter(Boolean));
}

function renderToolbar() {
  const tb = $('#tb');
  if (!tb.firstChild) {
    // Built once, so typing in the search box survives redraws.
    const view = (name, label) => h('button', { type: 'button', 'data-view': name, 'aria-label': label, title: `${label} (⌘${['icons', 'list', 'columns'].indexOf(name) + 1})`,
      onclick: () => setView(name) }, glyph(name));
    tb.append(
      h('div', { class: 'seg', role: 'group', 'aria-label': 'Back and forward' },
        h('button', { type: 'button', id: 'back', 'aria-label': 'Back', title: 'Back (⌘[)', onclick: goBack }, glyph('back')),
        h('button', { type: 'button', id: 'forward', 'aria-label': 'Forward', title: 'Forward (⌘])', onclick: goForward }, glyph('forward'))),
      h('h1', { class: 'tb-title', id: 'title' }),
      h('div', { class: 'seg seg-view', role: 'group', 'aria-label': 'View' }, view('icons', 'Icons'), view('list', 'List'), view('columns', 'Columns')),
      h('select', { 'aria-label': 'Sort', id: 'sort', onchange: (event) => {
        S.sort = event.target.value;
        store('cc-concept-sort', S.sort);
        render();
      } }, h('option', { value: 'active' }, 'Last active'), h('option', { value: 'name' }, 'Name')),
      h('label', { class: 'search' }, glyph('search'),
        h('input', { type: 'search', id: 'search', placeholder: 'Search', 'aria-label': 'Search names, folders and prompts', autocomplete: 'off',
          oninput: (event) => { S.q = event.target.value; S.selected = null; render(); } })),
      // Classic or Finder. Replacing the page keeps Back from bouncing between the two.
      h('button', { type: 'button', class: 'btn tb-settings', 'aria-label': 'Settings', title: 'Settings (⌘,)', onclick: () => window.ccSettings.open() }, glyph('settings')));
  }
  // The usage view has its own controls.
  tb.classList.toggle('is-usage', S.place.kind === 'usage');
  $('#back').disabled = !S.back.length;
  $('#forward').disabled = !S.forward.length;
  $('#title').textContent = S.q.trim() ? `Searching “${S.q.trim()}” in ${placeName()}` : placeName();
  $('#sort').value = S.sort;
  for (const button of tb.querySelectorAll('[data-view]')) button.setAttribute('aria-pressed', String(button.dataset.view === S.view));
}

function setView(view) {
  S.view = view;
  store('cc-concept-view', view);
  render();
  $('#content').focus();
}

// The icon for a folder or a session, with the session's status badge.
function iconFor(item, small) {
  if (item.kind === 'folder') return draw(ART.folder);
  const art = draw(ART.session);
  art.style.setProperty('--acct', accountColor(item.account));
  if (small) return art;
  return [art, h('span', { class: 'badge', 'data-status': item.live && !item.session ? 'working' : item.status, 'aria-hidden': 'true' })];
}

function statusText(e) {
  if (!e.session && e.live) return 'Live';
  return STATUS[e.status] || e.status;
}

function folderSub(node) {
  const open = node.all.filter((e) => e.open);
  const blocked = open.filter((e) => e.status === 'blocked').length;
  return [plural(node.all.length, 'item'), open.length > 0 && ` · ${open.length} open`,
    blocked > 0 && h('span', { class: 'is-blocked' }, ` · ${blocked} waiting`)];
}

function nameNode(item, where) {
  if (item.kind === 'session' && S.renaming === item.key) return renameBox(item);
  return h('span', { class: where === 'cell' ? 'cell-name' : 'label' }, item.name,
    item.kind === 'session' && h('span', { class: 'tagdot', style: { '--acct': accountColor(item.account) }, title: item.account }));
}

function cellId(key) {
  return `item-${key.replace(/[^A-Za-z0-9_-]/g, '_')}`;
}

function select(key) {
  S.selected = key;
  render();
  $('#content').focus({ preventScroll: true });
}

function renderContent() {
  const content = $('#content');
  if (S.place.kind === 'usage') {
    content.classList.remove('is-columns');
    content.removeAttribute('aria-activedescendant');
    content.replaceChildren(usageView());
    return;
  }
  const items = itemsIn();
  content.setAttribute('aria-activedescendant', S.selected && items.some((i) => i.key === S.selected) ? cellId(S.selected) : '');
  if (!items.length && S.view !== 'columns') {
    content.replaceChildren(h('p', { class: 'empty' }, !S.history ? 'Reading saved conversations…'
      : S.q.trim() ? `Nothing in ${placeName()} matches “${S.q.trim()}”.`
        : S.place.kind === 'needs' ? 'Nothing is waiting for you.' : 'Nothing here.'));
    return;
  }
  const scroll = content.scrollTop;
  content.classList.toggle('is-columns', S.view === 'columns');
  if (S.view === 'icons') content.replaceChildren(iconsView(items));
  else if (S.view === 'list') content.replaceChildren(listView(items));
  else content.replaceChildren(columnsView());
  content.scrollTop = scroll;
  const selected = S.selected && document.getElementById(cellId(S.selected));
  if (selected) selected.scrollIntoView({ block: 'nearest' });
}

function events(item) {
  return {
    onclick: (event) => { if (event.detail < 2) select(item.key); },
    ondblclick: () => open(item),
    oncontextmenu: (event) => { event.preventDefault(); select(item.key); showMenu(event.clientX, event.clientY, item); },
  };
}

function iconsView(items) {
  const grid = h('div', { class: 'grid', style: { '--size': `${Math.round(64 * S.scale)}px` } });
  for (const item of items) {
    const selected = S.selected === item.key;
    grid.append(h('div', { class: `cell${selected ? ' is-sel' : ''}${item.kind === 'session' && !item.open ? ' is-closed' : ''}`,
      id: cellId(item.key), role: 'option', 'aria-selected': String(selected), ...events(item) },
      h('div', { class: 'ic-wrap' }, iconFor(item)),
      nameNode(item, 'cell'),
      h('div', { class: 'cell-sub' }, item.kind === 'folder' ? folderSub(item.node)
        : [h('span', { class: item.status === 'blocked' ? 'is-blocked' : '' }, statusText(item)), ` · ${ago(item.at)}`])));
  }
  return grid;
}

function listView(items) {
  const sortBy = (sort) => () => { S.sort = sort; store('cc-concept-sort', sort); render(); };
  const body = h('tbody');
  const add = (item, depth) => {
    const selected = S.selected === item.key;
    const folder = item.kind === 'folder';
    const expanded = folder && S.expanded.has(item.node.id);
    body.append(h('tr', { class: `row${selected ? ' is-sel' : ''}`, id: cellId(item.key), role: 'option', 'aria-selected': String(selected), ...events(item) },
      h('td', { class: 'col-name' }, h('div', { class: 'row-name', style: { '--depth': String(depth) } },
        h('button', { type: 'button', class: `twisty${folder ? '' : ' is-none'}`, tabIndex: -1, 'aria-expanded': String(expanded), 'aria-label': folder ? `${expanded ? 'Collapse' : 'Expand'} ${item.name}` : null,
          onclick: (event) => { event.stopPropagation(); toggleExpanded(item.node.id); } }),
        iconFor(item, true),
        nameNode(item, 'row'))),
      h('td', {}, folder ? h('span', { class: 'muted' }, folderSub(item.node))
        : h('span', { class: `st${item.status === 'blocked' ? ' is-blocked' : ''}`, 'data-status': item.status }, statusText(item))),
      h('td', { class: 'muted' }, folder ? '' : item.account),
      h('td', { class: 'muted', title: fullDate(folder ? latest(item.node) : item.at) }, ago(folder ? latest(item.node) : item.at)),
      h('td', { class: 'muted' }, folder || !item.item ? '' : String(item.item.prompts)),
      h('td', { class: 'muted num' }, folder ? tokens(item.node.all.reduce((sum, e) => sum + metricOf(e.tokens), 0)) : item.tokens ? tokens(metricOf(item.tokens)) : '')));
    if (expanded) {
      for (const child of itemsIn({ kind: 'folder', id: item.node.id }, { search: '' })) add(child, depth + 1);
    }
  };
  for (const item of items) add(item, 0);
  return h('table', { class: 'list' },
    h('thead', {}, h('tr', {},
      h('th', {}, h('button', { type: 'button', 'aria-sort': S.sort === 'name' ? 'ascending' : null, onclick: sortBy('name') }, 'Name')),
      h('th', {}, 'Status'), h('th', {}, 'Account'),
      h('th', {}, h('button', { type: 'button', 'aria-sort': S.sort === 'active' ? 'descending' : null, onclick: sortBy('active') }, 'Last active')),
      h('th', {}, 'Prompts'), h('th', { class: 'num', title: metricLabel() }, 'Tokens'))),
    body);
}

function toggleExpanded(id) {
  if (S.expanded.has(id)) S.expanded.delete(id);
  else S.expanded.add(id);
  render();
}

// One column per folder from the top down to where you are, then the picked session.
function columnsView() {
  const wrap = h('div', { class: 'cols' });
  const chain = [];
  if (S.place.kind === 'folder') {
    for (let node = tree.byId.get(S.place.id) || tree.root; node; node = node.parent) chain.unshift(node);
  }
  const column = (items, onPath) => h('div', { class: 'col' }, items.map((item) => {
    const selected = S.selected === item.key;
    return h('div', { class: `col-item${selected ? ' is-sel' : onPath === item.key ? ' is-path' : ''}`, id: cellId(item.key), role: 'option',
      'aria-selected': String(selected), ...events(item),
      onclick: (event) => {
        if (event.detail > 1) return;
        if (item.kind === 'folder') { go({ kind: 'folder', id: item.node.id }); S.selected = item.key; render(); $('#content').focus(); } else select(item.key);
      } },
      iconFor(item, true), nameNode(item, 'col'), item.kind === 'folder' && h('span', { class: 'chev' }, '›'));
  }));
  if (chain.length) {
    chain.forEach((node, i) => wrap.append(column(itemsIn({ kind: 'folder', id: node.id }, { search: i === chain.length - 1 ? S.q : '' }),
      chain[i + 1] ? `f:${chain[i + 1].id}` : null)));
  } else {
    wrap.append(column(itemsIn(), null));
  }
  const picked = findItem(S.selected);
  if (picked && picked.kind === 'session') wrap.append(h('div', { class: 'preview' }, sessionFacts(picked)));
  requestAnimationFrame(() => { wrap.scrollLeft = wrap.scrollWidth; });
  return wrap;
}

function renderPath() {
  const bar = $('#pathbar');
  const crumb = (label, place, art) => h('button', { type: 'button', onclick: () => go(place) }, art, label);
  if (S.place.kind !== 'folder') {
    bar.replaceChildren(crumb(placeName(), S.place, glyph(S.place.kind === 'tag' ? 'folder' : S.place.kind)));
    return;
  }
  const chain = [];
  for (let node = tree.byId.get(S.place.id) || tree.root; node; node = node.parent) chain.unshift(node);
  bar.replaceChildren(...chain.flatMap((node, i) => [i > 0 && h('span', { class: 'sep' }, '›'),
    crumb(node.label, { kind: 'folder', id: node.id }, draw(ART.folder))]).filter(Boolean));
}

function renderStatus() {
  if (S.place.kind === 'usage') {
    $('#statusbar').replaceChildren(h('span', {}, 'Counted from the conversations Claude Code keeps on this Mac'));
    return;
  }
  const items = itemsIn();
  const sessions = items.filter((i) => i.kind === 'session');
  const open = sessions.filter((e) => e.open).length;
  const picked = findItem(S.selected);
  $('#statusbar').replaceChildren(...[
    h('span', {}, picked ? `1 of ${plural(items.length, 'item')} selected` : `${plural(items.length, 'item')}${open ? `, ${open} open` : ''}`),
    S.view === 'icons' && h('input', { type: 'range', min: '0.75', max: '1.75', step: '0.25', value: String(S.scale), 'aria-label': 'Icon size',
      oninput: (event) => { S.scale = Number(event.target.value); store('cc-concept-scale', String(S.scale)); render(); } }),
  ].filter(Boolean));
}

// ------------------------------------------------------------------- details

function sessionFacts(e) {
  const s = e.session;
  const item = e.item;
  const account = S.data.accounts.find((a) => a.name === e.account) || {};
  const later = item ? item.lastPrompts.filter((p) => p !== item.firstPrompt) : [];
  return [
    draw(ART.session),
    h('h2', { class: 'facts-title' }, e.name),
    h('div', { class: 'facts-meta' },
      h('span', {}, h('span', { class: 'tagdot', style: { '--acct': accountColor(e.account) } }), ' ', e.account),
      h('span', { class: 'st', 'data-status': e.status }, statusText(e)),
      h('span', { title: fullDate(e.at) }, ago(e.at))),
    h('div', { class: 'actions' }, actionButtons(e)),
    h('dl', { class: 'facts' }, [
      ['Folder', e.cwd],
      ['Account', account.email ? `${e.account} (${account.email})` : e.account],
      ['Started', fullDate((s && s.startedAt) || (item && item.startedAt))],
      ['Last status', s && s.detail],
      ['Tokens', e.tokens && tokenSum(e.tokens) ? `${tokens(tokenSum(e.tokens))}: ${tokens(e.tokens.input)} in, ${tokens(e.tokens.output)} out, ${tokens(e.tokens.cacheWrite)} cache written, ${tokens(e.tokens.cacheRead)} cache read` : null],
      ['Session', e.sessionId],
    ].filter(([, v]) => v).map(([k, v]) => [h('dt', {}, k), h('dd', {}, v)])),
    item && item.firstPrompt && [h('h3', { class: 'facts-h' }, 'First prompt'), h('p', { class: 'prompt' }, item.firstPrompt)],
    later.length > 0 && [h('h3', { class: 'facts-h' }, 'Latest prompts'), later.map((p) => h('p', { class: 'prompt' }, p))],
    e.artifacts.length > 0 && [h('h3', { class: 'facts-h' }, 'Artifacts'),
      h('dl', { class: 'facts' }, e.artifacts.map((a) => [h('dt', {}, ago(a.updatedAt)),
        h('dd', {}, h('a', { href: a.url, target: '_blank', rel: 'noopener noreferrer' }, a.title))]))],
    e.command && [h('h3', { class: 'facts-h' }, 'Resume it in a terminal'), h('div', { class: 'cmd' }, e.command)],
  ];
}

function actionButtons(e) {
  return [
    e.open ? h('button', { type: 'button', class: 'btn btn-danger', onclick: () => closeSessions([e.session]) }, window.ccIcon('circle-stop'), 'Close…')
      : e.command && h('button', { type: 'button', class: 'btn btn-primary', onclick: () => resume(e) }, window.ccIcon('play'), 'Resume'),
    e.command && h('button', { type: 'button', class: 'btn', onclick: () => copy(e.command) }, window.ccIcon('copy'), 'Copy command'),
    e.sessionId && h('button', { type: 'button', class: 'btn', onclick: () => { hideQuickLook(); startRename(e); } }, window.ccIcon('pencil'), 'Rename'),
    e.sessionId && h('button', { type: 'button', class: 'btn', onclick: () => pin(e) }, window.ccIcon(e.pinned ? 'pin-off' : 'pin'), e.pinned ? 'Unpin' : 'Pin'),
  ];
}

function renderQuickLook() {
  const e = findItem(S.selected);
  if (!e || e.kind !== 'session') return hideQuickLook();
  $('#ql').replaceChildren(h('div', { class: 'ql', role: 'dialog', 'aria-label': `Quick Look: ${e.name}`, onclick: (event) => event.stopPropagation() }, sessionFacts(e)));
  $('#ql').hidden = false;
}

function toggleQuickLook() {
  const e = findItem(S.selected);
  if (S.quickLook || !e || e.kind !== 'session') return hideQuickLook();
  S.quickLook = true;
  renderQuickLook();
}

function hideQuickLook() {
  S.quickLook = false;
  $('#ql').hidden = true;
  $('#content').focus({ preventScroll: true });
}

// --------------------------------------------------------------------- usage

const METRICS = {
  all: ['All tokens', 'Input, output and both kinds of cache tokens'],
  fresh: ['Without cache reads', 'Input, output and cache writes: leaves out re-reading the conversation, which is most of the total'],
  output: ['Output only', 'What the model wrote'],
};
const metricLabel = () => METRICS[S.metric][0];
const tokenSum = (t) => t.input + t.output + t.cacheWrite + t.cacheRead;
// The chosen measure, from a session's totals or from a [hour, input, output, write, read] row.
function metricOf(t) {
  if (!t) return 0;
  const [input, output, write, read] = Array.isArray(t) ? t.slice(-4) : [t.input, t.output, t.cacheWrite, t.cacheRead];
  return S.metric === 'output' ? output : S.metric === 'fresh' ? input + output + write : input + output + write + read;
}

async function loadUsage() {
  if (S.usageLoading || S.stopped) return;
  S.usageLoading = true;
  try {
    S.usage = await api('/api/usage');
    S.usageAt = Date.now();
  } catch (err) {
    toast(`Could not count tokens: ${err.message}`, 'error');
  }
  S.usageLoading = false;
  if (S.place.kind === 'usage' && $('#menu').hidden) render();
}

const dayKey = (date) => `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`;
const dayLabel = (date) => date.toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short', year: 'numeric' });

// Four steps of one blue, from the days actually used: each step holds a quarter of them.
function levels(values) {
  const used = values.filter((v) => v > 0).sort((a, b) => a - b);
  const at = (q) => used[Math.min(used.length - 1, Math.floor(q * used.length))] || 0;
  const cuts = [at(0.25), at(0.5), at(0.75)];
  return (v) => (v <= 0 ? 0 : v <= cuts[0] ? 1 : v <= cuts[1] ? 2 : v <= cuts[2] ? 3 : 4);
}

// One tooltip for every heat cell, filled from the cell's data with textContent.
function tip(event, title, lines) {
  const box = $('#tip');
  box.replaceChildren(h('strong', {}, title), ...lines.map((line) => h('div', {}, line)));
  box.hidden = false;
  const r = box.getBoundingClientRect();
  box.style.setProperty('left', `${Math.min(event.clientX + 14, innerWidth - r.width - 8)}px`);
  box.style.setProperty('top', `${Math.min(event.clientY + 14, innerHeight - r.height - 8)}px`);
}
const untip = () => { $('#tip').hidden = true; };

function usageView() {
  if (!S.usage || Date.now() - S.usageAt > 60000) loadUsage();
  if (!S.usage) return h('p', { class: 'empty' }, 'Counting tokens in your conversations…');
  const accounts = S.data.accounts.filter((a) => !a.removed && S.usage.accounts[a.name]);
  const chosen = S.usageAccount && S.usage.accounts[S.usageAccount] ? [S.usageAccount] : accounts.map((a) => a.name);

  // Every hour's tokens, placed on this browser's own days and hours.
  const days = new Map();
  const week = Array.from({ length: 7 }, () => new Array(24).fill(0));
  const perAccount = new Map();
  const perModel = new Map();
  for (const name of chosen) {
    const data = S.usage.accounts[name];
    let total = 0;
    for (const row of data.hours) {
      const date = new Date(row[0] * 3600000);
      const value = metricOf(row);
      total += value;
      const key = dayKey(date);
      const day = days.get(key) || { value: 0, parts: [0, 0, 0, 0] };
      day.value += value;
      for (let i = 0; i < 4; i++) day.parts[i] += row[i + 1];
      days.set(key, day);
      week[(date.getDay() + 6) % 7][date.getHours()] += value;
    }
    perAccount.set(name, total);
    for (const [model, row] of Object.entries(data.models)) perModel.set(model, (perModel.get(model) || 0) + metricOf(row));
  }

  const today = new Date();
  today.setHours(0, 0, 0, 0);
  const sumDays = (n) => {
    let sum = 0;
    for (let i = 0; i < n; i++) sum += (days.get(dayKey(new Date(today.getTime() - i * 86400000))) || { value: 0 }).value;
    return sum;
  };
  const busiest = [...days.entries()].sort((a, b) => b[1].value - a[1].value)[0];

  const chip = (pressed, label, onclick, color) => h('button', { type: 'button', class: 'chip', 'aria-pressed': String(pressed), onclick },
    color && h('span', { class: 'tagdot', style: { '--acct': color } }), label);
  const controls = h('div', { class: 'usage-controls' },
    h('div', { class: 'chips', role: 'group', 'aria-label': 'Account' },
      chip(!S.usageAccount, 'All accounts', () => { S.usageAccount = ''; render(); }),
      accounts.map((a) => chip(S.usageAccount === a.name, a.name, () => { S.usageAccount = a.name; render(); }, accountColor(a.name)))),
    h('div', { class: 'seg', role: 'group', 'aria-label': 'Measure' }, Object.entries(METRICS).map(([key, [label, hint]]) =>
      h('button', { type: 'button', 'aria-pressed': String(S.metric === key), title: hint, onclick: () => {
        S.metric = key;
        store('cc-concept-metric', key);
        render();
      } }, label))));

  const tile = (label, value, note) => h('div', { class: 'tile' }, h('div', { class: 'tile-value' }, tokens(value)), h('div', { class: 'tile-label' }, label),
    note && h('div', { class: 'tile-note' }, note));
  const tiles = h('div', { class: 'tiles' },
    tile('Today', sumDays(1)),
    tile('Last 7 days', sumDays(7)),
    tile('Last 30 days', sumDays(30)),
    busiest ? tile('Busiest day', busiest[1].value, dayLabel(new Date(`${busiest[0]}T00:00`))) : tile('Busiest day', 0));

  return h('div', { class: 'usage' },
    controls,
    tiles,
    calendar(days, today),
    h('div', { class: 'usage-row' }, hourMap(week), breakdown('By account', [...perAccount.entries()], (name) => accountColor(name))),
    h('div', { class: 'usage-row' }, breakdown('By model', [...perModel.entries()]), topSessions(chosen)),
    h('p', { class: 'usage-note' },
      'Counted from each reply in the conversations Claude Code keeps on this Mac. Work done by sub-agents is in conversations of its own and is not counted here. ',
      'Claude Code deletes conversations after their account\'s cleanupPeriodDays (30 days unless changed), so older days are empty.'));
}

// The last year, a column a week, Monday at the top.
function calendar(days, today) {
  const WEEKS = 53;
  const start = new Date(today);
  start.setDate(start.getDate() - ((start.getDay() + 6) % 7) - (WEEKS - 1) * 7);
  const cells = [];
  for (let i = 0; i < WEEKS * 7; i++) {
    const date = new Date(start);
    date.setDate(start.getDate() + i);
    cells.push(date);
  }
  const level = levels(cells.map((d) => (days.get(dayKey(d)) || { value: 0 }).value));
  const months = [];
  for (let w = 0; w < WEEKS; w++) {
    const first = cells[w * 7];
    const label = w === 0 || first.getDate() <= 7 ? first.toLocaleDateString(undefined, { month: 'short' }) : '';
    months.push(h('span', { class: 'cal-month', style: { 'grid-column': String(w + 2) } }, label));
  }
  const grid = h('div', { class: 'cal', role: 'img', 'aria-label': `Tokens per day for the last ${WEEKS} weeks. The table view lists every day.` },
    months,
    ['Mon', '', 'Wed', '', 'Fri', '', ''].map((d, i) => h('span', { class: 'cal-day', style: { 'grid-row': String(i + 2) } }, d)),
    cells.map((date, i) => {
      const day = days.get(dayKey(date));
      const future = date > today;
      const value = day ? day.value : 0;
      return h('span', { class: `heat l${future ? 'x' : level(value)}`, style: { 'grid-column': String(Math.floor(i / 7) + 2), 'grid-row': String((i % 7) + 2) },
        onpointermove: future ? null : (event) => tip(event, dayLabel(date), [value ? `${tokens(value)} tokens` : 'No tokens',
          ...(day ? [`${tokens(day.parts[0])} in · ${tokens(day.parts[1])} out · ${tokens(day.parts[2])} cache written · ${tokens(day.parts[3])} cache read`] : [])]),
        onpointerleave: untip });
    }));
  const used = [...days.entries()].filter(([, d]) => d.value > 0).sort((a, b) => (a[0] < b[0] ? 1 : -1));
  return h('section', { class: 'card' },
    h('header', { class: 'card-head' },
      h('h2', {}, `Tokens per day · ${metricLabel()}`),
      h('button', { type: 'button', class: 'btn btn-small', onclick: () => { S.usageTable = !S.usageTable; render(); } }, window.ccIcon(S.usageTable ? 'layout-grid' : 'list'), S.usageTable ? 'Show as calendar' : 'Show as table')),
    S.usageTable
      ? h('table', { class: 'list usage-table' },
        h('thead', {}, h('tr', {}, ['Day', metricLabel(), 'In', 'Out', 'Cache written', 'Cache read'].map((t, i) => h('th', { class: i ? 'num' : '' }, t)))),
        h('tbody', {}, used.map(([key, d]) => h('tr', { class: 'row' }, h('td', {}, dayLabel(new Date(`${key}T00:00`))),
          [d.value, ...d.parts].map((v) => h('td', { class: 'num' }, tokens(v)))))))
      : [grid, h('div', { class: 'legend' }, 'Less', [0, 1, 2, 3, 4].map((l) => h('span', { class: `heat l${l}`, 'aria-hidden': 'true' })), 'More')]);
}

// When in the week Claude is used: weekday by hour of the day, on this browser's clock.
function hourMap(week) {
  const level = levels(week.flat());
  const names = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
  return h('section', { class: 'card' },
    h('header', { class: 'card-head' }, h('h2', {}, 'When in the week')),
    h('div', { class: 'hours', role: 'img', 'aria-label': 'Tokens by weekday and hour of the day' },
      [0, 6, 12, 18].map((hour) => h('span', { class: 'cal-month', style: { 'grid-column': String(hour + 2) } }, `${String(hour).padStart(2, '0')}:00`)),
      names.map((name, d) => [h('span', { class: 'cal-day', style: { 'grid-row': String(d + 2) } }, name),
        week[d].map((value, hour) => h('span', { class: `heat l${level(value)}`, style: { 'grid-column': String(hour + 2), 'grid-row': String(d + 2) },
          onpointermove: (event) => tip(event, `${names[d]} ${String(hour).padStart(2, '0')}:00–${String(hour + 1).padStart(2, '0')}:00`, [value ? `${tokens(value)} tokens` : 'No tokens']),
          onpointerleave: untip }))])));
}

// Bars named directly, so colour is never the only way to tell them apart.
function breakdown(title, rows, colorOf) {
  const sorted = rows.filter(([, v]) => v > 0).sort((a, b) => b[1] - a[1]);
  const max = sorted.length ? sorted[0][1] : 1;
  const total = sorted.reduce((sum, [, v]) => sum + v, 0);
  return h('section', { class: 'card' },
    h('header', { class: 'card-head' }, h('h2', {}, title)),
    sorted.length ? h('div', { class: 'bars' }, sorted.map(([name, value]) => h('div', { class: 'bar-row' },
      h('span', { class: 'bar-name', title: name }, name),
      h('span', { class: 'bar-track' }, h('span', { class: 'bar', style: { width: `${Math.max(1, (value / max) * 100)}%`, ...(colorOf ? { '--fill': colorOf(name) } : {}) } })),
      h('span', { class: 'bar-value' }, `${tokens(value)} · ${value / total < 0.005 ? '<1' : Math.round((value / total) * 100)}%`))))
      : h('p', { class: 'muted' }, 'Nothing counted yet.'));
}

function topSessions(chosen) {
  const top = entries.filter((e) => e.tokens && chosen.includes(e.account) && metricOf(e.tokens) > 0)
    .sort((a, b) => metricOf(b.tokens) - metricOf(a.tokens)).slice(0, 8);
  const max = top.length ? metricOf(top[0].tokens) : 1;
  return h('section', { class: 'card' },
    h('header', { class: 'card-head' }, h('h2', {}, 'Sessions that used the most')),
    top.length ? h('div', { class: 'bars' }, top.map((e) => h('button', { type: 'button', class: 'bar-row bar-button', title: 'Quick Look',
      onclick: () => { S.selected = e.key; S.quickLook = true; render(); } },
      h('span', { class: 'bar-name' }, h('span', { class: 'tagdot', style: { '--acct': accountColor(e.account) } }), ' ', e.name),
      h('span', { class: 'bar-track' }, h('span', { class: 'bar', style: { width: `${Math.max(1, (metricOf(e.tokens) / max) * 100)}%`, '--fill': accountColor(e.account) } })),
      h('span', { class: 'bar-value' }, tokens(metricOf(e.tokens))))))
      : h('p', { class: 'muted' }, 'Nothing counted yet.'));
}

// ------------------------------------------------------------------- actions

function open(item) {
  if (item.kind === 'folder') {
    go({ kind: 'folder', id: item.node.id });
    $('#content').focus();
  } else {
    S.selected = item.key;
    S.quickLook = true;
    render();
  }
}

async function act(run, done) {
  try {
    const result = await run();
    if (done) toast(typeof done === 'function' ? done(result) : done);
    return result;
  } catch (err) {
    toast(err.message, 'error');
    return null;
  } finally {
    refresh();
  }
}

function resume(e) {
  hideQuickLook();
  return e.session
    ? act(() => api('/api/sessions/resume', { key: e.session.key }), (r) => `Opened ${e.name} in ${r.terminal}.`)
    : act(() => api('/api/history/resume', { account: e.account, sessionId: e.sessionId }), (r) => `Opened ${e.name} in ${r.terminal}.`);
}

async function copy(text) {
  try {
    await navigator.clipboard.writeText(text);
    toast('Copied the command that resumes it.');
  } catch {
    toast('Could not copy. Select the command and copy it by hand.', 'error');
  }
}

async function pin(e) {
  await act(() => api('/api/pins', { account: e.account, sessionId: e.sessionId, pinned: !e.pinned }), e.pinned ? 'Unpinned.' : 'Pinned.');
  loadHistory();
}

async function pinFolder(node, pinned) {
  await act(() => api('/api/pins', { folder: folderPath(node.id), pinned }), pinned ? `Added ${node.label} to Favourites.` : `Removed ${node.label} from Favourites.`);
}

// A sheet that lists what will be closed. Interactive sessions are someone's
// open terminal, so with several they start unticked.
function closeSessions(sessions) {
  hideQuickLook();
  const sheet = $('#sheet');
  const one = sessions.length === 1;
  const boxes = sessions.map((s) => h('label', { class: 'pick' },
    h('input', { type: 'checkbox', value: s.key, checked: one || s.kind === 'background' }),
    h('span', { class: 'tagdot', style: { '--acct': accountColor(s.account) } }), h('span', {}, s.name)));
  const confirm = h('button', { type: 'submit', class: 'btn btn-danger' }, one ? 'Close session' : 'Close ticked sessions');
  sheet.replaceChildren(h('form', { method: 'dialog', onsubmit: async (event) => {
    event.preventDefault();
    const keys = [...sheet.querySelectorAll('input:checked')].map((box) => box.value);
    sheet.close();
    if (!keys.length) return;
    const result = await act(() => api('/api/sessions/close', { keys }));
    if (!result) return;
    const closed = result.results.filter((r) => r.ok).length;
    if (closed) toast(closed === 1 ? 'Closed. The conversation is kept.' : `Closed ${closed} sessions. Their conversations are kept.`);
    for (const failure of result.results.filter((r) => !r.ok)) toast(`Could not close ${failure.key}: ${failure.message}`, 'error');
  } },
    h('h2', { id: 'sheet-title' }, one ? `Close “${sessions[0].name}”?` : `Close ${sessions.length} sessions?`),
    h('p', {}, 'It stops, and its conversation is kept, so you can resume it later.'),
    !one && h('div', { class: 'picks' }, boxes),
    h('div', { class: 'sheet-actions' }, h('button', { type: 'button', class: 'btn', onclick: () => sheet.close() }, 'Cancel'), confirm)));
  sheet.showModal();
  confirm.focus();
}

function startRename(e) {
  S.renaming = e.key;
  renderContent();
  const box = $('#content .rename');
  if (box) { box.focus(); box.select(); }
}

function renameBox(e) {
  let done = false;
  const finish = async (save) => {
    if (done) return;
    done = true;
    const value = box.value.trim();
    S.renaming = null;
    render();
    $('#content').focus({ preventScroll: true });
    if (!save || value === e.name || (!value && !e.renamed)) return;
    await act(() => api('/api/sessions/rename', { account: e.account, sessionId: e.sessionId, title: value }),
      value ? `Renamed to ${value}.` : 'Back to Claude Code\'s own title.');
    loadHistory();
  };
  const box = h('input', { type: 'text', class: 'rename', value: e.name, 'aria-label': 'Session name', spellcheck: false,
    onclick: (event) => event.stopPropagation(),
    ondblclick: (event) => event.stopPropagation(),
    onkeydown: (event) => {
      event.stopPropagation();
      if (event.key === 'Enter') finish(true);
      else if (event.key === 'Escape') finish(false);
    },
    onblur: () => finish(box.isConnected) });
  return box;
}

// ---------------------------------------------------------------------- menu

function showMenu(x, y, item) {
  const menu = $('#menu');
  const entryButton = (label, run, key, danger, icon) => h('button', { type: 'button', role: 'menuitem', class: danger ? 'danger' : '',
    onclick: () => { hideMenu(); run(); } }, h('span', { class: 'menu-label' }, icon && window.ccIcon(icon), label), key && h('span', { class: 'key' }, key));
  let entries_;
  if (item.kind === 'folder') {
    const node = item.node;
    const pinned = (S.data.pinnedFolders || []).map(folderId).includes(node.id);
    const open = node.all.filter((e) => e.open).map((e) => e.session);
    entries_ = [
      entryButton('Open', () => open_(item), '⌘↓', false, 'folder-open'),
      node.id && node.id !== '?' && entryButton(pinned ? 'Remove from Favourites' : 'Add to Favourites', () => pinFolder(node, !pinned), null, false, pinned ? 'pin-off' : 'pin'),
      open.length > 0 && h('hr'),
      open.length > 0 && entryButton(`Close ${plural(open.length, 'open session')}…`, () => closeSessions(open), null, true, 'circle-stop'),
    ];
  } else {
    const e = item;
    entries_ = [
      entryButton('Quick Look', () => { S.selected = e.key; toggleQuickLook(); }, 'Space', false, 'eye'),
      e.command && entryButton(e.open ? 'Open in a terminal' : 'Resume', () => resume(e), '⌘O', false, e.open ? 'square-terminal' : 'play'),
      e.command && entryButton('Copy command', () => copy(e.command), null, false, 'copy'),
      h('hr'),
      e.sessionId && entryButton('Rename', () => startRename(e), '↩', false, 'pencil'),
      e.sessionId && entryButton(e.pinned ? 'Unpin' : 'Pin to the top', () => pin(e), null, false, e.pinned ? 'pin-off' : 'pin'),
      e.open && h('hr'),
      e.open && entryButton('Close session…', () => closeSessions([e.session]), '⌘⌫', true, 'circle-stop'),
    ];
  }
  menu.replaceChildren(...entries_.filter(Boolean));
  menu.hidden = false;
  const box = menu.getBoundingClientRect();
  menu.style.setProperty('left', `${Math.min(x, innerWidth - box.width - 8)}px`);
  menu.style.setProperty('top', `${Math.min(y, innerHeight - box.height - 8)}px`);
  menu.querySelector('button').focus();
}
const open_ = (item) => open(item);

function hideMenu() {
  $('#menu').hidden = true;
}

// ------------------------------------------------------------------ keyboard

function move(delta) {
  const items = itemsIn();
  if (!items.length) return;
  const at = items.findIndex((i) => i.key === S.selected);
  const next = at < 0 ? 0 : Math.max(0, Math.min(items.length - 1, at + delta));
  S.selected = items[next].key;
  render();
}

// How many icons fit on a row, for Up and Down in the icons view.
function perRow() {
  const cells = [...document.querySelectorAll('#content .cell')];
  if (!cells.length) return 1;
  const top = cells[0].offsetTop;
  return Math.max(1, cells.filter((c) => c.offsetTop === top).length);
}

document.addEventListener('keydown', (event) => {
  // Keys inside a dialog (the close sheet, Settings) are the dialog's.
  if (document.querySelector('dialog[open]') || S.renaming) return;
  if (!$('#menu').hidden) {
    if (event.key === 'Escape') { hideMenu(); $('#content').focus(); }
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      const buttons = [...$('#menu').querySelectorAll('button')];
      const at = buttons.indexOf(document.activeElement);
      buttons[(at + (event.key === 'ArrowDown' ? 1 : buttons.length - 1)) % buttons.length].focus();
      event.preventDefault();
    }
    return;
  }
  const typing = event.target instanceof Element && event.target.closest('input, select, textarea');
  const cmd = event.metaKey || event.ctrlKey;
  if (cmd && event.key === ',') { event.preventDefault(); window.ccSettings.open(); return; }
  if (cmd && event.key === 'f') { event.preventDefault(); $('#search').focus(); return; }
  // Space and Enter on a button or link are that button's.
  if ((event.key === ' ' || event.key === 'Enter') && event.target instanceof Element && event.target.closest('button, a, [role=menuitem]')) return;
  if (typing) {
    if (event.key === 'Escape' || event.key === 'ArrowDown') { event.preventDefault(); $('#content').focus(); if (!S.selected) move(0); }
    return;
  }
  const item = findItem(S.selected);
  const views = { 1: 'icons', 2: 'list', 3: 'columns' };
  let handled = true;
  if (cmd && views[event.key]) setView(views[event.key]);
  else if (cmd && event.key === '[') goBack();
  else if (cmd && event.key === ']') goForward();
  else if (cmd && event.key === 'ArrowUp') goUp();
  else if (cmd && (event.key === 'ArrowDown' || event.key === 'o') && item) {
    if (item.kind === 'folder' || event.key === 'ArrowDown') open(item); else if (item.command) resume(item);
  } else if (cmd && event.key === 'Backspace' && item && item.open) closeSessions([item.session]);
  else if (event.key === ' ' ) toggleQuickLook();
  else if (event.key === 'Escape') { if (S.quickLook) hideQuickLook(); else { S.selected = null; render(); } }
  else if (event.key === 'Enter' && item) { if (item.kind === 'session' && item.sessionId) startRename(item); else open(item); }
  else if (event.key === 'ArrowRight') {
    if (S.view === 'icons') move(1);
    else if (item && item.kind === 'folder') { open(item); move(0); } else handled = false;
  } else if (event.key === 'ArrowLeft') {
    if (S.view === 'icons') move(-1);
    else if (S.view === 'columns' || (S.view === 'list' && S.place.kind === 'folder')) goUp(); else handled = false;
  } else if (event.key === 'ArrowDown') move(S.view === 'icons' ? perRow() : 1);
  else if (event.key === 'ArrowUp') move(S.view === 'icons' ? -perRow() : -1);
  else if (event.key === 'Home') { S.selected = null; move(0); }
  else if (event.key === 'End') move(Infinity);
  else handled = false;
  if (handled) event.preventDefault();
});

document.addEventListener('click', (event) => {
  if (!$('#menu').hidden && !event.target.closest('#menu')) hideMenu();
});
$('#ql').addEventListener('click', hideQuickLook);
$('#content').addEventListener('click', (event) => {
  if (event.target === event.currentTarget || event.target.classList.contains('grid')) { S.selected = null; render(); }
});
$('#content').addEventListener('contextmenu', (event) => {
  if (event.target === event.currentTarget || event.target.classList.contains('grid')) event.preventDefault();
});

// ---------------------------------------------------------------------- data

let signature = '';

async function refresh() {
  if (S.stopped) return;
  let data;
  try {
    data = await api('/api/state');
  } catch (err) {
    if (!S.data) $('#content').replaceChildren(h('p', { class: 'empty' }, err.message));
    return;
  }
  const next = JSON.stringify([data.accounts, data.sessions, data.pinnedFolders]);
  if (next === signature) return;
  signature = next;
  S.data = data;
  if ($('#menu').hidden) render();
}

async function loadHistory() {
  if (S.stopped) return;
  try {
    S.history = (await api('/api/history')).items;
  } catch (err) {
    toast(`Could not read saved conversations: ${err.message}`, 'error');
  }
  if ($('#menu').hidden) render();
}

// Accounts change in Settings and the window follows. A change of look is CSS's alone.
window.addEventListener('cc-settings-changed', (event) => {
  if (event.detail && event.detail.kind !== 'accounts') return;
  signature = '';
  S.usage = null;
  refresh().then(loadHistory);
});
window.addEventListener('cc-settings-closed', () => {
  if (document.activeElement === document.body) $('#content').focus();
});
window.addEventListener('cc-stopped', () => {
  S.stopped = true;
  $('#content').replaceChildren(h('p', { class: 'empty' }, 'Claude Sessions has stopped. Run it again from a terminal to come back here.'));
});

refresh().then(loadHistory);
setInterval(() => { if (!document.hidden) refresh(); }, 5000);
setInterval(() => { if (!document.hidden) loadHistory(); }, 20000);
setInterval(() => { if (!document.hidden && S.place.kind === 'usage') loadUsage(); }, 60000);
setInterval(() => { if (!document.hidden && $('#menu').hidden && !S.renaming) render(); }, 60000);
$('#content').focus();
