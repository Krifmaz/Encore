import { api } from '../api.js';
import { art, availability, debounce, emptyState, esc, fmt, icon, skeletonGrid, statusBadge } from '../ui.js';
import { albumTile, bindTiles, submitRequest } from './common.js';

const reqBtn = (attrs, label = 'Request') => `<button class="btn sm primary" ${attrs}>${icon.plus}${label}</button>`;

function trackRow(t) {
  return `<div class="list-item">
    <div class="thumb">${art(t.cover, t.album || t.title)}</div>
    <div class="grow">
      <div class="ellipsis" style="font-weight:600">${esc(t.title)}</div>
      <div class="small muted ellipsis"><a href="#/artist/${esc(t.artist_id)}">${esc(t.artist)}</a>${t.album ? ` · ${t.release_group_id ? `<a href="#/album/${esc(t.release_group_id)}">${esc(t.album)}</a>` : esc(t.album)}` : ''}${t.year ? ' · ' + esc(t.year) : ''}</div>
    </div>
    <span class="small faint hide-sm">${t.length ? fmt.dur(t.length) : ''}</span>
    ${availability(t) || reqBtn(`data-req-track='${esc(JSON.stringify({ mbid: t.id, artist: t.artist, title: t.title, album: t.album, cover_url: t.cover, year: t.year, length: t.length }))}'`)}
  </div>`;
}

function artistTile(a) {
  return `<div class="tile round" data-href="/artist/${esc(a.id)}">
    <div class="art"><div class="ph">${esc((a.name || '?')[0])}</div>
      ${a.in_library ? '<div class="corner" style="top:auto;bottom:8px;left:50%;transform:translateX(-50%)"><span class="badge ok">In library</span></div>' : ''}
    </div>
    <div class="t ellipsis" style="text-align:center">${esc(a.name)}</div>
    <div class="s ellipsis" style="text-align:center">${esc([a.type, a.country, a.disambiguation].filter(Boolean).join(' · ') || ' ')}</div>
  </div>`;
}

function albumActions(a) {
  if (a.in_library || a.request_status) return '';
  return `<button class="fab" title="Request album" data-req-album='${esc(JSON.stringify({ mbid: a.id, artist: a.artist, title: a.title, album: a.title, year: a.year, cover_url: a.cover }))}'>${icon.plus}</button>`;
}

export function bindRequestButtons(root) {
  root.addEventListener('click', async (e) => {
    const b = e.target.closest('[data-req-album],[data-req-track],[data-req-artist]');
    if (!b) return;
    e.stopPropagation();
    if (b.dataset.reqAlbum) await submitRequest({ kind: 'album', ...JSON.parse(b.dataset.reqAlbum) }, b);
    else if (b.dataset.reqTrack) await submitRequest({ kind: 'track', ...JSON.parse(b.dataset.reqTrack) }, b);
    else await submitRequest({ kind: 'artist', ...JSON.parse(b.dataset.reqArtist) }, b);
  });
}

export async function renderSearch(view, ctx) {
  const q = ctx.query.q || '';
  const type = ctx.query.type || 'all';
  view.innerHTML = `
    <div class="row wrap between" style="margin-bottom:18px">
      <h1>Discover</h1>
    </div>
    <form class="search" id="s-form" style="max-width:720px">${icon.search}<input class="input" name="q" value="${esc(q)}" placeholder="Search MusicBrainz for artists, albums, songs…" autofocus autocomplete="off"></form>
    <div class="tabs mt" id="s-tabs">
      ${[['all', 'Top results'], ['artist', 'Artists'], ['album', 'Albums'], ['track', 'Songs']].map(([k, l]) => `<button data-t="${k}" class="${k === type ? 'active' : ''}">${l}</button>`).join('')}
    </div>
    <div id="results" class="mt-lg"></div>`;
  const input = view.querySelector('input[name=q]');
  input.setSelectionRange(q.length, q.length);
  const go = (nq, nt = type) => ctx.navigate(`/search?q=${encodeURIComponent(nq)}&type=${nt}`);
  view.querySelector('#s-form').onsubmit = (e) => { e.preventDefault(); go(input.value.trim()); };
  input.addEventListener('input', debounce(() => { if (input.value.trim().length > 2) go(input.value.trim()); }, 650));
  view.querySelector('#s-tabs').onclick = (e) => { const t = e.target.closest('[data-t]'); if (t) go(input.value.trim(), t.dataset.t); };
  const results = view.querySelector('#results');
  bindTiles(results, ctx.navigate);
  bindRequestButtons(results);
  if (!q) {
    results.innerHTML = emptyState(icon.search, 'Search the entire MusicBrainz catalogue', 'Everything you find can be requested with one click.');
    return;
  }
  results.innerHTML = skeletonGrid(12);
  const data = await api.get(`/discover/search?q=${encodeURIComponent(q)}&type=${type}`);
  let html = '';
  if (data.artists.length) {
    html += `<div class="section-head"><h2>Artists</h2></div><div class="grid">${data.artists.slice(0, type === 'all' ? 6 : 60).map(artistTile).join('')}</div>`;
  }
  if (data.albums.length) {
    html += `<div class="section-head"><h2>Albums</h2></div><div class="grid">${data.albums.map(a => albumTile(a, { href: `/album/${a.id}`, actions: albumActions(a) })).join('')}</div>`;
  }
  if (data.tracks.length) {
    html += `<div class="section-head"><h2>Songs</h2></div><div class="card flat"><div class="list">${data.tracks.map(trackRow).join('')}</div></div>`;
  }
  results.innerHTML = html || emptyState(icon.search, `No results for “${esc(q)}”`);
}

export async function renderArtist(view, ctx) {
  view.innerHTML = '<div class="skeleton" style="height:220px"></div>' + skeletonGrid(8);
  const a = await api.get(`/discover/artist/${ctx.params[0]}`);
  const groups = {};
  for (const rg of a.release_groups) {
    const key = rg.secondary.length ? rg.secondary[0] : (rg.type || 'Other');
    (groups[key] ||= []).push(rg);
  }
  const order = ['Album', 'EP', 'Single', 'Compilation', 'Live', 'Soundtrack', 'Remix', 'DJ-mix', 'Mixtape/Street', 'Demo', 'Other', 'Broadcast', 'Interview', 'Spokenword', 'Audiobook'];
  const keys = Object.keys(groups).sort((x, y) => (order.indexOf(x) + 1 || 99) - (order.indexOf(y) + 1 || 99));
  const owned = a.release_groups.filter(r => r.in_library).length;
  const firstCover = a.release_groups.find(r => r.type === 'Album' && !r.secondary.length)?.cover;
  const links = Object.entries(a.links || {}).filter(([k]) => ['official homepage', 'bandcamp', 'soundcloud', 'youtube', 'streaming', 'free streaming', 'wikidata', 'discogs', 'last.fm'].includes(k)).slice(0, 6);
  view.innerHTML = `
    <div class="detail-head artist">
      <div class="cover">${art(firstCover, a.name)}</div>
      <div class="grow">
        <div class="kicker">${esc(a.type || 'Artist')}${a.country ? ' · ' + esc(a.country) : ''}${a.begin ? ' · since ' + esc(a.begin.slice(0, 4)) : ''}</div>
        <h1>${esc(a.name)}</h1>
        ${a.disambiguation ? `<p class="muted" style="margin:0 0 10px">${esc(a.disambiguation)}</p>` : ''}
        <div class="chips">${a.tags.map(t => `<span class="badge">${esc(t)}</span>`).join('')}</div>
        <div class="row wrap mt">
          ${a.request_status ? statusBadge(a.request_status) : `<button class="btn primary" data-req-artist='${esc(JSON.stringify({ mbid: a.id, artist: a.name, cover_url: firstCover }))}'>${icon.plus}Request discography</button>`}
          <span class="badge ${owned ? 'ok' : ''}">${owned} / ${a.release_groups.length} releases in library</span>
          ${a.library_albums.length ? `<a class="btn sm" href="#/library?artist=${encodeURIComponent(a.name)}">${icon.library}Open in library</a>` : ''}
          <a class="btn sm ghost" target="_blank" rel="noopener" href="https://musicbrainz.org/artist/${esc(a.id)}">${icon.external}MusicBrainz</a>
          ${links.map(([k, u]) => `<a class="btn sm ghost" target="_blank" rel="noopener noreferrer" href="${esc(u)}">${esc(k)}</a>`).join('')}
        </div>
      </div>
    </div>
    ${keys.map(k => `<div class="section-head"><h2>${esc(k)}s <span class="faint small">${groups[k].length}</span></h2></div>
      <div class="grid">${groups[k].map(rg => albumTile(rg, { href: `/album/${rg.id}`, actions: albumActions(rg), sub: esc([rg.year, rg.type].filter(Boolean).join(' · ')) })).join('')}</div>`).join('')}
    ${!keys.length ? emptyState(icon.disc, 'No releases on MusicBrainz') : ''}`;
  bindTiles(view, ctx.navigate);
  bindRequestButtons(view);
}

export async function renderAlbum(view, ctx) {
  view.innerHTML = '<div class="skeleton" style="height:220px"></div>';
  const a = await api.get(`/discover/album/${ctx.params[0]}`);
  const rel = a.release;
  const total = rel ? rel.tracks.reduce((s, t) => s + (t.length || 0), 0) : 0;
  const multi = rel && rel.disc_count > 1;
  view.innerHTML = `
    <div class="detail-head">
      <div class="cover">${art(a.cover.replace('front-250', 'front-500'), a.title)}</div>
      <div class="grow">
        <div class="kicker">${esc(a.type || 'Release')}${a.secondary.length ? ' · ' + esc(a.secondary.join(', ')) : ''}</div>
        <h1>${esc(a.title)}</h1>
        <div class="muted"><a href="#/artist/${esc(a.artist_id)}" style="color:var(--text);font-weight:650">${esc(a.artist)}</a>
          ${a.year ? ' · ' + esc(a.year) : ''}${rel ? ` · ${rel.tracks.length} tracks · ${fmt.longDur(total)}` : ''}${rel?.label ? ' · ' + esc(rel.label) : ''}</div>
        <div class="chips mt">${(a.tags || []).map(t => `<span class="badge">${esc(t)}</span>`).join('')}</div>
        <div class="row wrap mt">
          ${availability(a) || `<button class="btn primary" data-req-album='${esc(JSON.stringify({ mbid: a.id, artist: a.artist, title: a.title, album: a.title, year: a.year, cover_url: a.cover }))}'>${icon.plus}Request album</button>`}
          <a class="btn sm ghost" target="_blank" rel="noopener" href="https://musicbrainz.org/release-group/${esc(a.id)}">${icon.external}MusicBrainz</a>
        </div>
      </div>
    </div>
    ${rel ? `<div class="card flat"><div class="table-wrap"><table class="table">
      <thead><tr><th class="num">#</th><th>Title</th><th class="hide-sm">Artist</th><th>Length</th><th></th></tr></thead>
      <tbody>${rel.tracks.map(t => `<tr>
        <td class="num">${multi ? t.disc + '-' : ''}${t.track}</td>
        <td>${esc(t.title)}</td>
        <td class="hide-sm muted">${esc(t.artist)}</td>
        <td class="muted">${fmt.dur(t.length)}</td>
        <td style="text-align:right">${t.in_library ? '<span class="badge ok">In library</span>'
          : a.in_library || a.request_status ? '' : `<button class="btn sm" data-req-track='${esc(JSON.stringify({ mbid: t.recording_id, artist: t.artist, title: t.title, album: a.title, cover_url: a.cover, year: a.year, length: t.length }))}'>${icon.plus}Track</button>`}</td>
      </tr>`).join('')}</tbody></table></div></div>` : emptyState(icon.disc, 'No track listing available')}`;
  bindRequestButtons(view);
}
