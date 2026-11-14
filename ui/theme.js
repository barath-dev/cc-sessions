'use strict';

// Loaded before the page paints, so the chosen theme, template and primary
// colour never flash the defaults first, and the classic page hands over to the
// Finder view straight away when that was the last one used.
(() => {
  // Settings keeps choices in memory as well, for browsers that keep no storage.
  const memory = window.ccMemory || (window.ccMemory = {});
  const read = (key) => {
    if (key in memory) return memory[key];
    try { return localStorage.getItem(key); } catch { return null; }
  };

  if (read('cc-view') === 'finder' && location.pathname === '/') {
    location.replace(`/finder${location.search}`);
  }

  // White or near-black, whichever reads better on the colour.
  function textOn(hex) {
    const channel = (i) => {
      const v = parseInt(hex.slice(i, i + 2), 16) / 255;
      return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
    };
    const lum = 0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5);
    return (1.05 / (lum + 0.05)) >= ((lum + 0.05) / 0.0625) ? '#ffffff' : '#111111';
  }

  // Settings calls this again after a change.
  window.ccApplyAppearance = () => {
    const root = document.documentElement;
    const theme = read('cc-theme');
    if (theme === 'light' || theme === 'dark') root.dataset.theme = theme;
    else delete root.dataset.theme;
    const template = read('cc-template');
    if (template && /^[a-z-]+$/.test(template)) root.dataset.template = template;
    else delete root.dataset.template;
    const accent = read('cc-accent');
    if (accent && /^#[0-9a-f]{6}$/i.test(accent)) {
      root.dataset.accent = '';
      root.style.setProperty('--accent-user', accent);
      root.style.setProperty('--on-accent-user', textOn(accent));
      // Dark mode shows the colour lightened (a third of the way to white); its text is picked for that.
      const light = '#' + [1, 3, 5].map((i) => Math.round(parseInt(accent.slice(i, i + 2), 16) * 0.66 + 255 * 0.34).toString(16).padStart(2, '0')).join('');
      root.style.setProperty('--on-accent-dark-user', textOn(light));
    } else {
      delete root.dataset.accent;
      root.style.removeProperty('--accent-user');
      root.style.removeProperty('--on-accent-user');
      root.style.removeProperty('--on-accent-dark-user');
    }
  };
  window.ccApplyAppearance();
})();
