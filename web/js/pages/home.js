import { api } from '../api.js';
import { art, emptyState, esc, fmt, icon, modal, toast } from '../ui.js';
import { bindRequestActions, bindTiles, libraryTile, requestRow } from './common.js';

export async function render(view, ctx) {
  const hour = new Date().getHours();
  const greet = hour < 12 ? 'Good morning' : hour < 18 ? 'Good afternoon' : 'Good evening';
  view.innerHTML = `
    <section class="hero">
      <h1>${greet}, ${esc(ctx.user.username)}</h1>
      <p class="muted" style="margin:6px 0 0">Find any artist, album or song – or paste a link from anywhere.</p>
      <form class="search" id="home-search">${icon.search}<input class="input" name="q" type="search" enterkeyhint="search" placeholder="Search artists, albums, songs…" autocomplete="off"></form>
      <form class="search" id="home-link" style="margin-top:10px">${icon.link}<input class="input" name="url" type="url" enterkeyhint="go" placeholder="…or paste a song, album or playlist link to download" autocomplete="off"></form>
    </section>
    <div class="stats mt" id="stats"></div>
    <div id="made-for-you"></div>
    <div id="new-releases"></div>
    <div id="fy-preview"></div>
    <div id="reqs-wrap" class="hidden"><div class="section-head"><h2>Your downloads in progress</h2><a class="btn sm" href="#/requests">All requests</a></div>
      <div class="card flat" id="reqs"></div></div>
    <div class="section-head"><h2>Recently added</h2><a class="btn sm" href="#/library?tab=albums&sort=added&dir=desc">Library</a></div>
    <div id="recent"></div>`;
  view.querySelector('#home-search').onsubmit = (e) => {
    e.preventDefault();
    const q = new FormData(e.target).get('q').trim();
    if (q) ctx.navigate(`/search?q=${encodeURIComponent(q)}`);
  };
  view.querySelector('#home-link').onsubmit = async (e) => {
    e.preventDefault();
    const input = e.target.querySelector('input');
    const url = input.value.trim();
    if (!url) return;
    input.disabled = true;
    try {
      const r = await api.post('/requests/link', { url });
      toast(r.mode === 'matched' ? `${r.requested} song(s) requested${r.name ? ' from ' + r.name : ''}${r.already_requested ? `, ${r.already_requested} already requested` : ''}` : 'Download started', 'ok');
      input.value = '';
      window.dispatchEvent(new CustomEvent('encore:requests-changed'));
      loadReqs();
    } catch (err) { toast(err.message, 'err'); }
    input.disabled = false;
  };
  bindTiles(view, ctx.navigate);

  const loadReqs = async () => {
    const rows = await api.get('/requests?status=active&mine=true&limit=8');
    view.querySelector('#reqs-wrap').classList.toggle('hidden', !rows.length);
    view.querySelector('#reqs').innerHTML = rows.length ? `<div class="list">${rows.map(r => requestRow(r)).join('')}</div>` : '';
  };
  bindRequestActions(view.querySelector('#reqs'), loadReqs);

  // fun stats
  api.get('/stats/me').then(s => {
    view.querySelector('#stats').innerHTML = `
      <div class="stat"><b>${fmt.num(s.library.tracks)}</b><span>songs in the library</span></div>
      <div class="stat"><b>${fmt.longDur(s.library.duration)}</b><span>to listen to${s.library.duration > 86400 ? ` · ${(s.library.duration / 86400).toFixed(1)} days non-stop` : ''}</span></div>
      <div class="stat"><b>${fmt.num(s.me.plays)}</b><span>songs you played${s.me.listened ? ` · ${fmt.longDur(s.me.listened)}` : ''}</span></div>
      <div class="stat"><b>${fmt.num(s.me.requests)}</b><span>things you requested${s.me.rank ? ` · #${s.me.rank} on the <a href="#/leaderboard">leaderboard</a>` : ''}</span></div>
      ${s.me.top_artist ? `<div class="stat"><b class="ellipsis" style="display:block">${esc(s.me.top_artist)}</b><span>your most played artist</span></div>` : ''}
      ${s.fun ? `<div class="stat"><b>${esc(s.fun.value)}</b><span>${esc(s.fun.label)}</span></div>` : ''}`;
  }).catch(() => {});

  // ListenBrainz's own playlists for you – only once ListenBrainz is connected
  api.get('/discover/made-for-you').then(async d => {
    if (!d.connected || !d.playlists?.length || !view.isConnected) return;
    const fy = await import('./foryou.js');
    const playOwned = (p) => import('../player.js').then(pl => pl.playQueue(p.tracks.filter(t => t.library).map(t => t.library), 0, { context: 'list' }));
    const tile = (p, i) => `<div class="tile" data-mfy="${i}">
      <div class="art">${p.covers.length >= 4 ? `<div class="collage">${p.covers.slice(0, 4).map(c => `<div class="cell">${art(c, p.title)}</div>`).join('')}</div>` : p.covers.length ? art(p.covers[0], p.title) : `<div class="ph">${icon.spark}</div>`}
        <div class="corner"><span class="badge">${p.owned}/${p.total} in library</span></div>
        <div class="overlay">${p.owned ? `<button class="fab" data-mfy-play="${i}" aria-label="Play">${icon.play}</button>` : ''}</div></div>
      <div class="t ellipsis" title="${esc(p.title)}">${esc(p.title)}</div>
      <div class="s ellipsis">${esc(fy.KIND[p.kind] || 'From ListenBrainz')}</div></div>`;
    const el = view.querySelector('#made-for-you');
    el.innerHTML = `<div class="section-head"><div><h2>Made for you</h2><div class="small muted">ListenBrainz builds these from what ${esc(d.user)} listens to</div></div>
        <a class="btn sm ghost" target="_blank" rel="noopener" href="https://listenbrainz.org/user/${encodeURIComponent(d.user)}/playlists/" title="On ListenBrainz">${icon.external}</a></div>
      <div class="grid">${d.playlists.map(tile).join('')}</div>`;
    el.addEventListener('click', (e) => {
      const b = e.target.closest('[data-mfy-play]');
      if (b) { e.stopPropagation(); return playOwned(d.playlists[+b.dataset.mfyPlay]); }
      const t = e.target.closest('[data-mfy]');
      if (!t) return;
      const p = d.playlists[+t.dataset.mfy];
      const missing = p.tracks.filter(x => !x.library && !x.request_status);
      const m = modal({ title: p.title, wide: true, body: `<div class="row wrap" style="justify-content:space-between;margin-bottom:10px"><span class="small muted">${esc(fy.KIND[p.kind] || '')} · ${p.owned}/${p.total} in library</span>
          <span class="row">${p.owned ? `<button class="btn sm" id="mfy-play">${icon.play}Play owned</button>` : ''}${missing.length ? `<button class="btn sm primary" id="mfy-req">${icon.download}Request ${missing.length} missing</button>` : ''}</span></div>
        <div class="table-wrap"><table class="table"><tbody>${p.tracks.map(fy.trackRow).join('')}</tbody></table></div>` });
      const body = m.el.querySelector('.modal-body');
      fy.bindTracks(body, p.tracks);
      body.querySelector('#mfy-play')?.addEventListener('click', () => playOwned(p));
      body.querySelector('#mfy-req')?.addEventListener('click', (e) => fy.requestAll(missing, e.currentTarget));
    });
  }).catch(() => {});

  // new releases from library artists
  api.get('/discover/new-releases?limit=12').then(d => {
    if (!d.releases.length || !view.isConnected) return;
    view.querySelector('#new-releases').innerHTML = `<div class="section-head"><h2>New releases</h2><span class="small muted">from artists in your library</span></div>
      <div class="grid">${d.releases.map(r => `<div class="tile" data-href="${r.mbid ? `/album/${esc(r.mbid)}` : `/artist-name/${encodeURIComponent(r.artist)}`}">
        <div class="art">${art(r.cover, r.title)}
          <div class="corner">${r.in_library ? '<span class="badge ok">In library</span>': ''}</div>
</div>
        <div class="t ellipsis" title="${esc(r.title)}">${esc(r.title)}</div>
        <div class="s ellipsis">${esc(r.artist)} · ${esc(r.date)}${r.type && r.type !== 'Album' ? ' · ' + esc(r.type) : ''}</div></div>`).join('')}</div>`;
  }).catch(() => {});

  // a taste of For you
  api.get('/discover/for-you').then(d => {
    const pl = d.playlists?.[0];
    if (!pl || !view.isConnected) return;
    const tr = pl.tracks.slice(0, 6);
    view.querySelector('#fy-preview').innerHTML = `<div class="section-head"><h2>For you</h2><a class="btn sm" href="#/for-you">See all</a></div>
      <div class="card flat"><div class="list">${tr.map((t, i) => `<div class="list-item">
        ${t.library ? `<button class="thumb play-thumb" data-fy="${i}" aria-label="Play">${art(t.cover, t.title)}<i>${icon.play}</i></button>` : `<div class="thumb">${art(t.cover, t.title)}</div>`}
        <div class="grow" style="min-width:0"><div class="ellipsis" style="font-weight:600">${esc(t.title)}</div><div class="small muted ellipsis">${esc(t.artist)}</div></div>
        ${t.library ? '<span class="badge ok">In library</span>' : ''}</div>`).join('')}</div></div>`;
    view.querySelector('#fy-preview').addEventListener('click', async (e) => {
      const b = e.target.closest('[data-fy]');
      if (!b) return;
      const t = tr[+b.dataset.fy];
      const player = await import('../player.js');
      if (t.library) player.playQueue([t.library], 0, { context: 'single' });
    });
  }).catch(() => {});

  const [recent] = await Promise.all([api.get('/library/recent?limit=12'), loadReqs()]);
  view.querySelector('#recent').innerHTML = recent.length
    ? `<div class="grid">${recent.map(libraryTile).join('')}</div>`
    : emptyState(icon.library, 'Your library is empty', 'Search above or paste a link to get started.');
  const timer = setInterval(loadReqs, 5000);
  ctx.onLeave(() => clearInterval(timer));
}
