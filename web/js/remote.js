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

/** A readable name: "Android app", "Chrome on Windows", "Safari on iPhone" … (or the one you gave it) */
function deviceName() {
  try { const own = localStorage.getItem('encore.device.name'); if (own) return own; } catch { /* ignore */ }
  if (window.EncoreNative?.saveOffline) return 'Android app';
  const ua = navigator.userAgent;
  if (/Electron/.test(ua)) return `Desktop app (${/Mac/.test(ua) ? 'Mac' : /Windows/.test(ua) ? 'Windows' : 'Linux'})`;
  const os = /iPhone/.test(ua) ? 'iPhone' : /iPad/.test(ua) ? 'iPad' : /Android/.test(ua) ? 'Android' : /Mac/.test(ua) ? 'Mac' : /Windows/.test(ua) ? 'Windows' : /Linux/.test(ua) ? 'Linux' : 'device';
  const br = /Edg\//.test(ua) ? 'Edge' : /Firefox\//.test(ua) ? 'Firefox' : /Chrome\//.test(ua) ? 'Chrome' : /Safari\//.test(ua) ? 'Safari' : 'Browser';
  return `${br} on ${os}`;
}

const ME = deviceId();
const CHAIN = 'encore.link';

// Linked: this device is locked as a remote for another one until you unlink.
function savedLink() { try { return JSON.parse(localStorage.getItem(CHAIN) || 'null'); } catch { return null; } }
const sendTo = (id, body) => api.post(`/remote/${encodeURIComponent(id)}/command`, { ...body, from: deviceName() });

export function linkTo(d) {
  try { localStorage.setItem(CHAIN, JSON.stringify({ id: d.id, name: d.name })); } catch { /* ignore */ }
  player.setLink({ id: d.id, name: d.name, state: d.state || {}, online: true,
    send: (c) => sendTo(d.id, c).catch(e => toast(e.message, 'err')) });
  toast(`Linked – this device now controls ${d.name}`, 'ok');
  beat();
}
export function unlink() {
  try { localStorage.removeItem(CHAIN); } catch { /* ignore */ }
  player.setLink(null);
  toast('Unlinked – music plays on this device again', 'ok');
}

let missed = 0;
async function beat() {
  const link = player.linked();
  try {
    const state = link ? { playing: false, remote_of: link.id } : player.snapshot();
    const r = await api.post('/remote/heartbeat', { id: ME, name: deviceName(), state });
    for (const c of r.commands || []) {
      if (c.action === 'play_queue' && c.from) toast(`Music sent here from ${c.from}`, 'ok');
      player.command(c);
    }
    if (link) {
      const d = (await api.get('/remote/devices')).find(x => x.id === link.id);
      missed = d ? 0 : missed + 1;
      if (d) player.linkState(d.state, true); else if (missed > 1) player.linkState(link.state, false);
    }
  } catch { /* offline – try again next time */ }
}

export function start(session) {
  if (started || !(session.info?.features || []).includes('remote')) return;
  started = true;
  window.encoreRemote = { open, unlink };
  const l = savedLink();
  if (l) player.setLink({ ...l, state: {}, online: true, send: (c) => sendTo(l.id, c).catch(e => toast(e.message, 'err')) });
  const loop = () => {
    const busy = player.linked() || player.snapshot().playing;
    beat().finally(() => setTimeout(loop, document.hidden && !busy ? 6000 : 2000));
  };
  loop();
}

/** The Devices window: what every other device is playing, with controls, and Link / Unlink. */
export function open() {
  let timer;
  const m = modal({ title: 'Your devices', body: `<div id="dev-me" class="stack" style="margin-bottom:12px"></div><div id="dev-list" class="stack"><div class="skeleton" style="height:90px"></div></div>`, onMount: () => {} });
  const list = m.el.querySelector('#dev-list');
  const meBox = m.el.querySelector('#dev-me');
  const send = async (id, body) => {
    try { await sendTo(id, body); setTimeout(refresh, 400); } catch (e) { toast(e.message, 'err'); }
  };
  const paintMe = () => {
    const l = player.linked();
    meBox.innerHTML = `<div class="row wrap between" style="gap:8px"><div class="small muted">This device: <b style="color:var(--text)">${esc(deviceName())}</b> <a href="#" id="dev-rename" class="small">rename</a></div>
      ${l ? `<button class="btn sm" id="dev-unlink">${icon.link}Unlink from ${esc(l.name)}</button>` : ''}</div>
      ${l ? `<div class="small muted">🔗 Linked: everything you play here goes to <b>${esc(l.name)}</b>. Unlink to listen on this device.</div>` : ''}`;
    meBox.querySelector('#dev-unlink')?.addEventListener('click', () => { unlink(); paintMe(); refresh(); });
    meBox.querySelector('#dev-rename').onclick = (e) => {
      e.preventDefault();
      const n = prompt('Name this device (shown on your other devices)', deviceName());
      if (n === null) return;
      try { if (n.trim()) localStorage.setItem('encore.device.name', n.trim().slice(0, 60)); else localStorage.removeItem('encore.device.name'); } catch { /* ignore */ }
      paintMe(); beat();
    };
  };
  let devices = [];
  const byId = (id) => devices.find(d => d.id === id);
  const refresh = async () => {
    if (!list.isConnected) return clearInterval(timer);
    try { devices = (await api.get('/remote/devices')).filter(d => d.id !== ME); } catch { devices = []; }
    if (!devices.length) {
      list.innerHTML = `<p class="muted" style="margin:0">No other devices right now. Open Encore on your phone, computer or the apps (signed in as you) and they show up here.</p>`;
      return;
    }
    const l = player.linked();
    list.innerHTML = devices.map((d) => {
      const s = d.state || {};
      const isLinked = l?.id === d.id;
      const remoteOfMe = s.remote_of === ME;
      const now = s.remote_of ? `<div class="small muted">🔗 Remote for ${esc(s.remote_of === ME ? 'this device' : (devices.find(x => x.id === s.remote_of)?.name || 'another device'))}</div>`
        : s.title ? `<div class="row" style="gap:12px">
          <div class="thumb" style="width:52px;height:52px">${art(s.cover, s.album || s.title)}</div>
          <div class="grow"><div class="ellipsis" style="font-weight:650">${esc(s.title)}</div><div class="small muted ellipsis">${esc(s.artist || '')}${s.album ? ' · ' + esc(s.album) : ''}</div>
            <div class="small faint">${s.playing ? 'Playing' : 'Paused'} · ${fmt.dur(s.position || 0)} / ${fmt.dur(s.duration || 0)}</div></div></div>`
        : '<div class="small muted">Nothing playing</div>';
      return `<div class="card stack ${isLinked ? 'linked' : ''}" data-id="${esc(d.id)}" style="gap:10px">
        <div class="row between"><b>${icon.devices.replace('<svg', '<svg width="16" style="vertical-align:-3px;margin-right:6px"')}${esc(d.name)}</b>
          ${remoteOfMe || s.remote_of ? '' : isLinked ? `<button class="btn sm primary" data-a="unlink">${icon.link}Linked · Unlink</button>`
            : `<button class="btn sm" data-a="link" title="Lock this device as a remote for ${esc(d.name)}: everything you play here plays there">${icon.link}Link as remote</button>`}</div>
        ${now}
        ${s.remote_of ? '' : `<div class="row wrap" style="gap:8px">
          ${s.title ? `<button class="btn sm ghost icon" data-a="prev" aria-label="Previous">${icon.prev}</button>
            <button class="btn sm" data-a="toggle">${s.playing ? icon.pause + 'Pause' : icon.play + 'Play'}</button>
            <button class="btn sm ghost icon" data-a="next" aria-label="Next">${icon.next}</button>
            <input type="range" min="0" max="1" step="0.05" value="${s.volume ?? 0.9}" data-vol title="Volume" style="width:110px;accent-color:var(--accent)">
            ${isLinked ? '' : '<button class="btn sm" data-a="here" title="Continue this song on this device">Play here</button>'}` : ''}
          ${player.current() && !l ? `<button class="btn sm primary" data-a="send" title="Move what you're playing to ${esc(d.name)}">Send my music there</button>` : ''}
        </div>`}</div>`;
    }).join('');
  };
  list.addEventListener('click', (e) => {
    const b = e.target.closest('[data-a]');
    if (!b) return;
    const d = byId(b.closest('[data-id]').dataset.id);
    if (!d) return;
    const a = b.dataset.a;
    if (a === 'link') { linkTo(d); paintMe(); return refresh(); }
    if (a === 'unlink') { unlink(); paintMe(); return refresh(); }
    if (a === 'here') {
      const s = d.state || {};
      if (!s.queue?.length) return;
      if (player.linked()) unlink();
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
    if (v) send(v.closest('[data-id]').dataset.id, { action: 'volume', value: Number(v.value) });
  });
  paintMe();
  refresh();
  timer = setInterval(refresh, 2000);
}
