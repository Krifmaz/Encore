// Web player: streams the original file (bit-perfect for MP3/FLAC/AAC/Opus); formats a browser
// can't decode (ALAC/APE/WavPack/AIFF/WMA) are converted to FLAC on the fly by the server. On Auto quality it
// measures the connection and steps the stream down (256 → 32 kbps AAC/Opus) and back up as the bandwidth allows.
// Optional sound effects (EQ, crossfeed, balance, mono) run in this browser through Web Audio, and crossfade
// blends one song into the next with the two <audio> elements.
// Tap the bar for the full-screen player (lyrics + Up next). When the queue runs out it keeps going
// with a shuffle of the whole library: a single song → straight into the shuffle, an album → after its last song.
import { api } from './api.js';
import * as playback from './playback.js';
import { art, esc, fmt, icon, popMenu, shareTrack, toast } from './ui.js';

const state = { queue: [], index: -1, transcoded: false, lossy: false, rung: 0, dsp: false, dspRate: 0, offset: 0, dragging: false, context: 'list', repeat: 'off', full: false, pane: 'now', logged: false,
  stalls: [], stallAt: 0, seekAt: 0, stepAt: 0, switchedAt: 0, lastPreloadCheck: 0, slowStart: null };
const listeners = new Set();
let audio = null;
let spare = null;   // second <audio> that pre-buffers the next song, so skipping is instant
// A third <audio>, never wrapped in Web Audio: once the effects graph exists, 30-second previews (third-party
// URLs, often without CORS) play on it – through the graph they would fail or be silent. `parked` is the
// wrapped element set aside while it plays.
let plain = null;
let parked = null;
let lyr = { id: null, lines: [], plain: '', loading: false };
let prefs = playback.current();
let userVol = 0.9;           // the volume slider: the ceiling of every fade
let liked = null;            // Set of liked track ids, fetched the first time a heart is drawn
let ctx = null;              // AudioContext, only to read the device's output sample rate
const INFO = new Map();      // track id → /info (codec, sample rate, bit depth …)
let dspInfo = null;          // GET /api/playback/dsp: is server-side Cadenza processing on, and may this account use it
let dspFailed = false;       // a processed stream failed this session: play originals until the page reloads
let dspToasted = false;
const dspCache = new Map();  // track id → what the server's pre-render cache says: 'cached' | 'rendering' | 'queued' | 'busy'
const dspAsked = new Map();  // track id → when we last asked (prepare)
function fetchDsp() { api.get('/playback/dsp').then(i => { dspInfo = i; }).catch(() => { dspInfo = null; }); }
const dspKey = (p) => JSON.stringify([p.dsp, p.dsp_filter, p.dsp_dither, p.dsp_rate]);
function newPrefs(p) {
  if (dspKey(p) !== dspKey(prefs)) { dspCache.clear(); dspAsked.clear(); }
  prefs = p;
  if (playback.fxActive(prefs)) ensureFx();
  applyFx();
}
window.addEventListener('encore:playback-prefs', (e) => { newPrefs(e.detail); fetchDsp(); if (!prefs.preload) cancelPreload(); else maybePreload(true); if (state.full) renderFull(); });
window.addEventListener('storage', (e) => { if (e.key === 'encore.playback') newPrefs(playback.current()); });

// Songs heard this listening session (this tab): shuffles never repeat one until the whole library has played.
const played = new Set((() => { try { return JSON.parse(sessionStorage.getItem('encore.played') || '[]'); } catch { return []; } })());
function markPlayed(t) {
  if (!t || typeof t.id !== 'number' || played.has(t.id)) return;
  played.add(t.id);
  try { sessionStorage.setItem('encore.played', JSON.stringify([...played])); } catch { /* ignore */ }
}
/** Random library songs not yet heard (or queued) this session; when everything has played, start a fresh round. */
async function freshShuffle(limit = 50) {
  const queued = state.queue.map(t => t.id).filter(id => typeof id === 'number');
  let more = await api.post('/library/shuffle', { limit, exclude: [...new Set([...played, ...queued])] });
  if (!more.length) {
    played.clear();
    try { sessionStorage.removeItem('encore.played'); } catch { /* ignore */ }
    const cur = current()?.id;
    more = (await api.post('/library/shuffle', { limit, exclude: typeof cur === 'number' ? [cur] : [] }));
  }
  return more;
}
/** The Shuffle button: play the whole library in random order, no song twice this session. */
export async function shuffleLibrary() {
  const first = await freshShuffle(50);
  if (first.length) playQueue(first, 0, { context: 'library' });
  return first.length;
}

export function onChange(fn) { listeners.add(fn); return () => listeners.delete(fn); }
const emit = () => { listeners.forEach(fn => { try { fn(current()); } catch { /* ignore */ } }); native(); };

// Android / desktop apps: tell them what's playing (notification + lock screen + tray), and let them control us.
function native() {
  const n = window.EncoreNative;
  if (!n?.nowPlaying || !audio) return;
  const t = current();
  try {
    n.nowPlaying(JSON.stringify(t ? { title: t.title || '', artist: t.artist || '', album: t.album || '', playing: !audio.paused,
      art: cover(t) ? new URL(cover(t), location.href).href : '' } : { stopped: true }));
  } catch { /* ignore */ }
}
window.encorePlayer = { toggle: () => toggle(), next: () => next(), prev: () => prev(), open: () => setFull(true) };
export function current() { return state.queue[state.index] || null; }

// ---- linked: this device is locked as a remote for another one ----
// Everything you play or press here goes to that device; nothing plays locally until you unlink.
let link = null;   // { id, name, state, online, send(cmd) }
let local = false; // true while running a command another device sent us (never forward those)
let linkTick = null;
const viaLink = (c) => { if (!link || local) return false; link.send(c); return true; };
export function linked() { return link; }
export function setLink(l) {
  const was = !!link;
  link = l;
  if (l && !was && audio && !audio.paused) audio.pause();
  clearInterval(linkTick);
  // between check-ins, move the remote's seek bar along on our own
  if (l) linkTick = setInterval(() => { if (link?.state?.playing) { link.state.position = (link.state.position || 0) + 1; updateLinkedTime(); syncLyrics(); } }, 1000);
  if (l) cancelPreload();
  lyr = { id: null, lines: [], plain: '', loading: false };
  render();
  if (state.full) { renderFull(); loadLyrics(); }
}
/** New state from the linked device (every check-in). */
export function linkState(s, online = true) {
  if (!link) return;
  const before = JSON.stringify([link.state?.id, link.state?.playing, link.online, link.state?.volume, (link.state?.queue || []).length, link.state?.index]);
  link.state = s || {};
  link.online = online;
  if (JSON.stringify([link.state.id, link.state.playing, link.online, link.state.volume, (link.state.queue || []).length, link.state.index]) !== before) {
    render();
    if (state.full) { renderFull(); loadLyrics(); }
  } else updateLinkedTime();
}
/** The linked device's song as a track object (what the full-screen view, likes and the ⋯ menu work on). */
function remoteTrack() {
  const s = link?.state || {};
  if (!s.title) return null;
  return { id: s.id, title: s.title, artist: s.artist, album: s.album, album_id: s.album_id, format: s.format, duration: s.duration, cover: s.cover, remote: true };
}
/** The song in front of you: the linked device's when linked, else this device's. */
const nowTrack = () => link ? remoteTrack() : current();
const nowPosition = () => link ? (link.state?.position || 0) : position();
const nowPlaying = () => link ? !!link.state?.playing : !!(audio && !audio.paused);

// ---- remote control: what this device is playing, and commands from your other devices ----
const slim = (t) => ({ id: t.id, title: t.title, artist: t.artist, album: t.album, album_id: t.album_id, format: t.format, duration: t.duration });
export function snapshot() {
  const t = current();
  if (!t || !audio) return { playing: false };
  const { queue, index } = libWindow();
  return { ...slim(t), cover: cover(t), playing: !audio.paused, position: spotPosition(), duration: duration(), volume: userVol,
           signal: { label: quality(t), transport: transport()?.kind || null, output_rate: ctx?.sampleRate || null },
           index, queue: queue.map(slim) };
}
/** The library songs of the queue – at most 300, around the current song – and where the current song is in them. */
function libWindow() {
  let queue = state.queue.filter(isLib);
  let index = state.queue.slice(0, state.index).filter(isLib).length;
  if (queue.length > 300) {
    const from = Math.max(0, Math.min(index - 50, queue.length - 300));
    queue = queue.slice(from, from + 300);
    index -= from;
  }
  return { queue, index };
}
function localVolume(v) {
  if (!audio) return;
  const n = Number(v);
  userVol = Math.max(0, Math.min(1, Number.isFinite(n) ? n : 0.9));
  if (fade) stepFade(); else { setFadeGain(audio, 1); setFadeGain(spare, 1); }
  try { localStorage.setItem('encore.volume', userVol); } catch { /* ignore */ }
  for (const id of ['vol', 'fvol']) { const el = document.getElementById(id); if (el && +el.value !== userVol) el.value = userVol; }
  emit();
}
export function setVolume(v) {
  if (viaLink({ action: 'volume', value: Number(v) })) { if (link.state) link.state.volume = Number(v); return; }
  localVolume(v);
}
export function command(c) {
  local = true;
  try { runCommand(c); } finally { local = false; }
}
function runCommand(c) {
  switch (c.action) {
    case 'toggle': return toggle();
    case 'play': return current() && audio.play();
    case 'pause': return audio?.pause();
    case 'stop': audio?.pause(); return;
    case 'next': return next();
    case 'prev': return prev();
    case 'seek': return seekTo(Number(c.value) || 0);
    case 'volume': return setVolume(c.value);
    case 'enqueue': return Array.isArray(c.tracks) && enqueue(c.tracks);
    case 'play_next': return Array.isArray(c.tracks) && playNext(c.tracks);
    case 'play_queue':
      if (!Array.isArray(c.tracks) || !c.tracks.length) return;
      playQueue(c.tracks, c.index || 0, { context: 'list' });
      if (c.position > 1) {
        const at = Number(c.position);
        const once = () => { seekTo(at); audio.removeEventListener('loadedmetadata', once); };
        audio.addEventListener('loadedmetadata', once);
      }
      return;
    default:
  }
}
export function queue() { return { tracks: state.queue, index: state.index }; }

const cover = (t) => t.preview ? t.cover : t.album_id ? `/api/library/albums/${t.album_id}/cover` : t.cover;

/** context: 'album' | 'single' | 'list' – decides what happens when the queue ends. */
const sendable = (tracks) => tracks.filter(t => t && typeof t.id === 'number').map(slim);
export function playQueue(tracks, start = 0, { context = 'list' } = {}) {
  if (link && !local && context !== 'preview') {
    const list = tracks.filter(Boolean);
    const first = list[Math.max(0, Math.min(start, list.length - 1))];
    const out = sendable(list);
    if (!out.length) return toast('Only songs from your library can be played on another device', 'err');
    return viaLink({ action: 'play_queue', tracks: out, index: Math.max(0, out.findIndex(t => t.id === first?.id)) });
  }
  state.queue = tracks.filter(Boolean);
  state.index = Math.max(0, Math.min(start, state.queue.length - 1));
  state.context = context;
  load();
}

export function enqueue(tracks) {
  if (link && !local) { const out = sendable(tracks); return out.length && viaLink({ action: 'enqueue', tracks: out }); }
  state.queue.push(...tracks);
  if (state.index < 0) { state.index = 0; load(); } else { render(); emit(); preloadNext(); }
}

export function playNext(tracks) {
  if (link && !local) { const out = sendable(tracks); return out.length && viaLink({ action: 'play_next', tracks: out }); }
  if (state.index < 0) return playQueue(tracks);
  state.queue.splice(state.index + 1, 0, ...tracks);
  render(); emit(); preloadNext();
}

/** 30-second preview of a song that isn't in the library (Deezer/iTunes preview URL). */
export function preview(t) {
  playQueue([{ ...t, preview: true, id: `pv-${t.preview_url}` }], 0, { context: 'preview' });
}

// formats browsers can't decode → the server converts to FLAC; the player then seeks via the server
const CONVERT = new Set(['ape', 'wv', 'aiff', 'aif', 'wma', 'alac', 'dsf', 'dff', 'tta', 'mpc']);
// what this browser says it can decode, by file format (a Chromium build without AAC, Firefox on Linux, Safari without Ogg …)
const MIME = { mp3: 'audio/mpeg', flac: 'audio/flac', m4a: 'audio/mp4; codecs="mp4a.40.2"', mp4: 'audio/mp4; codecs="mp4a.40.2"', aac: 'audio/aac', ogg: 'audio/ogg; codecs=vorbis', oga: 'audio/ogg; codecs=vorbis', opus: 'audio/ogg; codecs=opus', wav: 'audio/wav', webm: 'audio/webm' };
const PLAYABLE = {};
function canPlay(format) {
  const f = (format || '').toLowerCase();
  if (!(f in MIME)) return false;
  if (!(f in PLAYABLE)) { try { PLAYABLE[f] = document.createElement('audio').canPlayType(MIME[f]) !== ''; } catch { PLAYABLE[f] = true; } }
  return PLAYABLE[f];
}

// the lighter streams' codec: AAC where the browser decodes it, Opus elsewhere (Chromium builds, Firefox on Linux)
const LOSSY = (() => { try { const a = document.createElement('audio'); return a.canPlayType('audio/aac') ? 'aac' : a.canPlayType('audio/ogg; codecs=opus') ? 'opus' : 'aac'; } catch { return 'aac'; } })();
/** The rate asked of the server's Cadenza: this device's output rate, 0 (= the highest the server allows) or a fixed rate. */
function dspRequestRate() {
  if (prefs.dsp_rate === 'device') return Math.round(outputRate()) || 0;
  if (prefs.dsp_rate === 'auto') return 0;
  return Number(prefs.dsp_rate) || 0;
}
/** The rate the server will send (its rule: at most max_rate, never below the file's; 0 → the highest multiple of the file's rate). */
function dspOutRate(t) {
  const src = INFO.get(t?.id)?.sample_rate || 44100;
  const max = dspInfo?.max_rate || 192000;
  const want = dspRequestRate();
  let r = want > 0 ? Math.min(want, max) : src;
  if (want <= 0) while (r * 2 <= max) r *= 2;
  return Math.max(r, src);
}
/** Upsample this library song on the server? Only when switched on and allowed; Data saver and Auto's lighter streams win. */
const wantDsp = (t, lossy) => !!(prefs.dsp && dspInfo?.allowed && t && !t.preview && typeof t.id === 'number' && !lossy && !dspFailed);
/** The server keeps finished Cadenza renders (pre-render cache): seeking and the next song need no new job. */
const dspCacheOn = () => (dspInfo?.cache?.max_mb || 0) > 0;
/** transcode: FLAC made by the server · rung: kbps of the lossy stream (0 = not lossy) · dsp: upsampled by the server's Cadenza · offset: where a server-made stream starts */
function streamUrl(t, { transcode = false, rung = 0, dsp = false, offset = 0 } = {}) {
  if (t.preview) return t.preview_url;
  const q = [];
  if (rung > 0) q.push(`q=${LOSSY}`, `kbps=${rung}`);
  else if (dsp) {
    q.push('dsp=1', `rate=${dspRequestRate()}`);
    if (prefs.dsp_filter) q.push(`filter=${encodeURIComponent(prefs.dsp_filter)}`);
    q.push(`dither=${encodeURIComponent(prefs.dsp_dither)}`);
  } else if (transcode) q.push('transcode=1');
  if (offset) q.push(`t=${offset.toFixed(1)}`);
  return `/api/library/tracks/${t.id}/stream${q.length ? '?' + q.join('&') : ''}`;
}
const convert = (t) => !t.preview && (CONVERT.has((t.format || '').toLowerCase()) || !canPlay(t.format));
/** kbit/s of a Cadenza stream at `rate` (24-bit stereo FLAC, roughly 65 % of the raw PCM). */
const dspKbps = (rate) => Math.round(rate * 48 * 0.65 / 1000);
/** kbit/s the song needs as the "original" rung: the Cadenza stream's when the server upsamples it, else the file's. */
function origKbps(t) {
  if (!t || t.preview) return 0;
  if (wantDsp(t, false)) return dspKbps(dspOutRate(t));
  return fileKbps(t);
}
/** kbit/s of the file itself (0 = unknown, counted as 1411). */
function fileKbps(t) {
  if (!t || t.preview) return 0;
  if (t.bitrate > 0) return t.bitrate;
  const i = INFO.get(t.id);
  if (i?.bitrate > 0) return i.bitrate;
  return t.size > 0 && t.duration > 0 ? Math.round(t.size * 8 / 1000 / t.duration) : 0;
}
/** Which rung a song starts on: Original never converts, Data saver is fixed at 96 kbps, Auto follows the estimated bandwidth. */
function rungFor(t) {
  if (!t || t.preview || prefs.quality === 'original') return 0;
  if (prefs.quality === 'saver') return playback.SAVER_KBPS;
  return playback.chooseRung(est.kbps, origKbps(t));
}
/** The URL a song starts with (what the spare element pre-loads, and what load() asks for at a song start). */
function startUrl(t) {
  const rung = rungFor(t);
  return streamUrl(t, { transcode: convert(t) && rung === 0, rung, dsp: wantDsp(t, rung > 0) });
}

/** Android app: keep songs on the phone so they play with no server. */
export const canSaveOffline = () => !!window.EncoreNative?.saveOffline;
export function saveOffline(tracks) {
  for (const t of tracks) {
    if (typeof t.id !== 'number') continue;
    const conv = convert(t);
    window.EncoreNative.saveOffline(JSON.stringify({
      id: String(t.id), title: t.title || '', artist: t.artist || '', album: t.album || '', ext: conv ? 'flac' : (t.format || 'audio'),
      url: new URL(`/api/library/tracks/${t.id}/stream${conv ? '?transcode=true' : ''}`, location.href).href,
      art: t.album_id ? new URL(`/api/library/albums/${t.album_id}/cover?s=600`, location.href).href : '',
    }));
  }
}

/**
 * Start the current song (or restart it at `offset`).
 * rung: kbps of the lossy stream (null = what the song should start on) · dsp: null = whatever the settings say
 * resume: the same song continues (a quality switch or a seek) – no new play / now-playing · fade: crossfade into it
 * hold: load it paused (a restored spot) – it starts when you press play
 */
function load({ transcode = false, offset = 0, rung = null, dsp = null, resume = false, fade: fading = false, hold = false } = {}) {
  const t = current();
  if (!t) return;
  held = hold;
  heldAt = hold ? offset : 0;
  if (hold) playedSince = false; else if (!resume) playedSince = true;
  if (!fading) endFade();
  const start = !offset && !resume;
  markPlayed(t);
  if (convert(t)) transcode = true;
  const prevRung = state.rung;
  state.rung = t.preview ? 0 : (rung === null ? rungFor(t) : rung);
  if (start && !t.preview && prefs.quality === 'auto') noteRungChange(prevRung, state.rung);
  state.lossy = state.rung > 0;
  state.dsp = (dsp === null ? wantDsp(t, state.lossy) : dsp) && wantDsp(t, state.lossy);
  state.dspRate = state.dsp ? dspOutRate(t) : 0;
  if (state.dsp && !INFO.has(t.id)) trackInfo(t).then(() => { if (current() === t && state.dsp) { state.dspRate = dspOutRate(t); render(); emit(); } });
  state.transcoded = !t.preview && (transcode || state.lossy || state.dsp);
  // server-made streams restart at the seek point (their clock starts at `offset`); the original file and a
  // pre-rendered Cadenza file are whole files the browser seeks in itself
  const serverOffset = state.transcoded && !(state.dsp && dspCache.get(t.id) === 'cached') ? offset : 0;
  state.offset = serverOffset;
  state.dragging = false;
  if (start) {
    state.logged = false;
    state.stalls = [];
    state.stepAt = 0;
    if (!hold) announce(t);
  }
  const url = streamUrl(t, { transcode: state.transcoded && !state.lossy && !state.dsp, rung: state.rung, dsp: state.dsp, offset: serverOffset });
  const usePlain = !!(t.preview && fx.built);
  if (!offset && !usePlain && spare && spare.dataset.url === url) {
    // already buffered in the background: swap players and start right away
    const old = audio;
    audio = spare;
    // the preview element never pre-loads: the parked wrapped one takes the spare's place
    spare = old === plain ? parked : old;
    if (old === plain) parked = null;
    if (!fading) { old.pause(); old.removeAttribute('src'); }
    delete old.dataset.url;
    audio.currentTime = 0;
    setFadeGain(audio, fading ? 0 : 1);
  } else {
    if (usePlain !== (audio === plain)) {
      // a preview with the effects graph built plays on the never-wrapped element; anything else on a wrapped one
      const old = audio;
      old.pause(); old.removeAttribute('src'); delete old.dataset.url;
      if (usePlain) { parked = old; audio = plain; } else { audio = parked; parked = null; }
      setFadeGain(audio, 1);
    }
    // without the pre-render cache a pre-buffered processed stream holds one of the server's Cadenza slots: free it first
    if (state.dsp && !dspCacheOn() && spare?.dataset.url?.includes('dsp=1')) cancelPreload();
    audio.src = url;
  }
  audio.dataset.url = url;
  if (offset && !serverOffset) {
    const el = audio;
    const go = () => { try { el.currentTime = offset; } catch { /* not seekable yet */ } };
    if (el.readyState >= 1) go(); else el.addEventListener('loadedmetadata', go, { once: true });
  }
  if (playback.fxActive(prefs)) ensureFx();
  route();
  resumeFx();
  if (!hold) audio.play().catch(() => {});
  clearTimeout(state.slowStart);
  if (start && !hold && !t.preview && prefs.quality === 'auto') {
    // can't even get going: six seconds in and nothing has played yet → one rung down
    state.slowStart = setTimeout(() => { if (current() === t && !audio.paused && audio.currentTime < 0.5 && audio.readyState < 3) stepDownNow(); }, 6000);
  }
  if (start && state.dsp && dspCacheOn()) prepareAhead(t);
  if ('mediaSession' in navigator) {
    navigator.mediaSession.metadata = new MediaMetadata({
      title: t.title || '', artist: t.artist || '', album: t.album || '',
      artwork: cover(t) ? [{ src: cover(t), sizes: '512x512' }] : [],
    });
  }
  document.body.classList.add('has-player');
  render();
  emit();
  if (!hold) saveSpot(true);
  if (state.full) loadLyrics();
  topUp();
}
/** "now playing" on Last.fm / ListenBrainz – once per song start. */
function announce(t) {
  if (t && typeof t.id === 'number' && state.announced !== t) { state.announced = t; api.post('/library/now-playing', { track_id: t.id }).catch(() => {}); }
}

// ---- remember where you left off: this device's queue and spot, put back paused when the app opens --------
const RESUME = 'encore.resume';
let held = false;      // a restored spot sits paused and hasn't played yet: its position may not be applied yet, so it isn't saved over
let heldAt = 0;        // where the held song was restored to (the original file only seeks there once its metadata is in)
let playedSince = false; // music actually played since the last save: only then is saved_at moved on
let spotSavedAt = 0;
const isLib = (x) => !!x && !x.preview && typeof x.id === 'number';
/** Save the queue and the spot (every 5 s while playing; at once on pause, a new song and when the page goes away). */
function saveSpot(force = false) {
  if (link || held || !audio) return;
  const t = current();
  if (!isLib(t)) return;
  const now = Date.now();
  if (!force && now - spotSavedAt < 5000) return;
  spotSavedAt = now;
  const { queue, index } = libWindow();
  // saved_at is when music last played here: a pause or a page closed while paused keeps the old one, so a
  // song played on another device since still wins when the app opens again
  const savedAt = playedSince ? now / 1000 : (Number(savedSpot()?.saved_at) || now / 1000);
  if (audio.paused) playedSince = false;
  try {
    localStorage.setItem(RESUME, JSON.stringify({ queue: queue.map(slim), index, position: position(), shuffle: state.context === 'library',
      repeat: state.repeat, context: state.context, saved_at: savedAt }));
  } catch { /* private mode */ }
}
/** Where the song is – for a held (restored) song whose file hasn't loaded yet, where it was restored to. */
function spotPosition() {
  return held && heldAt && audio.readyState < 1 ? heldAt : position();
}
/** What saveSpot saved on this device: { queue, index, position, shuffle, repeat, context, saved_at } – null when nothing is saved. */
export function savedSpot() {
  try {
    const s = JSON.parse(localStorage.getItem(RESUME) || 'null');
    return s && Array.isArray(s.queue) && s.queue.some(isLib) ? s : null;
  } catch { return null; }
}
/** Put a saved spot back: the queue, paused at the song and position you left off (one tap plays). */
export function restore(s = savedSpot()) {
  if (!s || link || current() || !audio || !Array.isArray(s.queue)) return false;
  const q = s.queue.filter(isLib);
  if (!q.length) return false;
  state.queue = q;
  state.index = Math.max(0, Math.min(Number(s.index) || 0, q.length - 1));
  state.context = ['album', 'single', 'list', 'library'].includes(s.context) ? s.context : s.shuffle ? 'library' : 'list';
  if (['off', 'all', 'one'].includes(s.repeat)) state.repeat = s.repeat;
  const t = current();
  const at = Number(s.position) || 0;
  load({ offset: at > 1 && !(t.duration > 0 && at > t.duration - 2) ? at : 0, hold: true });
  return true;
}

// ---- keeping the music going on a weak connection ---------------------------------------------
/** Seconds of this song already in the buffer past the play position. */
function bufferedAhead() {
  try {
    const b = audio.buffered, p = audio.currentTime;
    for (let i = 0; i < b.length; i++) if (b.start(i) <= p + 0.5 && b.end(i) >= p) return b.end(i) - p;
  } catch { /* no ranges yet */ }
  return 0;
}
function fullyBuffered() {
  const d = audio.duration;
  if (!d || !isFinite(d)) return false;
  const b = audio.buffered;
  return b.length > 0 && b.end(b.length - 1) >= d - 1;
}
/** Start buffering the song that plays next – once this one is safely ahead, never while it's short of bandwidth. */
function maybePreload(force = false) {
  if (!spare || !prefs.preload || link || fade) return;
  const n = state.queue[state.index + 1];
  if (!n || (n.preview && fx.built)) return;   // a preview plays on the never-wrapped element, not the spare
  const url = startUrl(n);
  if (spare.dataset.url === url) return;
  const t = current();
  const remaining = duration() - position();
  const quiet = Date.now() - state.stallAt > 15000;
  if (url.includes('dsp=1') && dspCacheOn() && state.dsp) {
    // a processed next song: ask the server to render it ahead, and buffer it only once that render exists
    // (cached or in progress) – then the pre-load takes no extra processing slot
    const s = dspCache.get(n.id);
    if (s !== 'cached' && s !== 'rendering') {
      if (Date.now() - (dspAsked.get(n.id) || 0) > 15000) prepareDsp(n).then(() => maybePreload(force));
      return;
    }
    if (!force && !(fullyBuffered() || (quiet && bufferedAhead() > 45) || remaining < 40)) return;
  } else if (!force) {
    const ok = (t?.preview || state.transcoded) ? remaining < 40 : (fullyBuffered() || (quiet && bufferedAhead() > 45) || remaining < 30);
    if (!ok) return;
  }
  spare.dataset.url = url;
  spare.src = url;
  spare.load();
}
const preloadNext = () => maybePreload(true);
function cancelPreload() {
  if (!spare?.dataset.url || fade) return;
  spare.removeAttribute('src');
  delete spare.dataset.url;
  spare.load();
}
/** The browser ran out of buffer mid-song. Give it the bandwidth back, and on Auto quality step the stream down. */
function stalled() {
  const now = Date.now();
  if (now - state.seekAt < 3000 || position() < 1 || audio.paused) return;   // a seek, a switch or the start, not a slow link
  state.stallAt = now;
  state.stalls.push(now);
  if (!fullyBuffered()) cancelPreload();
  if (prefs.quality !== 'auto' || current()?.preview) return;
  // stalling again within 90 s of stepping down: the new rung isn't small enough either
  if (state.stepAt && now - state.stepAt < 90000) return stepDownNow();
  const recent = state.stalls.filter(s => now - s < 90000);
  if (recent.length >= 2) return stepDownNow();
  // one long stall counts too: still waiting four seconds later
  setTimeout(() => { if (!audio.paused && audio.readyState < 3 && state.stallAt === now) stepDownNow(); }, 4000);
}
/** Auto: a stall forces one rung down right away, and the estimate drops to 60 % of what the rung needed. */
function stepDownNow() {
  const t = current();
  if (!t || t.preview || prefs.quality !== 'auto') return;
  clearTimeout(state.slowStart);
  const orig = origKbps(t);
  const lower = playback.stepDown(state.rung, orig);
  if (lower === state.rung) return;   // already at 32 kbps
  est.kbps = 0.6 * playback.needed(state.rung, orig);
  est.at = Date.now();
  est.fresh = false;
  est.upSince = 0;
  state.stalls = [];
  state.stepAt = Date.now();
  switchRung(lower);
}
/** Reload the song at the current position on another rung. */
function switchRung(rung) {
  const t = current();
  if (!t) return;
  const at = position();
  noteRungChange(state.rung, rung);
  state.switchedAt = state.seekAt = Date.now();
  load({ rung, offset: at, resume: true, transcode: rung > 0 });
}
let slowToastAt = 0;
function noteRungChange(was, now) {
  if (was === now) return;
  if (now > 0 && (was === 0 || now < was)) {
    if (Date.now() - slowToastAt > 60000) { slowToastAt = Date.now(); toast(`Connection is slow – lowering quality to ${now} kbps to keep playing`, ''); }
  } else if (now === 0) toast('Connection recovered – back to the original file', 'ok');
}

// ---- the bandwidth estimate (Auto quality) -----------------------------------------------------
// An exponentially weighted average of measured throughput in kbit/s: from how fast the playing song's buffer
// grows, from a small probe when nothing is downloading, and – until the first sample – the browser's own guess.
const est = { kbps: null, at: 0, fresh: false, upSince: 0, el: null, url: '', end: 0, wall: 0, probing: false, probeAt: 0 };
try { const d = navigator.connection?.downlink; if (d > 0) { est.kbps = d * 1000; est.at = Date.now(); } } catch { /* no Network Information API */ }
function addSample(kbps) {
  if (!(kbps >= 0) || !isFinite(kbps)) return;
  est.kbps = playback.ewma(est.kbps, kbps);
  est.at = Date.now();
  est.fresh = true;
}
/** kbit/s of the stream the element is playing (null when the server, not the network, sets the pace). */
function streamKbps() {
  if (state.rung > 0) return state.rung;
  if (state.dsp) return null;   // Cadenza renders at its own speed: its download rate says nothing about the connection
  return fileKbps(current()) || 1411;
}
/** Every 2 s: how far the buffer grew × the stream's bitrate ÷ the time it took. Returns whether a download is in progress. */
function sampleProgress() {
  const now = performance.now();
  let end = 0;
  try { const b = audio.buffered; if (b.length) end = b.end(b.length - 1); } catch { /* no ranges */ }
  const d = audio.duration;
  const downloading = audio.networkState === 2 && !(isFinite(d) && d > 0 && end >= d - 0.5);
  const url = audio.dataset.url || '';
  const kbps = streamKbps();
  // only while the element is still downloading: a finished download would read as a slow link
  if (est.el !== audio || est.url !== url || !downloading || !kbps) {
    est.el = audio; est.url = url; est.end = end; est.wall = now;
    return downloading;
  }
  const secs = (now - est.wall) / 1000;
  const grown = end - est.end;
  est.end = end; est.wall = now;
  if (secs < 0.25 || grown < 0) return true;
  if (grown === 0 && bufferedAhead() > 30) return true;   // the browser holding back with a full buffer, not a slow link
  addSample(grown * kbps / secs);
  return true;
}
/** Probe sizes: 128 KB, and when that arrives in under 0.25 s (too fast to time), 1 MB, then 4 MB. */
const PROBE_SIZES = [131072, 1048576, 4194304];
/** Fetch the first `size` bytes of a song, timed: { bytes, secs }, or null when the server refuses. */
async function probeOnce(id, size) {
  const t0 = performance.now();
  const r = await fetch(`/api/library/tracks/${id}/stream`, { headers: { Range: `bytes=0-${size - 1}` }, credentials: 'same-origin', cache: 'no-store' });
  if (r.status !== 206 && r.status !== 200) { r.body?.cancel().catch(() => {}); return null; }
  let bytes = 0;
  const reader = r.body.getReader();
  while (bytes < size) {
    const { done, value } = await reader.read();
    if (done) break;
    bytes += value.byteLength;
  }
  reader.cancel().catch(() => {});
  return { bytes: Math.min(bytes, size), secs: (performance.now() - t0) / 1000 };
}
/** Nothing downloading and the estimate is older than 20 s: time a piece of the current or next song. */
async function probe() {
  const now = Date.now();
  if (est.probing || now - est.at < 20000 || now - est.probeAt < 20000) return;
  const t = [current(), state.queue[state.index + 1]].find(x => x && !x.preview && typeof x.id === 'number' && !convert(x));
  if (!t) return;
  est.probing = true;
  est.probeAt = now;
  try {
    for (const size of PROBE_SIZES) {
      const p = await probeOnce(t.id, size);
      if (!p) return;
      const kbps = playback.rateOf(p.bytes, p.secs);
      if (kbps !== null) { addSample(kbps); return; }
      if (p.bytes < size || size === PROBE_SIZES[PROBE_SIZES.length - 1]) {
        // still under 0.25 s (the whole song, or 4 MB): "at least this fast" – it may lift the estimate, never lower it
        const atLeast = p.bytes * 8 / 1000 / 0.25;
        if (est.kbps === null || est.kbps < atLeast) addSample(atLeast);
        return;
      }
    }
  } catch { /* offline: try again later */ } finally { est.probing = false; }
}
function adaptTick() {
  if (link || !audio || !current() || current().preview) return;
  const downloading = sampleProgress();
  if (audio.paused) { est.upSince = 0; return; }
  if (!downloading) probe();
  if (prefs.quality === 'auto') decide();
}
/** Auto: follow the estimate – down at once (when the buffer is short), up when it has held 1.5× the higher rung's need for 15 s. */
function decide() {
  const t = current();
  const orig = origKbps(t);
  const l = playback.ladder(orig);
  const ci = l.indexOf(state.rung);
  const fresh = est.fresh;
  est.fresh = false;
  if (est.kbps === null || ci < 0) return;
  const now = Date.now();
  const ti = l.indexOf(playback.chooseRung(est.kbps, orig));
  if (ti > ci) {
    est.upSince = 0;
    if (fresh && now - state.switchedAt > 10000 && !fullyBuffered() && bufferedAhead() < 20) switchRung(l[ti]);
    return;
  }
  const up = l.find(r => playback.needed(r, orig) * 1.5 <= est.kbps);
  if (up === undefined || l.indexOf(up) >= ci) { est.upSince = 0; return; }
  if (!est.upSince) { est.upSince = now; return; }
  // safe to reload: 10 s in hand, or a server-made stream the browser has finished fetching (it shows little of it as buffered)
  const safe = bufferedAhead() > 10 || (state.transcoded && audio.networkState !== 2);
  if (now - est.upSince >= 15000 && safe) { est.upSince = 0; switchRung(up); }
}

// ---- server-side Cadenza: render ahead -----------------------------------------------------------
/** Ask the server to render a song ahead (or say how far it is): 'cached' | 'rendering' | 'queued' | 'busy'. */
async function prepareDsp(t) {
  if (!t || typeof t.id !== 'number') return null;
  dspAsked.set(t.id, Date.now());
  try {
    const r = await api.post('/playback/dsp/prepare', { track_id: t.id, rate: dspRequestRate(), filter: prefs.dsp_filter, dither: prefs.dsp_dither });
    const s = typeof r?.state === 'string' ? r.state : '';
    dspCache.set(t.id, s);
    return s;
  } catch { return null; }
}
/** The current song started as a processed stream: learn whether it is pre-rendered, and have the next one rendered now. */
function prepareAhead(t) {
  prepareDsp(t);
  const n = state.queue[state.index + 1];
  if (n && wantDsp(n, rungFor(n) > 0)) prepareDsp(n).then(() => maybePreload());
}
const dspNote = (s) => s === 'cached' ? ' (pre-rendered)' : s === 'rendering' || s === 'queued' ? ' (rendering ahead)' : '';

// ---- crossfade ---------------------------------------------------------------------------------
// The spare element already holds the next song: start it `crossfade` seconds before this one ends and blend
// the two with an equal-power curve (Web Audio gains when the effects graph runs, element volume otherwise).
let fade = null;   // { out: the element fading out, len: seconds, timer }
function maybeCrossfade() {
  const cf = prefs.crossfade;
  if (!cf || fade || link || state.repeat === 'one' || audio.paused || state.dragging) return;
  const t = current(), n = state.queue[state.index + 1];
  if (!t || !n) return;
  const d = duration();
  if (!d || d < 2 * cf) return;
  const remaining = d - position();
  if (remaining > cf || remaining < 1) return;
  if (prefs.crossfade_album_gapless && t.album_id != null && t.album_id === n.album_id) return;
  if (!spare.dataset.url || spare.dataset.url !== startUrl(n) || spare.readyState < 3) return;
  logPlay();
  fade = { out: audio, len: remaining, timer: null };
  state.index++;
  load({ fade: true });
  fade.timer = setInterval(stepFade, 50);
}
function stepFade() {
  if (!fade) return;
  const x = Math.min(1, (audio.currentTime || 0) / fade.len);
  setFadeGain(fade.out, Math.cos(x * Math.PI / 2));
  setFadeGain(audio, Math.sin(x * Math.PI / 2));
  if (x >= 1 || fade.out.ended) endFade();
}
/** Finish (or cut short – next/prev/seek) a crossfade: the old song stops, the new one plays at full volume. */
function endFade() {
  if (!fade) return;
  clearInterval(fade.timer);
  const out = fade.out;
  fade = null;
  out.pause();
  out.removeAttribute('src');
  delete out.dataset.url;
  try { out.load(); } catch { /* ignore */ }
  setFadeGain(out, 1);
  setFadeGain(audio, 1);
  route();
  maybePreload();
}
/** The fade level of one element: a Web Audio gain when it is routed through the effects graph, else its volume (under the slider). */
function setFadeGain(el, g) {
  if (!el) return;
  const s = fx.sources.get(el);
  if (s?.gain.gain) { s.gain.gain.value = g; el.volume = userVol; } else el.volume = userVol * g;
}

// ---- sound effects on this device (Web Audio) ----------------------------------------------------
// Built the first time an effect is switched on; until then the <audio> elements play straight out, untouched.
// An element wrapped in a MediaElementSource can only be heard through the graph, so once built the graph stays
// and turns into a pass-through when the effects are switched off.
const fx = { built: false, broken: false, sources: new Map(), input: null, key: '', eq: [], cross: [], trim: null, pan: null, nodes: [] };
const dbGain = (v) => Math.pow(10, v / 20);
const XFEED_DB = { light: -9, medium: -6, strong: -4.5 };
function ensureFx() {
  if (fx.built) return true;
  // a preview playing now stays out of the graph (it is built at the next library song)
  if (!audio || !spare || current()?.preview) return false;
  outputRate();
  if (!ctx) return false;
  try {
    fx.input = ctx.createGain();
    for (const el of [audio, spare]) {
      el.crossOrigin = 'anonymous';
      const src = ctx.createMediaElementSource(el);
      // kept at once: an element can be wrapped only once, and from now on is heard only through the graph
      const s = { src, gain: src, on: false };
      fx.sources.set(el, s);
      s.gain = ctx.createGain();
      src.connect(s.gain);
    }
  } catch {
    if (!fx.sources.size) { fx.input = null; return false; }   // nothing wrapped: the elements still play straight out
    // half built: keep what is wrapped audible through a plain pass-through
    fx.broken = true;
  }
  fx.built = true;
  fx.key = '';
  if (fx.broken) { try { fx.input?.connect(ctx.destination); } catch { /* ignore */ } } else applyFx();
  route();
  resumeFx();
  return true;
}
/** Resume the graph's context on play: suspended by the autoplay policy, or 'interrupted' (Safari after a call or Siri). */
function resumeFx() { if (fx.built && ctx && ctx.state !== 'running') ctx.resume().catch(() => {}); }
/** Only the element playing (and the one fading out) feeds the graph. */
function route() {
  if (!fx.built) return;
  for (const [el, s] of fx.sources) {
    const want = el === audio || (fade && el === fade.out);
    if (want && !s.on) { s.gain.connect(fx.input || ctx.destination); s.on = true; }
    else if (!want && s.on) { s.gain.disconnect(); s.on = false; }
  }
}
/** Bring the graph in line with the settings: rewire only when its shape changes, otherwise just set the values. */
function applyFx() {
  if (!fx.built || fx.broken) return;
  const on = playback.fxActive(prefs);
  const bands = on ? playback.eqBands(prefs) : [];
  const mono = on && prefs.mono;
  const xfeed = on && !mono ? prefs.crossfeed : 'off';
  const key = JSON.stringify([on, bands.map(b => b.type), xfeed !== 'off', mono]);
  if (key !== fx.key) {
    fx.key = key;
    try { fx.input.disconnect(); } catch { /* not connected */ }
    fx.nodes.forEach(n => { try { n.disconnect(); } catch { /* ignore */ } });
    fx.nodes = []; fx.eq = []; fx.cross = []; fx.trim = null; fx.pan = null;
    let last = fx.input;
    const add = (n) => { fx.nodes.push(n); last.connect(n); last = n; return n; };
    if (on) {
      for (const b of bands) fx.eq.push(add(Object.assign(ctx.createBiquadFilter(), { type: b.type })));
      if (xfeed !== 'off') {
        // each channel gets a low-passed, slightly delayed, quieter copy of the other one
        const pre = add(ctx.createGain());
        pre.channelCount = 2; pre.channelCountMode = 'explicit'; pre.channelInterpretation = 'speakers';
        const split = ctx.createChannelSplitter(2), merge = ctx.createChannelMerger(2);
        fx.nodes.push(split, merge);
        pre.connect(split);
        split.connect(merge, 0, 0);
        split.connect(merge, 1, 1);
        for (const [from, to] of [[0, 1], [1, 0]]) {
          const lp = Object.assign(ctx.createBiquadFilter(), { type: 'lowpass' });
          lp.frequency.value = 700; lp.Q.value = 0.5;
          const dl = ctx.createDelay(0.01);
          dl.delayTime.value = 0.0003;
          const g = ctx.createGain();
          fx.nodes.push(lp, dl, g);
          split.connect(lp, from); lp.connect(dl); dl.connect(g); g.connect(merge, 0, to);
          fx.cross.push(g);
        }
        last = merge;
        fx.trim = add(ctx.createGain());
      }
      if (mono) {
        const m = add(ctx.createGain());
        m.channelCount = 1; m.channelCountMode = 'explicit'; m.channelInterpretation = 'speakers';
        // back to two equal channels, so the panner sees stereo and stays at unity in the middle (a mono input would
        // take the panner's mono law: −3 dB)
        const up = add(ctx.createGain());
        up.channelCount = 2; up.channelCountMode = 'explicit'; up.channelInterpretation = 'speakers';
      }
      fx.pan = add(ctx.createStereoPanner());
    }
    last.connect(ctx.destination);
  }
  fx.input.gain.value = on ? dbGain(prefs.eq_preamp) : 1;
  fx.eq.forEach((f, i) => {
    const b = bands[i];
    f.type = b.type;
    f.frequency.value = b.freq;
    f.gain.value = b.gain;
    f.Q.value = b.q;
  });
  if (fx.cross.length) {
    const g = dbGain(XFEED_DB[xfeed] ?? -6);
    fx.cross.forEach(n => { n.gain.value = g; });
    fx.trim.gain.value = 1 / (1 + g);
  }
  if (fx.pan) fx.pan.pan.value = prefs.balance / 100;
}

/** Near the end of the queue: fetch the library shuffle ahead of time, so the next song is ready too. */
let topping = false;
async function topUp() {
  if (topping || state.context === 'preview' || state.index < state.queue.length - 2) return;
  if (!prefs.autoplay) return;
  topping = true;
  try {
    const more = await freshShuffle(50);
    if (more.length) {
      state.queue.push(...more);
      render(); emit();
      maybePreload();
    }
  } catch { /* offline: next() tries again */ }
  topping = false;
}

function duration() {
  const t = current();
  if (t?.preview) return audio.duration && isFinite(audio.duration) ? audio.duration : 30;
  if (state.transcoded) return t?.duration || 0;
  return audio.duration && isFinite(audio.duration) ? audio.duration : t?.duration || 0;
}
const position = () => state.offset + (audio.currentTime || 0);

export function seekTo(sec) {
  if (viaLink({ action: 'seek', value: Math.max(0, sec) })) { if (link.state) link.state.position = sec; return updateLinkedTime(); }
  const dur = duration();
  if (!current() || !dur) return;
  sec = Math.max(0, Math.min(sec, dur - 0.25));
  if (held) heldAt = sec;
  endFade();
  state.seekAt = Date.now();
  const t = current();
  if (state.dsp && dspCacheOn() && dspCache.get(t.id) !== 'cached' && Date.now() - (dspAsked.get(t.id) || 0) > 3000) {
    // maybe the render has finished since: then the browser seeks in the pre-rendered file itself
    prepareDsp(t).then(() => { if (current() === t) seekNow(sec); });
  } else seekNow(sec);
  updateTime();
  emit();
}
/** Whole files (the original, a pre-rendered Cadenza file) seek in the browser; server-made streams restart at the point. */
function seekNow(sec) {
  const t = current();
  const cached = state.dsp && dspCache.get(t?.id) === 'cached';
  let local = !state.transcoded;
  if (cached && state.offset === 0) {
    try { const s = audio.seekable; local = s.length > 0 && s.end(s.length - 1) >= sec - 0.5; } catch { local = false; }
  }
  state.seekAt = Date.now();
  if (local) audio.currentTime = sec;
  else load({ transcode: true, rung: state.rung, dsp: state.dsp, offset: sec, resume: true });
  updateTime();
}

export function toggle() {
  if (viaLink({ action: 'toggle' })) { if (link.state) link.state.playing = !link.state.playing; render(); if (state.full) renderFull(); return; }
  if (current()) { resumeFx(); (audio.paused ? audio.play() : audio.pause()); }
}

export async function next(auto = false) {
  if (!auto && viaLink({ action: 'next' })) return;
  if (auto && state.repeat === 'one') { seekTo(0); audio.play(); return; }
  if (state.index < state.queue.length - 1) { state.index++; load(); return; }
  if (state.repeat === 'all' && state.queue.length) { state.index = 0; load(); return; }
  if (state.context === 'preview') return;
  if (await continueWithLibrary()) { state.index++; load(); }
}

/** Queue ran out: keep the music going with a shuffle of the whole library. */
async function continueWithLibrary() {
  if (!prefs.autoplay) return false;
  try {
    const more = await freshShuffle(50);
    if (!more.length) return false;
    state.queue.push(...more);
    state.context = 'library';
    return true;
  } catch { return false; }
}

export function prev() {
  if (viaLink({ action: 'prev' })) return;
  if (position() > 3 || state.index === 0) { seekTo(0); return; }
  state.index--; load();
}

function jump(i) { state.index = i; load(); }
function removeAt(i) {
  if (i === state.index) return;
  state.queue.splice(i, 1);
  if (i < state.index) state.index--;
  render(); emit(); preloadNext();
}
function moveTo(from, to) {
  if (from === state.index || to <= state.index || to >= state.queue.length) return;
  const [t] = state.queue.splice(from, 1);
  state.queue.splice(to, 0, t);
  render(); emit(); preloadNext();
}
function shuffleUpNext() {
  const rest = state.queue.slice(state.index + 1).sort(() => Math.random() - 0.5);
  state.queue.splice(state.index + 1, rest.length, ...rest);
  render(); emit(); preloadNext();
}

function logPlay() {
  const t = current();
  if (!t || t.preview || state.logged || typeof t.id !== 'number') return;
  const p = position(), d = duration();
  // counts (and scrobbles) like Last.fm: half the song or 4 minutes, whichever comes first; very short songs at the end
  if (d ? (d < 30 ? p >= d - 1 : p >= Math.min(d / 2, 240)) : p >= 30) {
    state.logged = true;
    api.post('/library/plays', { track_id: t.id, seconds: p }).catch(() => {});
  }
}

// ---- likes, sharing and the ⋯ menu for the song in front of you -------------------------------------
async function likes() {
  if (!liked) { try { liked = new Set(await api.get('/likes')); } catch { liked = new Set(); } }
  return liked;
}
const isLiked = (t) => !!(t && typeof t.id === 'number' && liked?.has(t.id));
async function toggleLike(t) {
  if (!t || typeof t.id !== 'number') return;
  const s = await likes();
  try {
    if (s.has(t.id)) { await api.del(`/likes/${t.id}`); s.delete(t.id); toast('Removed from Liked Songs', 'ok'); }
    else { await api.post(`/likes/${t.id}`); s.add(t.id); toast('Added to Liked Songs', 'ok'); }
  } catch (e) { toast(e.message, 'err'); }
  window.dispatchEvent(new CustomEvent('encore:likes'));
  paintLikes();
}
/** Update every heart on screen without redrawing the whole bar. */
function paintLikes() {
  const t = nowTrack();
  document.querySelectorAll('.btn.like').forEach(b => { b.classList.toggle('on', isLiked(t)); b.title = isLiked(t) ? 'Liked – tap to remove' : 'Like'; });
}
const likeButton = (id, cls = '') => `<button class="btn ghost icon like ${cls}" id="${id}" aria-label="Like" title="Like">${icon.heart}</button>`;
const canLike = (t) => t && typeof t.id === 'number';

/** "⋯" for the song in front of you (this device's or the linked device's). */
function moreMenu(anchor) {
  const t = nowTrack();
  if (!t) return;
  const lib = typeof t.id === 'number';
  popMenu(anchor, [
    t.artist && !t.preview ? { label: 'Go to artist', icon: icon.user, href: `#/artist-name/${encodeURIComponent(t.artist)}` } : null,
    t.album_id ? { label: 'View album', icon: icon.disc, href: `#/library/album/${t.album_id}` } : null,
    lib ? { label: isLiked(t) ? 'Remove from Liked Songs' : 'Like', icon: icon.heart, onClick: () => toggleLike(t) } : null,
    lib ? { label: 'Add to playlist', icon: icon.plus, onClick: async () => (await import('./pages/playlists.js')).addToPlaylistModal([t.id]) } : null,
    lib ? { label: 'Lyrics', icon: icon.music, onClick: () => { state.pane = 'lyrics'; setFull(true); } } : null,
    lib ? { label: 'Share link', icon: icon.external, onClick: () => shareTrack(t) } : null,
    lib ? { label: 'Download this song', icon: icon.download, href: `/api/library/tracks/${t.id}/file`, download: true } : null,
    lib && !link && canSaveOffline() ? { label: 'Save for offline', icon: icon.download, onClick: () => { saveOffline([t]); toast('Saving to this phone…', 'ok'); } } : null,
    { label: 'Signal path', icon: icon.activity, onClick: signalPath },
    window.encoreRemote ? { label: link ? 'Your devices…' : 'Play on another device…', icon: icon.devices, onClick: () => window.encoreRemote.open() } : null,
  ]);
}

// ---- signal path: what the file is, what Encore sends, what this device does with it -----------------
async function trackInfo(t) {
  if (!t || typeof t.id !== 'number') return null;
  if (INFO.has(t.id)) return INFO.get(t.id);
  try { const i = await api.get(`/library/tracks/${t.id}/info`); INFO.set(t.id, i); return i; } catch { return null; }
}
/** What leaves the server for this device. */
function transport() {
  const t = current();
  if (!t) return null;
  if (t.preview) return { kind: 'preview', text: 'A 30-second preview from the catalogue (lossy) – not your file' };
  if (state.rung > 0) return { kind: `${LOSSY}-${state.rung}`, text: `${LOSSY.toUpperCase()} ${state.rung} kbps, lossy ${prefs.quality === 'saver' ? '(Data saver is on)' : '(adaptive – the connection could not keep up)'}` };
  if (state.dsp) return { kind: `cadenza-${state.dspRate}`, text: `Upsampled on the server by Cadenza to 24-bit / ${khz(state.dspRate)} kHz (lossless FLAC)${dspNote(dspCache.get(t.id))}` };
  if (state.transcoded) return { kind: 'flac', text: `Converted to FLAC by the server (lossless) – browsers can't play ${(t.format || '').toUpperCase()}` };
  return { kind: 'original', text: 'The original file, byte for byte – nothing was converted or resampled on the way' };
}
/** This device's output sample rate in Hz (0 if unknown) – Settings → Playback shows it for "Match this device". */
export function deviceRate() { return outputRate(); }
function outputRate() {
  try { if (!ctx) ctx = new (window.AudioContext || window.webkitAudioContext)(); return ctx.sampleRate; } catch { return 0; }
}
const khz = (hz) => String(Math.round(hz / 100) / 10);
/** kbit/s → "0.25" / "4.2" / "12" (Mbit/s) */
const mbit = (kbps) => kbps < 1000 ? (kbps / 1000).toFixed(2) : kbps < 10000 ? (kbps / 1000).toFixed(1) : String(Math.round(kbps / 1000));
function sourceLine(t, info) {
  if (!info) return `${(t.format || '').toUpperCase()}${t.bitrate ? ` · ${t.bitrate} kbps` : ''}`;
  const parts = [(info.codec || t.format || '').toUpperCase()];
  if (info.lossless && info.bit_depth && info.sample_rate) parts.push(`${info.bit_depth}-bit / ${khz(info.sample_rate)} kHz`);
  else if (info.sample_rate) parts.push(`${khz(info.sample_rate)} kHz`);
  if (info.channels) parts.push(info.channels === 2 ? 'stereo' : info.channels === 1 ? 'mono' : `${info.channels} channels`);
  if (info.bitrate) parts.push(`${info.bitrate} kbps`);
  return parts.join(' · ');
}
/** The Signal path window: three rows, and a plain verdict on whether what you hear is the file untouched. */
async function signalPath() {
  const { modal } = await import('./ui.js');
  const t = nowTrack();
  if (!t) return;
  const m = modal({ title: 'Signal path', body: '<div class="skeleton" style="height:120px"></div>' });
  const info = await trackInfo(t);
  if (!link && state.dsp && dspCacheOn() && current() === t) await prepareDsp(t);   // pre-rendered yet?
  const rows = [];
  rows.push(['Source', sourceLine(t, info), info?.lossless === false ? 'A lossy file: what the file lost is gone before Encore touches it' : '']);
  // a Cadenza device reports every stage of its own pipeline; a browser device just plays
  const stages = link && Array.isArray(link.state?.signal?.stages) ? link.state.signal.stages : [];
  const localEngine = !!link && window.encoreRemote?.engine?.()?.id === link.id;
  if (link) {
    const sig = link.state?.signal || {};
    const eng = link.state?.engine || {};
    rows.push([`Encore → ${esc(link.name)}`, sig.transport === 'original' ? 'The original file, byte for byte' : sig.transport ? (sig.transport === 'flac' ? 'Converted to FLAC by the server (lossless)' : sig.transport.startsWith('cadenza-') ? `Upsampled on the server by Cadenza (${khz(Number(sig.transport.slice(8)) || 0)} kHz)` : /^(aac|opus)-/.test(sig.transport) ? `${sig.transport.split('-')[0].toUpperCase()} ${sig.transport.split('-')[1]} kbps (lossy)` : sig.transport) : 'Not reported by that device', '']);
    if (stages.length) {
      for (const st of stages) rows.push([esc(st?.name || ''), String(st?.detail ?? ''), '']);
      rows.push(['Output', `${eng.output || ''}${eng.exclusive ? ' · exclusive' : ''}${sig.bitperfect ? ' · bit-perfect' : ''}`, '']);
    } else rows.push([esc(link.name), sig.output_rate ? `Plays through its browser audio at ${khz(sig.output_rate)} kHz` : 'Playing there', '']);
  } else {
    const tr = transport();
    const net = est.kbps > 0 ? ` · connection ≈ ${mbit(est.kbps)} Mbit/s` : '';
    rows.push(['Encore → this device', (tr?.text || '') + net, '']);
    const out = outputRate();
    const src = info?.sample_rate;
    const resampled = out && src && out !== src;
    rows.push(['This device', out ? `Browser audio → the system mixer at ${khz(out)} kHz${window.EncoreNative ? '' : ''}` : 'Browser audio → the system mixer',
      resampled ? `Your device resamples ${khz(src)} → ${khz(out)} kHz. A browser can't hand bits straight to the DAC; for true bit-perfect output play through a Cadenza device or an exclusive-mode player.` : out && src ? 'Same rate as the file – no resampling in the browser. Whether the operating system mixes it with other sounds is up to your audio settings.' : '']);
    const effects = playback.effectsSummary(prefs);
    if (effects) rows.push(['Effects on this device', effects, '']);
  }
  const effected = !link && playback.fxActive(prefs);
  const untouched = !link && transport()?.kind === 'original' && info?.lossless !== false;
  const pure = untouched && !effected;
  const cadenza = !link && (transport()?.kind || '').startsWith('cadenza');
  m.el.querySelector('.modal-body').innerHTML = `
    <div class="stack" style="gap:12px">
      ${rows.map(([k, v, note]) => `<div class="stack" style="gap:2px"><div class="small muted">${k}</div><div style="font-weight:600">${esc(v)}</div>${note ? `<div class="small muted">${esc(note)}</div>` : ''}</div>`).join('')}
      <div class="card" style="padding:12px"><b>${link ? (stages.length ? `Played by Cadenza on ${localEngine ? 'this computer' : esc(link.name)}` : 'Playing on another device') : cadenza ? 'Processed by Cadenza on the server' : pure ? 'Pure playback from Encore' : 'Not the untouched file'}</b>
        <div class="small muted" style="margin-top:4px">${link ? (stages.length ? 'Every stage above is what the engine reports. Bit-perfect means nothing touched the samples.' : 'The rows above are what that device reports. Open Encore there for its own signal path.')
          : cadenza ? 'Encore upsampled the file on the server with Cadenza and sent lossless 24-bit FLAC. Turn it off under Settings → Playback → Server processing to get the untouched file.'
          : pure ? 'Encore sent the file exactly as it is stored. Everything after that – volume, mixing, resampling – is your browser and operating system.'
          : untouched ? 'Encore sent the file untouched; this device adds the effects you chose under Settings → Playback → Sound.'
          : 'Encore converted this stream (see above). Settings → Playback → Quality → Original keeps the file untouched even when the connection is slow.'}</div></div>
    </div>`;
}

/** The server's Cadenza is off, busy or failed: the same spot again without processing, and no more processing this session. */
function dspFallback() {
  dspFailed = true;
  const at = spotPosition();
  if (!dspToasted) { dspToasted = true; toast("Server processing isn't available right now – playing the original", ''); }
  // a restored song stays paused until you press play
  load({ transcode: at > 0.5, offset: at > 0.5 ? at : 0, dsp: false, hold: held });
}

export function mount() {
  if (document.getElementById('player')) return;
  audio = document.getElementById('audio') || Object.assign(document.createElement('audio'), { id: 'audio' });
  if (!audio.isConnected) document.body.appendChild(audio);
  spare = Object.assign(document.createElement('audio'), { id: 'audio-next', preload: 'auto' });
  document.body.appendChild(spare);
  plain = Object.assign(document.createElement('audio'), { id: 'audio-preview', preload: 'auto' });
  document.body.appendChild(plain);
  audio.preload = 'auto';
  const el = document.createElement('div');
  el.className = 'player';
  el.id = 'player';
  document.body.appendChild(el);
  const full = document.createElement('div');
  full.className = 'np-full hidden';
  full.id = 'np-full';
  document.body.appendChild(full);
  let vol = 0.9;
  try { vol = parseFloat(localStorage.getItem('encore.volume') ?? '0.9'); } catch { /* ignore */ }
  userVol = Number.isFinite(vol) ? Math.max(0, Math.min(1, vol)) : 0.9;
  audio.volume = spare.volume = plain.volume = userVol;
  // every element gets the listeners; only the one currently playing counts
  const on = (type, fn) => [audio, spare, plain].forEach(a => a.addEventListener(type, (e) => { if (e.target === audio) fn(e); }));
  on('ended', () => { logPlay(); next(true); });
  on('play', () => { held = false; heldAt = 0; playedSince = true; announce(current()); resumeFx(); if (fade) fade.out.play().catch(() => {}); render(); emit(); });
  on('pause', () => { if (fade) fade.out.pause(); render(); emit(); saveSpot(true); });
  on('timeupdate', () => {
    updateTime(); logPlay(); syncLyrics(); maybeCrossfade();
    if (!audio.paused) saveSpot();
    const now = Date.now();
    if (now - state.lastPreloadCheck > 2000) { state.lastPreloadCheck = now; maybePreload(); }
  });
  on('progress', () => { const now = Date.now(); if (now - state.lastPreloadCheck > 2000) { state.lastPreloadCheck = now; maybePreload(); } });
  on('waiting', stalled);
  on('loadedmetadata', updateTime);
  // the server's Cadenza is off, busy or failed → the same spot again without processing, and no more processing this session
  // the browser couldn't decode the original (e.g. ALAC in .m4a) → ask the server for FLAC
  on('error', () => {
    if (!current() || !audio.getAttribute('src')) return;
    if (state.dsp) return dspFallback();
    if (!state.transcoded && !current().preview) load({ transcode: true, offset: spotPosition(), hold: held });
  });
  spare.addEventListener('error', () => { delete spare.dataset.url; });
  if ('mediaSession' in navigator) {
    navigator.mediaSession.setActionHandler('play', () => audio.play());
    navigator.mediaSession.setActionHandler('pause', () => audio.pause());
    navigator.mediaSession.setActionHandler('nexttrack', () => next());
    navigator.mediaSession.setActionHandler('previoustrack', prev);
    try {
      navigator.mediaSession.setActionHandler('seekto', (d) => seekTo(d.seekTime));
      navigator.mediaSession.setActionHandler('seekforward', (d) => seekTo(position() + (d.seekOffset || 10)));
      navigator.mediaSession.setActionHandler('seekbackward', (d) => seekTo(position() - (d.seekOffset || 10)));
    } catch { /* older browsers */ }
  }
  document.addEventListener('keydown', (e) => {
    const ae = document.activeElement;
    if (e.key === 'Escape' && state.full) { if (!document.querySelector('.modal-bg, .menu-pop')) setFull(false); return; }   // a window on top closes first
    // Space is play/pause anywhere except while typing – even when a player button has focus
    // (otherwise the browser "presses" the focused button again, e.g. Previous, which restarts the song)
    const typing = (ae.tagName === 'INPUT' && !/^(range|checkbox|radio|button|submit)$/.test(ae.type)) || /TEXTAREA|SELECT/.test(ae.tagName) || ae.isContentEditable;
    if (e.code === 'Space' && !typing && !document.querySelector('.modal-bg')) {
      e.preventDefault();
      if (current() || link) toggle();
      return;
    }
    if ((!current() && !link) || (/INPUT|TEXTAREA|SELECT|BUTTON/.test(ae.tagName) && !/seek/.test(ae.id))) return;
    const at = link ? (link.state?.position || 0) : position();
    if (e.code === 'ArrowRight') { e.preventDefault(); seekTo(at + 10); }
    if (e.code === 'ArrowLeft') { e.preventDefault(); seekTo(at - 10); }
  });
  // the button press itself happens on keyup – swallow it too
  document.addEventListener('keyup', (e) => {
    const ae = document.activeElement;
    if (e.code === 'Space' && ae.tagName === 'BUTTON' && !document.querySelector('.modal-bg')) e.preventDefault();
  });
  window.addEventListener('hashchange', () => { if (state.full) setFull(false); });
  window.addEventListener('pagehide', () => saveSpot(true));
  window.addEventListener('encore:likes', () => { liked = null; likes().then(paintLikes); });
  fetchDsp();
  setInterval(adaptTick, 2000);
  render();
}

export function setFull(on) {
  state.full = on;
  const f = document.getElementById('np-full');
  f.classList.toggle('hidden', !on);
  document.body.classList.toggle('np-open', on);
  if (on) { renderFull(); loadLyrics(); }
}

function updateTime() {
  if (link) return updateLinkedTime();
  const dur = duration();
  for (const [sid, cid, did] of [['seek', 't-cur', 't-dur'], ['fseek', 'f-cur', 'f-dur']]) {
    const seek = document.getElementById(sid);
    if (!seek) continue;
    if (!state.dragging) {
      seek.value = dur ? (position() / dur) * 1000 : 0;
      document.getElementById(cid).textContent = fmt.dur(position());
    }
    seek.style.setProperty('--p', `${seek.value / 10}%`);
    document.getElementById(did).textContent = fmt.dur(dur);
  }
}

function quality(t) {
  if (!t) return '';
  if (t.remote) return link?.state?.signal?.label || (t.format || '').toUpperCase();
  if (t.preview) return '30 s preview';
  const f = (t.format || '').toUpperCase();
  if (state.rung > 0) return `${f} → ${LOSSY.toUpperCase()} ${state.rung}k`;
  if (state.dsp) return `${f} → Cadenza ${khz(state.dspRate)} kHz`;
  return state.transcoded ? `${f} → FLAC` : `${f}${t.bitrate ? ' ' + t.bitrate + 'k' : ''}`;
}
function qualityTitle(t) {
  if (t?.remote) return `What ${link?.name || 'that device'} is playing`;
  if (t?.preview) return 'A 30-second preview – not in your library';
  if (state.rung > 0) return prefs.quality === 'saver' ? 'Data saver: a small 96 kbps stream' : `Adaptive quality: ${state.rung} kbps while the connection can't keep up with the original`;
  if (state.dsp) return 'Upsampled on the server by Cadenza (lossless 24-bit FLAC)';
  return state.transcoded ? 'Converted to FLAC for your browser (lossless)' : 'Original file, no conversion';
}

function bindSeek(seek, cur) {
  const startDrag = () => { state.dragging = true; };
  seek.addEventListener('pointerdown', startDrag);
  seek.addEventListener('touchstart', startDrag, { passive: true });
  seek.addEventListener('input', () => {
    state.dragging = true;
    document.getElementById(cur).textContent = fmt.dur((seek.value / 1000) * (link ? (link.state?.duration || 0) : duration()));
    seek.style.setProperty('--p', `${seek.value / 10}%`);
  });
  const commit = () => { if (state.dragging) { state.dragging = false; seekTo((seek.value / 1000) * (link ? (link.state?.duration || 0) : duration())); } };
  seek.addEventListener('change', commit);
  seek.addEventListener('pointerup', commit);
  seek.addEventListener('touchend', commit);
}

function updateLinkedTime() {
  if (!link || state.dragging) return;
  const s = link.state || {};
  const dur = s.duration || 0, pos = Math.min(s.position || 0, dur || Infinity);
  for (const [sid, cid, did] of [['seek', 't-cur', 't-dur'], ['fseek', 'f-cur', 'f-dur']]) {
    const seek = document.getElementById(sid);
    if (!seek) continue;
    seek.value = dur ? (pos / dur) * 1000 : 0;
    seek.style.setProperty('--p', `${seek.value / 10}%`);
    document.getElementById(cid).textContent = fmt.dur(pos);
    document.getElementById(did).textContent = fmt.dur(dur);
  }
}

/** The extra buttons on the right of the bar: like, ⋯, up next, devices, volume (and close when playing here). */
function rightHtml(t, { closable }) {
  return `<div class="right">
      ${t && !t.preview ? `<button class="badge hide-sm" id="p-signal" type="button" title="${esc(qualityTitle(t))} – click for the signal path">${esc(quality(t))}</button>` : ''}
      ${canLike(t) ? likeButton('p-like', isLiked(t) ? 'on' : '') : ''}
      <button class="btn ghost icon" id="p-more" aria-label="More" title="More">${icon.more}</button>
      <button class="btn ghost icon" id="p-queue" aria-label="Up next" title="Up next">${icon.list}</button>
      ${window.encoreRemote ? `<button class="btn ghost icon" id="p-devices" aria-label="Other devices" title="Play on or control your other devices">${icon.devices}</button>` : ''}
      <span class="faint vol-ic">${icon.volume.replace('<svg', '<svg width="18"')}</span>
      <input type="range" id="vol" min="0" max="1" step="0.01" value="${link ? (link.state?.volume ?? 0.9) : userVol}">
      ${closable ? `<button class="btn ghost icon" id="p-close" aria-label="Close player">${icon.x}</button>` : ''}
    </div>`;
}
function bindRight(el) {
  el.querySelector('#p-like')?.addEventListener('click', () => toggleLike(nowTrack()));
  el.querySelector('#p-more')?.addEventListener('click', (e) => moreMenu(e.currentTarget));
  el.querySelector('#p-queue')?.addEventListener('click', () => { state.pane = 'queue'; setFull(true); });
  el.querySelector('#p-devices')?.addEventListener('click', () => window.encoreRemote.open());
  el.querySelector('#p-signal')?.addEventListener('click', signalPath);
  const vol = el.querySelector('#vol');
  if (vol) {
    if (link) vol.onchange = (e) => setVolume(e.target.value);
    else vol.oninput = (e) => localVolume(e.target.value);
  }
  if (liked === null && canLike(nowTrack())) likes().then(paintLikes);
}

function renderLinked(el) {
  const s = link.state || {};
  const t = remoteTrack();
  const where = `<span class="linked-to" title="This device is a remote for ${esc(link.name)}">${icon.link || ''}${link.online ? '' : 'Offline · '}Playing on ${esc(link.name)}</span>`;
  document.body.classList.add('has-player');
  el.innerHTML = `
    <div class="now" id="p-open" role="button" tabindex="0" aria-label="Open the full-screen player" title="Open player">
      <div class="thumb">${s.title ? art(s.cover, s.album || s.title) : `<div class="ph">${icon.devices.replace('<svg', '<svg width="20"')}</div>`}</div>
      <div class="grow">
        <div class="ellipsis" style="font-weight:650">${esc(s.title || 'Nothing playing')}</div>
        <div class="ellipsis small muted">${s.title ? esc(s.artist || '') + (s.album ? ' · ' + esc(s.album) : '') : 'Play anything – it starts there'}</div>
        <div class="ellipsis small">${where}</div>
      </div>
    </div>
    <div class="ctrls">
      <div class="row">
        <button class="btn ghost icon" id="p-prev" aria-label="Previous">${icon.prev}</button>
        <button class="play-btn" id="p-toggle" aria-label="Play/Pause">${s.playing ? icon.pause : icon.play}</button>
        <button class="btn ghost icon" id="p-next" aria-label="Next">${icon.next}</button>
      </div>
      <div class="seek"><span id="t-cur">0:00</span><input type="range" id="seek" min="0" max="1000" value="0"><span id="t-dur">0:00</span></div>
    </div>
    ${rightHtml(t, { closable: false }).replace('<div class="right">', `<div class="right"><button class="btn sm hide-sm" id="p-unlink" title="Stop being a remote and play on this device again">${icon.link || ''}Unlink</button>`)}`;
  el.querySelector('#p-toggle').onclick = toggle;
  el.querySelector('#p-next').onclick = () => next();
  el.querySelector('#p-prev').onclick = prev;
  el.querySelector('#p-open').onclick = () => { state.pane = 'now'; setFull(true); };
  el.querySelector('#p-open').onkeydown = (e) => { if (e.key === 'Enter') setFull(true); };
  el.querySelector('#p-unlink').onclick = () => window.encoreRemote?.unlink();
  bindRight(el);
  bindSeek(el.querySelector('#seek'), 't-cur');
  updateLinkedTime();
}

function render() {
  const el = document.getElementById('player');
  if (!el) return;
  if (link) return renderLinked(el);
  const t = current();
  document.body.classList.add('has-player');
  if (!t) {
    el.innerHTML = `<div class="now"><div class="thumb"><div class="ph">${icon.music.replace('<svg', '<svg width="20"')}</div></div>
      <div class="grow"><div style="font-weight:650" class="faint">Nothing playing</div>
      <div class="small muted ellipsis">Open an album in <a href="#/library" style="color:var(--accent)">Library</a> and press ▶ Play</div></div></div>
      <div class="ctrls idle">
        <div class="row">
          <button class="btn ghost icon" id="p-prev" aria-label="Previous" disabled>${icon.prev}</button>
          <button class="play-btn" id="p-toggle" aria-label="Play/Pause" disabled>${icon.play}</button>
          <button class="btn ghost icon" id="p-next" aria-label="Next" disabled>${icon.next}</button>
        </div>
        <div class="seek"><span>0:00</span><input type="range" min="0" max="1000" value="0" disabled aria-label="Seek"><span>0:00</span></div>
      </div>
      <div class="right">
        ${window.encoreRemote ? `<button class="btn ghost icon" id="p-devices" aria-label="Other devices" title="Play on or control your other devices">${icon.devices}</button>` : ''}
        <span class="faint vol-ic">${icon.volume.replace('<svg', '<svg width="18"')}</span>
        <input type="range" id="vol" min="0" max="1" step="0.01" value="${userVol}">
      </div>`;
    el.querySelector('#p-devices')?.addEventListener('click', () => window.encoreRemote.open());
    el.querySelector('#vol').oninput = (e) => localVolume(e.target.value);
    if (state.full) setFull(false);
    return;
  }
  el.innerHTML = `
    <div class="now" id="p-open" role="button" tabindex="0" aria-label="Open the full-screen player" title="Open player">
      <div class="thumb">${art(cover(t), t.album || t.title)}</div>
      <div class="grow">
        <div class="ellipsis" style="font-weight:650">${esc(t.title)}</div>
        <div class="ellipsis small muted">${esc(t.artist)}${t.album ? ' · ' + esc(t.album) : ''}</div>
      </div>
    </div>
    <div class="ctrls">
      <div class="row">
        <button class="btn ghost icon" id="p-prev" aria-label="Previous">${icon.prev}</button>
        <button class="play-btn" id="p-toggle" aria-label="Play/Pause">${audio.paused ? icon.play : icon.pause}</button>
        <button class="btn ghost icon" id="p-next" aria-label="Next">${icon.next}</button>
      </div>
      <div class="seek"><span id="t-cur">0:00</span><input type="range" id="seek" min="0" max="1000" value="0"><span id="t-dur">0:00</span></div>
    </div>
    ${rightHtml(t, { closable: true })}`;
  el.querySelector('#p-toggle').onclick = toggle;
  el.querySelector('#p-next').onclick = () => next();
  el.querySelector('#p-prev').onclick = prev;
  el.querySelector('#p-open').onclick = () => { state.pane = 'now'; setFull(true); };
  el.querySelector('#p-open').onkeydown = (e) => { if (e.key === 'Enter') setFull(true); };
  bindRight(el);
  bindSeek(el.querySelector('#seek'), 't-cur');
  el.querySelector('#p-close').onclick = () => {
    endFade(); audio.pause(); audio.removeAttribute('src'); spare.removeAttribute('src'); delete spare.dataset.url; state.queue = []; state.index = -1;
    held = false; heldAt = 0;
    try { localStorage.removeItem(RESUME); } catch { /* ignore */ }
    render(); emit();
  };
  updateTime();
  if (state.full) renderFull();
}

function renderFull() {
  const f = document.getElementById('np-full');
  const t = nowTrack();
  if (!f) return;
  if (!t) {
    f.innerHTML = `<div class="np-top"><button class="btn ghost icon" id="f-close" aria-label="Close">${icon.x}</button><span></span><span></span></div>
      <div class="empty">${icon.devices}<h2>Nothing playing on ${esc(link?.name || 'this device')}</h2><p class="muted">Play anything – it starts there.</p></div>`;
    f.querySelector('#f-close').onclick = () => setFull(false);
    return;
  }
  const s = link?.state || {};
  const q = link ? (s.queue || []) : state.queue;
  const idx = link ? (s.index ?? -1) : state.index;
  const upNext = q.slice(idx + 1);
  const playing = nowPlaying();
  const volume = link ? (s.volume ?? 0.9) : userVol;
  f.style.setProperty('--np-bg', `url("${cover(t) || ''}")`);
  f.innerHTML = `
    <div class="np-backdrop"></div>
    <div class="np-top">
      <button class="btn ghost icon" id="f-close" aria-label="Close">${icon.x}</button>
      <div class="tabs np-tabs"><button data-pane="now" class="${state.pane === 'now' ? 'active' : ''}">Now playing</button><button data-pane="lyrics" class="${state.pane === 'lyrics' ? 'active' : ''}">Lyrics</button><button data-pane="queue" class="${state.pane === 'queue' ? 'active' : ''}">Up next</button></div>
      ${window.encoreRemote ? `<button class="btn sm ghost np-device" id="f-device" title="Choose where the music plays">${icon.devices}<span class="hide-sm">${link ? `Playing on ${esc(link.name)}` : 'This device'}</span>${icon.chevron}</button>` : '<span></span>'}
    </div>
    <div class="np-body pane-${state.pane}">
      <section class="np-main">
        <div class="np-art">${art(cover(t), t.album || t.title, "big")}</div>
        <div class="np-meta">
          <h2 class="ellipsis">${esc(t.title)}</h2>
          <div class="muted ellipsis">${t.preview ? esc(t.artist) : `<a href="#/artist-name/${encodeURIComponent(t.artist || '')}">${esc(t.artist)}</a>`}${t.album ? ` · ${t.album_id ? `<a href="#/library/album/${esc(t.album_id)}">${esc(t.album)}</a>` : esc(t.album)}` : ''}</div>
          <button class="badge mt np-quality" id="f-signal" title="Signal path: what the file is, what Encore sends and what this device does with it">${esc(quality(t))}</button>
        </div>
        <div class="seek np-seek"><span id="f-cur">0:00</span><input type="range" id="fseek" min="0" max="1000" value="0"><span id="f-dur">0:00</span></div>
        <div class="np-ctrls">
          ${link ? `<span class="np-gap"></span>` : `<button class="btn ghost icon" id="f-shuffle" title="Shuffle up next" aria-label="Shuffle up next">${icon.shuffle}</button>`}
          <button class="btn ghost icon" id="f-prev" aria-label="Previous">${icon.prev}</button>
          <button class="play-btn lg" id="f-toggle" aria-label="Play/Pause">${playing ? icon.pause : icon.play}</button>
          <button class="btn ghost icon" id="f-next" aria-label="Next">${icon.next}</button>
          ${link ? `<span class="np-gap"></span>` : `<button class="btn ghost icon ${state.repeat !== 'off' ? 'on' : ''}" id="f-repeat" title="Repeat: ${state.repeat}" aria-label="Repeat">${icon.refresh}${state.repeat === 'one' ? '<sup>1</sup>' : ''}</button>`}
        </div>
        <div class="row np-vol"><span class="faint">${icon.volume.replace('<svg', '<svg width="18"')}</span><input type="range" id="fvol" min="0" max="1" step="0.01" value="${volume}"></div>
        <div class="np-actions">
          ${canLike(t) ? likeButton('f-like', isLiked(t) ? 'on' : '') : ''}
          ${canLike(t) ? `<button class="btn ghost icon" id="f-share" aria-label="Share" title="Share link">${icon.external}</button>` : ''}
          <button class="btn ghost icon" id="f-more" aria-label="More" title="More">${icon.more}</button>
        </div>
      </section>
      <section class="np-lyrics" id="np-lyrics">${lyricsHtml()}</section>
      <section class="np-queue">
        <div class="row between"><h3>Up next${link ? ` on ${esc(link.name)}` : ''}</h3>
          ${link ? '' : `<label class="row small muted" style="gap:6px" title="When the queue ends, keep playing a shuffle of the whole library"><input type="checkbox" id="f-auto" ${prefs.autoplay ? 'checked' : ''}> Then shuffle library</label>`}</div>
        <div class="np-list">${upNext.length ? upNext.slice(0, 200).map((x, k) => {
          const i = idx + 1 + k;
          return `<div class="np-item" data-i="${i}">
            <div class="thumb">${art(cover(x), x.album || x.title)}</div>
            <div class="grow" style="min-width:0"><div class="ellipsis" style="font-weight:600">${esc(x.title)}</div><div class="small muted ellipsis">${esc(x.artist)}</div></div>
            ${link ? '' : `<button class="btn ghost icon sm" data-up="${i}" aria-label="Move up" ${i === idx + 1 ? 'disabled' : ''}>↑</button>
            <button class="btn ghost icon sm" data-rm="${i}" aria-label="Remove">${icon.x}</button>`}</div>`;
        }).join('') : `<div class="muted small" style="padding:16px 4px">${link ? 'Nothing queued there.' : prefs.autoplay ? 'Nothing queued – a shuffle of your library plays next.' : 'Nothing queued.'}</div>`}</div>
      </section>
    </div>`;
  f.querySelector('#f-close').onclick = () => setFull(false);
  f.querySelector('.np-tabs').onclick = (e) => { const b = e.target.closest('[data-pane]'); if (b) { state.pane = b.dataset.pane; renderFull(); if (state.pane === 'lyrics') loadLyrics(); } };
  f.querySelector('#f-device')?.addEventListener('click', () => window.encoreRemote.open());
  f.querySelector('#f-signal').onclick = signalPath;
  f.querySelector('#f-toggle').onclick = toggle;
  f.querySelector('#f-next').onclick = () => next();
  f.querySelector('#f-prev').onclick = prev;
  f.querySelector('#f-shuffle')?.addEventListener('click', shuffleUpNext);
  f.querySelector('#f-repeat')?.addEventListener('click', () => { state.repeat = { off: 'all', all: 'one', one: 'off' }[state.repeat]; renderFull(); });
  f.querySelector('#f-auto')?.addEventListener('change', (e) => { playback.save({ ...prefs, autoplay: e.target.checked }, { serverPrefs: (window.encoreSession?.info?.features || []).includes('theme') }); });
  const fvol = f.querySelector('#fvol');
  if (link) fvol.onchange = (e) => setVolume(e.target.value); else fvol.oninput = (e) => localVolume(e.target.value);
  f.querySelector('#f-like')?.addEventListener('click', () => toggleLike(t));
  f.querySelector('#f-share')?.addEventListener('click', () => shareTrack(t));
  f.querySelector('#f-more').onclick = (e) => moreMenu(e.currentTarget);
  bindSeek(f.querySelector('#fseek'), 'f-cur');
  f.querySelector('.np-list').onclick = (e) => {
    if (link) {
      const it = e.target.closest('[data-i]');
      if (it && s.queue?.length) viaLink({ action: 'play_queue', tracks: s.queue, index: +it.dataset.i });
      return;
    }
    const rm = e.target.closest('[data-rm]');
    if (rm) return removeAt(+rm.dataset.rm);
    const up = e.target.closest('[data-up]');
    if (up) return moveTo(+up.dataset.up, +up.dataset.up - 1);
    const it = e.target.closest('[data-i]');
    if (it) jump(+it.dataset.i);
  };
  f.querySelector('#np-lyrics').onclick = (e) => { const l = e.target.closest('[data-t]'); if (l) seekTo(+l.dataset.t); };
  if (liked === null && canLike(t)) likes().then(paintLikes);
  updateTime();
  syncLyrics(true);
}

// ---- lyrics ----------------------------------------------------------------------------------
function parseLrc(text) {
  const out = [];
  for (const line of (text || '').split(/\r?\n/)) {
    const stamps = [...line.matchAll(/\[(\d+):(\d+(?:\.\d+)?)\]/g)];
    const words = line.replace(/\[[^\]]*\]/g, '').trim();
    for (const m of stamps) out.push({ t: +m[1] * 60 + +m[2], text: words });
  }
  return out.sort((a, b) => a.t - b.t);
}

function lyricsHtml() {
  const t = nowTrack();
  if (!t) return '';
  if (t.preview) return '<div class="muted np-empty">Lyrics are available for songs in your library.</div>';
  if (typeof t.id !== 'number') return '<div class="muted np-empty">No lyrics for this song.</div>';
  if (lyr.loading || lyr.id !== t.id) return '<div class="muted np-empty"><span class="spinner"></span> Looking for lyrics…</div>';
  if (lyr.lines.length) return lyr.lines.map((l, i) => `<p data-t="${l.t}" data-li="${i}">${esc(l.text) || '♪'}</p>`).join('');
  if (lyr.plain) return `<div class="np-plain">${esc(lyr.plain)}</div>`;
  return `<div class="muted np-empty">${lyr.instrumental ? 'Instrumental' : 'No lyrics found for this song.'}</div>`;
}

async function loadLyrics() {
  const t = nowTrack();
  if (!t || t.preview || typeof t.id !== 'number' || lyr.id === t.id) { const box = document.getElementById('np-lyrics'); if (box) box.innerHTML = lyricsHtml(); return; }
  lyr = { id: t.id, lines: [], plain: '', loading: true };
  const box = document.getElementById('np-lyrics');
  if (box) box.innerHTML = lyricsHtml();
  try {
    const r = await api.get(`/library/tracks/${t.id}/lyrics`);
    if (lyr.id !== t.id) return;
    lyr = { id: t.id, lines: r.synced ? parseLrc(r.lyrics) : [], plain: r.synced ? '' : (r.lyrics || ''), loading: false, instrumental: r.instrumental };
  } catch { lyr = { id: t.id, lines: [], plain: '', loading: false }; }
  const b = document.getElementById('np-lyrics');
  if (b) { b.innerHTML = lyricsHtml(); syncLyrics(true); }
}

let lastLine = -1;
function syncLyrics(force = false) {
  if (!state.full || !lyr.lines.length || lyr.id !== nowTrack()?.id) return;
  const p = nowPosition() + 0.3;
  let i = -1;
  while (i + 1 < lyr.lines.length && lyr.lines[i + 1].t <= p) i++;
  if (i === lastLine && !force) return;
  lastLine = i;
  const box = document.getElementById('np-lyrics');
  if (!box) return;
  box.querySelectorAll('p.on').forEach(n => n.classList.remove('on'));
  const cur = box.querySelector(`p[data-li="${i}"]`);
  if (cur) {
    cur.classList.add('on');
    box.scrollTo({ top: cur.offsetTop - box.clientHeight / 2 + cur.offsetHeight / 2, behavior: force ? 'auto' : 'smooth' });
  }
}
