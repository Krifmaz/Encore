// Web player: streams the original file (bit-perfect for MP3/FLAC/AAC/Opus); formats a browser
// can't decode (ALAC/APE/WavPack/AIFF/WMA) are converted to FLAC on the fly by the server.
import { art, esc, fmt, icon } from './ui.js';

const state = { queue: [], index: -1, transcoded: false, offset: 0, dragging: false };
const listeners = new Set();
let audio = null;

export function onChange(fn) { listeners.add(fn); return () => listeners.delete(fn); }
const emit = () => listeners.forEach(fn => { try { fn(current()); } catch { /* ignore */ } });
export function current() { return state.queue[state.index] || null; }

export function playQueue(tracks, start = 0) {
  state.queue = tracks.filter(Boolean);
  state.index = Math.max(0, Math.min(start, state.queue.length - 1));
  load();
}

export function enqueue(tracks) {
  state.queue.push(...tracks);
  if (state.index < 0) { state.index = 0; load(); } else render();
}

// formats browsers can't decode → the server converts to FLAC; the player then seeks via the server
const CONVERT = new Set(['ape', 'wv', 'aiff', 'aif', 'wma', 'alac', 'dsf', 'dff', 'tta', 'mpc']);

function load(transcode = false, offset = 0) {
  const t = current();
  if (!t) return;
  if (CONVERT.has((t.format || '').toLowerCase())) transcode = true;
  state.transcoded = transcode;
  state.offset = offset;   // converted streams restart at the seek point, so their clock starts at `offset`
  state.dragging = false;
  audio.src = `/api/library/tracks/${t.id}/stream${transcode ? `?transcode=1${offset ? `&t=${offset.toFixed(1)}` : ''}` : ''}`;
  audio.play().catch(() => {});
  if ('mediaSession' in navigator) {
    navigator.mediaSession.metadata = new MediaMetadata({
      title: t.title || '', artist: t.artist || '', album: t.album || '',
      artwork: t.album_id ? [{ src: `/api/library/albums/${t.album_id}/cover`, sizes: '512x512' }] : [],
    });
  }
  document.body.classList.add('has-player');
  render();
  emit();
}

function duration() {
  if (state.transcoded) return current()?.duration || 0;
  return audio.duration && isFinite(audio.duration) ? audio.duration : current()?.duration || 0;
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
export function next() { if (state.index < state.queue.length - 1) { state.index++; load(); } }
export function prev() {
  if (position() > 3 || state.index === 0) { seekTo(0); return; }
  state.index--; load();
}

export function mount() {
  if (document.getElementById('player')) return;
  audio = document.getElementById('audio') || Object.assign(document.createElement('audio'), { id: 'audio', preload: 'none' });
  if (!audio.isConnected) document.body.appendChild(audio);
  const el = document.createElement('div');
  el.className = 'player';
  el.id = 'player';
  document.body.appendChild(el);
  try { audio.volume = parseFloat(localStorage.getItem('encore.volume') ?? '0.9'); } catch { /* ignore */ }
  audio.addEventListener('ended', next);
  audio.addEventListener('play', render);
  audio.addEventListener('pause', render);
  audio.addEventListener('timeupdate', updateTime);
  audio.addEventListener('loadedmetadata', updateTime);
  // the browser couldn't decode the original (e.g. ALAC in .m4a) → ask the server for FLAC
  audio.addEventListener('error', () => { if (current() && !state.transcoded) load(true, position()); });
  if ('mediaSession' in navigator) {
    navigator.mediaSession.setActionHandler('play', () => audio.play());
    navigator.mediaSession.setActionHandler('pause', () => audio.pause());
    navigator.mediaSession.setActionHandler('nexttrack', next);
    navigator.mediaSession.setActionHandler('previoustrack', prev);
  }
  document.addEventListener('keydown', (e) => {
    const ae = document.activeElement;
    if (!current() || (/INPUT|TEXTAREA|SELECT|BUTTON/.test(ae.tagName) && ae.id !== 'seek')) return;
    if (e.code === 'Space') { e.preventDefault(); toggle(); }
    if (e.code === 'ArrowRight') { e.preventDefault(); seekTo(position() + 10); }
    if (e.code === 'ArrowLeft') { e.preventDefault(); seekTo(position() - 10); }
  });
  if ('mediaSession' in navigator) {
    try {
      navigator.mediaSession.setActionHandler('seekto', (d) => seekTo(d.seekTime));
      navigator.mediaSession.setActionHandler('seekforward', (d) => seekTo(position() + (d.seekOffset || 10)));
      navigator.mediaSession.setActionHandler('seekbackward', (d) => seekTo(position() - (d.seekOffset || 10)));
    } catch { /* older browsers */ }
  }
  render();
}

function updateTime() {
  const seek = document.getElementById('seek');
  if (!seek) return;
  const dur = duration();
  if (!state.dragging) {
    seek.value = dur ? (position() / dur) * 1000 : 0;
    document.getElementById('t-cur').textContent = fmt.dur(position());
  }
  seek.style.setProperty('--p', `${seek.value / 10}%`);
  document.getElementById('t-dur').textContent = fmt.dur(dur);
}

function quality(t) {
  if (!t) return '';
  const f = (t.format || '').toUpperCase();
  return state.transcoded ? `${f} → FLAC` : `${f}${t.bitrate ? ' ' + t.bitrate + 'k' : ''}`;
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
    return;
  }
  el.innerHTML = `
    <div class="now">
      <div class="thumb">${art(`/api/library/albums/${t.album_id}/cover`, t.album || t.title)}</div>
      <div class="grow">
        <div class="ellipsis" style="font-weight:650">${esc(t.title)}</div>
        <div class="ellipsis small muted"><a href="#/library/album/${esc(t.album_id)}">${esc(t.artist)}${t.album ? ' · ' + esc(t.album) : ''}</a></div>
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
      <span class="badge" title="${state.transcoded ? 'Converted to FLAC for your browser (lossless)' : 'Original file, no conversion'}">${esc(quality(t))}</span>
      <span class="faint small">${state.index + 1}/${state.queue.length}</span>
      <span class="faint">${icon.volume.replace('<svg', '<svg width="18"')}</span>
      <input type="range" id="vol" min="0" max="1" step="0.01" value="${audio.volume}">
      <button class="btn ghost icon" id="p-close" aria-label="Close player">${icon.x}</button>
    </div>`;
  el.querySelector('#p-toggle').onclick = toggle;
  el.querySelector('#p-next').onclick = next;
  el.querySelector('#p-prev').onclick = prev;
  // dragging: move the thumb + time label freely, jump when the finger/mouse is released
  const seek = el.querySelector('#seek');
  const startDrag = () => { state.dragging = true; };
  seek.addEventListener('pointerdown', startDrag);
  seek.addEventListener('touchstart', startDrag, { passive: true });
  seek.addEventListener('input', () => {
    state.dragging = true;
    document.getElementById('t-cur').textContent = fmt.dur((seek.value / 1000) * duration());
    seek.style.setProperty('--p', `${seek.value / 10}%`);
  });
  const commit = () => { if (state.dragging) { state.dragging = false; seekTo((seek.value / 1000) * duration()); } };
  seek.addEventListener('change', commit);
  seek.addEventListener('pointerup', commit);
  seek.addEventListener('touchend', commit);
  el.querySelector('#vol').oninput = (e) => {
    audio.volume = e.target.value;
    try { localStorage.setItem('encore.volume', audio.volume); } catch { /* ignore */ }
  };
  el.querySelector('#p-close').onclick = () => {
    audio.pause(); audio.removeAttribute('src'); state.queue = []; state.index = -1;
    render(); emit();
  };
  updateTime();
}
