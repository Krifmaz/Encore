import { api } from '../api.js';
import { art, confirmDialog, emptyState, esc, icon, modal, statusBadge, toast } from '../ui.js';

const SRC = {
  spotify: 'Spotify', deezer: 'Deezer', tidal: 'Tidal', qobuz: 'Qobuz', apple: 'Apple Music', ytdlp: 'YouTube / SoundCloud',
  listenbrainz: 'ListenBrainz', web: 'Web', text: 'Pasted list', merged: 'Merged', manual: 'Encore', library: 'Encore',
  jellyfin: 'Jellyfin', plex: 'Plex', navidrome: 'Navidrome',
};
const TARGETS = { jellyfin: 'Jellyfin / Finamp', plex: 'Plex / Plexamp', navidrome: 'Navidrome', listenbrainz: 'ListenBrainz' };

function plCard(p) {
  const pct = p.total ? Math.round(p.matched / p.total * 100) : 0;
  const cover = p.kind === 'liked' ? '' : p.cover_url;
  return `<div class="tile" data-href="/playlists/${p.id}">
    <label class="corner" style="z-index:2" onclick="event.stopPropagation()">${p.kind === 'liked' ? '' : `<input type="checkbox" data-merge="${p.id}" title="Select to merge">`}</label>
    <div class="art">${p.kind === 'liked' ? `<div class="ph" style="background:var(--grad);color:#fff">♥</div>` : art(cover, p.name)}</div>
    <div class="t ellipsis">${esc(p.name)}</div>
    <div class="s ellipsis">${p.total} tracks · ${pct}% in library${p.follow ? ' · following' : ''}</div>
    <div class="s ellipsis faint">${esc(SRC[p.source] || p.source || '')}${p.sync_targets.length ? ' → ' + p.sync_targets.map(t => esc(TARGETS[t] || t)).join(', ') : ''}</div>
  </div>`;
}

export function importModal(ctx, { url = '', into = null } = {}) {
  const m = modal({
    title: into ? 'Add tracks from a link' : 'Import playlist',
    wide: true,
    body: `<div class="tabs" id="imp-tabs"><button data-m="url" class="active">From a link</button><button data-m="text">Paste a list</button></div>
      <div class="mt" id="imp-url"><input class="input" id="imp-link" value="${esc(url)}" placeholder="Spotify, Deezer, Tidal, Apple Music, Qobuz, YouTube (Music), SoundCloud, Bandcamp, ListenBrainz…">
        <p class="small faint" style="margin:6px 0 0">Connect Spotify/Tidal on the Connections page to import private playlists and Liked Songs (e.g. <span class="mono">https://open.spotify.com/collection/tracks</span>).</p></div>
      <div class="mt hidden" id="imp-text"><textarea class="input" id="imp-lines" rows="8" placeholder="One per line:  Artist - Title"></textarea></div>
      <div class="row wrap mt">
        ${into ? '' : `<input class="input" id="imp-name" placeholder="Playlist name (optional)" style="max-width:280px">
        <label class="switch"><input type="checkbox" id="imp-follow"><i></i>Keep following (pull new tracks)</label>`}
        <label class="switch"><input type="checkbox" id="imp-req"><i></i>Download missing tracks</label>
        <button class="btn" id="imp-preview">${icon.search}Preview</button>
      </div>
      <div id="imp-out" class="mt"></div>`,
    footer: `<button class="btn" data-close>Cancel</button><button class="btn primary" id="imp-go">${icon.download}Import</button>`,
  });
  let mode = 'url';
  const el = (s) => m.el.querySelector(s);
  el('#imp-tabs').onclick = (e) => {
    const b = e.target.closest('[data-m]');
    if (!b) return;
    mode = b.dataset.m;
    m.el.querySelectorAll('#imp-tabs button').forEach(x => x.classList.toggle('active', x === b));
    el('#imp-url').classList.toggle('hidden', mode !== 'url');
    el('#imp-text').classList.toggle('hidden', mode !== 'text');
    el('#imp-follow')?.closest('label').classList.toggle('hidden', mode !== 'url');
  };
  const body = () => (mode === 'url' ? { url: el('#imp-link').value.trim() } : { text: el('#imp-lines').value });
  el('#imp-preview').onclick = async () => {
    el('#imp-out').innerHTML = '<div class="empty"><span class="spinner"></span></div>';
    try {
      const p = await api.post('/playlists/preview', body());
      if (el('#imp-name') && !el('#imp-name').value) el('#imp-name').value = p.name || '';
      el('#imp-out').innerHTML = `<div class="row wrap" style="margin-bottom:10px">
          <b>${esc(p.name)}</b><span class="badge">${esc(SRC[p.source] || p.source)}</span>
          <span class="badge">${p.count} tracks</span><span class="badge ok">${p.in_library} already in library</span>
          ${p.duplicates ? `<span class="badge warn">${p.duplicates} duplicate(s) will be skipped</span>` : ''}</div>
        <div class="card flat" style="max-height:300px;overflow:auto"><div class="list">${p.tracks.map(t => `<div class="list-item" style="padding:7px 14px">
          <div class="grow ellipsis">${esc(t.artist)} – ${esc(t.title)} <span class="faint small">${t.album ? '· ' + esc(t.album) : ''}</span></div>
          ${t.isrc ? '<span class="badge" title="Has ISRC – exact matching">ISRC</span>' : ''}
          ${t.track_id ? `<span class="badge ok" title="${esc(t.match_reason || '')}">in library</span>` : '<span class="badge">missing</span>'}</div>`).join('')}</div></div>`;
    } catch (e) { el('#imp-out').innerHTML = `<div class="error-box">${esc(e.message)}</div>`; }
  };
  el('#imp-go').onclick = async (e) => {
    e.target.disabled = true;
    e.target.innerHTML = '<span class="spinner"></span> Importing…';
    try {
      const r = await api.post('/playlists/import', {
        ...body(), into, name: el('#imp-name')?.value.trim() || null,
        follow: el('#imp-follow')?.checked || false, request_missing: el('#imp-req').checked,
      });
      toast(`Imported ${r.added} track(s)${r.duplicates_skipped ? `, skipped ${r.duplicates_skipped} duplicate(s)` : ''}${r.requested ? `, requested ${r.requested}` : ''}`, 'ok');
      m.close();
      ctx.navigate(`/playlists/${r.id}`);
      if (into) window.dispatchEvent(new HashChangeEvent('hashchange'));
    } catch (err) {
      toast(err.message, 'err');
      e.target.disabled = false;
      e.target.innerHTML = `${icon.download}Import`;
    }
  };
}

export async function render(view, ctx) {
  view.innerHTML = `
    <div class="row wrap between" style="margin-bottom:18px">
      <div><h1>Playlists</h1><div class="small muted">Import from anywhere, merge without duplicates, and sync to Finamp / Plexamp.</div></div>
      <div class="row wrap">
        <button class="btn hidden" id="merge">${icon.list}Merge selected</button>
        <button class="btn" id="new">${icon.plus}New</button>
        <button class="btn primary" id="import">${icon.download}Import</button>
      </div>
    </div>
    <div id="pls"></div>`;
  const box = view.querySelector('#pls');
  const load = async () => {
    const rows = await api.get('/playlists');
    box.innerHTML = rows.length ? `<div class="grid">${rows.map(plCard).join('')}</div>` : emptyState(icon.list, 'No playlists yet');
  };
  box.addEventListener('click', (e) => {
    if (e.target.closest('input')) return;
    const t = e.target.closest('[data-href]');
    if (t) ctx.navigate(t.dataset.href);
  });
  box.addEventListener('change', () => {
    const n = box.querySelectorAll('[data-merge]:checked').length;
    const b = view.querySelector('#merge');
    b.classList.toggle('hidden', n < 2);
    b.innerHTML = `${icon.list}Merge ${n} playlists`;
  });
  view.querySelector('#import').onclick = () => importModal(ctx);
  view.querySelector('#new').onclick = async () => {
    const r = await api.post('/playlists', { name: 'New playlist' });
    ctx.navigate(`/playlists/${r.id}`);
  };
  view.querySelector('#merge').onclick = () => {
    const ids = [...box.querySelectorAll('[data-merge]:checked')].map(i => +i.dataset.merge);
    const m = modal({
      title: 'Merge playlists',
      body: `<p class="muted" style="margin-top:0">Tracks from ${ids.length} playlists are combined into one. The same song + version is only kept once – remixes, live versions and covers stay separate.</p>
        <label class="field"><span>Name</span><input class="input" id="mname" value="Merged playlist"></label>
        <label class="switch mt"><input type="checkbox" id="mdel"><i></i>Delete the original playlists</label>`,
      footer: '<button class="btn" data-close>Cancel</button><button class="btn primary" id="ok">Merge</button>',
    });
    m.el.querySelector('#ok').onclick = async () => {
      const r = await api.post('/playlists/merge', { ids, name: m.el.querySelector('#mname').value, delete_sources: m.el.querySelector('#mdel').checked });
      toast(`Merged ${r.total} tracks → ${r.added} unique (${r.duplicates_skipped} duplicates removed)`, 'ok');
      m.close();
      ctx.navigate(`/playlists/${r.id}`);
    };
  };
  await load();
}

export async function renderPlaylist(view, ctx) {
  const id = ctx.params[0];
  const [pl, conns] = await Promise.all([api.get(`/playlists/${id}`), api.get('/connections')]);
  const connected = Object.keys(conns.connections).filter(s => TARGETS[s]);
  const missing = pl.items.filter(i => !i.track_id && !i.request_id).length;
  view.innerHTML = `
    <div class="detail-head">
      <div class="cover">${pl.kind === 'liked' ? '<div class="ph" style="background:var(--grad);color:#fff;font-size:80px">♥</div>' : art(pl.cover_url, pl.name)}</div>
      <div class="grow">
        <div class="kicker">${pl.kind === 'liked' ? 'Liked songs · synced everywhere' : 'Playlist'} · ${esc(SRC[pl.source] || pl.source || '')}</div>
        <h1 ${pl.kind !== 'liked' ? 'contenteditable="true" spellcheck="false" id="pl-name" title="Click to rename"' : ''}>${esc(pl.name)}</h1>
        <div class="muted">${pl.total} tracks · <b style="color:var(--ok)">${pl.matched}</b> in library · ${pl.total - pl.matched} missing</div>
        <div class="progress mt" style="max-width:360px"><i style="width:${pl.total ? pl.matched / pl.total * 100 : 0}%"></i></div>
        <div class="row wrap mt">
          ${missing ? `<button class="btn primary" id="req">${icon.download}Download ${missing} missing</button>` : ''}
          <button class="btn" id="add">${icon.plus}Add from link</button>
          <button class="btn" id="dedupe">${icon.check}Remove duplicates</button>
          <button class="btn" id="rematch">${icon.refresh}Re-check library</button>
          ${pl.source_url ? `<button class="btn" id="refresh">${icon.refresh}Pull from ${esc(SRC[pl.source] || 'source')}</button>` : ''}
          ${pl.kind !== 'liked' ? `<button class="btn danger ghost" id="del">${icon.trash}</button>` : ''}
        </div>
      </div>
    </div>
    <div class="card row wrap between" style="margin-bottom:16px">
      <div class="row wrap">
        <b>Sync to</b>
        ${connected.length ? connected.filter(s => s !== pl.source).map(s => `<label class="switch"><input type="checkbox" data-target="${s}" ${pl.sync_targets.includes(s) ? 'checked' : ''}><i></i>${esc(TARGETS[s])}</label>`).join('')
          : '<span class="muted small">Connect Jellyfin, Plex or Navidrome on the <a href="#/connections" style="color:var(--accent)">Connections</a> page.</span>'}
        ${pl.kind === 'liked' ? '<span class="small faint">Liked songs also sync as favourites / loves automatically.</span>' : ''}
      </div>
      <div class="row">
        ${pl.source_url ? `<label class="switch"><input type="checkbox" id="follow" ${pl.follow ? 'checked' : ''}><i></i>Follow source</label>` : ''}
        ${connected.length ? `<button class="btn sm" id="sync">${icon.refresh}Sync now</button>` : ''}
      </div>
    </div>
    <div class="card flat"><div class="table-wrap"><table class="table">
      <thead><tr><th class="num">#</th><th>Title</th><th class="hide-sm">Album</th><th>Status</th><th></th></tr></thead>
      <tbody>${pl.items.map((it, i) => `<tr data-item="${it.id}">
        <td class="num">${i + 1}</td>
        <td><div style="font-weight:600">${esc(it.title)}</div><div class="small muted">${esc(it.artist)}</div></td>
        <td class="hide-sm muted">${it.album_id ? `<a href="#/library/album/${esc(it.album_id)}">${esc(it.album || '')}</a>` : esc(it.album || '')}</td>
        <td>${it.track_id ? `<span class="badge ok" title="${esc(it.match_reason || '')}">✓ in library</span>${it.verify_status === 'verified' ? ' <span class="badge ok" title="Audio fingerprint confirmed">verified</span>' : it.verify_status === 'mismatch' ? ' <span class="badge err">wrong audio</span>' : ''}`
          : it.request_status ? statusBadge(it.request_status) : '<span class="badge">missing</span>'}</td>
        <td style="text-align:right"><button class="btn sm ghost" data-rm title="Remove">${icon.x}</button></td></tr>`).join('')}
      </tbody></table></div></div>
    ${pl.items.length ? '' : emptyState(icon.music, 'Empty playlist', 'Add tracks from an album page or from a link.')}`;
  const reload = () => window.dispatchEvent(new HashChangeEvent('hashchange'));
  const act = async (fn, msg) => { try { const r = await fn(); toast(msg(r), 'ok'); reload(); } catch (e) { toast(e.message, 'err'); } };
  view.querySelector('#req')?.addEventListener('click', () => act(() => api.post(`/playlists/${id}/request-missing`), r => `Requested ${r.requested} track(s)`));
  view.querySelector('#dedupe').onclick = () => act(() => api.post(`/playlists/${id}/dedupe`), r => r.removed ? `Removed ${r.removed} duplicate(s)` : 'No duplicates');
  view.querySelector('#rematch').onclick = () => act(() => api.post(`/playlists/${id}/rematch`), r => `${r.matched} newly matched`);
  view.querySelector('#refresh')?.addEventListener('click', () => act(() => api.post(`/playlists/${id}/refresh`), r => `${r.added} new track(s)`));
  view.querySelector('#add').onclick = () => importModal(ctx, { into: +id });
  view.querySelector('#sync')?.addEventListener('click', () => act(() => api.post(`/playlists/${id}/sync`), r => r.errors ? `Synced with ${r.errors} error(s): ${r.log.join('; ')}` : 'Synced'));
  view.querySelector('#follow')?.addEventListener('change', (e) => api.patch(`/playlists/${id}`, { follow: e.target.checked }));
  view.querySelectorAll('[data-target]').forEach(cb => cb.addEventListener('change', async () => {
    const targets = [...view.querySelectorAll('[data-target]:checked')].map(c => c.dataset.target);
    await api.patch(`/playlists/${id}`, { sync_targets: targets });
    toast('Sync targets saved – syncing on the next pass (or press Sync now)', 'ok');
  }));
  const name = view.querySelector('#pl-name');
  if (name) {
    name.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); name.blur(); } });
    name.addEventListener('blur', () => { if (name.textContent.trim() && name.textContent.trim() !== pl.name) api.patch(`/playlists/${id}`, { name: name.textContent.trim() }); });
  }
  view.querySelector('#del')?.addEventListener('click', async () => {
    if (!await confirmDialog('Delete playlist', `Delete “${pl.name}”? Songs stay in the library.`, 'Delete', true)) return;
    await api.del(`/playlists/${id}`);
    ctx.navigate('/playlists');
  });
  view.querySelector('tbody').onclick = async (e) => {
    const b = e.target.closest('[data-rm]');
    if (!b) return;
    const tr = b.closest('[data-item]');
    await api.del(`/playlists/${id}/items/${tr.dataset.item}`);
    tr.remove();
  };
}

export async function addToPlaylistModal(trackIds) {
  const rows = await api.get('/playlists');
  const m = modal({
    title: 'Add to playlist',
    body: `<div class="list card flat">${rows.map(p => `<button class="list-item btn ghost" style="justify-content:flex-start;border-radius:0" data-pl="${p.id}">
      ${p.kind === 'liked' ? '♥' : icon.list}<span class="grow" style="text-align:left">${esc(p.name)}</span><span class="small faint">${p.total}</span></button>`).join('')}</div>
      <div class="row mt"><input class="input" id="newpl" placeholder="…or a new playlist name"><button class="btn" id="mk">${icon.plus}Create</button></div>`,
  });
  const add = async (pid) => {
    const r = await api.post(`/playlists/${pid}/tracks`, { track_ids: trackIds });
    toast(`Added ${r.added}${r.duplicates_skipped ? ` (${r.duplicates_skipped} already there)` : ''}`, 'ok');
    m.close();
  };
  m.el.onclick = (e) => { const b = e.target.closest('[data-pl]'); if (b) add(b.dataset.pl); };
  m.el.querySelector('#mk').onclick = async () => {
    const r = await api.post('/playlists', { name: m.el.querySelector('#newpl').value.trim() || 'New playlist' });
    add(r.id);
  };
}
