// Playback preferences (Settings → Playback): stream quality, pre-loading the next song and what happens
// when the queue ends. Kept on this device, and – on servers that store preferences – on the account too,
// the same way appearance.js does. The player reads them through current().
import { api } from './api.js';

/**
 * quality: 'auto'   – the original file; a smaller AAC stream for the rest of the song (and the next ten
 *                     minutes) when the connection can't keep up
 *          'original' – always the original file, whatever the connection does
 *          'saver'  – always the smaller AAC stream (mobile data, slow links)
 * kbps:    bitrate of the smaller stream – fixed at 96 (not a user choice)
 * preload: buffer the next song while this one plays (off on very slow connections)
 * autoplay: when the queue ends, keep going with a shuffle of the whole library
 * dsp:     have the server upsample streams with Cadenza (only where an admin switched it on and allowed the account)
 * dsp_filter: Cadenza upsampling filter id ('' = the server's default)
 * dsp_dither: Cadenza dither / noise shaping id
 * dsp_rate: 'device' (this device's output rate), 'auto' (highest the server allows) or a rate in Hz
 */
export const DEFAULTS = { quality: 'auto', kbps: 96, preload: true, autoplay: true, dsp: false, dsp_filter: '', dsp_dither: 'tpdf', dsp_rate: 'device' };
const KEY = 'encore.playback';
const QUALITIES = new Set(['auto', 'original', 'saver']);
// the smaller stream is always 96 kbps; older saved choices (160 / 256) are ignored
const SMALL_KBPS = 96;

function clean(p) {
  const c = { ...DEFAULTS, ...p };
  if (!QUALITIES.has(c.quality)) c.quality = DEFAULTS.quality;
  c.kbps = SMALL_KBPS;
  c.preload = c.preload !== false;
  c.autoplay = c.autoplay !== false;
  c.dsp = c.dsp === true;
  c.dsp_filter = typeof c.dsp_filter === 'string' ? c.dsp_filter : DEFAULTS.dsp_filter;
  c.dsp_dither = typeof c.dsp_dither === 'string' && c.dsp_dither ? c.dsp_dither : DEFAULTS.dsp_dither;
  if (c.dsp_rate !== 'device' && c.dsp_rate !== 'auto') {
    const r = Number(c.dsp_rate);
    c.dsp_rate = Number.isInteger(r) && r > 0 ? r : DEFAULTS.dsp_rate;
  }
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
