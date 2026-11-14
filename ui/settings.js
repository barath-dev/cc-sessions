'use strict';

// Settings, shared by the classic page and the Finder view: appearance, accounts
// and a few general choices, in one window. The page that loads this calls
// ccSettings.open(); it hears about changes as a 'cc-settings-changed' event.
//
// To add a template: add an entry to TEMPLATES below, and a block of colours for
// it in settings.css (one for light, one for dark).
(() => {
  const TEMPLATES = [
    { id: '', name: 'Each view’s own', note: 'Classic looks classic, Finder looks like a Mac.' },
    { id: 'mac', name: 'Mac', note: 'System font, soft greys, rounded corners.',
      light: ['#f2f2f4', '#ffffff', '#1d1d1f', '#6b6b70', '#0a6cdc'], dark: ['#1c1c1e', '#242426', '#e8e8ea', '#a1a1a6', '#2f8cff'] },
    { id: 'classic', name: 'Classic', note: 'The original look: cool blue-greys.',
      light: ['#eef1f4', '#ffffff', '#16202b', '#5c6b7a', '#2f5fd0'], dark: ['#11161c', '#19212a', '#e6ebf0', '#93a1af', '#86a8ff'] },
    { id: 'graphite', name: 'Graphite', note: 'Neutral greys, square corners, no colour cast.',
      light: ['#e8e9eb', '#f8f8f9', '#18191b', '#5d6066', '#4f5966'], dark: ['#121214', '#1b1c1f', '#e4e5e7', '#9a9da3', '#a7b0bc'] },
    { id: 'paper', name: 'Paper', note: 'Warm off-white, like a notebook.',
      light: ['#f3efe6', '#fffdf8', '#2b2620', '#6b6255', '#b0502a'], dark: ['#1a1815', '#24211d', '#ece6dc', '#a89e8f', '#e08a63'] },
  ];
  // Each reads at 4.5:1 or more on white; dark mode uses a lighter mix of it.
  const ACCENTS = [
    ['Blue', '#0070eb'], ['Purple', '#9a4fc9'], ['Pink', '#ca3e6f'], ['Red', '#d73537'],
    ['Orange', '#b55a08'], ['Green', '#278539'], ['Graphite', '#737378'],
  ];
  const ACCOUNT_COLORS = ['#0f9488', '#7c56d8', '#d0661c', '#c2417c', '#2f6fd0', '#4a8f2f', '#b08412', '#64768a'];
  const COLOR_NAMES = ['Teal', 'Purple', 'Orange', 'Pink', 'Blue', 'Green', 'Gold', 'Slate'];
  const SECTIONS = [['appearance', 'Appearance', 'palette'], ['accounts', 'Accounts', 'users'], ['general', 'General', 'sliders-horizontal']];
  const icon = (name) => window.ccIcon(name);

  // ---------------------------------------------------------------- helpers

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

  // Choices are kept in memory as well, which theme.js reads first, so they
  // still work for this page when the browser keeps no storage.
  const memory = window.ccMemory || (window.ccMemory = {});
  function store(key, value) {
    if (value === undefined) {
      if (key in memory) return memory[key];
      try { return localStorage.getItem(key); } catch { return null; }
    }
    memory[key] = value === '' ? null : value;
    try {
      if (value === '') localStorage.removeItem(key);
      else localStorage.setItem(key, value);
    } catch { /* no storage: the choice lasts until the page closes */ }
    return null;
  }

  const plural = (n, word) => `${n} ${word}${n === 1 ? '' : 's'}`;

  // While the window is open its messages show inside it, above its backdrop.
  function toast(message, kind = 'ok') {
    const box = win && win.open ? toasts : document.getElementById('toasts');
    if (!box) return;
    const el = h('div', { class: `toast toast-${kind}` }, message);
    box.append(el);
    setTimeout(() => el.remove(), kind === 'error' ? 9000 : 4500);
  }

  async function api(path, body) {
    const response = await fetch(path, {
      method: body ? 'POST' : 'GET',
      headers: { 'X-CC-Token': window.ccToken || store('cc-token') || '', ...(body ? { 'Content-Type': 'application/json' } : {}) },
      body: body ? JSON.stringify(body) : undefined,
    });
    const data = await response.json().catch(() => ({}));
    if (!response.ok) throw new Error(data.error || `Request failed (${response.status}).`);
    return data;
  }

  // `kind` is 'appearance' (only the look changed) or 'accounts' (the data did).
  const changed = (kind) => window.dispatchEvent(new CustomEvent('cc-settings-changed', { detail: { kind } }));

  // ------------------------------------------------------------- the window

  let win = null;
  let section = store('cc-settings-section') || 'appearance';
  let data = null;            // /api/state, for the accounts
  let aliasNotice = false;
  let poll = null;
  const toasts = h('div', { class: 'st-toasts', role: 'status', 'aria-live': 'polite' });

  function open(which) {
    if (which) section = which;
    if (!win) {
      win = h('dialog', { class: 'st-window', 'aria-labelledby': 'st-title' });
      win.addEventListener('close', () => {
        clearInterval(poll);
        win.remove();
        win = null;
        window.dispatchEvent(new CustomEvent('cc-settings-closed'));
      });
      document.body.append(win);
      // A login finishes in a terminal; keep the accounts current meanwhile.
      poll = setInterval(() => {
        if (win && section === 'accounts' && !document.querySelector('.st-dialog[open]')) load();
      }, 5000);
    }
    draw();
    if (!win.open) win.showModal();
    if (section === 'accounts') load();
  }

  function close() {
    if (win) win.close();
  }

  function go(which) {
    section = which;
    store('cc-settings-section', which);
    draw();
    if (which === 'accounts') load();
  }

  async function load() {
    try {
      data = await api('/api/state');
    } catch (err) {
      data = { error: err.message };
    }
    if (win && section === 'accounts') draw();
  }

  // Redraws keep focus on the same control (marked with data-fk) and the same scroll.
  function draw() {
    if (!win) return;
    const focused = document.activeElement && win.contains(document.activeElement) ? document.activeElement.dataset.fk : null;
    const scroll = win.querySelector('.st-body')?.scrollTop || 0;
    const title = SECTIONS.find(([id]) => id === section)?.[1] || 'Settings';
    const body = section === 'appearance' ? appearance() : section === 'accounts' ? accounts() : general();
    win.replaceChildren(
      h('nav', { class: 'st-side', 'aria-label': 'Settings sections' },
        h('h2', { class: 'st-title', id: 'st-title' }, 'Settings'),
        SECTIONS.map(([id, label, glyph]) => h('button', { type: 'button', class: 'st-nav', 'data-fk': `nav:${id}`, 'aria-current': id === section ? 'true' : null, onclick: () => go(id) }, icon(glyph), label))),
      h('div', { class: 'st-main' },
        h('header', { class: 'st-head' },
          h('h3', {}, title),
          h('button', { type: 'button', class: 'st-close', 'data-fk': 'close', 'aria-label': 'Close settings', title: 'Close (Esc)', onclick: close }, icon('x'))),
        h('div', { class: 'st-body' }, body)),
      toasts);
    const bodyEl = win.querySelector('.st-body');
    if (bodyEl) bodyEl.scrollTop = scroll;
    if (focused) win.querySelector(`[data-fk="${CSS.escape(focused)}"]`)?.focus();
  }

  // ------------------------------------------------------------- appearance

  function setLook(key, value) {
    store(key, value);
    if (window.ccApplyAppearance) window.ccApplyAppearance();
    changed('appearance');
    draw();
  }

  function appearance() {
    const theme = store('cc-theme') || 'system';
    const template = store('cc-template') || '';
    const accent = store('cc-accent') || '';
    const dark = document.documentElement.dataset.theme === 'dark'
      || (document.documentElement.dataset.theme !== 'light' && matchMedia('(prefers-color-scheme: dark)').matches);
    const radio = (name, value, current, label, onchange, glyph) => h('label', { class: 'st-choice' },
      h('input', { type: 'radio', name, value, 'data-fk': `${name}:${value}`, checked: value === current, onchange }), glyph && icon(glyph), h('span', {}, label));
    // Which page shows the sessions. Picking the other one goes there, and Settings opens again on it.
    const layout = location.pathname === '/' ? 'classic' : 'finder';
    const switchTo = (value) => {
      store('cc-view', value);
      if (value === layout) return;
      try { sessionStorage.setItem('cc-settings-reopen', 'appearance'); } catch { /* it just doesn't reopen */ }
      location.replace(value === 'finder' ? '/finder' : '/');
    };

    // A small picture of each template, in the current light or dark.
    const preview = (t) => {
      // "Each view's own" shows the look of the view this is open in.
      const own = TEMPLATES[location.pathname === '/' ? 2 : 1];
      const [ground, surface, ink, muted, accentColor] = (t.id ? t : own)[dark ? 'dark' : 'light'];
      return h('span', { class: 'st-mini', style: { background: ground, 'border-color': muted } },
        h('span', { class: 'st-mini-side', style: { background: ground } },
          h('span', { style: { background: accentColor } }), h('span', { style: { background: muted } }), h('span', { style: { background: muted } })),
        h('span', { class: 'st-mini-main', style: { background: surface } },
          h('span', { style: { background: ink } }), h('span', { style: { background: muted } }), h('span', { style: { background: muted } })));
    };

    return [
      h('section', { class: 'st-group' },
        h('h4', {}, 'Layout'),
        h('div', { class: 'st-choices', role: 'radiogroup', 'aria-label': 'Layout' },
          radio('st-layout', 'classic', layout, 'Classic: one list, with details beside it', () => switchTo('classic'), 'rows-3'),
          radio('st-layout', 'finder', layout, 'Finder: folders you open, like a Mac', () => switchTo('finder'), 'folder-open')),
        h('p', { class: 'st-hint' }, 'The page opens in this layout from now on.')),
      h('section', { class: 'st-group' },
        h('h4', {}, 'Theme'),
        h('div', { class: 'st-choices', role: 'radiogroup', 'aria-label': 'Theme' },
          radio('st-theme', 'light', theme, 'Light', () => setLook('cc-theme', 'light'), 'sun'),
          radio('st-theme', 'dark', theme, 'Dark', () => setLook('cc-theme', 'dark'), 'moon'),
          radio('st-theme', 'system', theme, 'Match system', () => setLook('cc-theme', 'system'), 'monitor'))),
      h('section', { class: 'st-group' },
        h('h4', {}, 'Template'),
        h('div', { class: 'st-templates', role: 'radiogroup', 'aria-label': 'Template' }, TEMPLATES.map((t) => h('label', { class: 'st-template' },
          h('input', { type: 'radio', name: 'st-template', value: t.id, 'data-fk': `st-template:${t.id}`, checked: t.id === template, onchange: () => setLook('cc-template', t.id) }),
          preview(t),
          h('span', { class: 'st-template-name' }, t.name),
          h('span', { class: 'st-template-note' }, t.note))))),
      h('section', { class: 'st-group' },
        h('h4', {}, 'Primary colour'),
        h('p', { class: 'st-hint' }, 'Buttons, links and what is selected. Leave it on the template’s own, or pick one.'),
        h('div', { class: 'st-swatches', role: 'radiogroup', 'aria-label': 'Primary colour' },
          h('label', { class: 'st-swatch st-swatch-auto', title: 'The template’s own' },
            h('input', { type: 'radio', name: 'st-accent', 'data-fk': 'st-accent:', checked: !accent, onchange: () => setLook('cc-accent', '') }), h('span', {}, 'Auto')),
          ACCENTS.map(([name, hex]) => h('label', { class: 'st-swatch', title: name, style: { '--swatch': hex } },
            h('input', { type: 'radio', name: 'st-accent', 'data-fk': `st-accent:${hex}`, 'aria-label': name, checked: accent === hex, onchange: () => setLook('cc-accent', hex) }))),
          h('label', { class: `st-swatch st-swatch-custom${accent && !ACCENTS.some(([, hex]) => hex === accent) ? ' is-on' : ''}`, title: 'Any other colour',
            style: accent ? { '--swatch': accent } : {} },
            h('input', { type: 'color', value: accent || '#0070eb', 'data-fk': 'st-accent:custom', 'aria-label': 'Any other colour', onchange: (event) => setLook('cc-accent', event.target.value) })))),
      h('p', { class: 'st-hint' }, 'These are kept in this browser, and apply to both the classic page and the Finder view.'),
    ];
  }

  // --------------------------------------------------------------- accounts

  function accountColor(name) {
    const list = (data && data.accounts) || [];
    const index = list.findIndex((a) => a.name === name);
    return (index >= 0 && list[index].color) || ACCOUNT_COLORS[(index < 0 ? 0 : index) % ACCOUNT_COLORS.length];
  }

  function accounts() {
    if (!data) return h('p', { class: 'st-hint' }, 'Loading accounts…');
    if (data.error) return h('p', { class: 'st-error' }, `Could not load the accounts: ${data.error}`);
    const active = data.accounts.filter((a) => !a.removed);
    const removed = data.accounts.filter((a) => a.removed);
    return [
      h('div', { class: 'st-row-head' },
        h('p', { class: 'st-hint' }, plural(active.length, 'account')),
        h('button', { type: 'button', class: 'st-btn st-btn-primary', 'data-fk': 'add', onclick: () => addAccount() }, icon('user-plus'), 'Add account')),
      aliasNotice && h('p', { class: 'st-notice' },
        h('span', {}, `Shortcuts changed. ${(data.platform || {}).aliasHint || 'Open a new terminal to use them.'}`),
        h('button', { type: 'button', class: 'st-btn', onclick: () => { aliasNotice = false; draw(); } }, 'Dismiss')),
      h('div', { class: 'st-accounts' }, active.map(accountRow)),
      data.unregistered.length > 0 && h('section', { class: 'st-group' },
        h('h4', {}, 'Folders not registered as accounts'),
        data.unregistered.map((folder) => h('div', { class: 'st-line' }, h('code', {}, folder),
          h('button', { type: 'button', class: 'st-btn', onclick: () => addAccount(folder) }, icon('user-plus'), 'Add as account')))),
      removed.length > 0 && h('section', { class: 'st-group' },
        h('h4', {}, 'Removed accounts'),
        h('p', { class: 'st-hint' }, 'Their folders, logins and history are still on disk. Only you can delete them.'),
        removed.map((a) => h('div', { class: 'st-line' },
          h('span', { class: 'st-dot', style: { '--acct': accountColor(a.name) } }), h('strong', {}, a.name), h('code', {}, a.dir),
          h('button', { type: 'button', class: 'st-btn', onclick: () => restoreAccount(a) }, icon('rotate-ccw'), 'Restore')))),
    ];
  }

  function accountRow(a) {
    const start = a.alias || (a.isDefault ? 'claude' : null);
    return h('article', { class: 'st-account', style: { '--acct': accountColor(a.name) } },
      h('header', { class: 'st-account-head' },
        h('span', { class: 'st-dot' }),
        h('strong', {}, a.name),
        h('span', { class: 'st-hint' }, plural(a.openSessions, 'open session')),
        h('span', { class: 'st-spacer' }),
        !a.loggedIn && h('button', { type: 'button', class: 'st-btn st-btn-primary', 'data-fk': `login:${a.name}`, onclick: () => login(a, false) }, icon('log-in'), 'Log in'),
        h('button', { type: 'button', class: 'st-btn', 'data-fk': `edit:${a.name}`, 'aria-label': `Edit ${a.name}`, onclick: () => editAccount(a) }, icon('pencil'), 'Edit…')),
      h('dl', { class: 'st-facts' },
        h('dt', {}, 'Login'), h('dd', {}, a.loggedIn ? a.email : h('span', { class: 'st-warn' }, 'Not logged in')),
        a.loggedIn && [h('dt', {}, 'Plan'), h('dd', {}, [a.plan || 'Unknown', a.org ? ` · ${a.org}` : ''])],
        h('dt', {}, 'Start with'), h('dd', {}, start ? h('code', {}, start) : 'No alias'),
        h('dt', {}, 'Folder'), h('dd', {}, h('code', {}, a.dir)),
        h('dt', { title: 'Claude Code deletes older conversations by itself (its cleanupPeriodDays setting)' }, 'History kept'), h('dd', {}, `${a.retentionDays} days`)));
  }

  // A dialog over the settings window. `aside` are other ways out: { label, value, danger }.
  function ask({ title, body = [], confirmLabel = 'Confirm', danger = false, onConfirm, aside = [] }) {
    return new Promise((resolve) => {
      const dialog = h('dialog', { class: 'st-dialog', 'aria-labelledby': 'st-dialog-title' });
      const error = h('p', { class: 'st-error', hidden: true, role: 'alert' });
      const confirm = h('button', { type: 'submit', class: `st-btn ${danger ? 'st-btn-danger' : 'st-btn-primary'}` }, confirmLabel);
      let done = false;
      let saving = false;
      const finish = (value) => {
        if (done) return;
        done = true;
        if (dialog.open) dialog.close();
        dialog.remove();
        resolve(value);
      };
      const busy = (on) => {
        saving = on;
        for (const button of dialog.querySelectorAll('button')) button.disabled = on;
      };
      const form = h('form', { class: 'st-form', onsubmit: async (event) => {
        event.preventDefault();
        busy(true);
        error.hidden = true;
        try {
          const result = onConfirm ? await onConfirm() : true;
          finish(result === undefined ? true : result);
        } catch (err) {
          error.textContent = err.message;
          error.hidden = false;
          busy(false);
        }
      } },
        h('h3', { id: 'st-dialog-title' }, title),
        body,
        error,
        h('div', { class: 'st-actions' },
          aside.filter(Boolean).length > 0 && h('div', { class: 'st-aside' }, aside.filter(Boolean).map((other) =>
            h('button', { type: 'button', class: `st-btn st-btn-text${other.danger ? ' st-btn-text-danger' : ''}`, onclick: () => finish(other.value) }, other.label))),
          h('button', { type: 'button', class: 'st-btn', onclick: () => finish(false) }, 'Cancel'),
          confirm));
      dialog.append(form);
      // Esc closes this dialog only. While a save is running it waits, so the result is not lost.
      dialog.addEventListener('cancel', (event) => { if (saving) event.preventDefault(); });
      dialog.addEventListener('close', () => finish(false));
      document.body.append(dialog);
      dialog.showModal();
    });
  }

  function field(label, value = '', hint = '') {
    const input = h('input', { type: 'text', value, autocomplete: 'off', autocapitalize: 'off', spellcheck: false });
    return [h('label', { class: 'st-field' }, h('span', { class: 'st-field-label' }, label), input, hint && h('span', { class: 'st-hint' }, hint)), input];
  }

  // Swatches from the palette, any other colour, and a way back to the automatic one ('').
  function colorField(a, chosen, onPick) {
    const auto = ACCOUNT_COLORS[Math.max(0, data.accounts.findIndex((x) => x.name === a.name)) % ACCOUNT_COLORS.length];
    const group = h('div', { class: 'st-swatches', role: 'group', 'aria-label': 'Account colour' });
    const paint = () => {
      const current = chosen || auto;
      const custom = !ACCOUNT_COLORS.includes(current);
      const pick = (color) => { chosen = color; onPick(color); paint(); };
      group.replaceChildren(
        ...ACCOUNT_COLORS.map((color, i) => h('button', { type: 'button', class: 'st-swatch', style: { '--swatch': color }, 'aria-pressed': String(color === current),
          'aria-label': COLOR_NAMES[i], title: COLOR_NAMES[i], onclick: () => pick(color === auto ? '' : color) })),
        h('label', { class: `st-swatch st-swatch-custom${custom ? ' is-on' : ''}`, style: custom ? { '--swatch': current } : {}, title: 'Any other colour' },
          h('input', { type: 'color', value: current, 'aria-label': 'Any other colour', onchange: (event) => pick(event.target.value) })),
        chosen && h('button', { type: 'button', class: 'st-link', onclick: () => pick('') }, 'Automatic'));
    };
    paint();
    return h('div', { class: 'st-field' }, h('span', { class: 'st-field-label' }, 'Colour'), group,
      h('span', { class: 'st-hint' }, 'Marks this account everywhere in Claude Sessions.'));
  }

  async function after(message) {
    if (message) toast(message);
    await load();
    changed('accounts');
  }

  async function addAccount(folder = '') {
    const suggested = folder ? folder.replace(/^.*\/\.claude-/, '').toLowerCase().replace(/[^a-z0-9-]/g, '-') : '';
    const [name, nameInput] = field('Name', suggested, 'Lowercase letters, digits and dashes. Shown everywhere in this app.');
    const [alias, aliasInput] = field('Alias', suggested ? `claude-${suggested}` : '', 'The command you type to start Claude Code as this account.');
    const [dir, dirInput] = field('Folder', folder, 'Where this account keeps its login and history. An existing folder is used as it is.');
    // Alias and folder follow the name until they are edited.
    let aliasEdited = false;
    let dirEdited = Boolean(folder);
    aliasInput.addEventListener('input', () => { aliasEdited = true; });
    dirInput.addEventListener('input', () => { dirEdited = true; });
    nameInput.addEventListener('input', () => {
      if (!aliasEdited) aliasInput.value = nameInput.value ? `claude-${nameInput.value}` : '';
      if (!dirEdited) dirInput.value = nameInput.value ? `~/.claude-${nameInput.value}` : '';
    });
    const added = await ask({
      title: 'Add account',
      body: [name, alias, dir],
      confirmLabel: 'Add account',
      onConfirm: () => api('/api/accounts/add', { name: nameInput.value.trim(), alias: aliasInput.value.trim(), folder: dirInput.value.trim() }),
    });
    if (!added) return;
    aliasNotice = true;
    after(`Added ${added.name}. ${added.created ? 'Log in to finish setting it up.' : `Using the existing folder ${added.dir}.`}`);
  }

  // Name, alias and colour together. Changing the login and removing are their own steps, with their own warnings.
  async function editAccount(a) {
    const [name, nameInput] = field('Name', a.name, 'Lowercase letters, digits and dashes.');
    const [alias, aliasInput] = field('Alias', a.alias || '', a.isDefault
      ? 'Optional. Plain claude already starts this account.'
      : 'The command you type to start Claude Code as this account.');
    // An alias that follows the claude-NAME pattern keeps following the name.
    let aliasFollows = a.alias === `claude-${a.name}`;
    aliasInput.addEventListener('input', () => { aliasFollows = false; });
    nameInput.addEventListener('input', () => { if (aliasFollows) aliasInput.value = `claude-${nameInput.value}`; });
    let color = a.color || '';
    const result = await ask({
      title: `Edit ${a.name}`,
      body: [name, alias, colorField(a, color, (picked) => { color = picked; }),
        h('p', { class: 'st-hint' }, `The folder ${a.dir} is not moved and nothing inside it changes.`)],
      confirmLabel: 'Save changes',
      aside: [a.loggedIn && { label: 'Change login', value: 'login' }, { label: 'Remove account', value: 'remove', danger: true }],
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
    if (!result) return after();
    if (result.alias !== undefined && result.alias !== a.alias) aliasNotice = true;
    after('Saved changes.');
  }

  async function login(a, relogin) {
    if (relogin) {
      const live = data.sessions.filter((s) => s.account === a.name && s.live);
      const ok = await ask({
        title: `Change the login for ${a.name}?`,
        body: [h('p', {}, `It is logged in as ${a.email}. A terminal window opens, logs that out, and asks you to sign in again.`),
          h('p', {}, 'History, settings and memory in its folder are kept.'),
          live.length > 0 && h('p', { class: 'st-warn' }, `${plural(live.length, 'running session')} of this account will lose the login:`),
          live.length > 0 && h('ul', { class: 'st-list' }, live.map((s) => h('li', {}, `${s.name} in ${s.cwd}`)))],
        confirmLabel: 'Change login',
        danger: true,
      });
      if (!ok) return;
    }
    try {
      const r = await api('/api/accounts/login', { name: a.name, relogin });
      after(`Opened ${r.terminal}. Finish signing in there; this updates when it is done.`);
    } catch (err) {
      toast(err.message, 'error');
    }
  }

  async function removeAccount(a) {
    const open = data.sessions.filter((s) => s.account === a.name && s.open);
    const ok = await ask({
      title: `Remove ${a.name}?`,
      body: [h('p', {}, `${a.name} is hidden from this app and its alias is taken away. You can restore it at any time.`),
        h('p', {}, `Nothing is deleted: the folder ${a.dir}, its login and its history stay on disk.`),
        open.length > 0 && h('p', { class: 'st-warn' }, `${plural(open.length, 'open session')} of this account will keep running but will no longer be shown here.`)],
      confirmLabel: 'Remove account',
      danger: true,
      onConfirm: () => api('/api/accounts/remove', { name: a.name }),
    });
    if (!ok) return;
    if (a.alias) aliasNotice = true;
    after(`Removed ${a.name}. Its folder and data are untouched.`);
  }

  async function restoreAccount(a) {
    try {
      await api('/api/accounts/restore', { name: a.name });
      aliasNotice = true;
      after(`Restored ${a.name}.`);
    } catch (err) {
      toast(err.message, 'error');
    }
  }

  // ---------------------------------------------------------------- general

  function general() {
    return [
      h('section', { class: 'st-group' },
        h('h4', {}, 'Disconnect'),
        h('p', { class: 'st-hint' }, 'Stops Claude Sessions, the server behind this page. Your Claude sessions keep running, and nothing is closed or deleted.'),
        h('button', { type: 'button', class: 'st-btn st-btn-danger', 'data-fk': 'stop', onclick: stop }, icon('power'), 'Disconnect…')),
    ];
  }

  async function stop() {
    const ok = await ask({
      title: 'Disconnect Claude Sessions?',
      body: [h('p', {}, 'This stops the Claude Sessions server and this page stops updating. Your Claude sessions keep running.')],
      confirmLabel: 'Disconnect',
      danger: true,
      onConfirm: () => api('/api/shutdown', {}),
    });
    if (!ok) return;
    close();
    const command = store('cc-command') || 'cc-sessions';
    toast(`Stopped. Run ${command} in a terminal to start it again.`);
    window.dispatchEvent(new CustomEvent('cc-stopped'));
  }

  window.ccSettings = { open, close };

  // Switching layout from Settings opens it again on the new page.
  try {
    const reopen = sessionStorage.getItem('cc-settings-reopen');
    if (reopen) {
      sessionStorage.removeItem('cc-settings-reopen');
      addEventListener('load', () => open(reopen));
    }
  } catch { /* no storage: nothing to reopen */ }
})();
