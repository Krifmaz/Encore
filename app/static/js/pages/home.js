import { api } from '../api.js';
import { esc, fmt, icon, emptyState } from '../ui.js';
import { bindRequestActions, bindTiles, libraryTile, requestRow } from './common.js';

export async function render(view, ctx) {
  const admin = ctx.user.role === 'admin';
  const hour = new Date().getHours();
  const greet = hour < 12 ? 'Good morning' : hour < 18 ? 'Good afternoon' : 'Good evening';
  view.innerHTML = `
    <section class="hero">
      <h1>${greet}, ${esc(ctx.user.username)}</h1>
      <p class="muted" style="margin:6px 0 0">Find any artist, album or song and request it – or browse what's already here.</p>
      <form class="search" id="home-search">${icon.search}<input class="input" name="q" placeholder="Search artists, albums, songs…" autocomplete="off"></form>
    </section>
    <div class="stats mt" id="stats"></div>
    ${admin ? '<div class="section-head"><h2>Services</h2><a class="btn sm" href="#/settings">Configure</a></div><div class="row wrap" id="services"><span class="spinner"></span></div>' : ''}
    <div class="section-head"><h2>Your active requests</h2><a class="btn sm" href="#/requests">All requests</a></div>
    <div class="card flat" id="reqs"><div class="empty"><span class="spinner"></span></div></div>
    <div class="section-head"><h2>Recently added</h2><a class="btn sm" href="#/library?sort=added">Library</a></div>
    <div id="recent"></div>`;
  view.querySelector('#home-search').onsubmit = (e) => {
    e.preventDefault();
    const q = new FormData(e.target).get('q').trim();
    if (q) ctx.navigate(`/search?q=${encodeURIComponent(q)}`);
  };
  bindTiles(view.querySelector('#recent'), ctx.navigate);

  const loadReqs = async () => {
    const rows = (await api.get('/requests?status=active&mine=true&limit=8'));
    view.querySelector('#reqs').innerHTML = rows.length ? `<div class="list">${rows.map(r => requestRow(r)).join('')}</div>`
      : emptyState(icon.inbox, 'No active requests', 'Search for something you want to hear.');
  };
  bindRequestActions(view.querySelector('#reqs'), loadReqs);

  const [stats, recent] = await Promise.all([api.get('/library/stats'), api.get('/library/recent?limit=12'), loadReqs()]);
  view.querySelector('#stats').innerHTML = `
    <div class="stat"><b>${fmt.num(stats.artists)}</b><span>Artists</span></div>
    <div class="stat"><b>${fmt.num(stats.albums)}</b><span>Albums</span></div>
    <div class="stat"><b>${fmt.num(stats.tracks)}</b><span>Tracks</span></div>
    <div class="stat"><b>${fmt.size(stats.size)}</b><span>${fmt.longDur(stats.duration)} of music</span></div>`;
  view.querySelector('#recent').innerHTML = recent.length
    ? `<div class="grid">${recent.map(libraryTile).join('')}</div>`
    : emptyState(icon.library, 'Your library is empty', admin ? 'Point the library path at your music folder in Settings, or make some requests.' : 'Request some music to get started.');

  const timer = setInterval(loadReqs, 5000);
  ctx.onLeave(() => clearInterval(timer));

  if (admin) {
    try {
      const st = await api.get('/system/status');
      const label = { musicbrainz: 'MusicBrainz', lidarr: 'Lidarr', slskd: 'slskd', ytdlp: 'yt-dlp', spotify: 'Spotify', ffmpeg: 'ffmpeg' };
      view.querySelector('#services').innerHTML = Object.entries(label).map(([k, name]) => {
        const s = st.services[k] || {};
        const off = s.enabled === false;
        return `<div class="card row" style="padding:10px 14px" title="${esc(s.error || '')}">
          <span class="dot ${off ? '' : s.ok ? 'ok' : 'err'}"></span><b>${name}</b>
          <span class="small faint">${off ? 'not configured' : s.ok ? esc(s.version || 'connected') : 'unreachable'}</span></div>`;
      }).join('') + (st.disk ? `<div class="card row" style="padding:10px 14px"><b>Disk</b><span class="small faint">${fmt.size(st.disk.free)} free of ${fmt.size(st.disk.total)}</span></div>` : '');
    } catch (e) {
      view.querySelector('#services').innerHTML = `<span class="muted">${esc(e.message)}</span>`;
    }
  }
}
