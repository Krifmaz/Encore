import { api } from './api.js';
import * as player from './player.js';
import { esc, icon, qs, toast } from './ui.js';
import * as authPages from './pages/auth.js';
import * as home from './pages/home.js';
import * as discover from './pages/discover.js';
import * as libraryPage from './pages/library.js';
import * as requestsPage from './pages/requests.js';
import * as downloadPage from './pages/download.js';
import * as activityPage from './pages/activity.js';
import * as adminPage from './pages/admin.js';
import * as settingsPage from './pages/settings.js';
import * as accountPage from './pages/account.js';
import * as playlistsPage from './pages/playlists.js';
import * as connectionsPage from './pages/connections.js';
import * as autoPage from './pages/autodownload.js';
import * as diagPage from './pages/diagnostics.js';
import * as followingPage from './pages/following.js';

export const session = { user: null, info: null };

const routes = [
  [/^\/$/, home.render],
  [/^\/search$/, discover.renderSearch],
  [/^\/artist\/([\w-]+)$/, discover.renderArtist],
  [/^\/artist-name\/(.+)$/, discover.renderArtistByName],
  [/^\/album\/([\w-]+)$/, discover.renderAlbum],
  [/^\/library$/, libraryPage.render],
  [/^\/library\/album\/([\w-]+)$/, libraryPage.renderAlbum],
  [/^\/requests$/, requestsPage.render],
  [/^\/playlists$/, playlistsPage.render],
  [/^\/playlists\/(\d+)$/, playlistsPage.renderPlaylist],
  [/^\/connections$/, connectionsPage.render],
  [/^\/following$/, followingPage.render],
  [/^\/download$/, downloadPage.render],
  [/^\/activity$/, activityPage.render, true],
  [/^\/admin\/users$/, adminPage.renderUsers, true],
  [/^\/admin\/invites$/, adminPage.renderInvites, true],
  [/^\/settings$/, settingsPage.render, true],
  [/^\/admin\/autodownload$/, autoPage.render, true],
  [/^\/admin\/diagnostics$/, diagPage.render, true],
  [/^\/account$/, accountPage.render],
];

let cleanup = [];

export function navigate(path) { location.hash = '#' + path; }

function parseHash() {
  const raw = location.hash.slice(1) || '/';
  const [path, q = ''] = raw.split('?');
  return { path, query: Object.fromEntries(new URLSearchParams(q)) };
}

function themeToggle() {
  const cur = document.documentElement.dataset.theme === 'light' ? 'dark' : 'light';
  document.documentElement.dataset.theme = cur;
  try { localStorage.setItem('encore.theme', cur); } catch { /* ignore */ }
  renderShell();
  route();
}

function navLink(href, ic, label, extra = '') {
  return `<a href="#${href}" data-nav="${href}">${ic}<span>${label}</span>${extra}</a>`;
}

function renderShell() {
  const u = session.user;
  const admin = u.role === 'admin';
  const appName = esc(session.info?.app_name || 'Encore');
  document.title = session.info?.app_name || 'Encore';
  qs('#app').innerHTML = `
    <div class="shell">
      <aside class="sidebar" id="sidebar">
        <a class="brand" href="#/"><img src="/favicon.svg" alt="">${appName}</a>
        <nav class="nav">
          ${navLink('/', icon.home, 'Home')}
          ${navLink('/search', icon.search, 'Discover')}
          ${navLink('/library', icon.library, 'Library')}
          ${navLink('/requests', icon.inbox, 'Requests', '<span class="badge warn hidden" id="pending-badge"></span>')}
          ${navLink('/playlists', icon.list, 'Playlists')}
          ${navLink('/following', icon.users, 'Following')}
          ${navLink('/download', icon.download, 'Downloader')}
          ${navLink('/connections', icon.link, 'Connections')}
          ${admin ? `<div class="nav-label">Admin</div>
            ${navLink('/activity', icon.activity, 'Activity')}
            ${navLink('/admin/users', icon.users, 'Users')}
            ${navLink('/admin/invites', icon.ticket, 'Invites')}
            ${navLink('/admin/autodownload', icon.download, 'Auto-download')}
            ${navLink('/settings', icon.settings, 'Settings')}
            ${navLink('/admin/diagnostics', icon.shield, 'Diagnostics')}` : ''}
        </nav>
        <div class="sidebar-foot">
          <a href="#/account" class="row grow" style="min-width:0">
            <div class="avatar">${esc(u.username[0])}</div>
            <div class="grow"><div class="ellipsis" style="font-weight:650">${esc(u.username)}</div><div class="small faint" title="Encore version / build">${admin ? 'Admin' : 'Member'} · v${esc(session.info?.version || '')}${session.info?.build && session.info.build !== 'dev' ? ' · ' + esc(session.info.build) : ''}</div></div>
          </a>
          <button class="btn ghost icon" id="theme-btn" title="Toggle theme">${document.documentElement.dataset.theme === 'light' ? icon.moon : icon.sun}</button>
          <button class="btn ghost icon" id="logout-btn" title="Sign out">${icon.logout}</button>
        </div>
      </aside>
      <main class="main">
        <button class="btn ghost icon mobile-only" id="menu-btn" style="margin:-6px 0 10px -6px">${icon.menu}</button>
        <div id="view"></div>
      </main>
    </div>`;
  qs('#logout-btn').onclick = async () => { await api.post('/auth/logout'); location.reload(); };
  qs('#theme-btn').onclick = themeToggle;
  qs('#menu-btn').onclick = (e) => { e.stopPropagation(); qs('#sidebar').classList.toggle('open'); };
  document.addEventListener('click', (e) => {
    const sb = qs('#sidebar');
    if (sb && sb.classList.contains('open') && !sb.contains(e.target)) sb.classList.remove('open');
  });
}

async function refreshPendingBadge() {
  if (session.user?.role !== 'admin') return;
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
    if (adminOnly && session.user.role !== 'admin') break;
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
  try {
    const t = localStorage.getItem('encore.theme');
    if (t) document.documentElement.dataset.theme = t;
  } catch { /* ignore */ }
  try { session.info = await api.get('/auth/info'); } catch { session.info = {}; }
  try { session.user = await api.get('/auth/me'); } catch { session.user = null; }
  if (session.user) {
    renderShell();
    player.mount();
    refreshPendingBadge();
    setInterval(refreshPendingBadge, 30000);
  }
  route();
}

export async function refreshUser() {
  session.user = await api.get('/auth/me');
}

window.addEventListener('hashchange', route);
window.addEventListener('encore:unauthorized', () => {
  if (session.user) { session.user = null; toast('Your session expired – please sign in again', 'err'); location.hash = '#/login'; location.reload(); }
});
window.addEventListener('encore:requests-changed', refreshPendingBadge);

boot();
