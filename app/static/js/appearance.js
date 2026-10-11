// Appearance: theme, accent colour, background style and content width. Applied before the app draws
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
export const DEFAULTS = { theme: 'dark', accent: 'violet', custom: '#8b5cf6', background: 'fade', width: 'comfortable' };
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
  root.dataset.theme = dark ? 'dark' : 'light';
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
  root.dataset.width = p.width === 'wide' ? 'wide' : 'comfortable';
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
