import * as appearance from './appearance.js';
import * as remote from './remote.js';
import * as playback from './playback.js';
import { api } from './api.js';
import * as player from './player.js';
import { esc, icon, qs, setTranslateMode, toast } from './ui.js';
import * as authPages from './pages/auth.js';
import * as home from './pages/home.js';
import * as discover from './pages/discover.js';
import * as libraryPage from './pages/library.js';
import * as requestsPage from './pages/requests.js';
import * as downloadPage from './pages/download.js';
import * as settingsPage from './pages/settings.js';
import * as playlistsPage from './pages/playlists.js';
import * as forYouPage from './pages/foryou.js';
import * as storagePage from './pages/storage.js';
import * as leaderboardPage from './pages/leaderboard.js';

export const session = { user: null, info: null };
window.encoreSession = session;

const routes = [
  [/^\/$/, home.render],
  [/^\/search$/, discover.renderSearch],
  [/^\/for-you$/, forYouPage.render],
  [/^\/tag\/(.+)$/, forYouPage.renderTag],
  [/^\/artist\/([\w-]+)$/, discover.renderArtist],
  [/^\/artist-name\/(.+)$/, discover.renderArtistByName],
  [/^\/album\/([\w-]+)$/, discover.renderAlbum],
  [/^\/library$/, libraryPage.render],
  [/^\/library\/album\/([\w-]+)$/, libraryPage.renderAlbum],
  [/^\/requests$/, requestsPage.render],
  [/^\/playlists$/, playlistsPage.render],
  [/^\/playlists\/(\d+)$/, playlistsPage.renderPlaylist],
  [/^\/connections$/, (v, c) => redirect('connections')],
  [/^\/following$/, () => navigate('/for-you')],
  [/^\/download$/, downloadPage.render],
  [/^\/soulseek$/, async (v, c) => (await import('./pages/soulseek.js')).render(v, c)],
  [/^\/activity$/, (v, c) => navigate('/download')],
  [/^\/admin\/users$/, () => redirect('users')],
  [/^\/admin\/invites$/, () => redirect('invites')],
  [/^\/settings$/, settingsPage.render],
  [/^\/admin\/autodownload$/, () => redirect('autodownload')],
  [/^\/admin\/diagnostics$/, () => redirect('diagnostics')],
  [/^\/account$/, () => redirect('account')],
  [/^\/storage$/, storagePage.render],
  [/^\/leaderboard$/, leaderboardPage.render],
];

let cleanup = [];

export function navigate(path) { location.hash = '#' + path; }
const redirect = (page) => { history.replaceState(null, '', `#/settings?s=${page}`); window.dispatchEvent(new HashChangeEvent('hashchange')); };

function parseHash() {
  const raw = location.hash.slice(1) || '/';
  const [path, q = ''] = raw.split('?');
  return { path, query: Object.fromEntries(new URLSearchParams(q)) };
}


/** Profile picture or the first letter of the name. */
export function avatarHtml(u, size = 32) {
  const style = size !== 32 ? ` style="width:${size}px;height:${size}px;font-size:${Math.round(size * 0.42)}px"` : '';
  return u.avatar_at
    ? `<img class="avatar" src="/api/users/${u.id}/avatar?v=${Math.round(u.avatar_at)}" alt=""${style}>`
    : `<div class="avatar"${style}>${esc((u.username || '?')[0])}</div>`;
}

function navLink(href, ic, label, extra = '') {
  return `<a href="#${href}" data-nav="${href}">${ic}<span>${label}</span>${extra}</a>`;
}

function renderShell() {
  const u = session.user;
  const admin = ['admin', 'owner'].includes(u.role);
  const appName = esc(session.info?.app_name || 'Encore');
  document.title = session.info?.app_name || 'Encore';
  qs('#app').innerHTML = `
    <div class="shell">
      <aside class="sidebar" id="sidebar">
        <a class="brand" href="#/">${appearance.logo()}${appName}</a>
        <nav class="nav">
          ${navLink('/', icon.home, 'Home')}
          ${navLink('/search', icon.search, 'Discover')}
          ${navLink('/for-you', icon.spark, 'For you')}
          ${navLink('/library', icon.library, 'Library')}
          ${navLink('/requests', icon.inbox, 'Requests', '<span class="badge warn hidden" id="pending-badge"></span>')}
          ${navLink('/playlists', icon.list, 'Playlists')}
          ${navLink('/download', icon.download, 'Downloader')}
          ${(session.info?.features || []).includes('soulseek') ? navLink('/soulseek', icon.users, 'Soulseek') : ''}
          ${navLink('/leaderboard', icon.activity, 'Leaderboard')}
        </nav>
        <div id="storage-mini" style="flex:none"></div>
        <div class="sidebar-foot" style="margin-top:0">
          <a href="#/settings?s=account" class="row grow" style="min-width:0">
            ${avatarHtml(u)}
            <div class="grow"><div class="ellipsis" style="font-weight:650">${esc(u.username)}</div><div class="small faint">${u.role === 'owner' ? 'Owner' : admin ? 'Admin' : 'Member'}</div></div>
          </a>
          <a class="btn ghost icon" href="#/settings" title="Settings" aria-label="Settings">${icon.settings}</a>
          <button class="btn ghost icon" id="logout-btn" title="Sign out">${icon.logout}</button>
        </div>
      </aside>
      <main class="main">
        <button class="btn ghost icon mobile-only" id="menu-btn" style="margin:-6px 0 10px -6px">${icon.menu}</button>
        <div id="view"></div>
      </main>
    </div>`;
  qs('#logout-btn').onclick = async () => {
    await api.post('/auth/logout');
    // the desktop app's Cadenza signed in with its own token: signing out here signs it out too
    try { window.EncoreNative?.engineLogout?.(); } catch { /* ignore */ }
    location.reload();
  };
  const meter = () => storagePage.sidebarMeter(qs('#storage-mini'));
  meter();
  setInterval(meter, 5 * 60 * 1000);
  qs('#menu-btn').onclick = (e) => { e.stopPropagation(); qs('#sidebar').classList.toggle('open'); };
  document.addEventListener('click', (e) => {
    const sb = qs('#sidebar');
    if (sb && sb.classList.contains('open') && !sb.contains(e.target)) sb.classList.remove('open');
  });
}

async function refreshPendingBadge() {
  if (!['admin', 'owner'].includes(session.user?.role)) return;
  try {
    const rows = await api.get('/requests?status=pending&limit=500');
    const b = qs('#pending-badge');
    if (b) { b.textContent = rows.length; b.classList.toggle('hidden', !rows.length); }
  } catch { /* ignore */ }
}

async function route() {
  cleanup.forEach(fn => { try { fn(); } catch { /* ignore */ } });
  cleanup = [];
  document.getElementById('modal-root').innerHTML = '';
  const { path, query } = parseHash();
  if (!session.user) {
    authPages.render(qs('#app'), { path, query, session });
    return;
  }
  if (!qs('#view')) renderShell();
  qs('#sidebar')?.classList.remove('open');
  document.querySelectorAll('[data-nav]').forEach(a => {
    const href = a.dataset.nav;
    a.classList.toggle('active', href === '/' ? path === '/' : path.startsWith(href));
  });
  // every navigation renders into a fresh container, so a slow page that finishes after the user
  // has moved on writes into a detached element instead of clobbering the current page
  const view = document.createElement('div');
  qs('#view').replaceChildren(view);
  for (const [re, fn, adminOnly] of routes) {
    const m = path.match(re);
    if (!m) continue;
    if (adminOnly && !['admin', 'owner'].includes(session.user.role)) break;
    window.scrollTo(0, 0);
    try {
      await fn(view, { params: m.slice(1), query, user: session.user, onLeave: (f) => (view.isConnected ? cleanup.push(f) : f()), navigate });
    } catch (e) {
      if (!view.isConnected) return;
      console.error(e);
      view.innerHTML = `<div class="error-box">${esc(e.message || e)}</div>`;
    }
    return;
  }
  view.innerHTML = `<div class="empty"><h2>Page not found</h2><p class="muted"><a href="#/">Go home</a></p></div>`;
}

export async function boot() {
  appearance.apply();
  try { session.info = await api.get('/auth/info'); } catch { session.info = {}; }
  try { session.user = await api.get('/auth/me'); } catch { session.user = null; }
  if (session.user) { appearance.adoptAccount(session.user); playback.adoptAccount(session.user); setTranslateMode(session.user.prefs?.translate_mode); }
  else appearance.apply(appearance.BRAND);
  if (session.user) {
    renderShell();
    remote.start(session);
    player.mount();
    remote.resume(session);
    refreshPendingBadge();
    setInterval(refreshPendingBadge, 30000);
  }
  route();
}

export async function refreshUser() {
  session.user = await api.get('/auth/me');
  setTranslateMode(session.user?.prefs?.translate_mode);
  appearance.apply();
  appearance.adoptAccount(session.user);
  renderShell();
  route();
}

window.addEventListener('hashchange', route);
if ('serviceWorker' in navigator && (location.protocol === 'https:' || location.hostname === 'localhost')) {
  navigator.serviceWorker.register('/sw.js').catch(() => {});
}
window.addEventListener('encore:unauthorized', () => {
  if (session.user) { session.user = null; toast('Your session expired – please sign in again', 'err'); location.hash = '#/login'; location.reload(); }
});
window.addEventListener('encore:requests-changed', refreshPendingBadge);
// connection banner: shown while the device is offline or the server stops answering
function connBanner(on, text = 'Not connected – check your internet or server') {
  let b = document.getElementById('conn-banner');
  if (!on) { b?.remove(); return; }
  if (!b) { b = document.createElement('div'); b.id = 'conn-banner'; document.body.appendChild(b); }
  b.innerHTML = `${icon.x}<span>${esc(text)}</span>${window.EncoreNative?.openOffline ? '<button class="btn sm" id="go-offline">Offline music</button>' : ''}`;
  b.querySelector('#go-offline')?.addEventListener('click', () => window.EncoreNative.openOffline());
}
window.addEventListener('offline', () => connBanner(true));
// the desktop / Android app reports things it did on our behalf (e.g. "Saved for offline: …")
window.addEventListener('encore-native', (e) => { if (e.detail?.text) toast(e.detail.text, e.detail.kind || 'ok'); });
window.addEventListener('online', () => connBanner(false));
window.addEventListener('encore:server-down', () => connBanner(true, 'The Encore server isn\'t answering'));
window.addEventListener('encore:server-up', () => connBanner(false));

boot();
