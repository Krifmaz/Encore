import { api } from '../api.js';
import * as player from '../player.js';
import { art, availability, confirmDialog, debounce, emptyState, esc, fmt, icon, modal, skeletonGrid, statusBadge, toast } from '../ui.js';
import { albumTile, bindTiles, libraryTile, submitRequest } from './common.js';
import { followModal } from './following.js';
import { artistCard, moreLikeThis } from './foryou.js';
import { pickerModal } from './picker.js';

const reqBtn = (attrs, label = 'Request') => `<button class="btn sm primary" ${attrs}>${icon.plus}${label}</button>`;

function trackRow(t) {
  return `<div class="list-item">
    ${t.in_library ? `<div class="thumb">${art(t.cover, t.album || t.title)}</div>` : `<div class="thumb">${art(t.cover, t.album || t.title)}</div>`}
    <div class="grow">
      <div class="ellipsis" style="font-weight:600">${esc(t.title)}</div>
      <div class="small muted ellipsis"><a href="#/artist/${esc(t.artist_id)}">${esc(t.artist)}</a>${t.album ? ` · ${t.release_group_id ? `<a href="#/album/${esc(t.release_group_id)}">${esc(t.album)}</a>` : esc(t.album)}` : ''}${t.year ? ' · ' + esc(t.year) : ''}</div>
    </div>
    <span class="small faint hide-sm">${t.length ? fmt.dur(t.length) : ''}</span>
    ${availability(t) || (() => {
      const j = { mbid: t.id, artist: t.artist, title: t.title, album: t.album, cover_url: t.cover, year: t.year, length: t.length };
      return `<div class="row req-group" style="gap:6px">
        <button class="btn sm primary" title="Just this song" data-req-track='${esc(JSON.stringify({ ...j, album_mode: 'song' }))}'>${icon.plus}Song</button>
        <button class="btn sm" title="Request the song and download the whole album it's on, in the best quality available" data-req-track='${esc(JSON.stringify({ ...j, album_mode: 'album' }))}'>${icon.disc}+ Album</button>
      </div>`;
    })()}
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
  let q = ctx.query.q || '';
  let type = ctx.query.type || 'all';
  view.innerHTML = `
    <div class="row wrap between" style="margin-bottom:18px">
      <h1>Discover</h1>
      <a class="btn sm" href="#/for-you">${icon.spark}For you</a>
    </div>
    <form class="search" id="s-form" style="max-width:720px">${icon.search}<input class="input" name="q" type="search" enterkeyhint="search" value="${esc(q)}" placeholder="Search MusicBrainz for artists, albums, songs…" autocomplete="off"></form>
    <div class="tabs mt" id="s-tabs">
      ${[['all', 'Top results'], ['artist', 'Artists'], ['album', 'Albums'], ['track', 'Songs']].map(([k, l]) => `<button type="button" data-t="${k}">${l}</button>`).join('')}
    </div>
    <div id="results" class="mt-lg"></div>`;
  const input = view.querySelector('input[name=q]');
  if (!q || !matchMedia('(pointer: coarse)').matches) { input.focus(); input.setSelectionRange(q.length, q.length); }
  const results = view.querySelector('#results');
  bindTiles(results, ctx.navigate);
  bindRequestButtons(results);
  let seq = 0;
  // results update in place – the page (and the search box with the phone keyboard) is never rebuilt while typing
  const run = async () => {
    view.querySelectorAll('#s-tabs button').forEach(b => b.classList.toggle('active', b.dataset.t === type));
    history.replaceState(null, '', `#/search?q=${encodeURIComponent(q)}&type=${type}`);
    const mine = ++seq;
    if (!q) {
      results.innerHTML = emptyState(icon.search, 'Search the entire MusicBrainz catalogue', 'Everything you find can be requested with one click.');
      return;
    }
    results.innerHTML = skeletonGrid(12);
    let data;
    try { data = await api.get(`/discover/search?q=${encodeURIComponent(q)}&type=${type}`); } catch (e) {
      if (mine === seq) results.innerHTML = `<div class="error-box">${esc(e.message)}</div>`;
      return;
    }
    if (mine !== seq || !view.isConnected) return;   // a newer search is already running
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
  };
  view.querySelector('#s-form').onsubmit = (e) => { e.preventDefault(); q = input.value.trim(); input.blur(); run(); };
  input.addEventListener('input', debounce(() => {
    const v = input.value.trim();
    if (v.length > 2 && v !== q) { q = v; run(); }
  }, 650));
  view.querySelector('#s-tabs').onclick = (e) => { const t = e.target.closest('[data-t]'); if (t) { type = t.dataset.t; q = input.value.trim(); run(); } };
  await run();
}

const ORDER = ['Album', 'EP', 'Single', 'Compilation', 'Live', 'Soundtrack', 'Remix', 'DJ-mix', 'Mixtape/Street', 'Demo', 'Other', 'Broadcast', 'Interview', 'Spokenword', 'Audiobook'];
const reqBody = (rg) => ({ kind: 'album', mbid: rg.id, artist: rg.artist, title: rg.title, album: rg.title, year: rg.year, cover_url: rg.cover });

export async function renderArtist(view, ctx) {
  view.innerHTML = '<div class="skeleton" style="height:220px"></div>' + skeletonGrid(8);
  const a = await api.get(`/discover/artist/${ctx.params[0]}`);
  const groups = {};
  for (const rg of a.release_groups) {
    const key = rg.secondary.length ? rg.secondary[0] : (rg.type || 'Other');
    (groups[key] ||= []).push(rg);
  }
  const keys = Object.keys(groups).sort((x, y) => (ORDER.indexOf(x) + 1 || 99) - (ORDER.indexOf(y) + 1 || 99));
  const owned = a.release_groups.filter(r => r.in_library).length;
  const firstCover = a.release_groups.find(r => r.type === 'Album' && !r.secondary.length)?.cover;
  const links = Object.entries(a.links || {}).filter(([k]) => ['official homepage', 'bandcamp', 'soundcloud', 'youtube', 'streaming', 'free streaming', 'wikidata', 'discogs', 'last.fm'].includes(k)).slice(0, 6);
  // library albums MusicBrainz doesn't list (bootlegs, untagged rips…) still show up
  const rgIds = new Set(a.release_groups.map(r => r.id));
  const titles = new Set(a.release_groups.map(r => r.title.toLowerCase()));
  const localOnly = a.library_albums.filter(l => !(l.rgid && rgIds.has(l.rgid)) && !titles.has((l.album || '').toLowerCase()));
  let filter = 'all';
  try { filter = localStorage.getItem('encore.artistFilter') || 'all'; } catch { /* ignore */ }

  view.innerHTML = `
    <div class="detail-head artist">
      <div class="cover" id="artist-photo">${art(firstCover, a.name)}</div>
      <div class="grow">
        <div class="kicker">${esc(a.type || 'Artist')}${a.country ? ' · ' + esc(a.country) : ''}${a.begin ? ' · since ' + esc(a.begin.slice(0, 4)) : ''}</div>
        <h1>${esc(a.name)}</h1>
        ${a.disambiguation ? `<p class="muted" style="margin:0 0 10px">${esc(a.disambiguation)}</p>` : ''}
        <div class="chips">${a.tags.map(t => `<a class="badge" href="#/tag/${encodeURIComponent(t)}">${esc(t)}</a>`).join('')}</div>
        <div class="row wrap mt">
          ${a.library_tracks ? `<button class="btn primary" id="play-artist">${icon.play}Play</button><button class="btn" id="shuffle-artist">Shuffle</button>` : ''}
          <button class="btn ${a.follow || a.library_tracks ? '' : 'primary'}" id="follow-btn">${a.follow ? `${icon.check}Following` : `${icon.plus}Follow`}</button>
          ${a.request_status ? statusBadge(a.request_status) : `<button class="btn" data-req-artist='${esc(JSON.stringify({ mbid: a.id, artist: a.name, cover_url: firstCover }))}'>${icon.download}Request discography</button>`}
        </div>
        <div class="row wrap mt small">
          <span class="badge ${owned ? 'ok' : ''}">${owned} / ${a.release_groups.length} releases in library</span>
          ${a.library_tracks ? `<span class="badge">${a.library_tracks} tracks</span>` : ''}
          <a class="btn sm ghost" target="_blank" rel="noopener" href="https://musicbrainz.org/artist/${esc(a.id)}">${icon.external}MusicBrainz</a>
          ${links.map(([k, u]) => `<a class="btn sm ghost" target="_blank" rel="noopener noreferrer" href="${esc(u)}">${esc(k)}</a>`).join('')}
        </div>
      </div>
    </div>
    <div id="artist-bio"></div>
    <div id="artist-top"></div>
    ${a.library_albums.length ? `<div class="section-head"><h2>In your library <span class="faint small">${a.library_albums.length}</span></h2></div>
      <div class="grid">${a.library_albums.map(l => libraryTile({ id: l.album_id, album: l.album, albumartist: l.albumartist, year: l.year })).join('')}</div>` : ''}
    <div class="section-head"><h2>Discography <span class="faint small">${a.release_groups.length}</span></h2>
      <div class="tabs" id="disco-filter">${[['all', 'All'], ['missing', 'Missing'], ['owned', 'In library']].map(([k, l]) => `<button data-f="${k}">${l}</button>`).join('')}</div>
    </div>
    <div id="disco"></div>
    <div id="ext-releases"></div>
    <div id="similar"></div>
    ${localOnly.length ? `<div class="section-head"><h2>Only in your library <span class="faint small">${localOnly.length}</span></h2></div>
      <p class="small muted" style="margin:-6px 0 12px">Albums MusicBrainz doesn't list for this artist (or not matched yet).</p>
      <div class="grid">${localOnly.map(l => libraryTile({ id: l.album_id, album: l.album, albumartist: l.albumartist, year: l.year })).join('')}</div>` : ''}`;

  const disco = view.querySelector('#disco');
  const paint = () => {
    view.querySelectorAll('#disco-filter button').forEach(b => b.classList.toggle('active', b.dataset.f === filter));
    const keep = rg => filter === 'all' || (filter === 'owned' ? rg.in_library : !rg.in_library);
    const html = keys.map(k => {
      const list = groups[k].filter(keep);
      if (!list.length) return '';
      const missing = groups[k].filter(rg => !rg.in_library && !rg.request_status);
      return `<div class="section-head"><h3>${esc(k)}s <span class="faint small">${list.length}</span></h3>
          ${missing.length ? `<button class="btn sm" data-req-missing="${esc(k)}">${icon.download}Request ${missing.length} missing</button>` : ''}</div>
        <div class="grid">${list.map(rg => albumTile(rg, { href: `/album/${rg.id}`, actions: albumActions(rg), sub: esc([rg.date || rg.year, rg.secondary.length ? rg.type : ''].filter(Boolean).join(' · ')) })).join('')}</div>`;
    }).join('');
    disco.innerHTML = html || emptyState(icon.disc, a.release_groups.length ? (filter === 'owned' ? 'Nothing from this artist in the library yet' : 'You have everything 🎉') : 'No releases on MusicBrainz');
  };
  paint();
  view.querySelector('#disco-filter').onclick = (e) => {
    const b = e.target.closest('[data-f]');
    if (!b) return;
    filter = b.dataset.f;
    try { localStorage.setItem('encore.artistFilter', filter); } catch { /* ignore */ }
    paint();
  };
  disco.addEventListener('click', async (e) => {
    const b = e.target.closest('[data-req-missing]');
    if (!b) return;
    const missing = groups[b.dataset.reqMissing].filter(rg => !rg.in_library && !rg.request_status);
    if (!await confirmDialog(`Request ${missing.length} ${b.dataset.reqMissing.toLowerCase()}(s)?`, missing.map(rg => rg.title).join(' · '), 'Request')) return;
    b.disabled = true;
    let ok = 0;
    for (const rg of missing) {
      try { const r = await api.post('/requests', reqBody(rg)); rg.request_status = r.status; ok++; } catch (err) { toast(err.message, 'err'); if (/limit/i.test(err.message)) break; }
    }
    toast(`${ok} release(s) requested`, 'ok');
    window.dispatchEvent(new CustomEvent('encore:requests-changed'));
    paint();
  });
  bindTiles(view, ctx.navigate);
  bindRequestButtons(view);
  view.querySelector('#follow-btn').onclick = () => followModal(a, () => window.dispatchEvent(new HashChangeEvent('hashchange')));
  const playAll = async (shuffle) => {
    let t = await api.get(`/discover/artist/${a.id}/library-tracks?name=${encodeURIComponent(a.name)}`);
    if (shuffle) t = t.map(x => [Math.random(), x]).sort((p, q) => p[0] - q[0]).map(p => p[1]);
    player.playQueue(t, 0);
  };
  view.querySelector('#play-artist')?.addEventListener('click', () => playAll(false));
  view.querySelector('#shuffle-artist')?.addEventListener('click', () => playAll(true));

  // similar artists (Last.fm → ListenBrainz → Deezer)
  api.get(`/discover/artist/${a.id}/similar`).then(x => {
    if (!view.isConnected || !x.artists.length) return;
    view.querySelector('#similar').innerHTML = `<div class="section-head"><h2>Similar artists <span class="faint small">${esc(x.source)}</span></h2></div>
      <div class="grid">${x.artists.map(artistCard).join('')}</div>`;
  }).catch(() => {});

  // releases on Deezer / Apple Music that MusicBrainz doesn't list
  const ext = view.querySelector('#ext-releases');
  ext.innerHTML = '<div class="small muted mt"><span class="spinner"></span> Checking Deezer and Apple Music for releases MusicBrainz is missing…</div>';
  api.get(`/discover/artist/${a.id}/catalog`).then(x => {
    if (!view.isConnected) return;
    if (!x.releases.length) { ext.innerHTML = ''; return; }
    const rel = x.releases;
    const extBody = (m) => ({ kind: 'album', artist: a.name, title: m.title, album: m.title, year: m.year, cover_url: m.cover,
      ext: m.ext });
    ext.innerHTML = `<div class="section-head" style="margin-top:28px"><div><h3>Not in MusicBrainz <span class="faint small">${rel.length}</span></h3>
        <div class="small muted">Found on ${[...new Set(rel.flatMap(m => m.sources.map(s => s.name)))].join(', ').replace(/, ([^,]*)$/, ' and $1')}. Requests skip Lidarr and download from Soulseek, streamrip, YouTube Music or Bandcamp; files are tagged from the source's data.
        Add them to MusicBrainz with Harmony and Encore re-tags them automatically.</div></div></div>
      <div class="grid">${rel.map((m, i) => `<div class="tile" data-ext-open="${i}">
        <div class="art">${art(m.cover, m.title)}
          <div class="corner">${m.in_library ? '<span class="badge ok">In library</span>' : m.request_status ? statusBadge(m.request_status) : '<span class="badge warn">Not in MusicBrainz</span>'}</div>
          <div class="overlay">${m.in_library || m.request_status ? '' : `<button class="fab" data-ext-req="${i}" title="Request">${icon.plus}</button>`}</div></div>
        <div class="t ellipsis" title="${esc(m.title)}">${esc(m.title)}</div>
        <div class="s ellipsis">${esc([m.date || m.year, m.type, m.tracks ? m.tracks + ' tracks' : ''].filter(Boolean).join(' · '))}</div>
        <div class="row small" style="gap:8px;flex-wrap:wrap;margin-top:4px">
          ${m.sources.map(s => `<a class="muted" target="_blank" rel="noopener" href="${esc(s.url)}">${esc(s.name)}</a>`).join('')}
          <a target="_blank" rel="noopener" href="${esc(m.harmony)}" title="Import into MusicBrainz with Harmony">Add to MB</a>
          <a href="#" data-ext-pick="${i}" title="Pick the exact download">Pick</a>
        </div></div>`).join('')}</div>`;
    ext.onclick = async (e) => {
      const r = e.target.closest('[data-ext-req]');
      if (r) { e.stopPropagation(); const res = await submitRequest(extBody(rel[+r.dataset.extReq]), r); if (res) rel[+r.dataset.extReq].request_status = res.status; return; }
      const p = e.target.closest('[data-ext-pick]');
      const open = !p && !e.target.closest('a, button') && e.target.closest('[data-ext-open]');
      if (open) {
        const m = rel[+open.dataset.extOpen];
        if (m.library_album_id) { ctx.navigate(`/library/album/${m.library_album_id}`); return; }
        const tracks = await api.post('/discover/catalog/tracks', m.ext).catch(err => { toast(err.message, 'err'); return null; });
        if (!tracks) return;
        modal({ title: m.title, body: tracks.length ? `<div class="table-wrap"><table class="table"><tbody>${tracks.map(t => `<tr>
            <td class="num">${t.track || ''}</td><td><div class="ellipsis" style="font-weight:600">${esc(t.title)}</div>${t.artist ? `<div class="small muted ellipsis">${esc(t.artist)}</div>` : ''}</td>
            <td class="muted" style="width:56px">${fmt.dur(t.length)}</td></tr>`).join('')}</tbody></table></div>` : '<p class="muted">No track list available.</p>' });
        return;
      }
      if (p) {
        e.preventDefault();
        const m = rel[+p.dataset.extPick];
        const b = extBody(m);
        pickerModal({ kind: 'album', artist: a.name, title: m.title, album: m.title, year: m.year, cover_url: m.cover, track_count: m.tracks, ext: b.ext });
      }
    };
  }).catch(() => { ext.innerHTML = ''; });

  // photo, bio and popular tracks load after the page (Deezer + Wikipedia, best-effort)
  api.get(`/discover/artist/${a.id}/extras`).then(x => {
    if (!view.isConnected) return;
    if (x.image) view.querySelector('#artist-photo').innerHTML = art(x.image, a.name);
    if (x.bio) {
      view.querySelector('#artist-bio').innerHTML = `<div class="card flat bio"><p class="clamp" id="bio-text">${esc(x.bio)}</p>
        <div class="row small"><button class="btn sm ghost" id="bio-more">Show more</button>${x.bio_url ? `<a class="muted" target="_blank" rel="noopener" href="${esc(x.bio_url)}">Wikipedia</a>` : ''}</div></div>`;
      view.querySelector('#bio-more').onclick = (e) => { const c = view.querySelector('#bio-text').classList.toggle('clamp'); e.target.textContent = c ? 'Show more' : 'Show less'; };
    }
    if (x.top?.length) {
      const top = x.top;
      const playable = top.filter(t => t.library).map(t => t.library);
      view.querySelector('#artist-top').innerHTML = `<div class="section-head"><h2>Popular</h2></div>
        <div class="card flat"><div class="table-wrap"><table class="table"><tbody>${top.map((t, i) => `<tr>
          <td class="num">${t.library ? `<button class="btn ghost icon sm" data-top-play="${i}" aria-label="Play">${icon.play.replace('<svg', '<svg width="14"')}</button>`
            : ''}</td>
          <td><div class="row" style="gap:10px"><div class="thumb">${art(t.cover, t.album)}</div><div><div style="font-weight:600">${esc(t.title)} ${t.version ? `<span class="muted">${esc(t.version)}</span>` : ''}</div><div class="small muted">${esc(t.album)}</div></div></div></td>
          <td class="muted hide-sm">${fmt.dur(t.duration)}</td>
          <td style="text-align:right;white-space:nowrap"><button class="btn sm ghost icon" data-more-top="${i}" title="More like this" aria-label="More like this">${icon.spark}</button>
            ${t.library ? '<span class="badge ok">In library</span>' : `<button class="btn sm" data-req-track='${esc(JSON.stringify({ artist: a.name, title: t.title + (t.version ? ' ' + t.version : ''), album: t.album, cover_url: t.cover, length: t.duration, isrc: t.isrc || undefined, album_mode: 'song' }))}'>${icon.plus}Track</button>`}</td>
        </tr>`).join('')}</tbody></table></div></div>`;
      view.querySelector('#artist-top').onclick = (e) => {
        const ml = e.target.closest('[data-more-top]');
        if (ml) return moreLikeThis(a.name, top[+ml.dataset.moreTop].title, null);
        const b = e.target.closest('[data-top-play]');
        if (b) player.playQueue(playable, playable.indexOf(top[+b.dataset.topPlay].library));
      };
    }
  }).catch(() => { /* extras are optional */ });
}

/** Library artist → full artist page (resolved through MusicBrainz), falling back to the library filter. */
export async function renderArtistByName(view, ctx) {
  const name = decodeURIComponent(ctx.params[0]);
  view.innerHTML = '<div class="skeleton" style="height:220px"></div>';
  let to = `#/library?artist=${encodeURIComponent(name)}`;
  try { to = `#/artist/${(await api.get(`/discover/artist-lookup?name=${encodeURIComponent(name)}`)).mbid}`; } catch { /* not on MusicBrainz */ }
  if (!view.isConnected) return;
  history.replaceState(null, '', to);           // no extra history entry, so Back skips the redirect
  window.dispatchEvent(new HashChangeEvent('hashchange'));
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
          <button class="btn" id="pick-album" title="Search every source and choose the exact download">${icon.search}Pick download</button>
          <a class="btn sm ghost" target="_blank" rel="noopener" href="https://musicbrainz.org/release-group/${esc(a.id)}">${icon.external}MusicBrainz</a>
        </div>
      </div>
    </div>
    ${rel ? `<div class="card flat"><div class="table-wrap"><table class="table">
      <thead><tr><th class="num">#</th><th>Title</th><th class="hide-sm">Artist</th><th>Length</th><th></th></tr></thead>
      <tbody>${rel.tracks.map(t => `<tr>
        <td class="num">${t.in_library ? `${multi ? t.disc + '-' : ''}${t.track}` : `${multi ? t.disc + '-' : ''}${t.track}`}</td>
        <td>${esc(t.title)}</td>
        <td class="hide-sm muted">${esc(t.artist)}</td>
        <td class="muted">${fmt.dur(t.length)}</td>
        <td style="text-align:right;white-space:nowrap"><button class="btn sm ghost icon" data-more-track="${esc(t.recording_id)}" title="More like this" aria-label="More like this">${icon.spark}</button><button class="btn sm ghost icon" data-pick-track="${esc(t.recording_id)}" title="Pick a download for this song" aria-label="Pick a download">${icon.search}</button>
          ${t.in_library ? '<span class="badge ok">In library</span>'
          : a.in_library || a.request_status ? '' : `<button class="btn sm" data-req-track='${esc(JSON.stringify({ mbid: t.recording_id, artist: t.artist, title: t.title, album: a.title, cover_url: a.cover, year: a.year, length: t.length, album_mode: 'song' }))}'>${icon.plus}Track</button>`}</td>
      </tr>`).join('')}</tbody></table></div></div>` : emptyState(icon.disc, 'No track listing available')}`;
  bindRequestButtons(view);
  const reload = () => window.dispatchEvent(new HashChangeEvent('hashchange'));
  view.querySelector('#pick-album').onclick = () => pickerModal({
    kind: 'album', mbid: a.id, artist: a.artist, artist_mbid: a.artist_id, title: a.title, album: a.title, year: a.year,
    cover_url: a.cover, track_count: rel ? rel.tracks.length : null, onDone: reload,
  });
  view.addEventListener('click', (e) => {
    const ml = e.target.closest('[data-more-track]');
    if (ml) { const t = rel.tracks.find(x => x.recording_id === ml.dataset.moreTrack); return moreLikeThis(t.artist, t.title, t.recording_id); }
    const b = e.target.closest('[data-pick-track]');
    if (!b) return;
    const t = rel.tracks.find(x => x.recording_id === b.dataset.pickTrack);
    pickerModal({ kind: 'track', mbid: t.recording_id, artist: t.artist, title: t.title, album: a.title, year: a.year, cover_url: a.cover, length: t.length, onDone: reload });
  });
}
