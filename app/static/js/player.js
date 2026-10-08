// Web player: streams the original file (bit-perfect for MP3/FLAC/AAC/Opus); formats a browser
// can't decode (ALAC/APE/WavPack/AIFF/WMA) are converted to FLAC on the fly by the server.
// Tap the bar for the full-screen player (lyrics + Up next). When the queue runs out it keeps going
// with a shuffle of the whole library: a single song → straight into the shuffle, an album → after its last song.
import { api } from './api.js';
import { art, esc, fmt, icon } from './ui.js';

const state = { queue: [], index: -1, transcoded: false, offset: 0, dragging: false, context: 'list', repeat: 'off', full: false, pane: 'now', logged: false };
const listeners = new Set();
let audio = null;
let spare = null;   // second <audio> that pre-buffers the next song, so skipping is instant
let lyr = { id: null, lines: [], plain: '', loading: false };

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
export function queue() { return { tracks: state.queue, index: state.index }; }

const cover = (t) => t.preview ? t.cover : t.album_id ? `/api/library/albums/${t.album_id}/cover` : t.cover;

/** context: 'album' | 'single' | 'list' – decides what happens when the queue ends. */
export function playQueue(tracks, start = 0, { context = 'list' } = {}) {
  state.queue = tracks.filter(Boolean);
  state.index = Math.max(0, Math.min(start, state.queue.length - 1));
  state.context = context;
  load();
}

export function enqueue(tracks) {
  state.queue.push(...tracks);
  if (state.index < 0) { state.index = 0; load(); } else { render(); emit(); preloadNext(); }
}

export function playNext(tracks) {
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

const streamUrl = (t, transcode = false, offset = 0) => t.preview ? t.preview_url
  : `/api/library/tracks/${t.id}/stream${transcode ? `?transcode=1${offset ? `&t=${offset.toFixed(1)}` : ''}` : ''}`;
const convert = (t) => !t.preview && CONVERT.has((t.format || '').toLowerCase());

function load(transcode = false, offset = 0) {
  const t = current();
  if (!t) return;
  if (convert(t)) transcode = true;
  state.transcoded = !t.preview && transcode;
  state.offset = offset;   // converted streams restart at the seek point, so their clock starts at `offset`
  state.dragging = false;
  if (!offset) state.logged = false;
  const url = streamUrl(t, state.transcoded, offset);
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
  setTimeout(preloadNext, 1500);      // let the current song get going first
}

/** Start buffering the song that plays next. */
function preloadNext() {
  if (!spare) return;
  const n = state.queue[state.index + 1];
  if (!n) return;
  const url = streamUrl(n, convert(n));
  if (spare.dataset.url === url) return;
  spare.dataset.url = url;
  spare.src = url;
  spare.load();
}

/** Near the end of the queue: fetch the library shuffle ahead of time, so the next song is ready too. */
let topping = false;
async function topUp() {
  if (topping || state.context === 'preview' || state.index < state.queue.length - 2) return;
  let enabled = true;
  try { enabled = localStorage.getItem('encore.autoplay') !== 'off'; } catch { /* ignore */ }
  if (!enabled) return;
  topping = true;
  try {
    const recent = state.queue.slice(-150).map(t => t.id).filter(id => typeof id === 'number');
    let more = await api.get(`/library/shuffle?limit=50&exclude=${recent.join(',')}`);
    if (!more.length) more = await api.get('/library/shuffle?limit=50');   // small library: allow repeats
    if (more.length) {
      state.queue.push(...more);
      render(); emit();
      preloadNext();
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
  const dur = duration();
  if (!current() || !dur) return;
  sec = Math.max(0, Math.min(sec, dur - 0.25));
  if (state.transcoded) load(true, sec);      // ask the server to start the conversion at that point
  else audio.currentTime = sec;
  updateTime();
}

export function toggle() { if (current()) (audio.paused ? audio.play() : audio.pause()); }

export async function next(auto = false) {
  if (auto && state.repeat === 'one') { seekTo(0); audio.play(); return; }
  if (state.index < state.queue.length - 1) { state.index++; load(); return; }
  if (state.repeat === 'all' && state.queue.length) { state.index = 0; load(); return; }
  if (state.context === 'preview') return;
  if (await continueWithLibrary()) { state.index++; load(); }
}

/** Queue ran out: keep the music going with a shuffle of the whole library. */
async function continueWithLibrary() {
  let enabled = true;
  try { enabled = localStorage.getItem('encore.autoplay') !== 'off'; } catch { /* ignore */ }
  if (!enabled) return false;
  try {
    const recent = state.queue.slice(-150).map(t => t.id).filter(id => typeof id === 'number');
    let more = await api.get(`/library/shuffle?limit=50&exclude=${recent.join(',')}`);
    if (!more.length) more = await api.get('/library/shuffle?limit=50');
    if (!more.length) return false;
    state.queue.push(...more);
    state.context = 'library';
    return true;
  } catch { return false; }
}

export function prev() {
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
  if (p >= 30 || (d && p >= d / 2)) {
    state.logged = true;
    api.post('/library/plays', { track_id: t.id, seconds: p }).catch(() => {});
  }
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
  on('play', () => { render(); native(); });
  on('pause', () => { render(); native(); });
  on('timeupdate', () => { updateTime(); logPlay(); syncLyrics(); });
  on('loadedmetadata', updateTime);
  // the browser couldn't decode the original (e.g. ALAC in .m4a) → ask the server for FLAC
  on('error', () => { if (current() && !state.transcoded && !current().preview && audio.getAttribute('src')) load(true, position()); });
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
    if (e.key === 'Escape' && state.full) { setFull(false); return; }
    if (!current() || (/INPUT|TEXTAREA|SELECT|BUTTON/.test(ae.tagName) && !/seek/.test(ae.id))) return;
    if (e.code === 'Space') { e.preventDefault(); toggle(); }
    if (e.code === 'ArrowRight') { e.preventDefault(); seekTo(position() + 10); }
    if (e.code === 'ArrowLeft') { e.preventDefault(); seekTo(position() - 10); }
  });
  window.addEventListener('hashchange', () => { if (state.full) setFull(false); });
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
  if (t.preview) return '30 s preview';
  const f = (t.format || '').toUpperCase();
  return state.transcoded ? `${f} → FLAC` : `${f}${t.bitrate ? ' ' + t.bitrate + 'k' : ''}`;
}

function bindSeek(seek, cur) {
  const startDrag = () => { state.dragging = true; };
  seek.addEventListener('pointerdown', startDrag);
  seek.addEventListener('touchstart', startDrag, { passive: true });
  seek.addEventListener('input', () => {
    state.dragging = true;
    document.getElementById(cur).textContent = fmt.dur((seek.value / 1000) * duration());
    seek.style.setProperty('--p', `${seek.value / 10}%`);
  });
  const commit = () => { if (state.dragging) { state.dragging = false; seekTo((seek.value / 1000) * duration()); } };
  seek.addEventListener('change', commit);
  seek.addEventListener('pointerup', commit);
  seek.addEventListener('touchend', commit);
}

function render() {
  const el = document.getElementById('player');
  if (!el) return;
  const t = current();
  document.body.classList.add('has-player');
  if (!t) {
    el.innerHTML = `<div class="now"><div class="thumb"><div class="ph">${icon.music.replace('<svg', '<svg width="20"')}</div></div>
      <div class="grow"><div style="font-weight:650" class="faint">Nothing playing</div>
      <div class="small muted ellipsis">Open an album in <a href="#/library" style="color:var(--accent)">Library</a> and press ▶ Play</div></div></div>`;
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
    <div class="right">
      <span class="badge" title="${state.transcoded ? 'Converted to FLAC for your browser (lossless)' : t.preview ? 'A 30-second preview – not in your library' : 'Original file, no conversion'}">${esc(quality(t))}</span>
      <button class="btn ghost icon" id="p-queue" aria-label="Up next" title="Up next">${icon.list}</button>
      <span class="faint vol-ic">${icon.volume.replace('<svg', '<svg width="18"')}</span>
      <input type="range" id="vol" min="0" max="1" step="0.01" value="${audio.volume}">
      <button class="btn ghost icon" id="p-close" aria-label="Close player">${icon.x}</button>
    </div>`;
  el.querySelector('#p-toggle').onclick = toggle;
  el.querySelector('#p-next').onclick = () => next();
  el.querySelector('#p-prev').onclick = prev;
  el.querySelector('#p-open').onclick = () => { state.pane = 'now'; setFull(true); };
  el.querySelector('#p-open').onkeydown = (e) => { if (e.key === 'Enter') setFull(true); };
  el.querySelector('#p-queue').onclick = () => { state.pane = 'queue'; setFull(true); };
  bindSeek(el.querySelector('#seek'), 't-cur');
  el.querySelector('#vol').oninput = (e) => {
    audio.volume = spare.volume = e.target.value;
    try { localStorage.setItem('encore.volume', audio.volume); } catch { /* ignore */ }
  };
  el.querySelector('#p-close').onclick = () => {
    audio.pause(); audio.removeAttribute('src'); spare.removeAttribute('src'); delete spare.dataset.url; state.queue = []; state.index = -1;
    render(); emit();
  };
  updateTime();
  if (state.full) renderFull();
}

function renderFull() {
  const f = document.getElementById('np-full');
  const t = current();
  if (!f || !t) return;
  const upNext = state.queue.slice(state.index + 1);
  let auto = true;
  try { auto = localStorage.getItem('encore.autoplay') !== 'off'; } catch { /* ignore */ }
  f.style.setProperty('--np-bg', `url("${cover(t) || ''}")`);
  f.innerHTML = `
    <div class="np-backdrop"></div>
    <div class="np-top">
      <button class="btn ghost icon" id="f-close" aria-label="Close">${icon.x}</button>
      <div class="tabs np-tabs"><button data-pane="now" class="${state.pane === 'now' ? 'active' : ''}">Now playing</button><button data-pane="lyrics" class="${state.pane === 'lyrics' ? 'active' : ''}">Lyrics</button><button data-pane="queue" class="${state.pane === 'queue' ? 'active' : ''}">Up next</button></div>
      <span></span>
    </div>
    <div class="np-body pane-${state.pane}">
      <section class="np-main">
        <div class="np-art">${art(cover(t), t.album || t.title)}</div>
        <div class="np-meta">
          <h2 class="ellipsis">${esc(t.title)}</h2>
          <div class="muted ellipsis">${t.preview ? esc(t.artist) : `<a href="#/artist-name/${encodeURIComponent(t.artist || '')}">${esc(t.artist)}</a>`}${t.album ? ` · ${t.album_id ? `<a href="#/library/album/${esc(t.album_id)}">${esc(t.album)}</a>` : esc(t.album)}` : ''}</div>
          <span class="badge mt">${esc(quality(t))}</span>
        </div>
        <div class="seek np-seek"><span id="f-cur">0:00</span><input type="range" id="fseek" min="0" max="1000" value="0"><span id="f-dur">0:00</span></div>
        <div class="np-ctrls">
          <button class="btn ghost icon" id="f-shuffle" title="Shuffle up next" aria-label="Shuffle up next">${icon.shuffle}</button>
          <button class="btn ghost icon" id="f-prev" aria-label="Previous">${icon.prev}</button>
          <button class="play-btn lg" id="f-toggle" aria-label="Play/Pause">${audio.paused ? icon.play : icon.pause}</button>
          <button class="btn ghost icon" id="f-next" aria-label="Next">${icon.next}</button>
          <button class="btn ghost icon ${state.repeat !== 'off' ? 'on' : ''}" id="f-repeat" title="Repeat: ${state.repeat}" aria-label="Repeat">${icon.refresh}${state.repeat === 'one' ? '<sup>1</sup>' : ''}</button>
        </div>
        <div class="row np-vol"><span class="faint">${icon.volume.replace('<svg', '<svg width="18"')}</span><input type="range" id="fvol" min="0" max="1" step="0.01" value="${audio.volume}"></div>
      </section>
      <section class="np-lyrics" id="np-lyrics">${lyricsHtml()}</section>
      <section class="np-queue">
        <div class="row between"><h3>Up next</h3>
          <label class="row small muted" style="gap:6px" title="When the queue ends, keep playing a shuffle of the whole library"><input type="checkbox" id="f-auto" ${auto ? 'checked' : ''}> Then shuffle library</label></div>
        <div class="np-list">${upNext.length ? upNext.slice(0, 200).map((q, k) => {
          const i = state.index + 1 + k;
          return `<div class="np-item" data-i="${i}">
            <div class="thumb">${art(cover(q), q.album || q.title)}</div>
            <div class="grow" style="min-width:0"><div class="ellipsis" style="font-weight:600">${esc(q.title)}</div><div class="small muted ellipsis">${esc(q.artist)}</div></div>
            <button class="btn ghost icon sm" data-up="${i}" aria-label="Move up" ${i === state.index + 1 ? 'disabled' : ''}>↑</button>
            <button class="btn ghost icon sm" data-rm="${i}" aria-label="Remove">${icon.x}</button></div>`;
        }).join('') : `<div class="muted small" style="padding:16px 4px">${auto ? 'Nothing queued – a shuffle of your library plays next.' : 'Nothing queued.'}</div>`}</div>
      </section>
    </div>`;
  f.querySelector('#f-close').onclick = () => setFull(false);
  f.querySelector('.np-tabs').onclick = (e) => { const b = e.target.closest('[data-pane]'); if (b) { state.pane = b.dataset.pane; renderFull(); if (state.pane === 'lyrics') loadLyrics(); } };
  f.querySelector('#f-toggle').onclick = toggle;
  f.querySelector('#f-next').onclick = () => next();
  f.querySelector('#f-prev').onclick = prev;
  f.querySelector('#f-shuffle').onclick = shuffleUpNext;
  f.querySelector('#f-repeat').onclick = () => { state.repeat = { off: 'all', all: 'one', one: 'off' }[state.repeat]; renderFull(); };
  f.querySelector('#f-auto').onchange = (e) => { try { localStorage.setItem('encore.autoplay', e.target.checked ? 'on' : 'off'); } catch { /* ignore */ } renderFull(); };
  f.querySelector('#fvol').oninput = (e) => { audio.volume = spare.volume = e.target.value; const v = document.getElementById('vol'); if (v) v.value = audio.volume; };
  bindSeek(f.querySelector('#fseek'), 'f-cur');
  f.querySelector('.np-list').onclick = (e) => {
    const rm = e.target.closest('[data-rm]');
    if (rm) return removeAt(+rm.dataset.rm);
    const up = e.target.closest('[data-up]');
    if (up) return moveTo(+up.dataset.up, +up.dataset.up - 1);
    const it = e.target.closest('[data-i]');
    if (it) jump(+it.dataset.i);
  };
  f.querySelector('#np-lyrics').onclick = (e) => { const l = e.target.closest('[data-t]'); if (l) seekTo(+l.dataset.t); };
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
  const t = current();
  if (!t) return '';
  if (t.preview) return '<div class="muted np-empty">Lyrics are available for songs in your library.</div>';
  if (lyr.loading || lyr.id !== t.id) return '<div class="muted np-empty"><span class="spinner"></span> Looking for lyrics…</div>';
  if (lyr.lines.length) return lyr.lines.map((l, i) => `<p data-t="${l.t}" data-li="${i}">${esc(l.text) || '♪'}</p>`).join('');
  if (lyr.plain) return `<div class="np-plain">${esc(lyr.plain)}</div>`;
  return `<div class="muted np-empty">${lyr.instrumental ? 'Instrumental' : 'No lyrics found for this song.'}</div>`;
}

async function loadLyrics() {
  const t = current();
  if (!t || t.preview || lyr.id === t.id) { const box = document.getElementById('np-lyrics'); if (box) box.innerHTML = lyricsHtml(); return; }
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
  if (!state.full || !lyr.lines.length || lyr.id !== current()?.id) return;
  const p = position() + 0.3;
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
