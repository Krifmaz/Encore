// Use any signed-in device as a remote for the others (Encore 2): every open app checks in every couple of
// seconds with what it's playing and runs the commands other devices sent it.
import { api } from './api.js';
import * as player from './player.js';
import { art, esc, fmt, icon, modal, toast } from './ui.js';

const KEY = 'encore.device';
let started = false;

function deviceId() {
  try {
    let id = localStorage.getItem(KEY);
    if (!id) { id = Math.random().toString(36).slice(2) + Date.now().toString(36); localStorage.setItem(KEY, id); }
    return id;
  } catch { return 'tab-' + Math.random().toString(36).slice(2); }
}

/** A readable name: "Android app", "Chrome on Windows", "Safari on iPhone" … */
function deviceName() {
  if (window.EncoreNative?.saveOffline) return 'Android app';
  const ua = navigator.userAgent;
  if (/Electron/.test(ua)) return `Desktop app (${/Mac/.test(ua) ? 'Mac' : /Windows/.test(ua) ? 'Windows' : 'Linux'})`;
  const os = /iPhone/.test(ua) ? 'iPhone' : /iPad/.test(ua) ? 'iPad' : /Android/.test(ua) ? 'Android' : /Mac/.test(ua) ? 'Mac' : /Windows/.test(ua) ? 'Windows' : /Linux/.test(ua) ? 'Linux' : 'device';
  const br = /Edg\//.test(ua) ? 'Edge' : /Firefox\//.test(ua) ? 'Firefox' : /Chrome\//.test(ua) ? 'Chrome' : /Safari\//.test(ua) ? 'Safari' : 'Browser';
  return `${br} on ${os}`;
}

const ME = deviceId();

async function beat() {
  try {
    const r = await api.post('/remote/heartbeat', { id: ME, name: deviceName(), state: player.snapshot() });
    for (const c of r.commands || []) {
      if (c.action === 'play_queue' && c.from) toast(`Music sent here from ${c.from}`, 'ok');
      player.command(c);
    }
  } catch { /* offline – try again next time */ }
}

export function start(session) {
  if (started || !(session.info?.features || []).includes('remote')) return;
  started = true;
  window.encoreRemote = { open };
  const loop = () => {
    beat().finally(() => setTimeout(loop, document.hidden && !player.snapshot().playing ? 6000 : 2000));
  };
  loop();
}

/** The Devices window: what every other device is playing, with controls. */
export function open() {
  let timer;
  const m = modal({ title: 'Your devices', body: '<div id="dev-list" class="stack"><div class="skeleton" style="height:90px"></div></div>', onMount: () => {} });
  const list = m.el.querySelector('#dev-list');
  const send = async (id, body) => {
    try { await api.post(`/remote/${encodeURIComponent(id)}/command`, { ...body, from: deviceName() }); setTimeout(refresh, 400); } catch (e) { toast(e.message, 'err'); }
  };
  let devices = [];
  const refresh = async () => {
    if (!list.isConnected) return clearInterval(timer);
    try { devices = (await api.get('/remote/devices')).filter(d => d.id !== ME); } catch { devices = []; }
    if (!devices.length) {
      list.innerHTML = `<p class="muted" style="margin:0">No other devices right now. Open Encore on your phone, computer or the apps (signed in as you) and they show up here.</p>`;
      return;
    }
    list.innerHTML = devices.map((d, i) => {
      const s = d.state || {};
      const now = s.title ? `<div class="row" style="gap:12px">
          <div class="thumb" style="width:52px;height:52px">${art(s.cover, s.album || s.title)}</div>
          <div class="grow"><div class="ellipsis" style="font-weight:650">${esc(s.title)}</div><div class="small muted ellipsis">${esc(s.artist || '')}${s.album ? ' · ' + esc(s.album) : ''}</div>
            <div class="small faint">${s.playing ? 'Playing' : 'Paused'} · ${fmt.dur(s.position || 0)} / ${fmt.dur(s.duration || 0)}</div></div></div>`
        : '<div class="small muted">Nothing playing</div>';
      return `<div class="card stack" data-i="${i}" style="gap:10px">
        <div class="row between"><b>${icon.devices.replace('<svg', '<svg width="16" style="vertical-align:-3px;margin-right:6px"')}${esc(d.name)}</b></div>
        ${now}
        <div class="row wrap" style="gap:8px">
          ${s.title ? `<button class="btn sm ghost icon" data-a="prev" aria-label="Previous">${icon.prev}</button>
            <button class="btn sm" data-a="toggle">${s.playing ? icon.pause + 'Pause' : icon.play + 'Play'}</button>
            <button class="btn sm ghost icon" data-a="next" aria-label="Next">${icon.next}</button>
            <input type="range" min="0" max="1" step="0.05" value="${s.volume ?? 0.9}" data-vol title="Volume" style="width:110px;accent-color:var(--accent)">
            <button class="btn sm" data-a="here" title="Continue this song on this device">Play here</button>` : ''}
          ${player.current() ? `<button class="btn sm primary" data-a="send" title="Move what you're playing to ${esc(d.name)}">Send my music there</button>` : ''}
        </div></div>`;
    }).join('');
  };
  list.addEventListener('click', (e) => {
    const b = e.target.closest('[data-a]');
    if (!b) return;
    const d = devices[+b.closest('[data-i]').dataset.i];
    const a = b.dataset.a;
    if (a === 'here') {
      const s = d.state || {};
      if (!s.queue?.length) return;
      player.playQueue(s.queue, s.index || 0);
      const at = s.position || 0;
      setTimeout(() => player.seekTo(at), 600);
      send(d.id, { action: 'pause' });
      return m.close();
    }
    if (a === 'send') {
      const snap = player.snapshot();
      if (!snap.queue?.length) return toast('Only songs from your library can be sent', 'err');
      send(d.id, { action: 'play_queue', tracks: snap.queue, index: snap.queue.findIndex(t => t.id === snap.id), position: snap.position });
      player.command({ action: 'pause' });
      return;
    }
    send(d.id, { action: a });
  });
  list.addEventListener('change', (e) => {
    const v = e.target.closest('[data-vol]');
    if (v) send(devices[+v.closest('[data-i]').dataset.i].id, { action: 'volume', value: Number(v.value) });
  });
  refresh();
  timer = setInterval(refresh, 2000);
}
