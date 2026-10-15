// Discovery: "For you" (ListenBrainz recommendations), genre pages, similar artists and "More like this".
import { api } from '../api.js';
import * as player from '../player.js';
import { art, emptyState, esc, fmt, icon, modal, statusBadge, toast, tr, trInline, trSub } from '../ui.js';
import { albumTile, bindTiles, submitRequest } from './common.js';

export const KIND = { 'weekly-exploration': 'New music picked for you every Monday', 'daily-jams': 'Songs you love, refreshed daily', 'weekly-jams': 'Your favourites from the past weeks', lastfm: 'Similar to your most played songs on Last.fm' };

/** One song row: play it if it's in the library, otherwise request it. */
export function trackRow(t, i) {
  return `<tr data-i="${i}">
    <td class="num">${t.library ? `<button class="btn ghost icon sm" data-play="${i}" aria-label="Play">${icon.play.replace('<svg', '<svg width="14"')}</button>`
      : ''}</td>
    <td><div class="row" style="gap:10px"><div class="thumb">${art(t.cover, t.album || t.title)}</div>
      <div style="min-width:0"><div class="ellipsis" style="font-weight:600">${esc(tr(t, 'title'))}</div>${trSub(t, 'title')}
      <div class="small muted ellipsis">${t.artist_mbid ? `<a href="#/artist/${esc(t.artist_mbid)}">${esc(trInline(t, 'artist'))}</a>` : `<a href="#/artist-name/${encodeURIComponent(t.artist)}">${esc(trInline(t, 'artist'))}</a>`}${t.album ? ' · ' + esc(trInline(t, 'album')) : ''}</div></div></div></td>
    <td class="muted hide-sm">${t.length ? fmt.dur(t.length) : ''}</td>
    <td style="text-align:right;white-space:nowrap">
      <button class="btn sm ghost icon" data-more="${i}" title="More like this" aria-label="More like this">${icon.spark}</button>
      ${t.library ? '<span class="badge ok">In library</span>' : t.request_status ? statusBadge(t.request_status)
        : `<button class="btn sm" data-req="${i}">${icon.plus}Request</button>`}
    </td></tr>`;
}

const reqBody = (t) => ({ kind: 'track', mbid: t.mbid || null, artist: t.artist, title: t.title, album: t.album || null, cover_url: t.cover, length: t.length, isrc: t.isrc || undefined });

/** Wire play / request / more-like-this on a table of track rows. */
export function bindTracks(root, tracks) {
  root.addEventListener('click', async (e) => {
    const p = e.target.closest('[data-play]');
    if (p) {
      const playable = tracks.filter(t => t.library).map(t => t.library);
      return player.playQueue(playable, playable.indexOf(tracks[+p.dataset.play].library), { context: 'list' });
    }
    const m = e.target.closest('[data-more]');
    if (m) { const t = tracks[+m.dataset.more]; return moreLikeThis(t.artist, t.title, t.mbid); }
    const r = e.target.closest('[data-req]');
    if (r) {
      const t = tracks[+r.dataset.req];
      const res = await submitRequest(reqBody(t), r);
      if (res) t.request_status = res.status;
    }
  });
}

export async function moreLikeThis(artist, title, mbid) {
  const m = modal({ title: `More like ${esc(title)}`, wide: true, body: '<div class="muted"><span class="spinner"></span> Finding similar songs…</div>' });
  try {
    const d = await api.get(`/discover/similar-tracks?artist=${encodeURIComponent(artist)}&title=${encodeURIComponent(title)}${mbid ? '&mbid=' + mbid : ''}`);
    const body = m.el.querySelector('.modal-body');
    if (!d.tracks.length) { body.innerHTML = emptyState(icon.spark, 'Nothing similar found', 'Add a Last.fm API key in Settings for the best results.'); return; }
    const missing = d.tracks.filter(t => !t.library && !t.request_status);
    body.innerHTML = `<div class="row wrap" style="justify-content:space-between;margin-bottom:10px"><span class="small muted">from ${esc(d.source)}</span>
        ${missing.length ? `<button class="btn sm" id="ml-all">${icon.download}Request ${missing.length} missing</button>` : ''}</div>
      <div class="table-wrap"><table class="table"><tbody>${d.tracks.map(trackRow).join('')}</tbody></table></div>`;
    bindTracks(body, d.tracks);
    body.querySelector('#ml-all')?.addEventListener('click', (e) => requestAll(missing, e.currentTarget));
  } catch (e) { m.el.querySelector('.modal-body').innerHTML = `<div class="error-box">${esc(e.message)}</div>`; }
}

export async function requestAll(tracks, btn) {
  btn.disabled = true;
  let ok = 0;
  for (const t of tracks) {
    try { const r = await api.post('/requests', reqBody(t)); t.request_status = r.status; ok++; } catch (err) { if (/limit/i.test(err.message)) { toast(err.message, 'err'); break; } }
  }
  toast(`${ok} song(s) requested`, 'ok');
  window.dispatchEvent(new CustomEvent('encore:requests-changed'));
}

/** Round artist tile for similar artists / genre pages (resolved to MusicBrainz lazily when there is no id). */
export function artistCard(a) {
  const href = a.mbid ? `/artist/${a.mbid}` : `/artist-name/${encodeURIComponent(a.name)}`;
  return `<div class="tile round" data-href="${esc(href)}">
    <div class="art">${a.image ? art(a.image, a.name) : `<div class="ph">${esc((a.name || '?')[0])}</div>`}
      ${a.in_library ? '<div class="corner" style="top:auto;bottom:8px;left:50%;transform:translateX(-50%)"><span class="badge ok">In library</span></div>' : ''}</div>
    <div class="t ellipsis" style="text-align:center">${esc(a.name)}</div>
  </div>`;
}

export async function render(view, ctx) {
  view.innerHTML = `<h1>For you</h1><p class="muted" style="margin:6px 0 20px">Recommendations from your ListenBrainz and Last.fm listening. Songs you already have play right here; request the rest in one click.</p>
    <div id="fy-body"><div class="skeleton" style="height:300px"></div></div>
    <div class="section-head"><h2>Browse by genre</h2></div><div id="fy-tags" class="chips"></div>`;
  api.get('/discover/tags').then(t => {
    const lib = t.library.slice(0, 15);
    view.querySelector('#fy-tags').innerHTML = [...lib.map(g => [g, true]), ...t.top.filter(g => !lib.includes(g.toLowerCase())).map(g => [g, false])]
      .map(([g, mine]) => `<a class="badge ${mine ? 'accent' : ''}" href="#/tag/${encodeURIComponent(g)}" style="padding:6px 12px;font-size:13px">${esc(g)}</a>`).join('');
  }).catch(() => {});
  const body = view.querySelector('#fy-body');
  let d;
  try { d = await api.get('/discover/for-you'); } catch (e) { body.innerHTML = `<div class="error-box">${esc(e.message)}</div>`; return; }
  if (!d.connected) {
    body.innerHTML = `<div class="card">${emptyState(icon.spark, 'Connect Last.fm or ListenBrainz to get recommendations',
      'Connect either one on the <a href="#/connections">Connections</a> page. Encore scrobbles what you play here to them, and your other players (Navidrome, Finamp, Plexamp…) can scrobble too.')}</div>`;
    return;
  }
  const lf = d.lastfm;
  if (!d.playlists.length && !lf) {
    body.innerHTML = `<div class="card">${emptyState(icon.spark, 'No recommendations yet', `ListenBrainz builds Weekly Exploration once ${esc(d.user)} has a few weeks of listens. Check that your players scrobble to ListenBrainz.`)}</div>`;
    return;
  }
  // Last.fm: shown as one more list of songs, plus artists you might like
  if (lf?.tracks?.length) d.playlists.unshift({ id: null, title: 'Songs for you', kind: 'lastfm', tracks: lf.tracks });
  body.innerHTML = (d.errors || []).map(e => `<div class="error-box" style="margin-bottom:12px">${esc(e)}</div>`).join('') + (lf?.artists?.length ? `<div class="section-head"><div><h2>Artists for you</h2>
      <div class="small muted">Similar to what ${esc(lf.user)} plays most on Last.fm</div></div></div><div class="grid" id="fy-artists">${lf.artists.map(artistCard).join('')}</div>` : '')
    + (lf?.albums?.length ? `<div class="section-head"><div><h2>Albums for you</h2><div class="small muted">The best-known album of artists you might like</div></div></div>
      <div class="grid" id="fy-albums">${lf.albums.map(a => albumTile(a, { href: `/search?q=${encodeURIComponent(`${a.artist} ${a.title}`)}&type=album` })).join('')}</div>` : '')
    + d.playlists.map((p, pi) => {
    const missing = p.tracks.filter(t => !t.library && !t.request_status).length;
    const owned = p.tracks.filter(t => t.library).length;
    return `<div class="section-head"><div><h2>${esc(p.title)}</h2><div class="small muted">${esc(KIND[p.kind] || '')}${p.tracks.length ? ` · ${owned}/${p.tracks.length} in library` : ''}</div></div>
        <div class="row">${owned ? `<button class="btn sm" data-play-pl="${pi}">${icon.play}Play owned</button>` : ''}
        ${missing ? `<button class="btn sm primary" data-req-pl="${pi}">${icon.download}Request ${missing} missing</button>` : ''}
        ${p.id ? `<a class="btn sm ghost" target="_blank" rel="noopener" href="https://listenbrainz.org/playlist/${esc(p.id)}">${icon.external}</a>` : ''}</div></div>
      <div class="card flat"><div class="table-wrap"><table class="table" data-pl="${pi}"><tbody>${p.tracks.map(trackRow).join('')}</tbody></table></div></div>`;
  }).join('');
  body.querySelectorAll('table[data-pl]').forEach(tb => bindTracks(tb, d.playlists[+tb.dataset.pl].tracks));
  if (lf?.artists?.length) bindTiles(body.querySelector('#fy-artists'), ctx.navigate);
  if (lf?.albums?.length) bindTiles(body.querySelector('#fy-albums'), ctx.navigate);
  body.addEventListener('click', (e) => {
    const r = e.target.closest('[data-req-pl]');
    if (r) return requestAll(d.playlists[+r.dataset.reqPl].tracks.filter(t => !t.library && !t.request_status), r).then(() => render(view, ctx));
    const p = e.target.closest('[data-play-pl]');
    if (p) player.playQueue(d.playlists[+p.dataset.playPl].tracks.filter(t => t.library).map(t => t.library), 0);
  });
}

export async function renderTag(view, ctx) {
  const tag = decodeURIComponent(ctx.params[0]);
  let page = +(ctx.query.page || 1);
  view.innerHTML = `<div class="kicker muted small" style="text-transform:uppercase;letter-spacing:.1em;font-weight:700">Genre</div><h1 style="text-transform:capitalize">${esc(tag)}</h1>
    <p class="muted" id="tag-sum" style="max-width:80ch"></p><div id="tag-grid">${'<div class="skeleton" style="height:200px"></div>'}</div>
    <div class="row mt" style="justify-content:center"><button class="btn hidden" id="tag-more">Load more</button></div>`;
  const grid = view.querySelector('#tag-grid');
  const more = view.querySelector('#tag-more');
  let first = true;
  const load = async () => {
    const d = await api.get(`/discover/tag/${encodeURIComponent(tag)}?page=${page}`);
    if (first) {
      grid.innerHTML = '<div class="grid" id="tag-tiles"></div>';
      view.querySelector('#tag-sum').innerHTML = d.summary ? esc(d.summary) + ` <span class="faint small">· ${esc(d.source)}</span>` : `<span class="faint small">Top artists from ${esc(d.source)}</span>`;
      first = false;
    }
    view.querySelector('#tag-tiles').insertAdjacentHTML('beforeend', d.artists.map(artistCard).join(''));
    more.classList.toggle('hidden', d.artists.length < 40);
    if (!d.artists.length && page === 1) grid.innerHTML = emptyState(icon.users, 'No artists for this genre');
  };
  bindTiles(view, ctx.navigate);
  more.onclick = async () => { more.disabled = true; page++; await load(); more.disabled = false; };
  try { await load(); } catch (e) { grid.innerHTML = `<div class="error-box">${esc(e.message)}</div>`; }
}
