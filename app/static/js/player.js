import { esc, fmt, icon, art } from './ui.js';

const audio = () => document.getElementById('audio');
const state = { queue: [], index: -1 };
const listeners = new Set();

export function onChange(fn) { listeners.add(fn); return () => listeners.delete(fn); }
const emit = () => listeners.forEach(fn => fn(current()));

export function current() { return state.queue[state.index] || null; }

export function playQueue(tracks, start = 0) {
  state.queue = tracks;
  state.index = start;
  load();
}

export function enqueue(tracks) {
  state.queue.push(...tracks);
  if (state.index < 0) { state.index = 0; load(); } else render();
}

function load() {
  const t = current();
  if (!t) return;
  const a = audio();
  a.src = `/api/library/tracks/${t.id}/stream`;
  a.play().catch(() => {});
  if ('mediaSession' in navigator) {
    navigator.mediaSession.metadata = new MediaMetadata({
      title: t.title, artist: t.artist, album: t.album,
      artwork: t.album_id ? [{ src: `/api/library/albums/${t.album_id}/cover`, sizes: '512x512' }] : [],
    });
  }
  render();
  emit();
}

export function toggle() {
  const a = audio();
  if (!current()) return;
  a.paused ? a.play() : a.pause();
}
export function next() { if (state.index < state.queue.length - 1) { state.index++; load(); } }
export function prev() {
  const a = audio();
  if (a.currentTime > 3 || state.index === 0) { a.currentTime = 0; return; }
  state.index--; load();
}

export function mount() {
  const el = document.createElement('div');
  el.className = 'player';
  el.id = 'player';
  document.body.appendChild(el);
  const a = audio();
  try { a.volume = parseFloat(localStorage.getItem('encore.volume') ?? '0.9'); } catch { /* ignore */ }
  a.addEventListener('ended', next);
  a.addEventListener('play', render);
  a.addEventListener('pause', render);
  a.addEventListener('timeupdate', updateTime);
  a.addEventListener('loadedmetadata', updateTime);
  if ('mediaSession' in navigator) {
    navigator.mediaSession.setActionHandler('nexttrack', next);
    navigator.mediaSession.setActionHandler('previoustrack', prev);
  }
  document.addEventListener('keydown', (e) => {
    if (e.code === 'Space' && !/INPUT|TEXTAREA|SELECT/.test(document.activeElement.tagName) && current()) {
      e.preventDefault(); toggle();
    }
  });
  render();
}

function updateTime() {
  const a = audio();
  const seek = document.getElementById('seek');
  if (!seek) return;
  if (!seek.matches(':active')) seek.value = a.duration ? (a.currentTime / a.duration) * 1000 : 0;
  document.getElementById('t-cur').textContent = fmt.dur(a.currentTime);
  document.getElementById('t-dur').textContent = fmt.dur(a.duration || current()?.duration || 0);
}

function render() {
  const el = document.getElementById('player');
  if (!el) return;
  const t = current();
  const a = audio();
  el.innerHTML = `
    <div class="now">
      <div class="thumb">${t ? art(`/api/library/albums/${t.album_id}/cover`, t.album) : `<div class="ph">${icon.music.replace('<svg', '<svg width="20"')}</div>`}</div>
      <div class="grow">
        <div class="ellipsis" style="font-weight:650">${t ? esc(t.title) : '<span class="faint">Nothing playing</span>'}</div>
        <div class="ellipsis small muted">${t ? `<a href="#/library?artist=${encodeURIComponent(t.albumartist || t.artist)}">${esc(t.artist)}</a>` : 'Pick something from your library'}</div>
      </div>
    </div>
    <div class="ctrls">
      <div class="row">
        <button class="btn ghost icon" id="p-prev" aria-label="Previous">${icon.prev}</button>
        <button class="play-btn" id="p-toggle" aria-label="Play/Pause">${!a.paused ? icon.pause : icon.play}</button>
        <button class="btn ghost icon" id="p-next" aria-label="Next">${icon.next}</button>
      </div>
      <div class="seek"><span id="t-cur">0:00</span><input type="range" id="seek" min="0" max="1000" value="0"><span id="t-dur">0:00</span></div>
    </div>
    <div class="right">
      <span class="faint small">${state.queue.length ? `${state.index + 1} / ${state.queue.length}` : ''}</span>
      <span class="faint">${icon.volume.replace('<svg', '<svg width="18"')}</span>
      <input type="range" id="vol" min="0" max="1" step="0.01" value="${a.volume}">
    </div>`;
  el.querySelector('#p-toggle').onclick = toggle;
  el.querySelector('#p-next').onclick = next;
  el.querySelector('#p-prev').onclick = prev;
  el.querySelector('#seek').oninput = (e) => { if (a.duration) a.currentTime = (e.target.value / 1000) * a.duration; };
  el.querySelector('#vol').oninput = (e) => {
    a.volume = e.target.value;
    try { localStorage.setItem('encore.volume', a.volume); } catch { /* ignore */ }
  };
  updateTime();
  emit();
}
