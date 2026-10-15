// Playback preferences (Settings → Playback): stream quality, pre-loading the next song, what happens
// when the queue ends, sound effects on this device and crossfade. Kept on this device, and – on servers
// that store preferences – on the account too, the same way appearance.js does. The player reads them through current().
import { api } from './api.js';

/**
 * quality: 'auto'     – the original file; Encore measures the connection while it plays and steps the stream down
 *                       (256 → 160 → 96 kbps; 64 only when 96 keeps stalling, never 32) as far as it needs to keep
 *                       playing, then back up
 *          'original' – always the original file, whatever the connection does
 *          'saver'    – always the small 96 kbps stream (mobile data, slow links)
 * preload: buffer the next song while this one plays (off on very slow connections)
 * autoplay: when the queue ends, keep going with a shuffle of the whole library
 * resume:  when the app opens, pick up where you left off (this device's queue and spot, or another device's)
 * dsp:     have the server upsample streams with Cadenza (only where an admin switched it on and allowed the account)
 * dsp_filter: Cadenza upsampling filter id ('' = the server's default)
 * dsp_dither: Cadenza dither / noise shaping id
 * dsp_rate: 'device' (this device's output rate), 'auto' (highest the server allows) or a rate in Hz
 * fx_on:   sound effects on this device (Web Audio): the master switch for everything below except crossfade
 * eq_preset / eq_bands / eq_preamp: parametric EQ (a named preset, or 'custom' with your own bands), preamp in dB
 * crossfeed: headphone crossfeed 'off' | 'light' | 'medium' | 'strong'
 * balance: -100 (left) … +100 (right) · mono: sum both channels
 * transition: how one song becomes the next – 'off' | 'crossfade' (equal-power, both fade) | 'overlap' (the next song
 *             starts at full volume `crossfade` seconds early, only the old one fades out) | 'smart' (Automix-style: the
 *             silent ends of both songs are skipped and the next song starts at the old one's last audible moment)
 * crossfade: the transition's length in seconds (1–12; 0 only with transition 'off') · crossfade_album_gapless: no
 *            transition within an album (every mode)
 */
export const DEFAULTS = { quality: 'auto', preload: true, autoplay: true, resume: true, dsp: false, dsp_filter: '', dsp_dither: 'tpdf', dsp_rate: 'device',
  fx_on: false, eq_preset: 'flat', eq_bands: [], eq_preamp: 0, crossfeed: 'off', balance: 0, mono: false,
  transition: 'off', crossfade: 0, crossfade_album_gapless: true };
export const TRANSITIONS = [['off', 'Off'], ['crossfade', 'Crossfade'], ['overlap', 'Overlap'], ['smart', 'Smart overlap']];
const TRANSITION_IDS = new Set(TRANSITIONS.map(t => t[0]));
/** Seconds a transition lasts when one is switched on with no length set yet. */
export const TRANSITION_DEFAULT_S = 6;
const KEY = 'encore.playback';
const QUALITIES = new Set(['auto', 'original', 'saver']);

// ---- sound effects -----------------------------------------------------------------------------
export const EQ_TYPES = [['peaking', 'Peaking'], ['lowshelf', 'Low shelf'], ['highshelf', 'High shelf'], ['lowpass', 'Low-pass'], ['highpass', 'High-pass'], ['notch', 'Notch']];
const B = (type, freq, gain, q) => ({ type, freq, gain, q });
/** EQ presets: id, label, preamp (dB, keeps boosts from clipping) and bands. */
export const EQ_PRESETS = [
  { id: 'flat', label: 'Flat', preamp: 0, bands: [] },
  { id: 'bass', label: 'Bass boost', preamp: -6, bands: [B('lowshelf', 105, 6, 0.7), B('peaking', 60, 2, 1)] },
  { id: 'treble', label: 'Treble boost', preamp: -5, bands: [B('highshelf', 6000, 5, 0.7)] },
  { id: 'vocal', label: 'Vocal', preamp: -3, bands: [B('highpass', 80, 0, 0.7), B('peaking', 250, -2, 1), B('peaking', 3000, 3, 1), B('peaking', 5000, 2, 1.2)] },
  { id: 'loudness', label: 'Loudness (V-shape)', preamp: -5, bands: [B('lowshelf', 90, 5, 0.7), B('peaking', 1000, -3, 0.7), B('highshelf', 10000, 4, 0.7)] },
  { id: 'classical', label: 'Classical', preamp: -2, bands: [B('lowshelf', 80, 2, 0.7), B('peaking', 500, -1, 0.8), B('highshelf', 12000, 2, 0.7)] },
  { id: 'electronic', label: 'Electronic', preamp: -5, bands: [B('peaking', 60, 5, 1), B('peaking', 400, -2, 1), B('peaking', 2500, 1, 1), B('highshelf', 10000, 4, 0.7)] },
  { id: 'podcast', label: 'Podcast / voice', preamp: -4, bands: [B('highpass', 90, 0, 0.7), B('peaking', 300, -3, 1), B('peaking', 3000, 4, 1), B('lowpass', 12000, 0, 0.7)] },
];
export const CROSSFEED = [['off', 'Off'], ['light', 'Light'], ['medium', 'Medium'], ['strong', 'Strong']];
const TYPES = new Set(EQ_TYPES.map(t => t[0]));
const PRESET_IDS = new Set([...EQ_PRESETS.map(x => x.id), 'custom']);
const num = (v, lo, hi, dflt, step) => {
  const n = Number(v);
  if (!Number.isFinite(n)) return dflt;
  const c = Math.max(lo, Math.min(hi, n));
  return step ? Math.round(c / step) * step : c;
};
const round = (v, digits) => Number(v.toFixed(digits));
function cleanBand(b) {
  const x = b && typeof b === 'object' ? b : {};
  return {
    type: TYPES.has(x.type) ? x.type : 'peaking',
    freq: Math.round(num(x.freq, 20, 20000, 1000)),
    gain: round(num(x.gain, -24, 24, 0), 1),
    q: round(num(x.q, 0.1, 18, 1), 2),
  };
}
/** The bands that play: a named preset's own, or your custom ones. */
export function eqBands(p) {
  if (p.eq_preset === 'custom') return p.eq_bands || [];
  return (EQ_PRESETS.find(x => x.id === p.eq_preset) || EQ_PRESETS[0]).bands;
}
export const presetLabel = (id) => id === 'custom' ? 'Custom' : (EQ_PRESETS.find(x => x.id === id) || EQ_PRESETS[0]).label;
/** Does any effect change the sound? (the master switch on and something set) */
export function fxActive(p) {
  if (!p.fx_on) return false;
  return eqBands(p).length > 0 || p.eq_preamp !== 0 || p.crossfeed !== 'off' || p.balance !== 0 || p.mono;
}
const db = (v) => `${v > 0 ? '+' : ''}${v} dB`;
export const balanceText = (v) => v === 0 ? 'Centre' : v < 0 ? `L${-v}` : `R${v}`;
/**
 * The two gains of a song transition at progress x (0…1) of its window, `elapsed` seconds in: [old song, new song].
 * Crossfade: equal-power (cos/sin, the sum of powers stays 1). Overlap: the new song at full volume at once (a
 * 0.15 s ramp, so it doesn't click), the old one fades out under it. Smart overlap: the same with a 0.5 s fade-in.
 */
export function transitionGains(mode, x, elapsed) {
  const p = Math.max(0, Math.min(1, Number(x) || 0));
  const out = Math.cos(p * Math.PI / 2);
  if (mode === 'crossfade') return [out, Math.sin(p * Math.PI / 2)];
  const rise = mode === 'smart' ? 0.5 : 0.15;
  return [out, Math.max(0, Math.min(1, (Number(elapsed) || 0) / rise))];
}
export const transitionLabel = (id) => (TRANSITIONS.find(t => t[0] === id) || TRANSITIONS[0])[1];
/** "EQ: Bass boost, preamp -6 dB · Crossfeed: medium · Balance: L10 · Mono · Crossfade: 6 s" – '' when nothing applies. */
export function effectsSummary(p, { crossfade = true } = {}) {
  const parts = [];
  if (fxActive(p)) {
    if (eqBands(p).length || p.eq_preamp !== 0) parts.push(`EQ: ${presetLabel(p.eq_preset)}${p.eq_preamp !== 0 ? `, preamp ${db(p.eq_preamp)}` : ''}`);
    if (p.crossfeed !== 'off' && !p.mono) parts.push(`Crossfeed: ${p.crossfeed}`);
    if (p.balance !== 0) parts.push(`Balance: ${balanceText(p.balance)}`);
    if (p.mono) parts.push('Mono');
  }
  if (crossfade && p.transition !== 'off' && p.crossfade > 0) parts.push(`${transitionLabel(p.transition)}: ${p.crossfade} s`);
  return parts.join(' · ');
}

// ---- adaptive quality (Auto): the ladder and the choice of rung from the estimated bandwidth ------------
/** Rungs in kbps, best first; 0 = the original file. */
export const RUNGS = [0, 256, 160, 96, 64, 32];
export const SAVER_KBPS = 96;
export const EWMA_ALPHA = 0.3;
/** Auto never goes below this (32 kbps sounds bad and low) … */
export const AUTO_FLOOR = 96;
/** … except 64 kbps, only after 96 kbps has stalled 3 times within 2 minutes. */
export const AUTO_LOW = 64;
/** The rungs a song can use: the original, then only the converted streams smaller than it and at least `floor`. */
export function ladder(origKbps, floor = AUTO_FLOOR) {
  return RUNGS.filter(r => r === 0 || ((!(origKbps > 0) || r < origKbps) && r >= floor));
}
/** kbit/s a rung needs: the rung itself, the original's bitrate for the original (1411 when unknown). */
export const needed = (rung, origKbps) => rung === 0 ? (origKbps > 0 ? origKbps : 1411) : rung;
/** Highest rung that fits in 75 % of the estimate; the original while there is no estimate yet; the floor at worst. */
export function chooseRung(estimate, origKbps, floor = AUTO_FLOOR) {
  const l = ladder(origKbps, floor);
  if (!(estimate > 0)) return estimate === null || estimate === undefined ? 0 : l[l.length - 1];
  return l.find(r => needed(r, origKbps) <= 0.75 * estimate) ?? l[l.length - 1];
}
/** One rung down from `rung` (stays at the floor). */
export function stepDown(rung, origKbps, floor = AUTO_FLOOR) {
  const l = ladder(origKbps, floor);
  const i = l.indexOf(rung);
  return l[Math.min(l.length - 1, (i < 0 ? 0 : i) + 1)];
}
/** Exponentially weighted moving average of throughput samples (kbit/s); the first sample sets it. */
export const ewma = (prev, sample) => (prev === null || prev === undefined) ? sample : prev + EWMA_ALPHA * (sample - prev);
/** kbit/s from bytes moved in `seconds`; null for transfers under 0.25 s (too short to say anything). */
export const rateOf = (bytes, seconds) => seconds < 0.25 ? null : (bytes * 8 / 1000) / seconds;

export function clean(p) {
  const c = { ...DEFAULTS, ...(p && typeof p === 'object' ? p : {}) };
  delete c.kbps;   // the player owns the stream bitrate now (adaptive)
  if (!QUALITIES.has(c.quality)) c.quality = DEFAULTS.quality;
  c.preload = c.preload !== false;
  c.autoplay = c.autoplay !== false;
  c.resume = c.resume !== false;
  c.dsp = c.dsp === true;
  c.dsp_filter = typeof c.dsp_filter === 'string' ? c.dsp_filter : DEFAULTS.dsp_filter;
  c.dsp_dither = typeof c.dsp_dither === 'string' && c.dsp_dither ? c.dsp_dither : DEFAULTS.dsp_dither;
  if (c.dsp_rate !== 'device' && c.dsp_rate !== 'auto') {
    const r = Number(c.dsp_rate);
    c.dsp_rate = Number.isInteger(r) && r > 0 ? r : DEFAULTS.dsp_rate;
  }
  c.fx_on = c.fx_on === true;
  c.eq_preset = PRESET_IDS.has(c.eq_preset) ? c.eq_preset : 'flat';
  c.eq_bands = Array.isArray(c.eq_bands) ? c.eq_bands.slice(0, 10).map(cleanBand) : [];
  c.eq_preamp = round(num(c.eq_preamp, -24, 12, 0), 1);
  c.crossfeed = CROSSFEED.some(x => x[0] === c.crossfeed) ? c.crossfeed : 'off';
  c.balance = Math.round(num(c.balance, -100, 100, 0));
  c.mono = c.mono === true;
  c.crossfade = Math.round(num(c.crossfade, 0, 12, 0));
  // before Song transitions existed, a crossfade length above 0 meant "crossfade"
  const raw = p && typeof p === 'object' ? p.transition : undefined;
  c.transition = TRANSITION_IDS.has(raw) ? raw : c.crossfade > 0 ? 'crossfade' : 'off';
  if (c.transition !== 'off' && c.crossfade < 1) c.crossfade = TRANSITION_DEFAULT_S;
  c.crossfade_album_gapless = c.crossfade_album_gapless !== false;
  return c;
}

export function current() {
  try {
    const stored = localStorage.getItem(KEY);
    if (stored) return clean(JSON.parse(stored));
    // before Settings → Playback existed, "Then shuffle library" lived in its own key
    return clean({ autoplay: localStorage.getItem('encore.autoplay') !== 'off' });
  } catch { return clean({}); }
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
