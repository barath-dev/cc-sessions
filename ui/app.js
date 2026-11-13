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
  folder: store('cc-folder') || '',   // the folder picked in the sidebar; '' is all of them
  collapsed: new Set(storedList('cc-collapsed')),
  expanded: new Set(),
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

function accountColor(name) {
  const index = state.data ? state.data.accounts.findIndex((a) => a.name === name) : -1;
  return ACCOUNT_COLORS[(index < 0 ? 0 : index) % ACCOUNT_COLORS.length];
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

function commandBlock(command) {
  const button = h('button', { type: 'button', class: 'btn btn-small', onclick: async () => {
    try {
      await navigator.clipboard.writeText(command);
      button.textContent = 'Copied';
      setTimeout(() => { button.textContent = 'Copy'; }, 1500);
    } catch {
      toast('Could not copy. Select the command and copy it by hand.', 'error');
    }
  } }, 'Copy');
  return h('div', { class: 'command-row' }, h('pre', { class: 'command' }, command), button);
}

// ----------------------------------------------------------------------- API

let token = new URLSearchParams(location.search).get('t');
if (token) {
  store('cc-token', token);
  history.replaceState(null, '', location.pathname);   // keep the token out of the address bar
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

function showDialog({ title, body = [], confirmLabel = 'Confirm', danger = false, onConfirm }) {
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
  const signature = JSON.stringify([data.accounts, data.sessions, data.unregistered]);
  if (signature === state.signature) return;
  const names = (d) => (d ? d.accounts.map((a) => `${a.name}:${a.removed}`).join() : '');
  const accountsChanged = names(data) !== names(state.data);
  state.signature = signature;
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

function setTab(tab) {
  state.tab = tab;
  store('cc-tab', tab);
  for (const button of document.querySelectorAll('.tab')) {
    button.setAttribute('aria-current', button.dataset.tab === tab ? 'page' : 'false');
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
  if (state.tab === 'sessions') {
    view.replaceChildren(h('div', { class: 'browser' },
      h('nav', { id: 'side', class: 'side', 'aria-label': 'Folders' }),
      h('div', { class: 'pane' },
        h('header', { id: 'pane-head', class: 'pane-head' }),
        toolbar(),
        h('div', { id: 'chips', class: 'chips' }), h('div', { id: 'notices' }), h('div', { id: 'list' }))));
    if (!state.history || Date.now() - state.historyAt > 30000) loadHistory();
  } else {
    view.replaceChildren(h('div', { id: 'list' }));
  }
  render();
}

function render() {
  if (!state.data) return;
  if (!$('#list')) return mount();
  if (state.tab === 'sessions') renderSessions();
  else renderAccounts();
}

function toolbar() {
  const filters = state.filters;
  const placeholder = 'Search name, folder or artifact';
  return h('div', { class: 'toolbar' },
    h('input', { type: 'search', class: 'search', placeholder, 'aria-label': placeholder, value: filters.q,
      oninput: (e) => { filters.q = e.target.value; render(); } }),
    h('select', { 'aria-label': 'Account', onchange: (e) => { filters.account = e.target.value; render(); } },
      h('option', { value: '' }, 'All accounts'),
      activeAccounts().map((a) => h('option', { value: a.name, selected: a.name === filters.account }, a.name))),
    h('button', { type: 'button', class: 'btn', onclick: () => { loadHistory(); refresh(); } }, 'Refresh'));
}

function toggle(key) {
  if (state.expanded.has(key)) state.expanded.delete(key);
  else state.expanded.add(key);
  render();
}

function details(pairs, extra = []) {
  return h('div', { class: 'details' },
    h('dl', { class: 'facts' }, pairs.filter(([, value]) => value).map(([label, value]) =>
      [h('dt', {}, label), h('dd', {}, value)])),
    extra);
}

function artifactList(artifacts) {
  if (!artifacts || !artifacts.length) return null;
  return [h('h3', {}, 'Artifacts'),
    h('ul', { class: 'artifacts' }, artifacts.map((a) => h('li', {},
      h('a', { href: a.url, target: '_blank', rel: 'noopener noreferrer' }, a.title),
      h('time', { title: fullDate(a.updatedAt) }, ago(a.updatedAt))))),
    h('p', { class: 'quiet' }, 'They open on claude.ai, for the Claude login that published them.')];
}

async function renameSession({ account, sessionId, name, renamed }) {
  const title = field('Name', 'title', name, renamed
    ? 'Shown in Claude Sessions only. Clear it to go back to Claude Code\'s own title.'
    : 'Shown in Claude Sessions only. Claude Code keeps its own title.');
  const input = title.querySelector('input');
  const saved = await showDialog({
    title: 'Rename session',
    body: [title],
    confirmLabel: 'Save name',
    onConfirm: () => {
      const value = input.value.trim();
      // Saving Claude Code's own title back would pin it; leave it to follow along.
      if (value === name || (!value && !renamed)) return false;
      return api('/api/sessions/rename', { account, sessionId, title: value });
    },
  });
  if (!saved) return;
  toast(saved.title ? `Renamed to ${saved.title}.` : 'Back to Claude Code\'s own title.');
  if (state.history) loadHistory();
  refresh();
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
  const cwd = (s && s.cwd) || (item && item.cwd) || '';
  return {
    key: s ? `s:${s.key}` : `h:${item.account}:${item.sessionId}`,
    session: s,
    item,
    account: (s || item).account,
    sessionId: (s || item).sessionId,
    name: s ? s.name : item.title,
    renamed: (s || item).renamed,
    cwd,
    folder: folderId(cwd),
    status: s ? s.status : 'closed',
    open: Boolean(s && s.open),
    at: Math.max((s && (s.closedAt || s.updatedAt)) || 0, (item && item.lastActivity) || 0),
    artifacts: (s && s.artifacts) || (item && item.artifacts) || [],
  };
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

function selectFolder(id) {
  state.folder = id;
  store('cc-folder', id);
  render();
}

function renderSide(entries, tree) {
  const rows = [];
  const walk = (node, depth) => {
    rows.push(sideRow(node, depth));
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
  $('#side').replaceChildren(
    sideRow({ id: '', label: 'All sessions', kids: [], all: entries }, 0),
    h('div', { class: 'side-head' },
      h('h2', { class: 'side-heading' }, 'Folders'),
      parents.length > 0 && h('button', { type: 'button', class: 'side-all', onclick: () => {
        state.collapsed = new Set(allClosed ? [] : parents);
        store('cc-collapsed', JSON.stringify(parents.filter((id) => state.collapsed.has(id))));
        render();
      } }, allClosed ? 'Expand all' : 'Collapse all')),
    ...rows);
}

function sideRow(node, depth) {
  const open = node.all.filter((e) => e.open);
  // The dot takes the colour of the most pressing open session in the folder.
  const pressing = ['blocked', 'working'].find((status) => open.some((e) => e.status === status)) || 'idle';
  const collapsed = state.collapsed.has(node.id);
  const selected = state.folder === node.id;
  return h('div', { class: `side-row${selected ? ' is-selected' : ''}`, style: { '--depth': String(depth) } },
    node.kids.length
      ? h('button', { type: 'button', class: 'side-twist', 'aria-expanded': String(!collapsed),
        'aria-label': `${collapsed ? 'Show' : 'Hide'} the folders in ${node.label}`, onclick: () => {
          if (collapsed) state.collapsed.delete(node.id);
          else state.collapsed.add(node.id);
          store('cc-collapsed', JSON.stringify([...state.collapsed]));
          render();
        } })
      : h('span', { class: 'side-twist' }),
    h('button', { type: 'button', class: 'side-item', 'aria-current': selected ? 'true' : null,
      title: folderPath(node.id) || null, onclick: () => selectFolder(node.id) },
      node.id && h('span', { class: 'side-icon' }),
      h('span', { class: 'side-name' }, node.label),
      open.length > 0 && h('span', { class: 'side-open', 'data-status': pressing, title: `${open.length} open` }),
      h('span', { class: 'side-count' }, String(node.all.length))));
}

function inFilter(e, filter) {
  if (filter === 'all') return true;
  if (filter === 'open') return e.open;
  return e.status === filter;
}

function renderSessions() {
  const filters = state.filters;
  const all = allEntries().filter((e) => !filters.account || e.account === filters.account);
  const tree = folderTree(all);
  const node = state.folder ? findFolder(tree, state.folder) : null;
  // A picked folder with nothing left in it falls back to everything, once the saved conversations are in.
  if (state.folder && !node && state.history) {
    state.folder = '';
    store('cc-folder', '');
  }
  const scope = node ? all.filter((e) => inFolder(e, node.id)) : all;
  renderSide(all, tree);

  const open = scope.filter((e) => e.open);
  $('#pane-head').replaceChildren(...[
    h('h2', {}, node ? pathEl(folderPath(node.id)) : 'All sessions'),
    h('span', { class: 'count' }, `${plural(scope.length, 'session')}${open.length ? `, ${open.length} open` : ''}`),
    node && open.length > 1 && h('button', { type: 'button', class: 'btn btn-small',
      onclick: () => closeFolder(folderPath(node.id), open.map((e) => e.session)) }, 'Close all open here'),
  ].filter(Boolean));

  const needle = filters.q.trim().toLowerCase();
  const visible = scope.filter((e) => !needle ||
    `${e.cwd} ${e.name} ${e.item ? e.item.title : ''} ${e.sessionId} ${e.session ? e.session.id : ''} ${e.artifacts.map((a) => a.title).join(' ')}`
      .toLowerCase().includes(needle));

  $('#chips').replaceChildren(...STATUS_FILTERS.map((filter) => {
    const count = visible.filter((e) => inFilter(e, filter)).length;
    const label = filter === 'open' ? 'Open' : filter === 'all' ? 'All' : STATUS_LABELS[filter];
    return h('button', { type: 'button', class: 'chip', 'aria-pressed': String(filters.status === filter),
      'data-status': filter, title: STATUS_HINTS[filter] || '',
      onclick: () => { filters.status = filter; render(); } },
      label, h('span', { class: 'chip-count' }, String(count)));
  }));

  $('#notices').replaceChildren(...[
    ...activeAccounts().filter((a) => a.error).map((a) =>
      h('p', { class: 'notice notice-error' }, `Could not read sessions for ${a.name}: ${a.error}`)),
    state.historyError && h('p', { class: 'notice notice-error' }, `Could not read saved conversations: ${state.historyError}`),
  ].filter(Boolean));

  const reading = !state.history && !state.historyError;
  const shown = visible.filter((e) => inFilter(e, filters.status));
  if (!shown.length) {
    $('#list').replaceChildren(h('p', { class: 'empty' }, reading ? 'Reading saved conversations…'
      : all.length ? 'No sessions match these filters.'
        : 'No sessions yet. Start Claude Code in any folder and it shows up here.'));
    return;
  }

  // Open sessions first, finished background ones after them, then the rest by when it was last active.
  const heading = (e) => (!e.open ? dateGroup(e.at) : e.status === 'done' ? 'Done, not closed' : 'Open now');
  const rank = (e) => (!e.open ? 2 : e.status === 'done' ? 1 : 0);
  const groups = groupBy([0, 1, 2].flatMap((n) => shown.filter((e) => rank(e) === n)), heading);
  // Rows name their folder only when the list spans several, and then from the picked folder down.
  const several = new Set(scope.map((e) => e.folder)).size > 1;
  const folderOf = (e) => {
    if (!several || (node && e.folder === node.id)) return '';
    return node ? e.folder.slice(node.id.length + 1) : folderPath(e.folder);
  };
  const days = [...new Set(activeAccounts().map((a) => a.retentionDays))].sort((a, b) => a - b);
  const span = days.length > 1 ? `${days[0]} to ${days[days.length - 1]}` : `${days[0] || 30}`;
  $('#list').replaceChildren(
    ...groups.map(([name, entries]) => h('section', { class: 'group' },
      h('header', { class: 'group-head' },
        h('h2', { class: 'group-date' }, name),
        h('span', { class: 'count' }, String(entries.length))),
      entries.map((e) => entryRow(e, folderOf(e))))),
    h('p', { class: 'quiet list-note' }, reading ? 'Reading saved conversations…'
      : `Claude Code itself deletes conversations after ${span} days without activity ` +
        '(its cleanupPeriodDays setting, shown per account under Accounts). Claude Sessions never deletes any.'));
}

function entryRow(e, folder) {
  const key = `e:${e.key}`;
  const expanded = state.expanded.has(key);
  const { session: s, item } = e;
  const sub = [
    folder && pathEl(folder),
    s && s.kind === 'background' && h('span', { class: 'row-tag' }, 'Background'),
    item && h('span', { class: 'row-tag' }, plural(item.prompts, 'prompt')),
    e.artifacts.length > 0 && h('span', { class: 'row-tag' }, plural(e.artifacts.length, 'artifact')),
  ].filter(Boolean);
  return h('div', { class: `row${e.open ? '' : ' is-closed'}`, style: { '--acct': accountColor(e.account) } },
    h('button', { type: 'button', class: 'row-main', 'aria-expanded': String(expanded), onclick: () => toggle(key) },
      h('span', { class: 'row-title' },
        h('span', { class: 'row-name' }, e.name),
        sub.length > 0 && h('span', { class: 'row-sub' }, sub)),
      accountTag(e.account),
      !s && item.live
        ? h('span', { class: 'status', 'data-status': 'working', title: 'This conversation is open right now' }, 'Live')
        : statusBadge(e.status),
      h('span', { class: 'row-time', title: fullDate(e.at) }, recentDate(e.at))),
    h('div', { class: 'row-actions' },
      e.sessionId && h('button', { type: 'button', class: 'btn btn-small', onclick: () => renameSession(e) }, 'Rename'),
      e.open
        ? h('button', { type: 'button', class: 'btn btn-small', onclick: () => closeOne(s) }, 'Close')
        : s
          ? h('button', { type: 'button', class: 'btn btn-small', onclick: () => resumeSession(s) }, 'Resume')
          : item.resumeCommand && h('button', { type: 'button', class: 'btn btn-small', onclick: () => resumeHistory(item) }, 'Resume')),
    expanded && entryDetails(e));
}

function entryDetails(e) {
  const { session: s, item } = e;
  const account = state.data.accounts.find((a) => a.name === e.account) || {};
  const timeline = s ? s.timeline.filter((t) => t.detail).reverse() : [];
  const later = item ? item.lastPrompts.filter((p) => p !== item.firstPrompt) : [];
  const command = (s && s.resumeCommand) || (item && item.resumeCommand);
  return details([
    ['Account', `${e.account}${account.email ? ` (${account.email})` : ''}`],
    ['Folder', e.cwd],
    ['Started', fullDate((s && s.startedAt) || (item && item.startedAt))],
    [e.open ? 'Updated' : s ? 'Closed' : 'Last activity', fullDate(e.at)],
    ['Last status', s && s.detail],
    ['Session', e.sessionId],
    ['Process', s && s.live && s.pid ? `pid ${s.pid}` : null],
  ], [
    timeline.length > 0 && h('div', { class: 'timeline' },
      h('h3', {}, 'Recent activity'),
      h('ol', {}, timeline.map((t) => h('li', {}, h('time', {}, ago(t.at)), h('span', {}, t.detail))))),
    item && item.firstPrompt && [h('h3', {}, 'First prompt'), h('p', { class: 'prompt' }, item.firstPrompt)],
    later.length > 0 && [h('h3', {}, 'Latest prompts'), later.map((p) => h('p', { class: 'prompt' }, p))],
    artifactList(e.artifacts),
    command
      ? [h('h3', {}, s && s.kind === 'background' ? 'Open it in a terminal' : 'Resume it in a terminal'), commandBlock(command)]
      : h('p', { class: 'quiet' }, 'This conversation has no folder recorded, so it cannot be resumed from here.'),
  ]);
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

async function closeFolder(cwd, sessions) {
  const boxes = sessions.map((s) => {
    // Interactive sessions are someone's open terminal: opt in, one by one.
    const box = h('input', { type: 'checkbox', value: s.key, checked: s.kind === 'background' });
    return h('label', { class: 'pick' }, box,
      h('span', { class: 'pick-name' }, s.name),
      accountTag(s.account),
      h('span', { class: 'row-kind' }, s.kind === 'interactive' ? 'Interactive' : 'Background'),
      statusBadge(s.status));
  });
  let keys = [];
  const ok = await showDialog({
    title: 'Close sessions in this folder?',
    body: [h('p', { class: 'dialog-path' }, pathEl(cwd)),
      h('div', { class: 'picks' }, boxes),
      h('p', {}, 'Ticked sessions are stopped. Their conversations are kept and can be resumed. ' +
        'Interactive sessions are open in a terminal, so they start unticked.')],
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
      h('span', { class: 'account-open' }, plural(a.openSessions, 'open session'))),
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
    h('div', { class: 'account-actions' },
      !a.loggedIn && h('button', { type: 'button', class: 'btn btn-primary btn-small', onclick: () => login(a, false) }, 'Log in'),
      h('button', { type: 'button', class: 'btn btn-small', onclick: () => renameAccount(a) }, 'Rename'),
      a.loggedIn && h('button', { type: 'button', class: 'btn btn-small', onclick: () => login(a, true) }, 'Change login'),
      h('button', { type: 'button', class: 'btn btn-small', onclick: () => removeAccount(a) }, 'Remove')));
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

async function renameAccount(a) {
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
  const renamed = await showDialog({
    title: `Rename ${a.name}`,
    body: [name, alias, h('p', { class: 'quiet' }, `The folder ${a.dir} is not moved and nothing inside it changes.`)],
    confirmLabel: 'Save changes',
    onConfirm: () => api('/api/accounts/rename', {
      name: a.name, newName: nameInput.value.trim(), alias: aliasInput.value.trim(),
    }),
  });
  if (!renamed) return;
  if (renamed.alias !== a.alias) state.aliasNotice = true;
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
  button.addEventListener('click', () => setTab(button.dataset.tab));
}
$('#disconnect').addEventListener('click', disconnect);

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

setTab(store('cc-tab') === 'accounts' ? 'accounts' : 'sessions');
if (!token) setOffline('token');
refresh();

setInterval(() => { if (!document.hidden) refresh(); }, POLL_MS);
// The server stops itself when no page has been open for a while. A hidden tab
// is still an open page, so it checks in now and then without fetching anything.
setInterval(() => { if (document.hidden && !state.offline) api('/api/ping').catch(() => {}); }, 5 * 60 * 1000);
setInterval(() => {
  if (!document.hidden && !state.offline && state.tab === 'sessions' && state.history) loadHistory();
}, 60000);
// Relative times drift even when nothing changes; redraw them once a minute.
setInterval(() => { if (!document.hidden && !$('#dialog').open) render(); }, 60000);
document.addEventListener('visibilitychange', () => { if (!document.hidden) refresh(); });
