'use strict';

// Loaded before the page paints, so a chosen theme never flashes the other one first.
try {
  const theme = localStorage.getItem('cc-theme');
  if (theme === 'light' || theme === 'dark') document.documentElement.dataset.theme = theme;
} catch { /* private window: follow the system */ }
