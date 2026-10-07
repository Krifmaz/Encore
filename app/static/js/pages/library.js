import { api } from '../api.js';
import { art, confirmDialog, debounce, emptyState, esc, fmt, icon, modal, skeletonGrid, toast } from '../ui.js';
import { bindTiles, libraryTile } from './common.js';
import { addToPlaylistModal } from './playlists.js';

const PAGE = 120;

export async function render(view, ctx) {
  const admin = ctx.user.role === 'admin';
  const st = {
    tab: ctx.query.tab || 'albums', q: ctx.query.q || '', sort: ctx.query.sort || 'name',
    filter: ctx.query.filter || '', artist: ctx.query.artist || '', offset: 0,
  };
  view.innerHTML = `
    <div class="row wrap between" style="margin-bottom:18px">
      <div><h1>Library</h1><div class="muted small" id="lib-sub"></div></div>
      ${admin ? `<div class="row wrap">
        <button class="btn sm" id="scan-btn">${icon.refresh}Scan</button>
        <button class="btn sm" id="autotag-btn">${icon.tag}Auto-tag untagged</button>
        <button class="btn sm" id="verify-btn">${icon.shield}Verify audio</button>
      </div>` : ''}
    </div>
    <div id="bulk" class="hidden card" style="margin-bottom:16px"></div>
    <div class="row wrap" style="margin-bottom:18px">
      <div class="tabs" id="lib-tabs">
        ${[['albums', 'Albums'], ['artists', 'Artists'], ['tracks', 'Songs'], ...(admin ? [['duplicates', 'Duplicates'], ['issues', 'Wrong audio']] : [])].map(([k, l]) => `<button data-t="${k}" class="${k === st.tab ? 'active' : ''}">${l}</button>`).join('')}
      </div>
      <div class="search grow" style="max-width:360px">${icon.search}<input class="input" id="lib-q" placeholder="Filter library…" value="${esc(st.q)}"></div>
      <select class="input" id="lib-sort" style="width:auto">
        <option value="name">Sort: Artist</option><option value="album">Sort: Album</option>
        <option value="year">Sort: Year</option><option value="added">Sort: Recently added</option>
      </select>
      <select class="input" id="lib-filter" style="width:auto">
        <option value="">All albums</option><option value="untagged">Needs tagging</option>
        <option value="tagged">MusicBrainz tagged</option><option value="lossless">Lossless only</option>
      </select>
      ${st.artist ? `<span class="badge accent">${esc(st.artist)} <a href="#/library" style="margin-left:4px">✕</a></span>` : ''}
    </div>
    <div id="lib-body"></div>
    <div class="row mt" style="justify-content:center"><button class="btn hidden" id="more">Load more</button></div>`;
  const body = view.querySelector('#lib-body');
  const more = view.querySelector('#more');
  view.querySelector('#lib-sort').value = st.sort;
  view.querySelector('#lib-filter').value = st.filter;
  bindTiles(body, ctx.navigate);

  const load = async (append = false) => {
    if (!append) { st.offset = 0; body.innerHTML = skeletonGrid(12); }
    const sortEl = view.querySelector('#lib-sort'), filtEl = view.querySelector('#lib-filter');
    sortEl.classList.toggle('hidden', st.tab !== 'albums');
    filtEl.classList.toggle('hidden', st.tab !== 'albums');
    view.querySelector('#lib-q').parentElement.classList.toggle('hidden', ['duplicates', 'issues'].includes(st.tab));
    if (st.tab === 'duplicates') return renderDuplicates(body, view, more, load);
    if (st.tab === 'issues') return renderIssues(body, view, more);
    if (st.tab === 'albums') {
      const res = await api.get(`/library/albums?q=${encodeURIComponent(st.q)}&sort=${st.sort}&filter=${st.filter}&artist=${encodeURIComponent(st.artist)}&limit=${PAGE}&offset=${st.offset}`);
      const html = res.items.map(libraryTile).join('');
      if (append) body.querySelector('.grid').insertAdjacentHTML('beforeend', html);
      else body.innerHTML = res.items.length ? `<div class="grid">${html}</div>` : emptyState(icon.library, 'No albums found', st.filter === 'untagged' ? 'Everything is tagged 🎉' : '');
      st.offset += res.items.length;
      more.classList.toggle('hidden', st.offset >= res.total);
      view.querySelector('#lib-sub').textContent = `${fmt.num(res.total)} albums`;
    } else if (st.tab === 'artists') {
      const rows = await api.get(`/library/artists?q=${encodeURIComponent(st.q)}`);
      more.classList.add('hidden');
      view.querySelector('#lib-sub').textContent = `${fmt.num(rows.length)} artists`;
      body.innerHTML = rows.length ? `<div class="grid">${rows.map(a => `
        <div class="tile round" data-href="/library?artist=${encodeURIComponent(a.name)}">
          <div class="art">${art(`/api/library/albums/${a.cover_album}/cover`, a.name)}</div>
          <div class="t ellipsis" style="text-align:center">${esc(a.name)}</div>
          <div class="s" style="text-align:center">${a.albums} album${a.albums === 1 ? '' : 's'} · ${a.tracks} tracks</div>
        </div>`).join('')}</div>` : emptyState(icon.users, 'No artists found');
    } else {
      const rows = await api.get(`/library/tracks?q=${encodeURIComponent(st.q)}&limit=300&offset=${append ? st.offset : 0}`);
      const html = rows.map((t, i) => `<tr data-i="${i}">
        <td><div style="font-weight:600">${esc(t.title)}</div><div class="small muted">${esc(t.artist)}</div></td>
        <td class="hide-sm"><a href="#/library/album/${esc(t.album_id)}">${esc(t.album || '')}</a></td>
        <td class="hide-sm"><span class="badge">${esc((t.format || '').toUpperCase())}${t.bitrate ? ' ' + t.bitrate + 'k' : ''}</span> ${verifyBadge(t)}</td>
        <td class="muted">${fmt.dur(t.duration)}</td></tr>`).join('');
      if (!append) {
        body.innerHTML = rows.length ? `<div class="card flat"><div class="table-wrap"><table class="table"><thead><tr><th>Title</th><th class="hide-sm">Album</th><th class="hide-sm">Format</th><th>Time</th></tr></thead><tbody>${html}</tbody></table></div></div>` : emptyState(icon.music, 'No songs found');
        body._tracks = rows;
      } else {
        body.querySelector('tbody').insertAdjacentHTML('beforeend', html);
        body._tracks = body._tracks.concat(rows);
      }
      st.offset = (append ? st.offset : 0) + rows.length;
      more.classList.toggle('hidden', rows.length < 300);
      view.querySelector('#lib-sub').textContent = '';
    }
  };

  view.querySelector('#lib-tabs').onclick = (e) => {
    const t = e.target.closest('[data-t]');
    if (!t) return;
    st.tab = t.dataset.t;
    view.querySelectorAll('#lib-tabs button').forEach(b => b.classList.toggle('active', b === t));
    load();
  };
  view.querySelector('#lib-q').addEventListener('input', debounce((e) => { st.q = e.target.value.trim(); load(); }, 300));
  view.querySelector('#lib-sort').onchange = (e) => { st.sort = e.target.value; load(); };
  view.querySelector('#lib-filter').onchange = (e) => { st.filter = e.target.value; load(); };
  more.onclick = () => load(true);

  if (admin) {
    const bulk = view.querySelector('#bulk');
    const poll = async () => {
      const s = await api.get('/library/stats');
      const scanning = s.scan.scanning, b = s.bulk;
      bulk.classList.toggle('hidden', !scanning && !b.running && !s.verify.running);
      const vc = s.verify_counts || {};
      view.querySelector('#verify-btn').innerHTML = `${icon.shield}Verify audio${vc.unchecked ? ` <span class="badge">${fmt.num(vc.unchecked)} unchecked</span>` : ''}${vc.mismatch ? ` <span class="badge err">${vc.mismatch} wrong</span>` : ''}`;
      view.querySelector('#verify-btn').dataset.acoustid = s.acoustid ? '1' : '';
      if (scanning) bulk.innerHTML = `<div class="row"><span class="spinner"></span><b>Scanning library</b><span class="muted">${s.scan.progress} / ${s.scan.total} files</span></div><div class="progress mt"><i style="width:${s.scan.total ? s.scan.progress / s.scan.total * 100 : 0}%"></i></div>`;
      else if (s.verify.running) bulk.innerHTML = `<div class="row"><span class="spinner"></span><b>Verifying audio fingerprints</b><span class="muted ellipsis">${s.verify.done} / ${s.verify.total} · ${s.verify.verified} ok · ${s.verify.mismatch} wrong · ${s.verify.unknown} unknown · ${esc(s.verify.current)}</span></div><div class="progress mt"><i style="width:${s.verify.total ? s.verify.done / s.verify.total * 100 : 0}%"></i></div>`;
      else if (b.running) bulk.innerHTML = `<div class="row"><span class="spinner"></span><b>Auto-tagging</b><span class="muted ellipsis">${b.done} / ${b.total} · ${b.tagged} tagged · ${b.skipped} skipped · ${esc(b.current)}</span></div><div class="progress mt"><i style="width:${b.total ? b.done / b.total * 100 : 0}%"></i></div>`;
      view.querySelector('#autotag-btn').innerHTML = `${icon.tag}Auto-tag untagged${s.untagged_albums ? ` <span class="badge warn">${s.untagged_albums}</span>` : ''}`;
      return scanning || b.running || s.verify.running;
    };
    let wasBusy = await poll();
    const timer = setInterval(async () => {
      const busy = await poll().catch(() => false);
      if (wasBusy && !busy) load();
      wasBusy = busy;
    }, 2500);
    ctx.onLeave(() => clearInterval(timer));
    view.querySelector('#scan-btn').onclick = async () => {
      try { await api.post('/library/scan'); toast('Library scan started', 'ok'); wasBusy = true; poll(); } catch (e) { toast(e.message, 'err'); }
    };
    view.querySelector('#verify-btn').onclick = async (e) => {
      if (!e.currentTarget.dataset.acoustid) {
        toast('Add a free AcoustID API key in Settings → Verification first', 'err');
        return;
      }
      if (!await confirmDialog('Verify audio', 'Every unchecked file is fingerprinted and compared against AcoustID/MusicBrainz, so wrong songs, remixes posing as originals, covers and mislabeled tracks get flagged. Takes ~0.4s per track.', 'Start')) return;
      try { await api.post('/library/verify'); toast('Verification started', 'ok'); wasBusy = true; poll(); } catch (err) { toast(err.message, 'err'); }
    };
    view.querySelector('#autotag-btn').onclick = async () => {
      if (!await confirmDialog('Auto-tag library', 'Every album without MusicBrainz tags will be matched automatically. Confident matches get retagged and renamed using your naming template; the rest are left for manual matching.', 'Start')) return;
      try { await api.post('/library/autotag'); toast('Auto-tagging started', 'ok'); wasBusy = true; poll(); } catch (e) { toast(e.message, 'err'); }
    };
  }
  await load();
}

function verifyBadge(t) {
  if (t.verify_status === 'verified') return `<span class="badge ok" title="${esc(t.verify_detail || '')}">✓ verified</span>`;
  if (t.verify_status === 'mismatch') return `<span class="badge err" title="${esc(t.verify_detail || '')}">wrong audio</span>`;
  return '';
}

async function renderDuplicates(body, view, more, reload) {
  more.classList.add('hidden');
  body.innerHTML = '<div class="empty"><span class="spinner"></span><p class="muted">Comparing fingerprints, ISRCs, MusicBrainz ids and tags…</p></div>';
  const groups = await api.get('/library/duplicates');
  view.querySelector('#lib-sub').textContent = `${groups.length} duplicate group(s)`;
  if (!groups.length) {
    body.innerHTML = emptyState(icon.check, 'No duplicates found', 'Remixes, live versions, covers and edits are kept apart – only true copies of the same recording are listed here. Run “Verify audio” to also catch copies with different tags.');
    return;
  }
  body.innerHTML = `
    <div class="card row wrap between" style="margin-bottom:16px">
      <div class="muted small">The best copy (verified → lossless → bitrate → tagged) is preselected to keep. Playlists pointing at removed copies are moved to the kept one.</div>
      <button class="btn danger" id="dup-all">${icon.trash}Resolve all ${groups.length}</button>
    </div>
    ${groups.map(g => `<div class="card flat mt" data-key="${esc(g.key)}">
      <div class="row between" style="padding:12px 16px;border-bottom:1px solid var(--border)">
        <div><b>${esc(g.tracks[0].artist)} – ${esc(g.tracks[0].title)}</b> <span class="badge accent">${esc(g.reason)}</span></div>
        <div class="row"><button class="btn sm ghost" data-ignore>Not duplicates</button><button class="btn sm danger" data-resolve>Delete unchecked</button></div>
      </div>
      <div class="list">${g.tracks.map(t => `<label class="list-item" style="cursor:pointer">
        <input type="radio" name="keep-${esc(g.key)}" value="${t.id}" ${t.id === g.keep ? 'checked' : ''}>
        <div class="grow"><div class="ellipsis">${esc(t.title)} <span class="muted">· ${esc(t.album || '')}</span></div>
          <div class="small faint ellipsis">${esc(t.relpath)}</div></div>
        <span class="badge">${esc((t.format || '').toUpperCase())}${t.bitrate ? ' ' + t.bitrate + 'k' : ''}</span>
        ${verifyBadge(t)}<span class="small muted">${fmt.dur(t.duration)} · ${fmt.size(t.size)}</span>
      </label>`).join('')}</div></div>`).join('')}`;
  body.querySelector('#dup-all').onclick = async () => {
    if (!await confirmDialog('Resolve all duplicates', `Delete the lower-quality copies in ${groups.length} groups? Files are removed from disk.`, 'Delete copies', true)) return;
    const r = await api.post('/library/duplicates/resolve-all');
    toast(`Removed ${r.deleted} duplicate file(s)`, 'ok');
    reload();
  };
  body.onclick = async (e) => {
    const card = e.target.closest('[data-key]');
    if (!card) return;
    const key = card.dataset.key;
    if (e.target.closest('[data-ignore]')) {
      await api.post('/library/duplicates/ignore', { key });
      card.remove();
    } else if (e.target.closest('[data-resolve]')) {
      const keep = +card.querySelector('input:checked').value;
      const del = [...card.querySelectorAll('input')].map(i => +i.value).filter(v => v !== keep);
      await api.post('/library/duplicates/resolve', { key, keep, delete: del });
      toast(`Removed ${del.length} copy/copies`, 'ok');
      card.remove();
    }
  };
}

async function renderIssues(body, view, more) {
  more.classList.add('hidden');
  const rows = await api.get('/library/mismatches');
  view.querySelector('#lib-sub').textContent = `${rows.length} file(s) whose audio doesn't match their tags`;
  body.innerHTML = rows.length ? `<div class="card flat"><div class="list">${rows.map(t => `<div class="list-item">
      <div class="grow"><div><b>${esc(t.artist)} – ${esc(t.title)}</b> <span class="muted">· <a href="#/library/album/${esc(t.album_id)}">${esc(t.album || '')}</a></span></div>
      <div class="small" style="color:var(--err)">${esc(t.verify_detail || '')}</div></div>
      <a class="btn sm" href="#/library/album/${esc(t.album_id)}">${icon.tag}Fix</a>
    </div>`).join('')}</div></div>`
    : emptyState(icon.shield, 'Nothing flagged', 'Run “Verify audio” to fingerprint your library.');
}

export async function renderAlbum(view, ctx) {
  const admin = ctx.user.role === 'admin';
  const id = ctx.params[0];
  const a = await api.get(`/library/albums/${id}`);
  const multi = new Set(a.tracks.map(t => t.disc)).size > 1;
  const formats = [...new Set(a.tracks.map(t => (t.format || '').toUpperCase()))].join(' / ');
  const untagged = a.tracks.filter(t => !t.mb_albumid).length;
  view.innerHTML = `
    <div class="detail-head">
      <div class="cover">${art(`/api/library/albums/${id}/cover`, a.album)}</div>
      <div class="grow">
        <div class="kicker">Album · In your library</div>
        <h1>${esc(a.album || 'Unknown album')}</h1>
        <div class="muted"><a href="#/library?artist=${encodeURIComponent(a.albumartist)}" style="color:var(--text);font-weight:650">${esc(a.albumartist)}</a>
          ${a.year ? ' · ' + esc(a.year) : ''} · ${a.tracks.length} tracks · ${fmt.longDur(a.duration)} · ${fmt.size(a.size)}</div>
        <div class="chips mt">
          <span class="badge">${esc(formats)}</span>
          ${a.genre ? `<span class="badge">${esc(a.genre)}</span>` : ''}
          ${untagged ? `<span class="badge warn">${untagged} track(s) not MusicBrainz-tagged</span>` : '<span class="badge ok">MusicBrainz tagged</span>'}
        </div>
        <div class="row wrap mt">
          <button class="btn" id="add-pl">${icon.list}Add to playlist</button>
          <a class="btn" href="/api/library/albums/${esc(id)}/download">${icon.download}Download</a>
          ${a.mb_releasegroupid ? `<a class="btn ghost" href="#/album/${esc(a.mb_releasegroupid)}">${icon.disc}Release info</a>` : ''}
          ${admin ? `<button class="btn" id="match-btn">${icon.tag}Match MusicBrainz</button>
            <button class="btn" id="edit-btn">${icon.edit}Edit tags</button>
            <button class="btn danger ghost" id="del-btn">${icon.trash}</button>` : ''}
        </div>
      </div>
    </div>
    <div class="card flat"><div class="table-wrap"><table class="table tracklist">
      <thead><tr><th class="num">#</th><th>Title</th><th class="hide-sm">Artist</th><th class="hide-sm">Format</th><th>Time</th><th></th></tr></thead>
      <tbody>${a.tracks.map((t, i) => `<tr data-i="${i}">
        <td class="num">${multi ? t.disc + '-' : ''}${t.track || '–'}</td>
        <td><div style="font-weight:600">${esc(t.title)}</div>${admin ? `<div class="small faint ellipsis" style="max-width:520px">${esc(t.relpath)}</div>` : ''}</td>
        <td class="hide-sm muted">${esc(t.artist)}</td>
        <td class="hide-sm"><span class="badge">${esc((t.format || '').toUpperCase())}${t.bitrate ? ' ' + t.bitrate + 'k' : ''}</span>${t.mb_trackid ? '' : ' <span class="badge warn" title="No MusicBrainz id">?</span>'} ${verifyBadge(t)}</td>
        <td class="muted">${fmt.dur(t.duration)}</td>
        <td style="text-align:right;white-space:nowrap"><button class="btn sm ghost" data-like="${t.id}" title="Like (syncs to Finamp/Plexamp)">${icon.heart}</button><button class="btn sm ghost" data-addpl="${t.id}" title="Add to playlist">${icon.plus}</button></td></tr>`).join('')}</tbody>
    </table></div></div>`;
  view.querySelector('#add-pl').onclick = () => addToPlaylistModal(a.tracks.map(t => t.id));
  const liked = new Set(await api.get('/likes'));
  const paintLikes = () => view.querySelectorAll('[data-like]').forEach(b => {
    const on = liked.has(+b.dataset.like);
    b.style.color = on ? 'var(--accent-2)' : '';
    b.querySelector('svg').setAttribute('fill', on ? 'currentColor' : 'none');
  });
  paintLikes();
  view.querySelector('tbody').addEventListener('click', async (e) => {
    const lb = e.target.closest('[data-like]');
    if (lb) {
      const tid = +lb.dataset.like;
      if (liked.has(tid)) { await api.del(`/likes/${tid}`); liked.delete(tid); } else { await api.post(`/likes/${tid}`); liked.add(tid); }
      paintLikes();
    }
    const ab = e.target.closest('[data-addpl]');
    if (ab) addToPlaylistModal([+ab.dataset.addpl]);
  });
  if (!admin) return;
  view.querySelector('#match-btn').onclick = () => matchModal(id, a, ctx);
  view.querySelector('#edit-btn').onclick = () => editModal(id, a, ctx);
  view.querySelector('#del-btn').onclick = async () => {
    if (!await confirmDialog('Delete album', `Permanently delete "${a.album}" (${a.tracks.length} files) from disk?`, 'Delete', true)) return;
    await api.del(`/library/albums/${id}`);
    toast('Album deleted', 'ok');
    ctx.navigate('/library');
  };
}

function matchModal(id, a, ctx) {
  const m = modal({
    title: 'Match on MusicBrainz',
    wide: true,
    body: `<form class="row wrap" id="mq">
        <input class="input grow" name="artist" value="${esc(a.albumartist)}" placeholder="Artist" style="max-width:260px">
        <input class="input grow" name="album" value="${esc(a.album || '')}" placeholder="Album">
        <button class="btn" type="submit">${icon.search}Search</button>
        <input class="input" name="rid" placeholder="…or paste a MusicBrainz release URL/ID" style="max-width:340px">
      </form>
      <div id="mres" class="mt"></div>`,
  });
  const res = m.el.querySelector('#mres');
  const search = async (artist, album) => {
    res.innerHTML = '<div class="empty"><span class="spinner"></span></div>';
    try {
      const d = await api.get(`/library/albums/${id}/match?artist=${encodeURIComponent(artist)}&album=${encodeURIComponent(album)}`);
      res.innerHTML = d.candidates.length ? `<p class="small muted">Your files: ${d.track_count} tracks. Pick the matching release:</p>
        <div class="card flat"><div class="list">${d.candidates.map(c => `
        <div class="list-item" style="cursor:pointer" data-rid="${esc(c.id)}">
          <div class="thumb">${art(c.cover, c.title)}</div>
          <div class="grow"><div style="font-weight:600" class="ellipsis">${esc(c.title)} <span class="muted">· ${esc(c.artist)}</span></div>
            <div class="small muted">${esc([c.date, c.country, c.formats, c.label, c.status, c.type].filter(Boolean).join(' · '))}</div></div>
          <span class="badge ${c.track_count === d.track_count ? 'ok' : 'warn'}">${c.track_count} tracks</span>
          <span class="badge ${c.match > .85 ? 'ok' : c.match > .6 ? 'warn' : 'err'}">${Math.round(c.match * 100)}%</span>
        </div>`).join('')}</div></div>` : emptyState(icon.search, 'No releases found', 'Try editing the artist/album, or paste a release ID.');
    } catch (e) { res.innerHTML = `<div class="error-box">${esc(e.message)}</div>`; }
  };
  const preview = async (rid) => {
    res.innerHTML = '<div class="empty"><span class="spinner"></span></div>';
    try {
      const pv = await api.get(`/library/albums/${id}/preview?release_id=${encodeURIComponent(rid)}`);
      const r = pv.release;
      res.innerHTML = `
        <div class="row" style="margin-bottom:12px">
          <div class="thumb lg">${art(r.cover, r.title)}</div>
          <div class="grow"><h3>${esc(r.title)}</h3><div class="small muted">${esc([r.artist, r.date, r.country, r.label].filter(Boolean).join(' · '))}</div></div>
          <span class="badge ${pv.confidence > .85 ? 'ok' : pv.confidence > .6 ? 'warn' : 'err'}">Confidence ${Math.round(pv.confidence * 100)}%</span>
        </div>
        <div class="card flat"><div class="table-wrap"><table class="table">
          <thead><tr><th>Current</th><th>→ MusicBrainz</th><th>New path</th></tr></thead>
          <tbody>${pv.rows.map(row => `<tr>
            <td><div>${esc(row.old_track ?? '–')}. ${esc(row.old_title)}</div><div class="small faint">${esc(row.old_artist)}</div></td>
            <td>${row.new_title ? `<div class="${row.new_title !== row.old_title ? 'diff-new' : ''}">${row.new_disc > 1 ? row.new_disc + '-' : ''}${row.new_track}. ${esc(row.new_title)}</div><div class="small faint">${esc(row.new_artist)}</div>` : '<span class="badge err">no match – left untouched</span>'}</td>
            <td class="small mono faint">${esc(row.new_path || '')}</td></tr>`).join('')}</tbody>
        </table></div></div>
        <div class="row between mt">
          <label class="switch"><input type="checkbox" id="rename" checked><i></i>Rename & move files using naming template</label>
          <div class="row"><button class="btn" id="back">Back</button><button class="btn primary" id="apply">${icon.check}Apply tags</button></div>
        </div>`;
      res.querySelector('#back').onclick = () => { const f = new FormData(m.el.querySelector('#mq')); search(f.get('artist'), f.get('album')); };
      res.querySelector('#apply').onclick = async (e) => {
        e.target.disabled = true;
        e.target.innerHTML = '<span class="spinner"></span> Writing tags…';
        try {
          const out = await api.post(`/library/albums/${id}/retag`, { release_id: rid, rename: res.querySelector('#rename').checked });
          toast('Album tagged from MusicBrainz', 'ok');
          m.close();
          if (out.album_id && out.album_id !== id) ctx.navigate(`/library/album/${out.album_id}`);
          else window.dispatchEvent(new HashChangeEvent('hashchange'));
        } catch (err) { toast(err.message, 'err'); e.target.disabled = false; e.target.textContent = 'Apply tags'; }
      };
    } catch (e) { res.innerHTML = `<div class="error-box">${esc(e.message)}</div>`; }
  };
  res.onclick = (e) => { const it = e.target.closest('[data-rid]'); if (it) preview(it.dataset.rid); };
  m.el.querySelector('#mq').onsubmit = (e) => {
    e.preventDefault();
    const f = new FormData(e.target);
    const rid = (f.get('rid') || '').match(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/i);
    if (rid) preview(rid[0]); else search(f.get('artist'), f.get('album'));
  };
  search(a.albumartist === 'Unknown Artist' ? '' : a.albumartist, a.album || '');
}

function editModal(id, a, ctx) {
  const m = modal({
    title: 'Edit tags',
    wide: true,
    body: `<div class="form-grid">
        <label class="field"><span>Album artist</span><input class="input" name="albumartist" value="${esc(a.albumartist)}"></label>
        <label class="field"><span>Album</span><input class="input" name="album" value="${esc(a.album || '')}"></label>
        <label class="field"><span>Year / date</span><input class="input" name="date" value="${esc(a.year || '')}"></label>
        <label class="field"><span>Genre</span><input class="input" name="genre" value="${esc(a.genre || '')}"></label>
      </div>
      <div class="card flat mt"><div class="table-wrap"><table class="table"><thead><tr><th>Disc</th><th>#</th><th>Title</th><th>Artist</th></tr></thead><tbody>
      ${a.tracks.map(t => `<tr data-id="${t.id}">
        <td style="width:70px"><input class="input" name="discnumber" value="${esc(t.disc || 1)}"></td>
        <td style="width:70px"><input class="input" name="tracknumber" value="${esc(t.track || '')}"></td>
        <td><input class="input" name="title" value="${esc(t.title)}"></td>
        <td><input class="input" name="artist" value="${esc(t.artist)}"></td></tr>`).join('')}
      </tbody></table></div></div>`,
    footer: '<button class="btn" data-close>Cancel</button><button class="btn primary" id="save">Save tags</button>',
  });
  m.el.querySelector('#save').onclick = async (e) => {
    e.target.disabled = true;
    try {
      const ids = a.tracks.map(t => t.id);
      const vals = {};
      m.el.querySelectorAll('.form-grid input').forEach(i => { vals[i.name] = i.value.trim(); });
      let out = await api.post('/library/tags', { track_ids: ids, values: vals });
      for (const tr of m.el.querySelectorAll('tbody tr')) {
        const t = a.tracks.find(x => String(x.id) === tr.dataset.id);
        const v = Object.fromEntries([...tr.querySelectorAll('input')].map(i => [i.name, i.value.trim()]));
        if (v.title !== t.title || v.artist !== t.artist || v.tracknumber !== String(t.track || '') || v.discnumber !== String(t.disc || 1)) {
          out = await api.post('/library/tags', { track_ids: [t.id], values: v });
        }
      }
      toast('Tags saved', 'ok');
      m.close();
      if (out.album_id && out.album_id !== id) ctx.navigate(`/library/album/${out.album_id}`);
      else window.dispatchEvent(new HashChangeEvent('hashchange'));
    } catch (err) { toast(err.message, 'err'); e.target.disabled = false; }
  };
}
