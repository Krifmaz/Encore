// Appearance: theme, accent colour and background style. Applied before the app draws
// (from this device's copy), then synced with the account on servers that store preferences (Encore 2).
import { api } from './api.js';

export const ACCENTS = [
  { id: 'violet', name: 'Violet', a: '#8b5cf6', b: '#ec4899' },
  { id: 'encore', name: 'Encore', a: '#f2a7e4', b: '#9fd2fb' },     // the logo's pink → blue
  { id: 'lilac', name: 'Lilac', a: '#b48cf2', b: '#7cc4fa' },
  { id: 'pink', name: 'Pink', a: '#ec4899', b: '#f97316' },
  { id: 'sky', name: 'Sky', a: '#0ea5e9', b: '#8b5cf6' },
  { id: 'teal', name: 'Teal', a: '#14b8a6', b: '#0ea5e9' },
  { id: 'green', name: 'Green', a: '#22c55e', b: '#14b8a6' },
  { id: 'amber', name: 'Amber', a: '#f59e0b', b: '#ef4444' },
  { id: 'red', name: 'Red', a: '#ef4444', b: '#ec4899' },
];
// The Encore logo, drawn in your accent colours (follows Settings → Appearance).
let logoN = 0;
const LOGO_PATHS = ['M249.205 114.329 L510.699 32.377 C510.699 32.377 525.463 25.913 538.636 31.22 C551.81 36.528 551.358 49.709 551.358 49.709 L550.464 99.763 C550.464 99.763 550.049 113.161 538.636 121.587 C527.224 130.012 514.95 133.407 514.95 133.407 L308.537 197.811 C298.175 201.165 275.923 206.119 273.697 229.869 C273.697 229.869 275.227 403.645 275.227 443.582 C275.227 493.942 241.622 582.872 154.358 582.872 C65.76 582.872 47.709 513.389 47.709 470.406 C47.709 412.557 102.003 373.129 140.138 369.574 C178.273 366.019 204.127 385.087 204.127 385.087 L201.373 166.271 C199.204 125.931 244.035 116.052 249.205 114.329 Z', 'M345.928 420.623 C345.928 420.623 518.078 368.274 535.733 363.243 C553.388 358.212 564.789 368.142 564.274 383.837 L564.274 419.323 C564.274 449.986 539.212 459.963 525.178 464.079 L348.687 519.256 C329.357 525.979 320.186 517.423 320.186 498.97 L320.186 446.803 C320.186 430.002 326.687 425.907 345.928 420.623 Z', 'M339.143 254.402 L494.664 206.043 C507.578 202.394 520.795 206.043 520.795 221.613 L520.795 239.527 C520.795 274.769 508.753 302.967 450.793 319.539 L341.142 356.25 C330.333 359.74 319.414 354.699 320.186 340.976 L319.414 282.394 C319.414 269.641 322.93 259.403 339.143 254.402 Z'];
export function logo(size = 30) {
  const id = `lg${++logoN}`;
  return `<svg class="logo" viewBox="16 15 580 580" width="${size}" height="${size}" aria-hidden="true"><defs><linearGradient id="${id}" x1="71" y1="351" x2="551" y2="257" gradientUnits="userSpaceOnUse">`
    + `<stop offset="0" style="stop-color:var(--accent)"/><stop offset="1" style="stop-color:var(--accent-2)"/></linearGradient></defs>`
    + LOGO_PATHS.map(d => `<path d="${d}" fill="url(#${id})"/>`).join('') + '</svg>';
}

/** One-tap looks; any of them can be tweaked afterwards (theme, colour, background). */
export const PRESETS = [
  { id: 'default', name: 'Encore dark', theme: 'dark', accent: 'violet', background: 'fade' },
  { id: 'oled', name: 'OLED black', theme: 'oled', accent: 'violet', background: 'solid' },
  { id: 'pastel', name: 'Pastel', theme: 'dark', accent: 'encore', background: 'fade' },
  { id: 'light', name: 'Daylight', theme: 'light', accent: 'sky', background: 'fade' },
  { id: 'mono', name: 'Mono', theme: 'oled', accent: 'custom', custom: '#e5e5e5', background: 'solid' },
];

export const DEFAULTS = { theme: 'dark', accent: 'violet', custom: '#8b5cf6', background: 'fade' };
const KEY = 'encore.appearance';

const rgb = (hex) => {
  const h = hex.replace('#', '');
  const n = parseInt(h.length === 3 ? h.split('').map(c => c + c).join('') : h, 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
};
const toHex = (r, g, b) => '#' + [r, g, b].map(v => Math.max(0, Math.min(255, Math.round(v))).toString(16).padStart(2, '0')).join('');
// light accents (pastels) get dark text on buttons, dark ones white
const luminance = (hex) => rgb(hex).map(v => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; })
  .reduce((s, v, i) => s + v * [0.2126, 0.7152, 0.0722][i], 0);
// a custom colour gets a partner shifted around the colour wheel for the gradient
function partner(hex) {
  let [r, g, b] = rgb(hex).map(v => v / 255);
  const max = Math.max(r, g, b), min = Math.min(r, g, b), l = (max + min) / 2;
  let h = 0, s = 0;
  if (max !== min) {
    const d = max - min;
    s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
    h = max === r ? (g - b) / d + (g < b ? 6 : 0) : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
    h /= 6;
  }
  h = (h + 0.1) % 1;
  const q = l < 0.5 ? l * (1 + s) : l + s - l * s, p = 2 * l - q;
  const f = (t) => { t = (t + 1) % 1; return t < 1 / 6 ? p + (q - p) * 6 * t : t < 1 / 2 ? q : t < 2 / 3 ? p + (q - p) * (2 / 3 - t) * 6 : p; };
  return toHex(f(h + 1 / 3) * 255, f(h) * 255, f(h - 1 / 3) * 255);
}

export function current() {
  try { return { ...DEFAULTS, ...JSON.parse(localStorage.getItem(KEY) || '{}') }; } catch { return { ...DEFAULTS }; }
}

let mq;
export function apply(p = current()) {
  const root = document.documentElement;
  const dark = p.theme === 'system' ? matchMedia('(prefers-color-scheme: dark)').matches : p.theme !== 'light';
  root.dataset.theme = !dark ? 'light' : p.theme === 'oled' ? 'oled' : 'dark';
  if (p.theme === 'system' && !mq) {
    mq = matchMedia('(prefers-color-scheme: dark)');
    mq.addEventListener('change', () => apply(current()));
  }
  const preset = ACCENTS.find(x => x.id === p.accent);
  const a = preset ? preset.a : (p.custom || DEFAULTS.custom);
  const b = preset ? preset.b : partner(a);
  root.style.setProperty('--accent', a);
  root.style.setProperty('--accent-2', b);
  root.style.setProperty('--accent-rgb', rgb(a).join(', '));
  root.style.setProperty('--accent-2-rgb', rgb(b).join(', '));
  root.style.setProperty('--on-accent', (luminance(a) + luminance(b)) / 2 > 0.45 ? '#16161f' : '#fff');
  root.dataset.bg = p.background === 'solid' ? 'solid' : 'fade';
  document.querySelector('meta[name=theme-color]')?.setAttribute('content', getComputedStyle(root).getPropertyValue('--bg').trim() || '#0b0b12');
}

/** Save on this device and – where the server keeps preferences – on the account (follows you everywhere). */
export async function save(p, { serverPrefs = false } = {}) {
  const clean = { ...DEFAULTS, ...p };
  try { localStorage.setItem(KEY, JSON.stringify(clean)); } catch { /* private mode */ }
  apply(clean);
  if (serverPrefs) {
    try { await api.put('/auth/prefs', { appearance: clean }); } catch { /* kept on this device */ }
  }
}

/** After sign-in: the account's appearance wins over this device's (older Encore servers keep it per device). */
export function adoptAccount(user) {
  const a = user?.prefs?.appearance;
  if (a && typeof a === 'object') save(a);
}

// migrate the old dark/light toggle
try {
  const old = localStorage.getItem('encore.theme');
  if (old && !localStorage.getItem(KEY)) localStorage.setItem(KEY, JSON.stringify({ ...DEFAULTS, theme: old }));
} catch { /* ignore */ }
