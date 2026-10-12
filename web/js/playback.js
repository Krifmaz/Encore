// Playback preferences (Settings → Playback): stream quality, pre-loading the next song and what happens
// when the queue ends. Kept on this device, and – on servers that store preferences – on the account too,
// the same way appearance.js does. The player reads them through current().
import { api } from './api.js';

/**
 * quality: 'auto'   – the original file; a smaller AAC stream for the rest of the song (and the next ten
 *                     minutes) when the connection can't keep up
 *          'original' – always the original file, whatever the connection does
 *          'saver'  – always the smaller AAC stream (mobile data, slow links)
 * kbps:    bitrate of the smaller stream (96 / 160 / 256)
 * preload: buffer the next song while this one plays (off on very slow connections)
 * autoplay: when the queue ends, keep going with a shuffle of the whole library
 */
export const DEFAULTS = { quality: 'auto', kbps: 256, preload: true, autoplay: true };
const KEY = 'encore.playback';
const QUALITIES = new Set(['auto', 'original', 'saver']);
const RATES = new Set([96, 160, 256]);

function clean(p) {
  const c = { ...DEFAULTS, ...p };
  if (!QUALITIES.has(c.quality)) c.quality = DEFAULTS.quality;
  c.kbps = RATES.has(+c.kbps) ? +c.kbps : DEFAULTS.kbps;
  c.preload = c.preload !== false;
  c.autoplay = c.autoplay !== false;
  return c;
}

export function current() {
  try {
    const stored = localStorage.getItem(KEY);
    if (stored) return clean(JSON.parse(stored));
    // before Settings → Playback existed, "Then shuffle library" lived in its own key
    return clean({ autoplay: localStorage.getItem('encore.autoplay') !== 'off' });
  } catch { return { ...DEFAULTS }; }
}

/** Save on this device and – where the server keeps preferences – on the account (follows you everywhere). */
export async function save(p, { serverPrefs = false } = {}) {
  const c = clean(p);
  try {
    localStorage.setItem(KEY, JSON.stringify(c));
    localStorage.setItem('encore.autoplay', c.autoplay ? 'on' : 'off');
  } catch { /* private mode */ }
  window.dispatchEvent(new CustomEvent('encore:playback-prefs', { detail: c }));
  if (serverPrefs) {
    try { await api.put('/auth/prefs', { playback: c }); } catch { /* kept on this device */ }
  }
  return c;
}

/** After sign-in: the account's playback settings win over this device's. */
export function adoptAccount(user) {
  const a = user?.prefs?.playback;
  if (a && typeof a === 'object') save(a);
}
