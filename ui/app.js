'use strict';

// One colour per account, in registry order, so an account is recognisable everywhere.
const ACCOUNT_COLORS = ['#0f9488', '#7c56d8', '#d0661c', '#c2417c', '#2f6fd0', '#4a8f2f', '#b08412', '#5a6b7c'];
const STATUS_LABELS = { working: 'Working', blocked: 'Blocked', idle: 'Idle', done: 'Done', closed: 'Closed' };
const STATUS_HINTS = {
  working: 'Claude is working',
  blocked: 'Waiting for your input',
  idle: 'Open in a terminal, not doing anything',
  done: 'Finished its task; not closed yet',
  closed: 'Stopped. The conversation is kept',
};
const STATUS_FILTERS = ['all', 'open', 'working', 'blocked', 'idle', 'done', 'closed'];
const POLL_MS = 5000;

const $ = (selector) => document.querySelector(selector);

const state = {
  tab: 'sessions',
  data: null,          // { accounts, sessions, unregistered }
  history: null,       // [items], the saved conversations
  historyAt: 0,
  historyError: null,
  offlineAt: 0,
  filters: { q: '', account: '', status: 'all' },
  folder: '',          // picked in the sidebar: a folder, 'needs', or '' for all of them
  collapsed: new Set(storedList('cc-collapsed')),
  selected: null,      // the entry whose details are shown
  ticked: new Set(),   // keys of open sessions ticked for closing together
  renaming: null,      // { key, where } while a name is being typed
  sort: store('cc-sort') === 'name' ? 'name' : 'active',
  alerts: store('cc-alerts') === 'on',
  aliasNotice: false,
  offline: null,       // null | 'stopped' | 'disconnected' | 'token'
  signature: '',
};

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
  } catch { /* private window: carry on without it */ }
  return null;
}

function storedList(key) {
  try {
    const list = JSON.parse(store(key) || '[]');
    return Array.isArray(list) ? list : [];
  } catch {
    return [];
  }
}

function ago(ms) {
  if (!ms) return '';
  const seconds = Math.max(0, (Date.now() - ms) / 1000);
  if (seconds < 60) return 'just now';
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
  return `${Math.floor(seconds / 86400)}d ago`;
}

function fullDate(ms) {
  return ms ? new Date(ms).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' }) : '';
}

function shortDate(ms) {
  return ms ? new Date(ms).toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' }) : '';
}

// Recent things read better as "3h ago"; older ones as a date.
function recentDate(ms) {
  return Date.now() - ms < 7 * 86400000 ? ago(ms) : shortDate(ms);
}

// The heading a closed conversation falls under.
function dateGroup(ms) {
  const today = new Date();
  today.setHours(0, 0, 0, 0);
  const days = Math.ceil((today - ms) / 86400000);
  if (days <= 0) return 'Today';
  if (days === 1) return 'Yesterday';
  if (days < 7) return 'Last 7 days';
  if (days < 30) return 'Last 30 days';
  return new Date(ms).toLocaleDateString(undefined, { month: 'long', year: 'numeric' });
}

function plural(n, word) {
  return `${n} ${word}${n === 1 ? '' : 's'}`;
}

function activeAccounts() {
  return state.data ? state.data.accounts.filter((a) => !a.removed) : [];
}

// The colour picked for an account, or else one from the palette by its place in the registry.
function accountColor(name) {
  const index = state.data ? state.data.accounts.findIndex((a) => a.name === name) : -1;
  const picked = index < 0 ? null : state.data.accounts[index].color;
  return picked || autoColor(index);
}

function autoColor(index) {
  return ACCOUNT_COLORS[(index < 0 ? 0 : index) % ACCOUNT_COLORS.length];
}

// Small line icons, drawn in the current text colour. Fixed markup, nothing from data.
const ICONS = {
  edit: '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M10.9 2.4a1.6 1.6 0 0 1 2.3 0l.4.4a1.6 1.6 0 0 1 0 2.3L5.8 12.9 2.5 13.5l.6-3.3z M9.6 3.7l2.7 2.7" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"/></svg>',
};

function icon(name) {
  const el = h('span', { class: 'icon' });
  el.innerHTML = ICONS[name];
  return el;
}

function accountTag(name) {
  return h('span', { class: 'acct', style: { '--acct': accountColor(name) } }, name);
}

function statusBadge(status) {
  return h('span', { class: 'status', 'data-status': status, title: STATUS_HINTS[status] || '' },
    STATUS_LABELS[status] || status);
}

function pathEl(path) {
  if (!path) return h('span', { class: 'path' }, h('span', { class: 'path-leaf' }, 'Unknown folder'));
  const cut = path.lastIndexOf('/') + 1;
  return h('span', { class: 'path' },
    h('span', { class: 'path-parent' }, path.slice(0, cut)),
    h('span', { class: 'path-leaf' }, path.slice(cut)));
}

function groupBy(items, keyOf) {
  const groups = new Map();
  for (const item of items) {
    const key = keyOf(item);
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(item);
  }
  return [...groups.entries()];
}

function toast(message, kind = 'ok') {
  const el = h('div', { class: `toast toast-${kind}` }, message);
  $('#toasts').append(el);
  setTimeout(() => el.remove(), kind === 'error' ? 10000 : 5000);
}

async function copyText(text, button) {
  button.dataset.label = button.dataset.label || button.textContent;
  try {
    await navigator.clipboard.writeText(text);
    button.textContent = 'Copied';
    setTimeout(() => { button.textContent = button.dataset.label; }, 1500);
  } catch {
    toast('Could not copy. Select the command and copy it by hand.', 'error');
  }
}

function commandBlock(command) {
  return h('div', { class: 'command-row' }, h('pre', { class: 'command' }, command),
    h('button', { type: 'button', class: 'btn btn-small', onclick: (event) => copyText(command, event.currentTarget) }, 'Copy'));
}

// ----------------------------------------------------------------------- API

let token = new URLSearchParams(location.search).get('t');
if (token) {
  store('cc-token', token);
  history.replaceState(null, '', location.pathname + location.hash);   // keep the token out of the address bar
} else {
  token = store('cc-token') || '';
}

async function api(path, body) {
  let response;
  try {
    response = await fetch(path, {
      method: body ? 'POST' : 'GET',
      headers: { 'X-CC-Token': token, ...(body ? { 'Content-Type': 'application/json' } : {}) },
      body: body ? JSON.stringify(body) : undefined,
    });
  } catch {
    if (state.offline !== 'disconnected') setOffline('stopped');
    throw new Error('Claude Sessions is not running.');
  }
  if (response.status === 401) {
    setOffline('token');
    throw new Error('This page is not signed in to the server.');
  }
  const data = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(data.error || `Request failed (${response.status}).`);
  return data;
}

// ---------------------------------------------------------------- connection

const OFFLINE_TEXT = {
  stopped: ['Disconnected', 'The Claude Sessions server is not running. Start it again from any terminal:',
    'It stops by itself after a while with no page open. This page reconnects once the server is back.'],
  disconnected: ['Disconnected', 'The Claude Sessions server has stopped. Start it again from any terminal:',
    'Your Claude sessions, accounts and data were not touched and keep running.'],
  token: ['Open this page from the terminal', 'This browser does not have the access key for the server. Run:',
    'That opens the page with the key, and it is remembered after that.'],
};

function setOffline(reason) {
  state.offline = reason;
  state.offlineAt = Date.now();
  const [title, text, note] = OFFLINE_TEXT[reason];
  $('#offline-title').textContent = title;
  $('#offline-text').textContent = text;
  $('#offline-note').textContent = note;
  // Setup lets the command be renamed; show the one this machine uses.
  $('#offline-command').textContent = store('cc-command') || 'cc-sessions';
  $('#offline').hidden = false;
  document.body.classList.add('is-offline');
  if ($('#dialog').open) $('#dialog').close();
  setConnState('offline', 'Disconnected');
}

function setOnline() {
  if (state.offline) {
    state.offline = null;
    $('#offline').hidden = true;
    document.body.classList.remove('is-offline');
  }
  setConnState('online', 'Connected');
}

function setConnState(value, label) {
  const el = $('#conn-state');
  el.dataset.state = value;
  el.textContent = label;
}

async function disconnect() {
  const ok = await showDialog({
    title: 'Disconnect Claude Sessions?',
    body: [h('p', {}, 'This stops the Claude Sessions server and this page stops updating.'),
      h('p', {}, 'Your Claude sessions keep running. Nothing is closed or deleted.')],
    confirmLabel: 'Disconnect',
    onConfirm: () => api('/api/shutdown', {}),
  });
  if (ok) setOffline('disconnected');
}

// ------------------------------------------------------------------- dialogs

// `aside` holds other ways out, shown on the left: { label, value, danger }.
// Picking one closes the dialog with its value.
function showDialog({ title, body = [], confirmLabel = 'Confirm', danger = false, onConfirm, aside = [] }) {
  return new Promise((resolve) => {
    const dialog = $('#dialog');
    const error = h('p', { class: 'dialog-error', hidden: true, role: 'alert' });
    const confirm = h('button', { type: 'submit', class: `btn ${danger ? 'btn-danger' : 'btn-primary'}` }, confirmLabel);
    const finish = (value) => { dialog.close(); resolve(value); };
    const form = h('form', { class: 'dialog-form', onsubmit: async (event) => {
      event.preventDefault();
      confirm.disabled = true;
      error.hidden = true;
      try {
        const result = onConfirm ? await onConfirm(form) : true;
        finish(result === undefined ? true : result);
      } catch (err) {
        // Keep the dialog open so the input is not lost.
        error.textContent = err.message;
        error.hidden = false;
        confirm.disabled = false;
      }
    } },
      h('h2', {}, title),
      body,
      error,
      h('div', { class: 'dialog-actions' },
        aside.length > 0 && h('div', { class: 'dialog-aside' }, aside.filter(Boolean).map((other) =>
          h('button', { type: 'button', class: `btn btn-text${other.danger ? ' btn-text-danger' : ''}`,
            onclick: () => finish(other.value) }, other.label))),
        h('button', { type: 'button', class: 'btn', onclick: () => finish(false) }, 'Cancel'),
        confirm));
    dialog.replaceChildren(form);
    dialog.oncancel = () => resolve(false);
    dialog.showModal();
  });
}

function field(label, name, value = '', hint = '') {
  return h('label', { class: 'field' },
    h('span', { class: 'field-label' }, label),
    h('input', { type: 'text', name, value, autocomplete: 'off', autocapitalize: 'off', spellcheck: 'false' }),
    hint && h('span', { class: 'field-hint' }, hint));
}

// ---------------------------------------------------------------------- data

async function refresh() {
  if (state.offline === 'token') return;
  // Right after Disconnect the server may still answer for a moment; do not flip back.
  if (state.offline === 'disconnected' && Date.now() - state.offlineAt < 3000) return;
  let data;
  try {
    data = await api('/api/state');
  } catch {
    return;
  }
  setOnline();
  const signature = JSON.stringify([data.accounts, data.sessions, data.unregistered, data.pinnedFolders]);
  if (signature === state.signature) return;
  const names = (d) => (d ? d.accounts.map((a) => `${a.name}:${a.removed}`).join() : '');
  const accountsChanged = names(data) !== names(state.data);
  state.signature = signature;
  noticeChanges(state.data && state.data.sessions, data.sessions);
  state.data = data;
  if (data.platform && data.platform.command) store('cc-command', data.platform.command);
  // The account picker lives in the fixed toolbar, so rebuild it when accounts change.
  if (accountsChanged) mount();
  else render();
}

async function loadHistory() {
  state.historyAt = Date.now();
  try {
    state.history = (await api('/api/history')).items;
    state.historyError = null;
  } catch (err) {
    state.historyError = err.message;
  }
  if (state.tab === 'sessions') render();
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

// -------------------------------------------------------------------- render

// A wide window with a mouse gets the details panel and actions on hover;
// anything else opens details under the row. style.css has the same query.
const wide = matchMedia('(min-width: 1180px) and (hover: hover)');

function viewHash() {
  if (state.tab === 'accounts') return '#accounts';
  if (state.folder === 'needs') return '#needs';
  return state.folder ? `#f=${encodeURIComponent(state.folder)}` : '';
}

function readHash() {
  const hash = location.hash;
  state.tab = hash === '#accounts' ? 'accounts' : 'sessions';
  if (state.tab === 'accounts') return;
  let folder = '';
  try {
    if (hash.startsWith('#f=')) folder = decodeURIComponent(hash.slice(3));
  } catch { /* a mangled link shows everything */ }
  state.folder = hash === '#needs' ? 'needs' : folder;
}

// Show a tab or a folder. Each is a step in the browser's history, so Back returns to the last one.
function go({ tab = state.tab, folder = state.folder }) {
  const sameTab = tab === state.tab;
  state.tab = tab;
  state.folder = folder;
  store('cc-tab', tab);
  store('cc-folder', folder);
  if (viewHash() !== location.hash) history.pushState(null, '', location.pathname + viewHash());
  if (sameTab && $('#list')) render();
  else show();
}

function show() {
  for (const button of document.querySelectorAll('.tab')) {
    button.setAttribute('aria-current', button.dataset.tab === state.tab ? 'page' : 'false');
  }
  mount();
}

// Build the view's fixed parts (sidebar, toolbar, containers) once, so typing in
// the search box survives the list being redrawn underneath it.
function mount() {
  const view = $('#view');
  view.classList.toggle('view-wide', state.tab === 'sessions');
  if (!state.data) {
    view.replaceChildren(h('p', { class: 'empty' }, 'Loading…'));
    return;
  }
  state.renaming = null;
  panelShown = '';
  if (state.tab === 'sessions') {
    view.replaceChildren(h('div', { class: 'browser' },
      h('nav', { id: 'side', class: 'side', 'aria-label': 'Folders' }),
      h('div', { class: 'pane' },
        h('header', { id: 'pane-head', class: 'pane-head' }),
        toolbar(),
        h('div', { id: 'chips', class: 'chips' }),
        h('div', { id: 'notices' }),
        h('div', { id: 'bulk' }),
        h('div', { id: 'list' })),
      wide.matches && h('aside', { id: 'panel', class: 'panel', 'aria-label': 'Session details', hidden: true })));
    if (!state.history || Date.now() - state.historyAt > 30000) loadHistory();
  } else {
    view.replaceChildren(h('div', { id: 'list' }));
  }
  render();
}

// `force` redraws even while a name is being edited, which a refresh must not do:
// it would take the box away mid-word.
function render(force) {
  if (!state.data) return;
  const waiting = state.data.sessions.filter(needsYou).length;
  document.title = `${waiting ? `(${waiting}) ` : ''}Claude Sessions`;
  if (state.renaming && !force) return;
  if (!$('#list')) return mount();
  if (state.tab === 'sessions') renderSessions();
  else renderAccounts();
}

function toolbar() {
  const filters = state.filters;
  const placeholder = 'Search name, folder, prompt or artifact';
  return h('div', { class: 'toolbar' },
    h('input', { type: 'search', class: 'search', placeholder, 'aria-label': placeholder, value: filters.q,
      oninput: (e) => { filters.q = e.target.value; render(); } }),
    h('select', { 'aria-label': 'Sort', onchange: (e) => {
      state.sort = e.target.value;
      store('cc-sort', state.sort);
      render();
    } },
      h('option', { value: 'active', selected: state.sort === 'active' }, 'Sort: last active'),
      h('option', { value: 'name', selected: state.sort === 'name' }, 'Sort: name')),
    h('button', { type: 'button', class: 'btn', onclick: () => { loadHistory(); refresh(); } }, 'Refresh'));
}

function artifactList(artifacts) {
  if (!artifacts || !artifacts.length) return null;
  return [h('h3', {}, 'Artifacts'),
    h('ul', { class: 'artifacts' }, artifacts.map((a) => h('li', {},
      h('a', { href: a.url, target: '_blank', rel: 'noopener noreferrer' }, a.title),
      h('time', { title: fullDate(a.updatedAt) }, ago(a.updatedAt))))),
    h('p', { class: 'quiet' }, 'They open on claude.ai, for the Claude login that published them.')];
}

// -- sessions

// One entry per conversation: a session Claude Code reports, the saved
// conversation behind it, or both when they are the same one.
function allEntries() {
  const saved = new Map((state.history || []).map((item) => [`${item.account}|${item.sessionId}`, item]));
  const entries = state.data.sessions.map((s) => {
    const id = `${s.account}|${s.sessionId}`;
    const item = s.sessionId ? saved.get(id) : undefined;
    saved.delete(id);
    return entry(s, item);
  });
  for (const item of saved.values()) entries.push(entry(null, item));
  return entries.sort((a, b) => b.at - a.at);
}

function entry(s, item) {
  const { account, sessionId } = s || item;
  const cwd = (s && s.cwd) || (item && item.cwd) || '';
  return {
    // The same key whether the conversation is open or only saved, so it stays picked when it is resumed.
    key: sessionId ? `${account}|${sessionId}` : `s:${s.key}`,
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
    at: Math.max((s && (s.closedAt || s.updatedAt)) || 0, (item && item.lastActivity) || 0),
    artifacts: (s && s.artifacts) || (item && item.artifacts) || [],
    command: (s && s.resumeCommand) || (item && item.resumeCommand) || '',
  };
}

// Waiting for its person: stuck on a question, or finished and not yet closed.
// Works on a session and on an entry alike.
function needsYou(x) {
  return x.open && (x.status === 'blocked' || x.status === 'done');
}

// A folder's place in the sidebar: its path with one kind of slash. '?' is no folder.
function folderId(cwd) {
  const parts = cwd.split(/[\\/]+/).filter(Boolean);
  return parts.length ? `/${parts.join('/')}` : '?';
}

// The path to show for a folder. The server already shortens the home folder to ~.
function folderPath(id) {
  if (id === '?') return '';
  return /^\/(~|[A-Za-z]:)/.test(id) ? id.slice(1) : id;
}

function inFolder(e, id) {
  return !id || e.folder === id || e.folder.startsWith(`${id}/`);
}

// The folders as a tree. A folder that only leads to one other folder is folded
// into it, so the tree starts where the paths part ways.
function folderTree(entries) {
  const root = { id: '', label: '', children: new Map(), own: [] };
  const unknown = { id: '?', label: 'Unknown folder', kids: [], all: [] };
  for (const e of entries) {
    if (e.folder === '?') {
      unknown.all.push(e);
      continue;
    }
    let node = root;
    for (const part of e.folder.slice(1).split('/')) {
      if (!node.children.has(part)) {
        node.children.set(part, { id: `${node.id}/${part}`, label: part, children: new Map(), own: [] });
      }
      node = node.children.get(part);
    }
    node.own.push(e);
  }
  let top = fold(root).kids;
  while (top.length === 1 && !top[0].own.length && top[0].kids.length) top = top[0].kids;
  for (const node of top) node.label = folderPath(node.id);
  return unknown.all.length ? [...top, unknown] : top;
}

function fold(node) {
  const kids = [...node.children.values()].map(fold)
    .sort((a, b) => a.label.localeCompare(b.label, undefined, { sensitivity: 'base' }));
  if (node.id && !node.own.length && kids.length === 1) return { ...kids[0], label: `${node.label}/${kids[0].label}` };
  return { id: node.id, label: node.label, own: node.own, kids, all: node.own.concat(...kids.map((kid) => kid.all)) };
}

function findFolder(nodes, id) {
  for (const node of nodes) {
    const hit = node.id === id ? node : findFolder(node.kids, id);
    if (hit) return hit;
  }
  return null;
}

function pinnedFolderIds() {
  return (state.data.pinnedFolders || []).map(folderId).filter((id) => id !== '?');
}

function renderSide(entries, tree) {
  const rows = [];
  const walk = (node, depth) => {
    rows.push(sideRow(node, depth, 'folder'));
    if (!state.collapsed.has(node.id)) for (const kid of node.kids) walk(kid, depth + 1);
  };
  for (const node of tree) walk(node, 0);
  // Every folder that has folders inside it; one button closes or opens them all.
  const parents = [];
  const collect = (node) => {
    if (node.kids.length) parents.push(node.id);
    node.kids.forEach(collect);
  };
  tree.forEach(collect);
  const allClosed = parents.every((id) => state.collapsed.has(id));
  const pinned = pinnedFolderIds().map((id) => (
    { id, label: id.slice(id.lastIndexOf('/') + 1), kids: [], all: entries.filter((e) => inFolder(e, id)) }));
  $('#side').replaceChildren(...[
    sideRow({ id: '', label: 'All sessions', kids: [], all: entries }, 0, 'view'),
    sideRow({ id: 'needs', label: 'Needs you', kids: [], all: entries.filter(needsYou) }, 0, 'view'),
    pinned.length > 0 && h('div', { class: 'side-head' }, h('h2', { class: 'side-heading' }, 'Pinned')),
    ...pinned.map((node) => sideRow(node, 0, 'pin')),
    h('div', { class: 'side-head' },
      h('h2', { class: 'side-heading' }, 'Folders'),
      parents.length > 0 && h('button', { type: 'button', class: 'side-all', onclick: () => {
        state.collapsed = new Set(allClosed ? [] : parents);
        store('cc-collapsed', JSON.stringify(parents.filter((id) => state.collapsed.has(id))));
        render();
      } }, allClosed ? 'Expand all' : 'Collapse all')),
    ...rows,
  ].filter(Boolean));
}

// `kind` is 'view' for the lists at the top, 'pin' for a pinned folder, 'folder' for one in the tree.
function sideRow(node, depth, kind) {
  const open = node.all.filter((e) => e.open);
  // The dot takes the colour of the most pressing open session in the folder.
  const pressing = ['blocked', 'working'].find((status) => open.some((e) => e.status === status)) || 'idle';
  const collapsed = state.collapsed.has(node.id);
  const selected = state.folder === node.id;
  const needs = node.id === 'needs';
  return h('div', { class: `side-row${selected ? ' is-selected' : ''}`, style: { '--depth': String(depth) } },
    kind === 'folder' && node.kids.length
      ? h('button', { type: 'button', class: 'side-twist', 'aria-expanded': String(!collapsed),
        'aria-label': `${collapsed ? 'Show' : 'Hide'} the folders in ${node.label}`, onclick: () => {
          if (collapsed) state.collapsed.delete(node.id);
          else state.collapsed.add(node.id);
          store('cc-collapsed', JSON.stringify([...state.collapsed]));
          render();
        } })
      : h('span', { class: 'side-twist' }),
    h('button', { type: 'button', class: 'side-item', 'aria-current': selected ? 'true' : null,
      title: kind === 'view' ? null : folderPath(node.id) || null, onclick: () => go({ folder: node.id }) },
      kind !== 'view' && h('span', { class: 'side-icon' }),
      h('span', { class: 'side-name' }, node.label),
      !needs && open.length > 0 && h('span', { class: 'side-open', 'data-status': pressing, title: `${open.length} open` }),
      h('span', { class: `side-count${needs && node.all.length ? ' is-hot' : ''}` }, String(node.all.length))));
}

function inFilter(e, filter) {
  if (filter === 'all') return true;
  if (filter === 'open') return e.open;
  return e.status === filter;
}

// Where a search matched: true for the name, folder, id or an artifact, or the
// words around it when it was something typed in the conversation.
function searchHit(e, needle) {
  const item = e.item;
  const facts = `${e.cwd} ${e.name} ${item ? item.title : ''} ${e.sessionId} ${e.session ? e.session.id : ''} ${e.artifacts.map((a) => a.title).join(' ')}`;
  if (facts.toLowerCase().includes(needle)) return true;
  const text = (item && item.promptText) || '';
  const at = text.toLowerCase().indexOf(needle);
  if (at < 0) return null;
  const lineStart = text.lastIndexOf('\n', at) + 1;
  const lineEnd = text.indexOf('\n', at) < 0 ? text.length : text.indexOf('\n', at);
  const from = Math.max(lineStart, at - 40);
  const to = Math.min(lineEnd, at + needle.length + 90);
  return {
    before: (from > lineStart ? '…' : '') + text.slice(from, at),
    match: text.slice(at, at + needle.length),
    after: text.slice(at + needle.length, to) + (to < lineEnd ? '…' : ''),
  };
}

function renderSessions() {
  const filters = state.filters;
  const everything = allEntries();
  const all = everything.filter((e) => !filters.account || e.account === filters.account);
  const tree = folderTree(all);
  const needs = state.folder === 'needs';
  const folder = needs ? '' : state.folder;
  const pinnedFolder = pinnedFolderIds().includes(folder);
  // A picked folder with nothing left in it falls back to everything, once the
  // saved conversations are in. A pinned folder stays, empty.
  if (folder && state.history && !pinnedFolder && !findFolder(tree, folder)) {
    state.folder = '';
    store('cc-folder', '');
    history.replaceState(null, '', location.pathname + viewHash());
    return renderSessions();
  }
  const scope = needs ? all.filter(needsYou) : all.filter((e) => inFolder(e, folder));
  renderSide(all, tree);
  renderPanel(everything);

  const open = scope.filter((e) => e.open);
  const path = folderPath(folder);
  $('#pane-head').replaceChildren(
    h('h2', {}, needs ? 'Needs you' : folder ? pathEl(path) : 'All sessions'),
    h('span', { class: 'count' }, `${plural(scope.length, 'session')}${open.length && !needs ? `, ${open.length} open` : ''}`),
    h('div', { class: 'pane-actions' },
      folder && folder !== '?' && h('button', { type: 'button', class: 'btn btn-small',
        onclick: () => pin({ folder: path }, !pinnedFolder) }, pinnedFolder ? 'Unpin folder' : 'Pin folder'),
      folder && open.length > 1 && h('button', { type: 'button', class: 'btn btn-small', onclick: () => closeMany({
        title: 'Close sessions in this folder?',
        where: path,
        note: 'Ticked sessions are stopped. Their conversations are kept and can be resumed. ' +
          'Interactive sessions are open in a terminal, so they start unticked.',
        sessions: open.map((e) => e.session),
      }) }, 'Close all open here')));

  const needle = filters.q.trim().toLowerCase();
  const hits = new Map();
  const visible = scope.filter((e) => {
    if (!needle) return true;
    const hit = searchHit(e, needle);
    if (hit) hits.set(e.key, hit);
    return Boolean(hit);
  });

  const accounts = activeAccounts();
  const chip = (pressed, onclick, ...label) => h('button', { type: 'button', class: 'chip', 'aria-pressed': String(pressed), onclick }, label);
  $('#chips').replaceChildren(...[
    accounts.length > 1 && h('div', { class: 'chip-set', role: 'group', 'aria-label': 'Account' },
      chip(!filters.account, () => { filters.account = ''; render(); }, 'All accounts'),
      accounts.map((a) => chip(filters.account === a.name, () => {
        filters.account = filters.account === a.name ? '' : a.name;
        render();
      }, accountTag(a.name)))),
    h('div', { class: 'chip-set', role: 'group', 'aria-label': 'Status' }, STATUS_FILTERS.map((filter) => {
      const count = visible.filter((e) => inFilter(e, filter)).length;
      // A status nothing has is left out, unless it is the one picked.
      if (!count && !['all', 'open', filters.status].includes(filter)) return null;
      const label = filter === 'open' ? 'Open' : filter === 'all' ? 'All' : STATUS_LABELS[filter];
      return h('button', { type: 'button', class: 'chip', 'aria-pressed': String(filters.status === filter),
        'data-status': filter, title: STATUS_HINTS[filter] || '',
        onclick: () => { filters.status = filter; render(); } },
        label, h('span', { class: 'chip-count' }, String(count)));
    })),
  ].filter(Boolean));

  $('#notices').replaceChildren(...[
    ...accounts.filter((a) => a.error).map((a) =>
      h('p', { class: 'notice notice-error' }, `Could not read sessions for ${a.name}: ${a.error}`)),
    state.historyError && h('p', { class: 'notice notice-error' }, `Could not read saved conversations: ${state.historyError}`),
  ].filter(Boolean));

  // Ticks only last while the session they are on is open.
  const ticked = state.data.sessions.filter((s) => s.open && state.ticked.has(s.key));
  state.ticked = new Set(ticked.map((s) => s.key));
  $('#bulk').replaceChildren(...(ticked.length ? [h('div', { class: 'bulk' },
    h('span', {}, `${plural(ticked.length, 'session')} selected`),
    h('button', { type: 'button', class: 'btn btn-small', onclick: () => closeMany({
      title: 'Close the selected sessions?',
      note: 'Ticked sessions are stopped. Their conversations are kept and can be resumed.',
      sessions: ticked,
      tickInteractive: true,
    }) }, 'Close selected'),
    h('button', { type: 'button', class: 'btn btn-small', onclick: () => { state.ticked.clear(); render(); } }, 'Clear'))] : []));
  $('#list').classList.toggle('is-ticking', ticked.length > 0);

  const reading = !state.history && !state.historyError;
  const shown = visible.filter((e) => inFilter(e, filters.status));
  if (!shown.length) {
    $('#list').replaceChildren(h('p', { class: 'empty' }, reading ? 'Reading saved conversations…'
      : needs && !scope.length ? 'Nothing is waiting for you.'
        : all.length ? 'No sessions match these filters.'
          : 'No sessions yet. Start Claude Code in any folder and it shows up here.'));
    return;
  }

  // By name is one list. Otherwise: pinned, then open, then finished background
  // sessions, then the rest by when it was last active.
  const rank = (e) => (e.pinned ? 0 : !e.open ? 3 : e.status === 'done' ? 2 : 1);
  const heading = (e) => ['Pinned', 'Open now', 'Done, not closed'][rank(e)] || dateGroup(e.at);
  const groups = state.sort === 'name'
    ? [['', [...shown].sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: 'base' }))]]
    : groupBy([0, 1, 2, 3].flatMap((n) => shown.filter((e) => rank(e) === n)), heading);
  // Rows name their folder only when the list spans several, and then from the picked folder down.
  const several = new Set(scope.map((e) => e.folder)).size > 1;
  const folderOf = (e) => {
    if (!several || (folder && e.folder === folder)) return '';
    return folder ? e.folder.slice(folder.length + 1) : folderPath(e.folder);
  };
  const days = [...new Set(accounts.map((a) => a.retentionDays))].sort((a, b) => a - b);
  const span = days.length > 1 ? `${days[0]} to ${days[days.length - 1]}` : `${days[0] || 30}`;
  $('#list').replaceChildren(
    ...groups.map(([name, entries]) => {
      const finished = name === 'Done, not closed' ? entries.map((e) => e.session) : [];
      return h('section', { class: 'group' },
        name && h('header', { class: 'group-head' },
          h('h2', { class: 'group-date' }, name),
          finished.length > 1
            ? h('button', { type: 'button', class: 'btn btn-small', onclick: () => closeMany({
              title: 'Close all finished sessions?',
              note: 'Ticked sessions are stopped. Their conversations are kept and can be resumed.',
              sessions: finished,
              tickInteractive: true,
            }) }, `Close all ${finished.length} finished`)
            : h('span', { class: 'count' }, String(entries.length))),
        entries.map((e) => entryRow(e, folderOf(e), hits.get(e.key))));
    }),
    h('p', { class: 'quiet list-note' }, reading ? 'Reading saved conversations…'
      : `Claude Code itself deletes conversations after ${span} days without activity ` +
        '(its cleanupPeriodDays setting, shown per account under Accounts). Claude Sessions never deletes any.'));
}

// Pick the session whose details are shown, or none. Beside the list only the
// highlight moves, so the row under a double-click is still there for the second click.
function select(key) {
  state.selected = key;
  if (!$('#panel')) return render();
  for (const row of document.querySelectorAll('#list .row')) {
    row.classList.toggle('is-selected', row.dataset.key === key);
    row.querySelector('.row-main').setAttribute('aria-expanded', String(row.dataset.key === key));
  }
  renderPanel(allEntries());
}

function statusEl(e) {
  return !e.session && e.item.live
    ? h('span', { class: 'status', 'data-status': 'working', title: 'This conversation is open right now' }, 'Live')
    : statusBadge(e.status);
}

function entryRow(e, folder, hit) {
  const { session: s, item } = e;
  const selected = state.selected === e.key;
  const sub = [
    folder && pathEl(folder),
    s && s.kind === 'background' && h('span', { class: 'row-tag' }, 'Background'),
    item && h('span', { class: 'row-tag' }, plural(item.prompts, 'prompt')),
    e.artifacts.length > 0 && h('span', { class: 'row-tag' }, plural(e.artifacts.length, 'artifact')),
  ].filter(Boolean);
  return h('div', { class: `row${e.open ? '' : ' is-closed'}${selected ? ' is-selected' : ''}`, 'data-key': e.key,
    style: { '--acct': accountColor(e.account) } },
    h('span', { class: 'row-tick' }, e.open && h('input', { type: 'checkbox', checked: state.ticked.has(s.key),
      'aria-label': `Select ${e.name}`, onchange: (event) => {
        if (event.target.checked) state.ticked.add(s.key);
        else state.ticked.delete(s.key);
        render();
      } })),
    h('div', { class: 'row-main', role: 'button', tabIndex: 0, 'aria-expanded': String(selected),
      onclick: (event) => {
        // The second click of a double-click is the start of a rename, not another pick.
        if (event.detail > 1) return;
        select(selected && !wide.matches ? null : e.key);
      },
      onkeydown: (event) => {
        if (event.target !== event.currentTarget || (event.key !== 'Enter' && event.key !== ' ')) return;
        event.preventDefault();
        select(selected ? null : e.key);
      } },
      h('span', { class: 'row-title' },
        nameEl(e, 'row'),
        sub.length > 0 && h('span', { class: 'row-sub' }, sub),
        hit && hit !== true && h('span', { class: 'row-hit' }, hit.before, h('mark', {}, hit.match), hit.after)),
      accountTag(e.account),
      statusEl(e),
      h('span', { class: 'row-time', title: fullDate(e.at) }, recentDate(e.at))),
    h('div', { class: 'row-actions' },
      e.command && h('button', { type: 'button', class: 'btn btn-small row-quiet', title: 'Copy the command that resumes it',
        onclick: (event) => copyText(e.command, event.currentTarget) }, 'Copy'),
      pinButton(e, 'row-quiet'),
      mainAction(e)),
    selected && !wide.matches && entryDetails(e, 'row'));
}

// Close for a session that is open, Resume for one that is not.
function mainAction(e) {
  const { session: s, item } = e;
  const button = (label, onclick) => h('button', { type: 'button', class: 'btn btn-small', onclick }, label);
  if (e.open) return button('Close', () => closeOne(s));
  if (s) return button('Resume', () => resumeSession(s));
  return item.resumeCommand ? button('Resume', () => resumeHistory(item)) : null;
}

function pinButton(e, extra = '') {
  return e.sessionId && h('button', { type: 'button', class: `btn btn-small ${extra}`,
    title: e.pinned ? 'Take it off the top of the list' : 'Keep it at the top of the list',
    onclick: () => pin({ account: e.account, sessionId: e.sessionId }, !e.pinned) }, e.pinned ? 'Unpin' : 'Pin');
}

async function pin(what, pinned) {
  await act(() => api('/api/pins', { ...what, pinned }));
  if (state.history) loadHistory();
}

// -- names

// A session's name; double-click it to type a new one. `where` is 'row' or
// 'panel', so only the one that was double-clicked turns into a box.
function nameEl(e, where) {
  const editing = state.renaming && state.renaming.key === e.key && state.renaming.where === where;
  if (editing) return renameBox(e);
  return h('span', { class: 'row-name', title: e.sessionId ? 'Double-click to rename' : null,
    ondblclick: e.sessionId ? (event) => { event.stopPropagation(); startRename(e, where); } : null }, e.name);
}

function startRename(e, where) {
  state.renaming = { key: e.key, where };
  render(true);
  const box = $('.rename');
  if (box) {
    box.focus();
    box.select();
  }
}

function renameBox(e) {
  let done = false;
  const finish = (save) => {
    if (done) return;
    done = true;
    const value = box.value.trim();
    state.renaming = null;
    render();
    // Saving Claude Code's own title back would pin it; leave it to follow along.
    if (save && value !== e.name && (value || e.renamed)) saveName(e, value);
  };
  const box = h('input', { type: 'text', class: 'rename', value: e.name, 'aria-label': 'Session name',
    placeholder: 'Empty goes back to Claude Code\'s title', autocomplete: 'off', spellcheck: 'false',
    onclick: (event) => event.stopPropagation(),
    ondblclick: (event) => event.stopPropagation(),
    onkeydown: (event) => {
      event.stopPropagation();
      if (event.key === 'Enter') finish(true);
      else if (event.key === 'Escape') finish(false);
    },
    // Clicking away keeps what was typed; a box that was redrawn away does not.
    onblur: () => finish(box.isConnected) });
  return box;
}

async function saveName(e, title) {
  try {
    const saved = await api('/api/sessions/rename', { account: e.account, sessionId: e.sessionId, title });
    toast(saved.title ? `Renamed to ${saved.title}.` : 'Back to Claude Code\'s own title.');
  } catch (err) {
    toast(err.message, 'error');
  }
  if (state.history) loadHistory();
  refresh();
}

// -- details

let panelShown = '';
let panelKey = null;

// The details panel beside the list. It is rebuilt only when what it shows
// changed, so text being selected in it is left alone by the regular refresh.
function renderPanel(entries) {
  const panel = $('#panel');
  if (!panel) return;
  const e = state.selected ? entries.find((candidate) => candidate.key === state.selected) : null;
  panel.parentElement.classList.toggle('has-panel', Boolean(e));
  panel.hidden = !e;
  const shown = e ? JSON.stringify([e.session, e.item, state.renaming, Math.floor(Date.now() / 60000)]) : '';
  if (shown === panelShown) return;
  panelShown = shown;
  if (!e) {
    panel.replaceChildren();
    return;
  }
  panel.replaceChildren(
    h('header', { class: 'panel-head' },
      h('div', { class: 'panel-top' },
        h('h2', { class: 'panel-name' }, nameEl(e, 'panel')),
        h('button', { type: 'button', class: 'panel-close', 'aria-label': 'Close details', title: 'Close (Esc)',
          onclick: () => select(null) }, '×')),
      h('div', { class: 'panel-meta' },
        accountTag(e.account), statusEl(e),
        h('span', { class: 'row-time', title: fullDate(e.at) }, recentDate(e.at)))),
    entryDetails(e, 'panel'));
  if (panelKey !== e.key) panel.scrollTop = 0;
  panelKey = e.key;
}

function entryDetails(e, where) {
  const { session: s, item } = e;
  const account = state.data.accounts.find((a) => a.name === e.account) || {};
  const timeline = s ? s.timeline.filter((t) => t.detail).reverse() : [];
  const later = item ? item.lastPrompts.filter((p) => p !== item.firstPrompt) : [];
  return h('div', { class: 'details' },
    h('div', { class: 'details-actions' },
      mainAction(e),
      e.command && h('button', { type: 'button', class: 'btn btn-small',
        onclick: (event) => copyText(e.command, event.currentTarget) }, 'Copy command'),
      e.sessionId && h('button', { type: 'button', class: 'btn btn-small', onclick: () => startRename(e, where) }, 'Rename'),
      pinButton(e)),
    h('dl', { class: 'facts' }, [
      ['Account', `${e.account}${account.email ? ` (${account.email})` : ''}`],
      ['Folder', e.cwd],
      ['Started', fullDate((s && s.startedAt) || (item && item.startedAt))],
      [e.open ? 'Updated' : s ? 'Closed' : 'Last activity', fullDate(e.at)],
      ['Last status', s && s.detail],
      ['Session', e.sessionId],
      ['Process', s && s.live && s.pid ? `pid ${s.pid}` : null],
    ].filter(([, value]) => value).map(([label, value]) => [h('dt', {}, label), h('dd', {}, value)])),
    timeline.length > 0 && h('div', { class: 'timeline' },
      h('h3', {}, 'Recent activity'),
      h('ol', {}, timeline.map((t) => h('li', {}, h('time', {}, ago(t.at)), h('span', {}, t.detail))))),
    item && item.firstPrompt && [h('h3', {}, 'First prompt'), h('p', { class: 'prompt' }, item.firstPrompt)],
    later.length > 0 && [h('h3', {}, 'Latest prompts'), later.map((p) => h('p', { class: 'prompt' }, p))],
    artifactList(e.artifacts),
    e.command
      ? [h('h3', {}, s && s.kind === 'background' ? 'Open it in a terminal' : 'Resume it in a terminal'), commandBlock(e.command)]
      : h('p', { class: 'quiet' }, 'This conversation has no folder recorded, so it cannot be resumed from here.'));
}

function sessionSummary(s) {
  return h('ul', { class: 'summary' },
    h('li', {}, h('strong', {}, s.name)),
    h('li', {}, s.cwd),
    h('li', {}, accountTag(s.account)));
}

async function closeOne(s) {
  const interactive = s.kind === 'interactive';
  // Windows cannot ask a terminal program to exit, so the server says when a close is forced.
  const graceful = (state.data.platform || {}).gracefulClose !== false;
  const ok = await showDialog({
    title: 'Close this session?',
    body: [sessionSummary(s), h('p', {}, !interactive
      ? 'It will be stopped. The conversation is kept and can be resumed.'
      : graceful
        ? 'It is open in a terminal. Closing ends it there. The conversation is kept and can be resumed.'
        : 'It is open in a terminal. Closing ends it there immediately, cutting off anything it is in the middle of. The conversation so far is kept and can be resumed.')],
    confirmLabel: 'Close session',
    danger: true,
  });
  if (!ok) return;
  const result = await act(() => api('/api/sessions/close', { keys: [s.key] }));
  if (result) reportClosed(result.results, [s]);
}

// Close several sessions after a look at the list. Interactive sessions are
// someone's open terminal, so they start unticked unless they were picked already.
async function closeMany({ title, where, note, sessions, tickInteractive = false }) {
  const boxes = sessions.map((s) => {
    const box = h('input', { type: 'checkbox', value: s.key, checked: tickInteractive || s.kind === 'background' });
    return h('label', { class: 'pick' }, box,
      h('span', { class: 'pick-name' }, s.name),
      accountTag(s.account),
      h('span', { class: 'row-kind' }, s.kind === 'interactive' ? 'Interactive' : 'Background'),
      statusBadge(s.status));
  });
  let keys = [];
  const ok = await showDialog({
    title,
    body: [where && h('p', { class: 'dialog-path' }, pathEl(where)), h('div', { class: 'picks' }, boxes), h('p', {}, note)],
    confirmLabel: 'Close ticked sessions',
    danger: true,
    onConfirm: (form) => {
      keys = [...form.querySelectorAll('input[type=checkbox]:checked')].map((box) => box.value);
      if (!keys.length) throw new Error('Tick at least one session, or cancel.');
    },
  });
  if (!ok) return;
  const result = await act(() => api('/api/sessions/close', { keys }));
  if (result) reportClosed(result.results, sessions);
  for (const key of keys) state.ticked.delete(key);
}

function reportClosed(results, sessions) {
  const name = (key) => (sessions.find((s) => s.key === key) || {}).name || key;
  const closed = results.filter((r) => r.ok);
  if (closed.length === 1) toast(`Closed ${name(closed[0].key)}.`);
  else if (closed.length) toast(`Closed ${closed.length} sessions.`);
  for (const failure of results.filter((r) => !r.ok)) {
    toast(`Could not close ${name(failure.key)}: ${failure.message}`, 'error');
  }
}

function resumeSession(s) {
  return act(() => api('/api/sessions/resume', { key: s.key }), (r) => `Opened ${s.name} in ${r.terminal}.`);
}

function resumeHistory(item) {
  return act(() => api('/api/history/resume', { account: item.account, sessionId: item.sessionId }),
    (r) => `Opened in ${r.terminal}.`);
}

// -- alerts

// Desktop notifications when a session starts waiting. They need the page open
// in some tab, because the page is what notices the change.
function alertsOn() {
  return state.alerts && 'Notification' in window && Notification.permission === 'granted';
}

function showAlerts() {
  const button = $('#alerts');
  if (!('Notification' in window)) {
    button.hidden = true;
    return;
  }
  button.setAttribute('aria-pressed', String(alertsOn()));
  button.textContent = alertsOn() ? 'Alerts on' : 'Alerts off';
  button.title = alertsOn()
    ? 'A notification appears when a session is waiting for you or finishes, while this page is open in a tab.'
    : 'Get a notification when a session is waiting for you or finishes.';
}

async function toggleAlerts() {
  if (alertsOn()) {
    state.alerts = false;
  } else {
    const permission = Notification.permission === 'default' ? await Notification.requestPermission() : Notification.permission;
    if (permission !== 'granted') {
      toast('Notifications are blocked for this page. Allow them in the browser\'s site settings, then try again.', 'error');
      state.alerts = false;
    } else {
      state.alerts = true;
      toast('Alerts on. Keep this page open in a tab to get them.');
    }
  }
  store('cc-alerts', state.alerts ? 'on' : 'off');
  showAlerts();
}

// Compare the sessions before and after a refresh and tell about each one that
// has just started waiting. Nothing is said on the first load.
function noticeChanges(before, after) {
  if (!before) return;
  const was = new Map(before.map((s) => [s.key, s.status]));
  for (const s of after) {
    const old = was.get(s.key);
    if (!old || old === s.status || !needsYou(s)) continue;
    const what = s.status === 'blocked' ? `${s.name} is waiting for you` : `${s.name} has finished`;
    const key = s.sessionId ? `${s.account}|${s.sessionId}` : `s:${s.key}`;
    if (!document.hidden) {
      toast(`${what}.`);
    } else if (alertsOn()) {
      const note = new Notification(what, { body: `${s.cwd} · ${s.account}`, tag: s.key });
      note.onclick = () => {
        window.focus();
        note.close();
        go({ tab: 'sessions' });
        select(key);
      };
    }
  }
}

// -- accounts

function renderAccounts() {
  const active = activeAccounts();
  const removed = state.data.accounts.filter((a) => a.removed);
  // Through h() so the conditional sections that are off drop out.
  $('#list').replaceChildren(...h('div', {},
    h('div', { class: 'toolbar' },
      h('p', { class: 'count' }, plural(active.length, 'account')),
      h('button', { type: 'button', class: 'btn btn-primary', onclick: () => addAccount() }, 'Add account')),
    state.aliasNotice && h('p', { class: 'notice notice-action' },
      h('span', {}, `Shortcuts changed. ${(state.data.platform || {}).aliasHint || 'Open a new terminal to use them.'}`),
      h('button', { type: 'button', class: 'btn btn-small', onclick: () => { state.aliasNotice = false; render(); } }, 'Dismiss')),
    h('div', { class: 'accounts' }, active.map(accountCard)),
    state.data.unregistered.length > 0 && h('section', { class: 'aside' },
      h('h2', {}, 'Folders not registered as accounts'),
      state.data.unregistered.map((folder) => h('div', { class: 'aside-row' },
        h('code', {}, folder),
        h('button', { type: 'button', class: 'btn btn-small', onclick: () => addAccount(folder) }, 'Add as account')))),
    removed.length > 0 && h('section', { class: 'aside' },
      h('h2', {}, 'Removed accounts'),
      h('p', { class: 'quiet' }, 'Their folders, logins and history are still on disk. Only you can delete them.'),
      removed.map((a) => h('div', { class: 'aside-row' },
        accountTag(a.name), h('code', {}, a.dir),
        h('button', { type: 'button', class: 'btn btn-small', onclick: () => restoreAccount(a) }, 'Restore'))))).childNodes);
}

function accountCard(a) {
  const start = a.alias || (a.isDefault ? 'claude' : null);
  return h('article', { class: 'account', style: { '--acct': accountColor(a.name) } },
    h('header', { class: 'account-head' },
      h('h2', {}, a.name),
      h('span', { class: 'account-open' }, plural(a.openSessions, 'open session')),
      h('button', { type: 'button', class: 'icon-btn', 'aria-label': `Edit ${a.name}`, title: 'Edit name, alias, colour or login',
        onclick: () => editAccount(a) }, icon('edit'))),
    h('dl', { class: 'facts' },
      h('dt', {}, 'Login'),
      h('dd', {}, a.loggedIn ? a.email : h('span', { class: 'warn' }, 'Not logged in')),
      a.loggedIn && [h('dt', {}, 'Organisation'), h('dd', {}, a.org || 'None')],
      a.loggedIn && [h('dt', {}, 'Plan'), h('dd', {}, a.plan || 'Unknown')],
      h('dt', {}, 'Start with'),
      h('dd', {}, start ? h('code', {}, start) : 'No alias'),
      h('dt', {}, 'Folder'),
      h('dd', {}, h('code', {}, a.dir)),
      h('dt', { title: 'Claude Code deletes older conversations by itself (its cleanupPeriodDays setting)' }, 'History kept'),
      h('dd', {}, `${a.retentionDays} days`)),
    !a.loggedIn && h('div', { class: 'account-actions' },
      h('button', { type: 'button', class: 'btn btn-primary btn-small', onclick: () => login(a, false) }, 'Log in')));
}

// Swatches from the palette, any other colour from the system picker, and a
// way back to the colour the account gets by its place. `chosen` is '' for that one.
function colorField(a, chosen, onPick) {
  const auto = autoColor(state.data.accounts.findIndex((x) => x.name === a.name));
  const group = h('div', { class: 'swatches', role: 'group', 'aria-label': 'Colour' });
  const draw = () => {
    const current = chosen || auto;
    const custom = !ACCOUNT_COLORS.includes(current);
    const pick = (color) => { chosen = color; onPick(color); draw(); };
    group.replaceChildren(
      ...ACCOUNT_COLORS.map((color) => h('button', { type: 'button', class: 'swatch', style: { '--swatch': color },
        'aria-pressed': String(color === current), 'aria-label': color,
        onclick: () => pick(color === auto ? '' : color) })),
      h('label', { class: `swatch swatch-custom${custom ? ' is-on' : ''}`, style: custom ? { '--swatch': current } : {},
        title: 'Any other colour' },
        h('input', { type: 'color', value: current, 'aria-label': 'Any other colour', onchange: (event) => pick(event.target.value) })),
      chosen && h('button', { type: 'button', class: 'btn-link', onclick: () => pick('') }, 'Automatic'));
  };
  draw();
  return h('div', { class: 'field' },
    h('span', { class: 'field-label' }, 'Colour'),
    group,
    h('span', { class: 'field-hint' }, 'Marks this account everywhere in Claude Sessions.'));
}

async function addAccount(folder = '') {
  const suggested = folder ? folder.replace(/^.*\/\.claude-/, '').toLowerCase().replace(/[^a-z0-9-]/g, '-') : '';
  const name = field('Name', 'name', suggested, 'Lowercase letters, digits and dashes. Shown everywhere in this app.');
  const alias = field('Alias', 'alias', suggested ? `claude-${suggested}` : '', 'The command you type to start Claude Code as this account.');
  const dir = field('Folder', 'folder', folder, 'Where this account keeps its login and history. An existing folder is used as it is.');
  const [nameInput, aliasInput, dirInput] = [name, alias, dir].map((el) => el.querySelector('input'));
  // Fill alias and folder from the name until the person edits them.
  let aliasEdited = false;
  let dirEdited = Boolean(folder);
  aliasInput.addEventListener('input', () => { aliasEdited = true; });
  dirInput.addEventListener('input', () => { dirEdited = true; });
  nameInput.addEventListener('input', () => {
    if (!aliasEdited) aliasInput.value = nameInput.value ? `claude-${nameInput.value}` : '';
    if (!dirEdited) dirInput.value = nameInput.value ? `~/.claude-${nameInput.value}` : '';
  });
  const added = await showDialog({
    title: 'Add account',
    body: [name, alias, dir],
    confirmLabel: 'Add account',
    onConfirm: () => api('/api/accounts/add', {
      name: nameInput.value.trim(), alias: aliasInput.value.trim(), folder: dirInput.value.trim(),
    }),
  });
  if (!added) return;
  state.aliasNotice = true;
  toast(`Added ${added.name}. ${added.created ? 'Log in to finish setting it up.' : `Using the existing folder ${added.dir}.`}`);
  refresh();
}

// Everything about an account that can be changed, in one dialog. Changing the
// login and removing the account are their own steps, with their own warnings.
async function editAccount(a) {
  const name = field('Name', 'name', a.name, 'Lowercase letters, digits and dashes.');
  const alias = field('Alias', 'alias', a.alias || '', a.isDefault
    ? 'Optional. Plain claude already starts this account.'
    : 'The command you type to start Claude Code as this account.');
  const [nameInput, aliasInput] = [name, alias].map((el) => el.querySelector('input'));
  // An alias that follows the claude-NAME pattern keeps following the name.
  let aliasFollows = a.alias === `claude-${a.name}`;
  aliasInput.addEventListener('input', () => { aliasFollows = false; });
  nameInput.addEventListener('input', () => {
    if (aliasFollows) aliasInput.value = `claude-${nameInput.value}`;
  });
  let color = a.color || '';
  const result = await showDialog({
    title: `Edit ${a.name}`,
    body: [name, alias, colorField(a, color, (picked) => { color = picked; }),
      h('p', { class: 'quiet' }, `The folder ${a.dir} is not moved and nothing inside it changes.`)],
    confirmLabel: 'Save changes',
    aside: [
      a.loggedIn && { label: 'Change login', value: 'login' },
      { label: 'Remove account', value: 'remove', danger: true },
    ],
    onConfirm: async () => {
      const newName = nameInput.value.trim();
      const newAlias = aliasInput.value.trim();
      // The colour first, under the name the account has now.
      if (color !== (a.color || '')) {
        await api('/api/accounts/color', { name: a.name, color });
        a.color = color;
      }
      if (newName === a.name && newAlias === (a.alias || '')) return { saved: true };
      return api('/api/accounts/rename', { name: a.name, newName, alias: newAlias });
    },
  });
  if (result === 'login') return login(a, true);
  if (result === 'remove') return removeAccount(a);
  if (!result) return refresh();
  if (result.alias !== undefined && result.alias !== a.alias) state.aliasNotice = true;
  toast('Saved changes.');
  refresh();
}

async function login(a, relogin) {
  if (relogin) {
    const live = state.data.sessions.filter((s) => s.account === a.name && s.live);
    const ok = await showDialog({
      title: `Change the login for ${a.name}?`,
      body: [h('p', {}, `It is logged in as ${a.email}. A terminal window opens, logs that out, and asks you to sign in again.`),
        h('p', {}, 'History, settings and memory in its folder are kept.'),
        live.length > 0 && h('p', { class: 'warn' }, `${plural(live.length, 'running session')} of this account will lose the login:`),
        live.length > 0 && h('ul', { class: 'summary' }, live.map((s) => h('li', {}, `${s.name} in ${s.cwd}`)))],
      confirmLabel: 'Change login',
      danger: true,
    });
    if (!ok) return;
  }
  await act(() => api('/api/accounts/login', { name: a.name, relogin }),
    (r) => `Opened ${r.terminal}. Finish signing in there; this page updates when it is done.`);
}

async function removeAccount(a) {
  const open = state.data.sessions.filter((s) => s.account === a.name && s.open);
  const ok = await showDialog({
    title: `Remove ${a.name}?`,
    body: [h('p', {}, `${a.name} is hidden from this app and its alias is taken away. You can restore it at any time.`),
      h('p', {}, `Nothing is deleted: the folder ${a.dir}, its login and its history stay on disk.`),
      open.length > 0 && h('p', { class: 'warn' }, `${plural(open.length, 'open session')} of this account will keep running but will no longer be shown here.`)],
    confirmLabel: 'Remove account',
    danger: true,
    onConfirm: () => api('/api/accounts/remove', { name: a.name }),
  });
  if (!ok) return;
  if (a.alias) state.aliasNotice = true;
  state.history = null;
  toast(`Removed ${a.name}. Its folder and data are untouched.`);
  refresh();
}

async function restoreAccount(a) {
  const restored = await act(() => api('/api/accounts/restore', { name: a.name }), `Restored ${a.name}.`);
  if (restored) {
    state.aliasNotice = true;
    state.history = null;
  }
}

// ---------------------------------------------------------------------- start

for (const button of document.querySelectorAll('.tab')) {
  button.addEventListener('click', () => go({ tab: button.dataset.tab }));
}
$('#disconnect').addEventListener('click', disconnect);
$('#alerts').addEventListener('click', toggleAlerts);
showAlerts();

// The view is in the address (#accounts, #needs, #f=<folder>), so Back steps through it.
addEventListener('popstate', () => {
  const tab = state.tab;
  readHash();
  store('cc-tab', state.tab);
  store('cc-folder', state.folder);
  if (tab === state.tab && $('#list')) render();
  else show();
});
wide.addEventListener('change', () => mount());
document.addEventListener('keydown', (event) => {
  if (event.key === 'Escape' && state.selected && !state.renaming && !$('#dialog').open) select(null);
});

// Theme: follow the system unless Light or Dark was picked. theme.js applies the
// saved choice before first paint; this keeps the picker and the choice in step.
const themePicker = $('#theme');
themePicker.value = document.documentElement.dataset.theme || 'system';
themePicker.addEventListener('change', () => {
  const theme = themePicker.value;
  if (theme === 'system') delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = theme;
  store('cc-theme', theme);
});

if (location.hash) {
  readHash();
} else {
  state.tab = store('cc-tab') === 'accounts' ? 'accounts' : 'sessions';
  state.folder = store('cc-folder') || '';
  history.replaceState(null, '', location.pathname + viewHash());
}
show();
if (!token) setOffline('token');
refresh();

// A hidden page keeps an eye out every third time when alerts are on, and rests otherwise.
let ticks = 0;
setInterval(() => { if (!document.hidden || (alertsOn() && ++ticks % 3 === 0)) refresh(); }, POLL_MS);
// The server stops itself when no page has been open for a while. A hidden tab
// is still an open page, so it checks in now and then without fetching anything.
setInterval(() => { if (document.hidden && !state.offline) api('/api/ping').catch(() => {}); }, 5 * 60 * 1000);
setInterval(() => {
  if (!document.hidden && !state.offline && state.tab === 'sessions' && state.history) loadHistory();
}, 60000);
// Relative times drift even when nothing changes; redraw them once a minute.
setInterval(() => { if (!document.hidden && !$('#dialog').open) render(); }, 60000);
document.addEventListener('visibilitychange', () => { if (!document.hidden) refresh(); });
