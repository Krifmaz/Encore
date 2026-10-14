import { api } from './api.js';
export const esc = (s) => String(s ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
export const qs = (sel, el = document) => el.querySelector(sel);
export const qsa = (sel, el = document) => [...el.querySelectorAll(sel)];

const P = (d) => `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${d}</svg>`;
export const icon = {
  home: P('<path d="M3 10.5 12 3l9 7.5V20a1 1 0 0 1-1 1h-5v-6H9v6H4a1 1 0 0 1-1-1z"/>'),
  search: P('<circle cx="11" cy="11" r="7"/><path d="m21 21-4.3-4.3"/>'),
  library: P('<path d="M4 4v16M9 4v16M14 4l6 16"/>'),
  inbox: P('<path d="M22 12h-6l-2 3h-4l-2-3H2"/><path d="M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z"/>'),
  download: P('<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><path d="m7 10 5 5 5-5M12 15V3"/>'),
  activity: P('<path d="M22 12h-4l-3 9L9 3l-3 9H2"/>'),
  users: P('<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M22 21v-2a4 4 0 0 0-3-3.87M16 3.13a4 4 0 0 1 0 7.75"/>'),
  ticket: P('<path d="M2 9a3 3 0 0 0 0 6v2a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-2a3 3 0 0 0 0-6V7a2 2 0 0 0-2-2H4a2 2 0 0 0-2 2z"/><path d="M13 5v2M13 17v2M13 11v2"/>'),
  settings: P('<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/>'),
  play: '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M7 4.5v15a1 1 0 0 0 1.5.86l12-7.5a1 1 0 0 0 0-1.72l-12-7.5A1 1 0 0 0 7 4.5z"/></svg>',
  pause: '<svg viewBox="0 0 24 24" fill="currentColor"><rect x="6" y="4" width="4" height="16" rx="1"/><rect x="14" y="4" width="4" height="16" rx="1"/></svg>',
  next: '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M5 5.5v13a1 1 0 0 0 1.5.86L16 13.5V18a1 1 0 0 0 2 0V6a1 1 0 0 0-2 0v4.5L6.5 4.64A1 1 0 0 0 5 5.5z"/></svg>',
  prev: '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M19 5.5v13a1 1 0 0 1-1.5.86L8 13.5V18a1 1 0 0 1-2 0V6a1 1 0 0 1 2 0v4.5l9.5-5.86A1 1 0 0 1 19 5.5z"/></svg>',
  plus: P('<path d="M12 5v14M5 12h14"/>'),
  check: P('<path d="M20 6 9 17l-5-5"/>'),
  x: P('<path d="M18 6 6 18M6 6l12 12"/>'),
  menu: P('<path d="M3 6h18M3 12h18M3 18h18"/>'),
  logout: P('<path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4M16 17l5-5-5-5M21 12H9"/>'),
  tag: P('<path d="M20.59 13.41 13.42 20.58a2 2 0 0 1-2.83 0L2 12V2h10l8.59 8.59a2 2 0 0 1 0 2.82z"/><circle cx="7" cy="7" r="1.5"/>'),
  edit: P('<path d="M12 20h9M16.5 3.5a2.12 2.12 0 1 1 3 3L7 19l-4 1 1-4z"/>'),
  trash: P('<path d="M3 6h18M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/>'),
  refresh: P('<path d="M21 12a9 9 0 1 1-2.64-6.36L21 8M21 3v5h-5"/>'),
  link: P('<path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/>'),
  music: P('<path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/>'),
  disc: P('<circle cx="12" cy="12" r="10"/><circle cx="12" cy="12" r="3"/>'),
  user: P('<path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2"/><circle cx="12" cy="7" r="4"/>'),
  sun: P('<circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41"/>'),
  moon: P('<path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/>'),
  copy: P('<rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>'),
  volume: P('<path d="M11 5 6 9H2v6h4l5 4zM15.54 8.46a5 5 0 0 1 0 7.07M19.07 4.93a10 10 0 0 1 0 14.14"/>'),
  external: P('<path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6M15 3h6v6M10 14 21 3"/>'),
  shield: P('<path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/>'),
  heart: P('<path d="M20.84 4.61a5.5 5.5 0 0 0-7.78 0L12 5.67l-1.06-1.06a5.5 5.5 0 0 0-7.78 7.78l1.06 1.06L12 21.23l7.78-7.78 1.06-1.06a5.5 5.5 0 0 0 0-7.78z"/>'),
  spark: P('<path d="M12 3l1.9 5.6L19.5 10l-5.6 1.9L12 17.5l-1.9-5.6L4.5 10l5.6-1.4z"/><path d="M19 15l.8 2.2L22 18l-2.2.8L19 21l-.8-2.2L16 18l2.2-.8z"/>'),
  chevron: P('<polyline points="6 9 12 15 18 9"/>'),
  more: P('<circle cx="12" cy="5" r="1.6"/><circle cx="12" cy="12" r="1.6"/><circle cx="12" cy="19" r="1.6"/>'),
  shuffle: P('<path d="M16 3h5v5M4 20 21 3M21 16v5h-5M15 15l6 6M4 4l5 5"/>'),
  list: P('<path d="M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01"/>'),
  devices: P('<rect x="2" y="4" width="13" height="10" rx="2"/><path d="M6 18h5M8.5 14v4"/><rect x="17" y="8" width="5" height="12" rx="1.5"/><path d="M19.5 17h.01"/>'),
};

export function toast(msg, type = '') {
  const el = document.createElement('div');
  el.className = `toast ${type}`;
  el.textContent = msg;
  document.getElementById('toasts').appendChild(el);
  setTimeout(() => { el.style.opacity = '0'; el.style.transition = 'opacity .3s'; }, 3800);
  setTimeout(() => el.remove(), 4200);
}

export function modal({ title, body, footer = '', wide = false, onMount }) {
  const root = document.getElementById('modal-root');
  const bg = document.createElement('div');
  bg.className = 'modal-bg';
  bg.innerHTML = `<div class="modal ${wide ? 'wide' : ''}" role="dialog" aria-modal="true">
    <div class="modal-head"><h2>${title}</h2><button class="btn ghost icon" data-close aria-label="Close">${icon.x}</button></div>
    <div class="modal-body">${body}</div>
    ${footer ? `<div class="modal-foot">${footer}</div>` : ''}
  </div>`;
  const close = () => { bg.remove(); document.removeEventListener('keydown', onKey); };
  const onKey = (e) => { if (e.key === 'Escape') close(); };
  bg.addEventListener('mousedown', (e) => { if (e.target === bg) close(); });
  bg.querySelectorAll('[data-close]').forEach(b => b.addEventListener('click', close));
  document.addEventListener('keydown', onKey);
  root.appendChild(bg);
  const m = { el: bg.querySelector('.modal'), close };
  onMount && onMount(m);
  return m;
}

export function confirmDialog(title, text, okLabel = 'Confirm', danger = false) {
  return new Promise((resolve) => {
    let done = false;
    const m = modal({
      title: esc(title),
      body: `<p class="muted" style="margin:0">${esc(text)}</p>`,
      footer: `<button class="btn" data-close>Cancel</button><button class="btn ${danger ? 'danger' : 'primary'}" data-ok>${esc(okLabel)}</button>`,
    });
    m.el.querySelector('[data-ok]').onclick = () => { done = true; m.close(); resolve(true); };
    const obs = new MutationObserver(() => { if (!document.body.contains(m.el)) { obs.disconnect(); if (!done) resolve(false); } });
    obs.observe(document.getElementById('modal-root'), { childList: true });
  });
}

export const fmt = {
  dur(s) {
    s = Math.round(s || 0);
    const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), sec = s % 60;
    return h ? `${h}:${String(m).padStart(2, '0')}:${String(sec).padStart(2, '0')}` : `${m}:${String(sec).padStart(2, '0')}`;
  },
  /** "4 mo 22 days 8 hr 24 min" – years, months (30 days), days, hours, minutes; parts that are 0 are left out. */
  longDur(s) {
    let m = Math.round((s || 0) / 60);
    const parts = [];
    for (const [n, one, many] of [[525600, 'yr', 'yr'], [43200, 'mo', 'mo'], [1440, 'day', 'days'], [60, 'hr', 'hr']]) {
      const v = Math.floor(m / n);
      if (v) { parts.push(`${v} ${v === 1 ? one : many}`); m -= v * n; }
    }
    if (m || !parts.length) parts.push(`${m} min`);
    return parts.join(' ');
  },
  size(b) {
    if (!b) return '0 B';
    const u = ['B', 'KB', 'MB', 'GB', 'TB'];
    const i = Math.min(Math.floor(Math.log(b) / Math.log(1024)), 4);
    return `${(b / 1024 ** i).toFixed(i > 1 ? 1 : 0)} ${u[i]}`;
  },
  ago(ts) {
    if (!ts) return 'never';
    const d = Date.now() / 1000 - ts;
    if (d < 60) return 'just now';
    if (d < 3600) return `${Math.floor(d / 60)}m ago`;
    if (d < 86400) return `${Math.floor(d / 3600)}h ago`;
    if (d < 86400 * 30) return `${Math.floor(d / 86400)}d ago`;
    return new Date(ts * 1000).toLocaleDateString();
  },
  date(ts) { return ts ? new Date(ts * 1000).toLocaleString() : '—'; },
  day(ts) { return ts ? new Date(ts * 1000).toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' }) : '—'; },
  num(n) { return (n || 0).toLocaleString(); },
};

export function initials(s) {
  return (s || '?').replace(/^the\s+/i, '').trim().slice(0, 1).toUpperCase() || '?';
}

// Image with graceful fallback to a placeholder letter.
export function art(src, label, cls = '') {
  const ph = `<div class="ph">${esc(initials(label))}</div>`;
  if (!src) return ph;
  // library covers: a small cached version for lists/tiles, a large one for big views
  if (/\/api\/(library\/albums\/[^/]+|share\/[^/]+)\/cover$/.test(src)) src += `?s=${cls === 'big' ? 1000 : 400}`;
  return `${ph}<img class="${cls}" src="${esc(src)}" alt="" loading="lazy" onerror="this.remove()">`;
}

export const STATUS = {
  pending: ['warn', 'Awaiting approval'],
  queued: ['info', 'Queued'],
  searching: ['info', 'Searching'],
  monitoring: ['accent', 'Monitoring'],
  downloading: ['accent', 'Downloading'],
  importing: ['accent', 'Importing'],
  completed: ['ok', 'Available'],
  failed: ['err', 'Failed'],
  denied: ['err', 'Denied'],
  cancelled: ['', 'Cancelled'],
};

export function statusBadge(status) {
  const [cls, label] = STATUS[status] || ['', status];
  const spin = ['searching', 'downloading', 'importing', 'monitoring'].includes(status) ? '<span class="spinner" style="width:10px;height:10px;border-width:2px"></span>' : '';
  return `<span class="badge ${cls}">${spin}${esc(label)}</span>`;
}

export function availability(item) {
  if (item.in_library) return `<span class="badge ok">${icon.check.replace('<svg', '<svg width="12" height="12"')} In library</span>`;
  if (item.request_status) return statusBadge(item.request_status);
  return '';
}

export function debounce(fn, ms = 300) {
  let t;
  return (...a) => { clearTimeout(t); t = setTimeout(() => fn(...a), ms); };
}

export function emptyState(ic, title, text = '') {
  return `<div class="empty">${ic}<h3>${esc(title)}</h3>${text ? `<p class="muted">${text}</p>` : ''}</div>`;
}

export function skeletonGrid(n = 12) {
  return `<div class="grid">${Array.from({ length: n }, () => '<div><div class="skeleton" style="aspect-ratio:1"></div><div class="skeleton" style="height:14px;margin-top:10px;width:70%"></div></div>').join('')}</div>`;
}

/** Share a library song: the system share sheet where there is one, otherwise the link goes to the clipboard. */
export function shareTrack(t) {
  return shareLink(`/library/tracks/${t.id}/share`, undefined, t.title, `${t.title} – ${t.artist}`);
}

/** Share a library album (same as shareTrack). */
export function shareAlbum(id, a) {
  return shareLink(`/library/albums/${id}/share`, undefined, a.album || '', `${a.album || ''} – ${a.albumartist || ''}`);
}

/** Share a library artist by any spelling of its name (same as shareTrack). */
export function shareArtist(name) {
  return shareLink('/library/artists/share', { name }, name, name);
}

async function shareLink(path, body, title, text) {
  try {
    const r = await api.post(path, body);
    const url = new URL(r.path, location.href).href;
    if (navigator.share) { try { await navigator.share({ title, text, url }); return; } catch { /* cancelled → copy */ } }
    await copyText(url);
    toast('Link copied – anyone with it can play this song', 'ok');
  } catch (e) { toast(e.message, 'err'); }
}

export async function copyText(t) {
  try { await navigator.clipboard.writeText(t); toast('Copied to clipboard', 'ok'); }
  catch { toast('Copy failed – select and copy manually', 'err'); }
}

/**
 * Small action menu ("⋯"). items: [{ label, icon?, onClick, danger?, href?, download? } | '-'].
 * Pops up next to the button on desktop, slides up as a sheet on phones.
 */
export function popMenu(anchor, items) {
  document.querySelectorAll('.menu-pop, .menu-bg').forEach(n => n.remove());
  const bg = document.createElement('div');
  bg.className = 'menu-bg';
  const el = document.createElement('div');
  el.className = 'menu-pop';
  el.setAttribute('role', 'menu');
  el.innerHTML = items.filter(Boolean).map((it, i) => it === '-' ? '<hr>'
    : it.href ? `<a role="menuitem" class="${it.danger ? 'danger' : ''}" href="${esc(it.href)}" ${it.download ? 'download' : ''} data-i="${i}">${it.icon || ''}<span>${esc(it.label)}</span></a>`
    : `<button role="menuitem" class="${it.danger ? 'danger' : ''}" data-i="${i}">${it.icon || ''}<span>${esc(it.label)}</span></button>`).join('');
  const close = () => { el.remove(); bg.remove(); document.removeEventListener('keydown', onKey); };
  const onKey = (e) => { if (e.key === 'Escape') close(); };
  bg.onclick = close;
  el.onclick = (e) => {
    const b = e.target.closest('[data-i]');
    if (!b) return;
    const it = items.filter(Boolean)[+b.dataset.i];
    close();
    if (it.onClick) { e.preventDefault(); it.onClick(); }
  };
  document.addEventListener('keydown', onKey);
  document.body.append(bg, el);
  if (!matchMedia('(max-width: 760px)').matches) {
    const r = anchor.getBoundingClientRect();
    const w = el.offsetWidth, h = el.offsetHeight;
    el.style.left = Math.max(8, Math.min(r.right - w, innerWidth - w - 8)) + 'px';
    el.style.top = (r.bottom + h + 8 > innerHeight ? Math.max(8, r.top - h - 6) : r.bottom + 6) + 'px';
  }
  return close;
}

// ---- show / hide for anything sensitive ----------------------------------------------------------
// Every <input type="password"> gets an eye button automatically (also ones added later), and any
// element with data-secret shows dots until its eye is pressed. Inline SVG, no extra requests.
export const EYE = '<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M2 12s3.6-7 10-7 10 7 10 7-3.6 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/></svg>';
export const EYE_OFF = '<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M17.9 17.9A10.4 10.4 0 0 1 12 19c-6.4 0-10-7-10-7a18.5 18.5 0 0 1 5.1-5.9M9.9 5.2A9.6 9.6 0 0 1 12 5c6.4 0 10 7 10 7a18.6 18.6 0 0 1-2.2 3.2M14.1 14.1a3 3 0 1 1-4.2-4.2"/><path d="m2 2 20 20"/></svg>';

function eyeButton(onToggle) {
  const b = document.createElement('button');
  b.type = 'button';
  b.className = 'eye-btn';
  b.setAttribute('aria-label', 'Show');
  b.title = 'Show';
  b.innerHTML = EYE;
  let shown = false;
  b.addEventListener('click', (e) => {
    e.preventDefault();
    e.stopPropagation();
    shown = !shown;
    b.innerHTML = shown ? EYE_OFF : EYE;
    b.title = shown ? 'Hide' : 'Show';
    b.setAttribute('aria-label', b.title);
    onToggle(shown);
  });
  return b;
}

function wrapPassword(input) {
  input.dataset.eye = '1';
  const wrap = document.createElement('span');
  wrap.className = 'pw-wrap' + (input.classList.contains('grow') ? ' grow' : '');
  if (input.style.maxWidth) wrap.style.maxWidth = input.style.maxWidth;
  input.parentNode.insertBefore(wrap, input);
  wrap.appendChild(input);
  wrap.appendChild(eyeButton((shown) => { input.type = shown ? 'text' : 'password'; }));
}

/** Show a secret value as dots with an eye toggle (value kept in el.dataset.value). */
export function setSecretText(el, value) {
  el.dataset.value = value;
  const btn = el.nextElementSibling?.classList.contains('eye-btn') ? el.nextElementSibling : null;
  el.textContent = el.dataset.shown === '1' ? value : '•'.repeat(Math.min(Math.max(value.length, 8), 32));
  if (!btn) {
    el.dataset.eye = '1';
    el.after(eyeButton((shown) => { el.dataset.shown = shown ? '1' : ''; el.textContent = shown ? el.dataset.value : '•'.repeat(Math.min(Math.max(el.dataset.value.length, 8), 32)); }));
  }
}

function enhanceSecrets(root) {
  if (!root.querySelectorAll) return;
  root.querySelectorAll('input[type=password]:not([data-eye])').forEach(wrapPassword);
  root.querySelectorAll('[data-secret]:not([data-eye])').forEach(el => setSecretText(el, el.dataset.value ?? el.textContent));
}

if (typeof document !== 'undefined') {
  const start = () => {
    enhanceSecrets(document.body);
    new MutationObserver((muts) => {
      for (const m of muts) m.addedNodes.forEach(n => { if (n.nodeType === 1) enhanceSecrets(n.matches?.('input[type=password]') ? n.parentNode : n); });
    }).observe(document.body, { childList: true, subtree: true });
  };
  if (document.body) start(); else document.addEventListener('DOMContentLoaded', start);
}
