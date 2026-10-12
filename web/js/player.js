// Web player: streams the original file (bit-perfect for MP3/FLAC/AAC/Opus); formats a browser
// can't decode (ALAC/APE/WavPack/AIFF/WMA) are converted to FLAC on the fly by the server, and when the
// connection can't keep up (Settings → Playback → Auto) the rest of the song comes as a smaller AAC stream.
// Tap the bar for the full-screen player (lyrics + Up next). When the queue runs out it keeps going
// with a shuffle of the whole library: a single song → straight into the shuffle, an album → after its last song.
import { api } from './api.js';
import * as playback from './playback.js';
import { art, esc, fmt, icon, popMenu, shareTrack, toast } from './ui.js';

const state = { queue: [], index: -1, transcoded: false, lossy: false, offset: 0, dragging: false, context: 'list', repeat: 'off', full: false, pane: 'now', logged: false,
  stalls: [], stallAt: 0, seekAt: 0, switched: false, lastPreloadCheck: 0, slowStart: null };
const listeners = new Set();
let audio = null;
let spare = null;   // second <audio> that pre-buffers the next song, so skipping is instant
let lyr = { id: null, lines: [], plain: '', loading: false };
let prefs = playback.current();
let lossyUntil = 0;          // Auto quality: songs started before this time use the smaller stream
let liked = null;            // Set of liked track ids, fetched the first time a heart is drawn
let ctx = null;              // AudioContext, only to read the device's output sample rate
const INFO = new Map();      // track id → /info (codec, sample rate, bit depth …)
window.addEventListener('encore:playback-prefs', (e) => { prefs = e.detail; if (!prefs.preload) cancelPreload(); else maybePreload(true); if (state.full) renderFull(); });
window.addEventListener('storage', (e) => { if (e.key === 'encore.playback') prefs = playback.current(); });

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

// ---- linked: this device is locked as a remote for another one (Encore 2) ----
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

// ---- remote control (Encore 2): what this device is playing, and commands from your other devices ----
const slim = (t) => ({ id: t.id, title: t.title, artist: t.artist, album: t.album, album_id: t.album_id, format: t.format, duration: t.duration });
export function snapshot() {
  const t = current();
  if (!t || !audio) return { playing: false };
  return { ...slim(t), cover: cover(t), playing: !audio.paused, position: position(), duration: duration(), volume: audio.volume,
           signal: { label: quality(t), transport: transport()?.kind || null, output_rate: ctx?.sampleRate || null },
           index: state.index, queue: state.queue.filter(x => typeof x.id === 'number').slice(0, 300).map(slim) };
}
function localVolume(v) {
  if (!audio) return;
  audio.volume = spare.volume = Math.max(0, Math.min(1, Number(v)));
  try { localStorage.setItem('encore.volume', audio.volume); } catch { /* ignore */ }
  for (const id of ['vol', 'fvol']) { const el = document.getElementById(id); if (el && +el.value !== audio.volume) el.value = audio.volume; }
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

// the smaller stream's codec: AAC where the browser decodes it, Opus elsewhere (Chromium builds, Firefox on Linux)
const LOSSY = (() => { try { const a = document.createElement('audio'); return a.canPlayType('audio/aac') ? 'aac' : a.canPlayType('audio/ogg; codecs=opus') ? 'opus' : 'aac'; } catch { return 'aac'; } })();
/** transcode: FLAC made by the server · lossy: the smaller stream · offset: where a server-made stream starts */
function streamUrl(t, { transcode = false, lossy = false, offset = 0 } = {}) {
  if (t.preview) return t.preview_url;
  const q = [];
  if (lossy) q.push(`q=${LOSSY}`, `kbps=${prefs.kbps}`);
  else if (transcode) q.push('transcode=1');
  if (offset) q.push(`t=${offset.toFixed(1)}`);
  return `/api/library/tracks/${t.id}/stream${q.length ? '?' + q.join('&') : ''}`;
}
const convert = (t) => !t.preview && (CONVERT.has((t.format || '').toLowerCase()) || !canPlay(t.format));
/** Should this song start as the smaller stream? Data saver always; Auto for a while after the connection fell behind. */
const wantLossy = (t) => !t.preview && (prefs.quality === 'saver' || (prefs.quality === 'auto' && Date.now() < lossyUntil));

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

function load({ transcode = false, offset = 0, lossy = null } = {}) {
  const t = current();
  if (!t) return;
  markPlayed(t);
  if (convert(t)) transcode = true;
  if (lossy === null) lossy = wantLossy(t);
  state.lossy = !t.preview && lossy;
  state.transcoded = !t.preview && (transcode || lossy);
  state.offset = offset;   // server-made streams restart at the seek point, so their clock starts at `offset`
  state.dragging = false;
  if (!offset) {
    state.logged = false;
    state.stalls = [];
    state.switched = false;
    // "now playing" on Last.fm / ListenBrainz (Encore 2)
    if (typeof t.id === 'number' && state.announced !== t) { state.announced = t; api.post('/library/now-playing', { track_id: t.id }).catch(() => {}); }
  }
  const url = streamUrl(t, { transcode: state.transcoded && !state.lossy, lossy: state.lossy, offset });
  if (!offset && spare && spare.dataset.url === url) {
    // already buffered in the background: swap players and start right away
    const old = audio;
    audio = spare;
    spare = old;
    old.pause();
    old.removeAttribute('src');
    delete old.dataset.url;
    audio.volume = old.volume;
    audio.currentTime = 0;
  } else {
    audio.src = url;
  }
  audio.dataset.url = url;
  audio.play().catch(() => {});
  clearTimeout(state.slowStart);
  if (!offset && !state.lossy && !t.preview && prefs.quality === 'auto') {
    // can't even get going: six seconds in and nothing has played yet → the smaller stream from the start
    state.slowStart = setTimeout(() => { if (current() === t && !audio.paused && audio.currentTime < 0.5 && audio.readyState < 3) downgrade(); }, 6000);
  }
  if ('mediaSession' in navigator) {
    navigator.mediaSession.metadata = new MediaMetadata({
      title: t.title || '', artist: t.artist || '', album: t.album || '',
      artwork: cover(t) ? [{ src: cover(t), sizes: '512x512' }] : [],
    });
  }
  document.body.classList.add('has-player');
  render();
  emit();
  if (state.full) loadLyrics();
  topUp();
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
  if (!spare || !prefs.preload || link) return;
  const n = state.queue[state.index + 1];
  if (!n) return;
  const url = streamUrl(n, { transcode: convert(n), lossy: wantLossy(n) });
  if (spare.dataset.url === url) return;
  const t = current();
  if (!force) {
    const remaining = duration() - position();
    const quiet = Date.now() - state.stallAt > 15000;
    const ok = (t?.preview || state.transcoded) ? remaining < 40 : (fullyBuffered() || (quiet && bufferedAhead() > 45) || remaining < 30);
    if (!ok) return;
  }
  spare.dataset.url = url;
  spare.src = url;
  spare.load();
}
const preloadNext = () => maybePreload(true);
function cancelPreload() {
  if (!spare?.dataset.url) return;
  spare.removeAttribute('src');
  delete spare.dataset.url;
  spare.load();
}
/** The browser ran out of buffer mid-song. Give it the bandwidth back, and on Auto quality drop to the smaller stream. */
function stalled() {
  const now = Date.now();
  if (now - state.seekAt < 3000 || position() < 1 || audio.paused) return;   // a seek or the start, not a slow link
  state.stallAt = now;
  state.stalls.push(now);
  if (!fullyBuffered()) cancelPreload();
  if (prefs.quality !== 'auto' || state.lossy || current()?.preview || state.switched) return;
  const recent = state.stalls.filter(s => now - s < 90000);
  if (recent.length >= 2) return downgrade();
  // one long stall counts too: still waiting four seconds later
  setTimeout(() => { if (!audio.paused && audio.readyState < 3 && state.stallAt === now && !state.switched) downgrade(); }, 4000);
}
function downgrade() {
  const t = current();
  if (!t || t.preview || state.lossy) return;
  state.switched = true;
  lossyUntil = Date.now() + 10 * 60000;
  load({ transcode: true, lossy: true, offset: position() });
  toast(`Connection can't keep up – playing a smaller ${prefs.kbps} kbps stream for a while (Settings → Playback)`, '');
  clearTimeout(state.slowStart);
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
  state.seekAt = Date.now();
  if (state.transcoded) load({ transcode: true, lossy: state.lossy, offset: sec });      // ask the server to start the stream at that point
  else audio.currentTime = sec;
  updateTime();
  emit();
}

export function toggle() {
  if (viaLink({ action: 'toggle' })) { if (link.state) link.state.playing = !link.state.playing; render(); if (state.full) renderFull(); return; }
  if (current()) (audio.paused ? audio.play() : audio.pause());
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
  if (state.lossy) return { kind: `${LOSSY}-${prefs.kbps}`, text: `${LOSSY.toUpperCase()} ${prefs.kbps} kbps (lossy) – ${prefs.quality === 'saver' ? 'Data saver is on' : 'the connection could not keep up with the original'}` };
  if (state.transcoded) return { kind: 'flac', text: `Converted to FLAC by the server (lossless) – browsers can't play ${(t.format || '').toUpperCase()}` };
  return { kind: 'original', text: 'The original file, byte for byte – nothing was converted or resampled on the way' };
}
function outputRate() {
  try { if (!ctx) ctx = new (window.AudioContext || window.webkitAudioContext)(); return ctx.sampleRate; } catch { return 0; }
}
const khz = (hz) => String(Math.round(hz / 100) / 10);
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
  const rows = [];
  rows.push(['Source', sourceLine(t, info), info?.lossless === false ? 'A lossy file: what the file lost is gone before Encore touches it' : '']);
  if (link) {
    const sig = link.state?.signal || {};
    rows.push([`Encore → ${esc(link.name)}`, sig.transport === 'original' ? 'The original file, byte for byte' : sig.transport ? (sig.transport === 'flac' ? 'Converted to FLAC by the server (lossless)' : /^(aac|opus)-/.test(sig.transport) ? `${sig.transport.split('-')[0].toUpperCase()} ${sig.transport.split('-')[1]} kbps (lossy)` : sig.transport) : 'Not reported by that device', '']);
    rows.push([esc(link.name), sig.output_rate ? `Plays through its browser audio at ${khz(sig.output_rate)} kHz` : 'Playing there', '']);
  } else {
    const tr = transport();
    rows.push(['Encore → this device', tr?.text || '', '']);
    const out = outputRate();
    const src = info?.sample_rate;
    const resampled = out && src && out !== src;
    rows.push(['This device', out ? `Browser audio → the system mixer at ${khz(out)} kHz${window.EncoreNative ? '' : ''}` : 'Browser audio → the system mixer',
      resampled ? `Your device resamples ${khz(src)} → ${khz(out)} kHz. A browser can't hand bits straight to the DAC; for true bit-perfect output play through a Cadenza device or an exclusive-mode player.` : out && src ? 'Same rate as the file – no resampling in the browser. Whether the operating system mixes it with other sounds is up to your audio settings.' : '']);
  }
  const pure = !link && transport()?.kind === 'original' && info?.lossless !== false;
  m.el.querySelector('.modal-body').innerHTML = `
    <div class="stack" style="gap:12px">
      ${rows.map(([k, v, note]) => `<div class="stack" style="gap:2px"><div class="small muted">${k}</div><div style="font-weight:600">${esc(v)}</div>${note ? `<div class="small muted">${esc(note)}</div>` : ''}</div>`).join('')}
      <div class="card" style="padding:12px"><b>${link ? 'Playing on another device' : pure ? 'Pure playback from Encore' : 'Not the untouched file'}</b>
        <div class="small muted" style="margin-top:4px">${link ? 'The rows above are what that device reports. Open Encore there for its own signal path.'
          : pure ? 'Encore sent the file exactly as it is stored. Everything after that – volume, mixing, resampling – is your browser and operating system.'
          : 'Encore converted this stream (see above). Settings → Playback → Quality → Original keeps the file untouched even when the connection is slow.'}</div></div>
    </div>`;
}

export function mount() {
  if (document.getElementById('player')) return;
  audio = document.getElementById('audio') || Object.assign(document.createElement('audio'), { id: 'audio' });
  if (!audio.isConnected) document.body.appendChild(audio);
  spare = Object.assign(document.createElement('audio'), { id: 'audio-next', preload: 'auto' });
  document.body.appendChild(spare);
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
  audio.volume = spare.volume = vol;
  // both elements get the listeners; only the one currently playing counts
  const on = (type, fn) => [audio, spare].forEach(a => a.addEventListener(type, (e) => { if (e.target === audio) fn(e); }));
  on('ended', () => { logPlay(); next(true); });
  on('play', () => { render(); emit(); });
  on('pause', () => { render(); emit(); });
  on('timeupdate', () => {
    updateTime(); logPlay(); syncLyrics();
    const now = Date.now();
    if (now - state.lastPreloadCheck > 2000) { state.lastPreloadCheck = now; maybePreload(); }
  });
  on('progress', () => { const now = Date.now(); if (now - state.lastPreloadCheck > 2000) { state.lastPreloadCheck = now; maybePreload(); } });
  on('waiting', stalled);
  on('loadedmetadata', updateTime);
  // the browser couldn't decode the original (e.g. ALAC in .m4a) → ask the server for FLAC
  on('error', () => { if (current() && !state.transcoded && !current().preview && audio.getAttribute('src')) load({ transcode: true, offset: position() }); });
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
    if ((!current() && !link) || (/INPUT|TEXTAREA|SELECT|BUTTON/.test(ae.tagName) && !/seek/.test(ae.id))) return;
    if (e.code === 'Space') { e.preventDefault(); toggle(); }
    const at = link ? (link.state?.position || 0) : position();
    if (e.code === 'ArrowRight') { e.preventDefault(); seekTo(at + 10); }
    if (e.code === 'ArrowLeft') { e.preventDefault(); seekTo(at - 10); }
  });
  window.addEventListener('hashchange', () => { if (state.full) setFull(false); });
  window.addEventListener('encore:likes', () => { liked = null; likes().then(paintLikes); });
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
  if (state.lossy) return `${f} → ${LOSSY.toUpperCase()} ${prefs.kbps}k`;
  return state.transcoded ? `${f} → FLAC` : `${f}${t.bitrate ? ' ' + t.bitrate + 'k' : ''}`;
}
function qualityTitle(t) {
  if (t?.remote) return `What ${link?.name || 'that device'} is playing`;
  if (t?.preview) return 'A 30-second preview – not in your library';
  if (state.lossy) return 'A smaller lossy stream, because the connection could not keep up (or Data saver is on)';
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
      ${t && !t.preview ? `<span class="badge hide-sm" title="${esc(qualityTitle(t))}">${esc(quality(t))}</span>` : ''}
      ${canLike(t) ? likeButton('p-like', isLiked(t) ? 'on' : '') : ''}
      <button class="btn ghost icon" id="p-more" aria-label="More" title="More">${icon.more}</button>
      <button class="btn ghost icon" id="p-queue" aria-label="Up next" title="Up next">${icon.list}</button>
      ${window.encoreRemote ? `<button class="btn ghost icon" id="p-devices" aria-label="Other devices" title="Play on or control your other devices">${icon.devices}</button>` : ''}
      <span class="faint vol-ic">${icon.volume.replace('<svg', '<svg width="18"')}</span>
      <input type="range" id="vol" min="0" max="1" step="0.01" value="${link ? (link.state?.volume ?? 0.9) : audio.volume}">
      ${closable ? `<button class="btn ghost icon" id="p-close" aria-label="Close player">${icon.x}</button>` : ''}
    </div>`;
}
function bindRight(el) {
  el.querySelector('#p-like')?.addEventListener('click', () => toggleLike(nowTrack()));
  el.querySelector('#p-more')?.addEventListener('click', (e) => moreMenu(e.currentTarget));
  el.querySelector('#p-queue')?.addEventListener('click', () => { state.pane = 'queue'; setFull(true); });
  el.querySelector('#p-devices')?.addEventListener('click', () => window.encoreRemote.open());
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
      ${window.encoreRemote ? `<div class="right"><button class="btn ghost icon" id="p-devices" aria-label="Other devices" title="Play on or control your other devices">${icon.devices}</button></div>` : ''}`;
    el.querySelector('#p-devices')?.addEventListener('click', () => window.encoreRemote.open());
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
    audio.pause(); audio.removeAttribute('src'); spare.removeAttribute('src'); delete spare.dataset.url; state.queue = []; state.index = -1;
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
  const volume = link ? (s.volume ?? 0.9) : audio.volume;
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
