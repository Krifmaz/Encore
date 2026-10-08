// Web player: streams the original file (bit-perfect for MP3/FLAC/AAC/Opus); formats a browser
// can't decode (ALAC/APE/WavPack/AIFF/WMA) are converted to FLAC on the fly by the server.
import { art, esc, fmt, icon } from './ui.js';

const state = { queue: [], index: -1, transcoded: false };
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

function load(transcode = false) {
  const t = current();
  if (!t) return;
  state.transcoded = transcode;
  audio.src = `/api/library/tracks/${t.id}/stream${transcode ? '?transcode=1' : ''}`;
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

export function toggle() { if (current()) (audio.paused ? audio.play() : audio.pause()); }
export function next() { if (state.index < state.queue.length - 1) { state.index++; load(); } }
export function prev() {
  if (audio.currentTime > 3 || state.index === 0) { audio.currentTime = 0; return; }
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
  audio.addEventListener('error', () => { if (current() && !state.transcoded) load(true); });
  if ('mediaSession' in navigator) {
    navigator.mediaSession.setActionHandler('play', () => audio.play());
    navigator.mediaSession.setActionHandler('pause', () => audio.pause());
    navigator.mediaSession.setActionHandler('nexttrack', next);
    navigator.mediaSession.setActionHandler('previoustrack', prev);
  }
  document.addEventListener('keydown', (e) => {
    if (e.code === 'Space' && current() && !/INPUT|TEXTAREA|SELECT|BUTTON/.test(document.activeElement.tagName)) {
      e.preventDefault(); toggle();
    }
  });
  render();
}

function updateTime() {
  const seek = document.getElementById('seek');
  if (!seek) return;
  const dur = audio.duration && isFinite(audio.duration) ? audio.duration : current()?.duration || 0;
  if (!seek.matches(':active')) seek.value = dur ? (audio.currentTime / dur) * 1000 : 0;
  document.getElementById('t-cur').textContent = fmt.dur(audio.currentTime);
  document.getElementById('t-dur').textContent = fmt.dur(dur);
  const bar = document.getElementById('p-mini');
  if (bar) bar.style.width = `${dur ? (audio.currentTime / dur) * 100 : 0}%`;
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
  el.classList.toggle('hidden', !t);
  if (!t) return;
  el.innerHTML = `
    <div class="p-mini"><i id="p-mini"></i></div>
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
  el.querySelector('#seek').oninput = (e) => {
    const dur = audio.duration && isFinite(audio.duration) ? audio.duration : 0;
    if (dur) audio.currentTime = (e.target.value / 1000) * dur;
  };
  el.querySelector('#vol').oninput = (e) => {
    audio.volume = e.target.value;
    try { localStorage.setItem('encore.volume', audio.volume); } catch { /* ignore */ }
  };
  el.querySelector('#p-close').onclick = () => {
    audio.pause(); audio.removeAttribute('src'); state.queue = []; state.index = -1;
    document.body.classList.remove('has-player'); render(); emit();
  };
  updateTime();
}
