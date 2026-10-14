// Use any signed-in device as a remote for the others: every open app checks in every couple of
// seconds with what it's playing and runs the commands other devices sent it. With the server's event stream
// open, commands and the other devices' state arrive the moment they change instead of at the next check-in,
// and this device checks in right away whenever its own playback changes – so a remote feels instant.
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
  const ua = navigator.userAgent;
  if (/EncoreAndroid/.test(ua) || window.EncoreNative?.saveOffline) return 'Android app';
  if (/EncoreDesktop|Electron/.test(ua) || window.EncoreNative?.platform === 'desktop') return `Desktop app (${/Mac/.test(ua) ? 'Mac' : /Windows/.test(ua) ? 'Windows' : 'Linux'})`;
  const os = /iPhone/.test(ua) ? 'iPhone' : /iPad/.test(ua) ? 'iPad' : /Android/.test(ua) ? 'Android' : /Mac/.test(ua) ? 'Mac' : /Windows/.test(ua) ? 'Windows' : /Linux/.test(ua) ? 'Linux' : 'device';
  const br = /Edg\//.test(ua) ? 'Edge' : /Firefox\//.test(ua) ? 'Firefox' : /Chrome\//.test(ua) ? 'Chrome' : /Safari\//.test(ua) ? 'Safari' : 'Browser';
  return `${br} on ${os}`;
}

const ME = deviceId();
const CHAIN = 'encore.link';
// The Cadenza engine built into the desktop app: it signs in with its own token and shows up as a device of
// its own, which this window links to on its own – unless you unlinked it (encore.engine.off = "1").
const ENGINE_OFF = 'encore.engine.off';
export const ENGINE_LABEL = 'This computer (Cadenza)';
let engine = null;         // { id, name } of this computer's engine (desktop app only), known before it is online
let devices = [];          // the other devices, as the server last told us (pushed, or fetched)
let onDevices = null;      // the open Devices window repaints through this
const seen = [];           // commands already run (each one can arrive pushed and again on the next check-in)
let es = null;             // the server's event stream
let beatTimer = null;
let missed = 0;

// Linked: this device is locked as a remote for another one until you unlink.
function savedLink() { try { return JSON.parse(localStorage.getItem(CHAIN) || 'null'); } catch { return null; } }
const sendTo = (id, body) => api.post(`/remote/${encodeURIComponent(id)}/command`, { ...body, from: deviceName() });

/** This computer's Cadenza engine as a device `{ id, name }` – only in the desktop app, null elsewhere. */
export function engineDevice() { return engine; }
export const isEngine = (id) => !!engine && id === engine.id;
/** The name a device is shown under: the built-in engine is "This computer (Cadenza)". */
export const shownName = (d) => isEngine(d.id) ? ENGINE_LABEL : d.name;
/** You unlinked this computer's engine: it is not linked to again on its own. */
export const engineOff = () => { try { return localStorage.getItem(ENGINE_OFF) === '1'; } catch { return false; } };

export function linkTo(d, { silent = false } = {}) {
  const name = shownName(d);
  if (isEngine(d.id)) { try { localStorage.removeItem(ENGINE_OFF); } catch { /* ignore */ } }
  try { localStorage.setItem(CHAIN, JSON.stringify({ id: d.id, name })); } catch { /* ignore */ }
  player.setLink({ id: d.id, name, state: d.state || {}, online: true,
    send: (c) => sendTo(d.id, c).catch(e => toast(e.message, 'err')) });
  if (!silent) toast(`Linked – this device now controls ${name}`, 'ok');
  beat();
}
export function unlink() {
  if (isEngine(player.linked()?.id)) { try { localStorage.setItem(ENGINE_OFF, '1'); } catch { /* ignore */ } }
  try { localStorage.removeItem(CHAIN); } catch { /* ignore */ }
  player.setLink(null);
  toast('Unlinked – music plays on this device again', 'ok');
  beat();
}

/** Desktop app: sign the built-in engine in. The app knows the engine's id and name before it runs; the
 *  server hands out a token for them, and the app starts the engine with it. Idempotent – safe on every start. */
async function engineSignIn() {
  const n = window.EncoreNative;
  if (!n?.engine || !n.engineStatus || !n.engineLogin) return;
  try {
    let st = await n.engineStatus();
    if (typeof st === 'string') st = JSON.parse(st);
    if (!st?.device_id) return;
    engine = { id: st.device_id, name: st.name || 'Cadenza' };
    // already running with a token and no error: keep it – a new token would restart the engine's link
    if (st.running && !st.error) return;
    const r = await api.post('/auth/app-token', { device_id: st.device_id, device_name: engine.name });
    let r2 = await n.engineLogin(JSON.stringify({ token: r.token, server: location.origin }));
    if (typeof r2 === 'string') { try { r2 = JSON.parse(r2); } catch { r2 = null; } }
    // the app answers with the id it signed in under – that is the device to look for
    if (r2?.device_id) engine = { id: r2.device_id, name: r2.name || engine.name };
  } catch { /* Settings → Playback → Cadenza engine says what is wrong */ }
}

/** Run a command another device sent us – once, however many ways it arrived. */
function runCommand(c) {
  if (!c) return;
  if (c.seq != null) {
    if (seen.includes(c.seq)) return;
    seen.push(c.seq);
    if (seen.length > 200) seen.splice(0, 100);
  }
  if (c.action === 'play_queue' && c.from) toast(`Music sent here from ${c.from}`, 'ok');
  player.command(c);
}

/** The other devices changed (pushed by the server, or fetched). */
function gotDevices(list) {
  devices = (list || []).filter(d => d.id !== ME);
  let link = player.linked();
  // this computer's engine came online: link to it on its own (quietly), unless you unlinked it
  if (engine && !link && !engineOff()) {
    const d = devices.find(x => x.id === engine.id);
    if (d) { linkTo(d, { silent: true }); link = player.linked(); }
  }
  if (link) {
    const d = devices.find(x => x.id === link.id);
    missed = d ? 0 : missed + 1;
    if (d) player.linkState(d.state, true); else if (missed > 1) player.linkState(link.state, false);
  }
  onDevices?.();
}

async function beat() {
  const link = player.linked();
  try {
    const state = link ? { playing: false, remote_of: link.id } : player.snapshot();
    const r = await api.post('/remote/heartbeat', { id: ME, name: deviceName(), state });
    for (const c of r.commands || []) runCommand(c);
    // with the event stream open the server pushes the others' state; linked, we still ask so a device that went quiet shows as offline
    if (link || !streaming()) gotDevices(await api.get('/remote/devices'));
  } catch { /* offline – try again next time */ }
}
/** Check in right away (debounced): something about this device's playback just changed. */
function beatSoon() {
  clearTimeout(beatTimer);
  beatTimer = setTimeout(beat, 150);
}

const streaming = () => !!es && es.readyState === 1;
/** Open the server's event stream: our commands as they are sent, every other device's state as it changes. */
function connect() {
  if (es || typeof EventSource === 'undefined') return;
  es = new EventSource(`/api/remote/events?device=${encodeURIComponent(ME)}`);
  es.addEventListener('command', (e) => { try { runCommand(JSON.parse(e.data).command); } catch { /* ignore */ } });
  es.addEventListener('devices', (e) => { try { gotDevices(JSON.parse(e.data).devices); } catch { /* ignore */ } });
  es.onerror = () => {
    // the browser reconnects on its own; when it gives up (signed out, server gone) try again later – the heartbeat covers the gap
    if (es?.readyState === 2) { es = null; setTimeout(connect, 30000); }
  };
}

export function start(session) {
  if (started || !(session.info?.features || []).includes('remote')) return;
  started = true;
  window.encoreRemote = { open, unlink, engine: engineDevice };
  const l = savedLink();
  if (l) player.setLink({ ...l, state: {}, online: true, send: (c) => sendTo(l.id, c).catch(e => toast(e.message, 'err')) });
  engineSignIn();
  connect();
  player.onChange(beatSoon);
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
    try { await sendTo(id, body); if (!streaming()) setTimeout(refresh, 400); } catch (e) { toast(e.message, 'err'); }
  };
  const paintMe = () => {
    const l = player.linked();
    meBox.innerHTML = `<div class="row wrap between" style="gap:8px"><div class="small muted">This device: <b style="color:var(--text)">${esc(deviceName())}</b> <a href="#" id="dev-rename" class="small">rename</a></div>
      ${l ? `<button class="btn sm" id="dev-unlink">${icon.link}Unlink from ${esc(l.name)}</button>` : ''}</div>
      ${l ? `<div class="small muted">🔗 Linked: everything you play here goes to <b>${esc(l.name)}</b>. Unlink to listen on this device.</div>` : ''}`;
    meBox.querySelector('#dev-unlink')?.addEventListener('click', () => { unlink(); paintMe(); paint(); });
    meBox.querySelector('#dev-rename').onclick = (e) => {
      e.preventDefault();
      const n = prompt('Name this device (shown on your other devices)', deviceName());
      if (n === null) return;
      try { if (n.trim()) localStorage.setItem('encore.device.name', n.trim().slice(0, 60)); else localStorage.removeItem('encore.device.name'); } catch { /* ignore */ }
      paintMe(); beat();
    };
  };
  const byId = (id) => devices.find(d => d.id === id);
  // Roon zones (when Roon is enabled): shown before the Encore devices, refreshed with them
  const roonOn = (window.encoreSession?.info?.features || []).includes('roon');
  let zones = [];
  const zoneCard = (z) => {
    const np = z.now_playing;
    const vol = (z.outputs || []).find(o => o.volume)?.volume;
    const v = vol ? Math.max(0, Math.min(1, (vol.value - vol.min) / ((vol.max - vol.min) || 1))) : null;
    return `<div class="card stack" data-zone="${esc(z.zone_id)}" style="gap:10px">
        <div class="row between"><b>${icon.devices.replace('<svg', '<svg width="16" style="vertical-align:-3px;margin-right:6px"')}${esc(z.name)} <span class="badge" style="margin-left:6px">Roon</span></b>
          <button class="btn sm" data-z="search" title="Search Roon's library and play the first match here">Play in Roon…</button></div>
        ${np ? `<div class="row" style="gap:12px">
          <div class="thumb" style="width:52px;height:52px">${art(np.image_key ? `/api/roon/image/${encodeURIComponent(np.image_key)}?s=104` : '', np.line1 || '')}</div>
          <div class="grow"><div class="ellipsis" style="font-weight:650">${esc(np.line1 || '')}</div><div class="small muted ellipsis">${esc(np.line2 || '')}${np.line3 ? ' · ' + esc(np.line3) : ''}</div>
            <div class="small faint">${z.state === 'playing' ? 'Playing' : z.state === 'paused' ? 'Paused' : z.state === 'loading' ? 'Loading' : 'Stopped'}${np.length ? ` · ${fmt.dur(np.seek_position || 0)} / ${fmt.dur(np.length)}` : ''}</div></div></div>`
        : '<div class="small muted">Nothing playing</div>'}
        <div class="row wrap" style="gap:8px">
          <button class="btn sm ghost icon" data-z="prev" aria-label="Previous" ${z.is_previous_allowed ? '' : 'disabled'}>${icon.prev}</button>
          <button class="btn sm" data-z="toggle">${z.state === 'playing' ? icon.pause + 'Pause' : icon.play + 'Play'}</button>
          <button class="btn sm ghost icon" data-z="next" aria-label="Next" ${z.is_next_allowed ? '' : 'disabled'}>${icon.next}</button>
          ${v !== null ? `<input type="range" min="0" max="1" step="0.02" value="${v}" data-zvol title="Volume" style="width:110px;accent-color:var(--accent)">` : ''}
        </div></div>`;
  };
  const paint = () => {
    if (!list.isConnected) { clearInterval(timer); onDevices = null; return; }
    if (!devices.length && !zones.length) {
      list.innerHTML = `<p class="muted" style="margin:0">No other devices right now. Open Encore on your phone, computer or the apps (signed in as you) and they show up here.${roonOn ? ' Roon zones appear once Encore is connected to your Roon Core (Settings → Apps & sources → Roon).' : ''}</p>`;
      return;
    }
    const l = player.linked();
    list.innerHTML = zones.map(zoneCard).join('') + devices.map((d) => {
      const s = d.state || {};
      const isLinked = l?.id === d.id;
      const remoteOfMe = s.remote_of === ME;
      const name = shownName(d);
      const now = s.remote_of ? `<div class="small muted">🔗 Remote for ${esc(s.remote_of === ME ? 'this device' : (devices.find(x => x.id === s.remote_of)?.name || 'another device'))}</div>`
        : s.title ? `<div class="row" style="gap:12px">
          <div class="thumb" style="width:52px;height:52px">${art(s.cover, s.album || s.title)}</div>
          <div class="grow"><div class="ellipsis" style="font-weight:650">${esc(s.title)}</div><div class="small muted ellipsis">${esc(s.artist || '')}${s.album ? ' · ' + esc(s.album) : ''}</div>
            <div class="small faint">${s.playing ? 'Playing' : 'Paused'} · ${fmt.dur(s.position || 0)} / ${fmt.dur(s.duration || 0)}${s.signal?.label ? ' · ' + esc(s.signal.label) : ''}</div></div></div>`
        : '<div class="small muted">Nothing playing</div>';
      return `<div class="card stack ${isLinked ? 'linked' : ''}" data-id="${esc(d.id)}" style="gap:10px">
        <div class="row between"><b>${icon.devices.replace('<svg', '<svg width="16" style="vertical-align:-3px;margin-right:6px"')}${esc(name)}</b>
          ${remoteOfMe || s.remote_of ? '' : isLinked ? `<button class="btn sm primary" data-a="unlink">${icon.link}Linked · Unlink</button>`
            : `<button class="btn sm" data-a="link" title="Lock this device as a remote for ${esc(name)}: everything you play here plays there">${icon.link}Link as remote</button>`}</div>
        ${now}
        ${s.remote_of ? '' : `<div class="row wrap" style="gap:8px">
          ${s.title ? `<button class="btn sm ghost icon" data-a="prev" aria-label="Previous">${icon.prev}</button>
            <button class="btn sm" data-a="toggle">${s.playing ? icon.pause + 'Pause' : icon.play + 'Play'}</button>
            <button class="btn sm ghost icon" data-a="next" aria-label="Next">${icon.next}</button>
            <input type="range" min="0" max="1" step="0.05" value="${s.volume ?? 0.9}" data-vol title="Volume" style="width:110px;accent-color:var(--accent)">
            ${isLinked ? '' : '<button class="btn sm" data-a="here" title="Continue this song on this device">Play here</button>'}` : ''}
          ${player.current() && !l ? `<button class="btn sm primary" data-a="send" title="Move what you're playing to ${esc(name)}">Send my music there</button>` : ''}
        </div>`}</div>`;
    }).join('');
  };
  const refreshZones = async () => {
    if (!roonOn || !list.isConnected) return;
    try { const z = await api.get('/roon/zones'); if (JSON.stringify(z) !== JSON.stringify(zones)) { zones = z; paint(); } } catch { /* Roon away */ }
  };
  const refresh = async () => {
    if (!list.isConnected) { clearInterval(timer); onDevices = null; return; }
    try { gotDevices(await api.get('/remote/devices')); } catch { devices = []; paint(); }
    refreshZones();
  };
  list.addEventListener('click', async (e) => {
    const b = e.target.closest('[data-z]');
    if (!b) return;
    const id = b.closest('[data-zone]').dataset.zone;
    const z = zones.find(x => x.zone_id === id);
    try {
      if (b.dataset.z === 'search') {
        const q = prompt(`Play in Roon (${z?.name || 'zone'}): artist, album or song`, '');
        if (!q || !q.trim()) return;
        const r = await api.post(`/roon/zones/${encodeURIComponent(id)}/play`, { query: q.trim() });
        toast(`Playing ${r.playing} in ${z?.name || 'Roon'}`, 'ok');
      } else {
        await api.post(`/roon/zones/${encodeURIComponent(id)}/command`, { action: b.dataset.z });
      }
      setTimeout(refreshZones, 500);
    } catch (err) { toast(err.message, 'err'); }
  });
  list.addEventListener('change', (e) => {
    const v = e.target.closest('[data-zvol]');
    if (v) api.post(`/roon/zones/${encodeURIComponent(v.closest('[data-zone]').dataset.zone)}/command`, { action: 'volume', value: Number(v.value) }).catch(err => toast(err.message, 'err'));
  });
  list.addEventListener('click', (e) => {
    const b = e.target.closest('[data-a]');
    if (!b) return;
    const d = byId(b.closest('[data-id]').dataset.id);
    if (!d) return;
    const a = b.dataset.a;
    if (a === 'link') { linkTo(d); paintMe(); return paint(); }
    if (a === 'unlink') { unlink(); paintMe(); return paint(); }
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
  onDevices = paint;
  paintMe();
  refresh();
  // pushed updates repaint the list; this is the fallback when the stream is down (Roon zones are always polled)
  timer = setInterval(() => { if (!streaming()) refresh(); else refreshZones(); }, 2000);
}
