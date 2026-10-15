import { api } from '../api.js';
import { art, confirmDialog, coverUrl, debounce, emptyState, esc, fmt, icon, modal, popMenu, preloadCovers, shareAlbum, shareArtist, shareTrack, skeletonGrid, toast } from '../ui.js';
import { bindTiles, libraryTile } from './common.js';
import { addToPlaylistModal } from './playlists.js';
import * as player from '../player.js';
import { scrubber } from '../scrubber.js';

export async function playAlbum(id, start = 0) {
  const a = await api.get(`/library/albums/${id}`);
  player.playQueue(a.tracks.map(t => ({ ...t, album_id: id })), start, { context: 'album' });
}

function sourceLine(t) {
  if (!t.source) return '';
  const when = t.downloaded_at ? ` · ${fmt.ago(t.downloaded_at)}` : '';
  const link = t.source_url ? ` · <a href="${esc(t.source_url)}" target="_blank" rel="noopener noreferrer" style="color:var(--accent)">link</a>` : '';
  const who = t.requested_by ? ` · requested by ${esc(t.requested_by)}${t.requested_at ? ' on ' + fmt.day(t.requested_at) : ''}` : '';
  return `<div class="small faint ellipsis" style="max-width:520px" title="${esc([t.source, t.source_detail].filter(Boolean).join(' – '))}">⬇ ${esc(t.source)}${t.source_detail ? ' · ' + esc(t.source_detail) : ''}${link}${when}${who}</div>`;
}

const PAGE = 120;

/** "⋯" for one song: play next / queue / playlist / download the file / more like this / delete. */
export function trackMenu(anchor, t, { admin = false, onDeleted } = {}) {
  popMenu(anchor, [
    { label: 'Play next', icon: icon.next, onClick: () => { player.playNext([t]); toast('Plays next', 'ok'); } },
    { label: 'Add to queue', icon: icon.list, onClick: () => { player.enqueue([t]); toast('Added to queue', 'ok'); } },
    { label: 'Add to playlist', icon: icon.plus, onClick: () => addToPlaylistModal([t.id]) },
    { label: 'More like this', icon: icon.spark, onClick: async () => (await import('./foryou.js')).moreLikeThis(t.artist, t.title, t.mb_trackid) },
    { label: 'Lyrics', icon: icon.music, onClick: async () => (await import('./lyrics.js')).lyricsModal(t) },
    { label: 'Share link', icon: icon.external, onClick: () => shareTrack(t) },
    { label: 'Download this song', icon: icon.download, href: `/api/library/tracks/${t.id}/file`, download: true },
    player.canSaveOffline() ? { label: 'Save for offline', icon: icon.download, onClick: () => { player.saveOffline([t]); toast('Saving to this phone…', 'ok'); } } : null,
    t.album_id ? { label: 'Go to album', icon: icon.disc, href: `#/library/album/${t.album_id}` } : null,
    admin ? { label: 'Edit song details…', icon: icon.edit, onClick: () => fixSongModal(t) } : null,
    (admin || (window.encoreSession?.user?.perms?.fix && t.request_id)) ? (t.verify_status === 'manual'
      ? { label: 'Undo “confirmed”', icon: icon.x, onClick: async () => { await api.post('/library/verify-manual', { track_ids: [t.id], correct: false }); toast('Will be checked again', 'ok'); onDeleted?.(); } }
      : { label: 'This is the right song', icon: icon.check, onClick: async () => { try { await api.post('/library/verify-manual', { track_ids: [t.id] }); toast('Marked as confirmed – the audio check won’t flag it again', 'ok'); onDeleted?.(); } catch (e) { toast(e.message, 'err'); } } }) : null,
    ...(admin || (window.encoreSession?.user?.perms?.fix && t.request_id) ? ['-', { label: admin ? 'Delete song from disk' : 'Wrong song – remove it', icon: icon.trash, danger: true, onClick: async () => {
      if (!await confirmDialog('Delete song', `Permanently delete "${t.title}" from disk?`, 'Delete', true)) return;
      try { await api.del(`/library/tracks/${t.id}`); toast('Song deleted', 'ok'); onDeleted?.(); } catch (e) { toast(e.message, 'err'); }
    } }] : []),
  ]);
}

/** Correct one song (wrong match, or a different version that got lumped in as a duplicate). */
export function fixSongModal(t) {
  const F = [['title', 'Title', t.title], ['artist', 'Artist', t.artist], ['album', 'Album', t.album || ''], ['albumartist', 'Album artist', t.albumartist || t.artist],
    ['tracknumber', 'Track #', t.track || ''], ['date', 'Year', t.year || '']];
  const m = modal({
    title: 'Edit song details',
    wide: true,
    body: `<p class="small muted" style="margin-top:0">Change what this file is. Give a different version its own title (e.g. “… (Remix)”) or album so it stops counting as a duplicate. The file is moved to match and is never merged with another song.</p>
      <div class="form-grid">${F.map(([k, l, v]) => `<label class="field"><span>${l}</span><input class="input" data-k="${k}" value="${esc(v)}"></label>`).join('')}</div>
      <label class="switch mt"><input type="checkbox" id="fx-move" checked><i></i>Move the file to match (Artist/Album/NN Title)</label>
      <div class="row mt" style="gap:8px"><button class="btn sm" id="fx-id">${icon.shield}Identify by its audio</button><span class="small muted" id="fx-msg"></span></div>
      <div id="fx-res" class="mt"></div>`,
    footer: '<button class="btn" data-close>Cancel</button><button class="btn primary" id="fx-save">Save</button>',
  });
  const el = (q) => m.el.querySelector(q);
  el('#fx-id').onclick = async (e) => {
    e.target.disabled = true;
    el('#fx-msg').innerHTML = '<span class="spinner"></span> Listening…';
    try {
      const res = await api.get(`/library/tracks/${t.id}/identify`);
      el('#fx-msg').textContent = res.length ? 'Pick what it really is:' : 'No match for this audio.';
      el('#fx-res').innerHTML = `<div class="list">${res.map((r, i) => `<button class="list-item" data-r="${i}" style="text-align:left;width:100%;border:0;background:none;color:inherit;cursor:pointer">
        <div class="grow"><div style="font-weight:600">${esc(r.title)}</div><div class="small muted">${esc(r.artist)}${r.album ? ' · ' + esc(r.album) : ''}</div></div><span class="badge ok">${r.score}%</span></button>`).join('')}</div>`;
      el('#fx-res').onclick = (ev) => {
        const b = ev.target.closest('[data-r]');
        if (!b) return;
        const r = res[+b.dataset.r];
        el('[data-k=title]').value = r.title; el('[data-k=artist]').value = r.artist;
        if (r.album) el('[data-k=album]').value = r.album;
        toast('Filled in – check and save', 'ok');
      };
    } catch (err) { el('#fx-msg').textContent = err.message; }
    e.target.disabled = false;
  };
  el('#fx-save').onclick = async (e) => {
    e.target.disabled = true;
    const values = Object.fromEntries([...m.el.querySelectorAll('[data-k]')].map(i => [i.dataset.k, i.value]));
    try {
      const r = await api.post(`/library/tracks/${t.id}/fix`, { values, move: el('#fx-move').checked });
      toast('Song fixed', 'ok');
      m.close();
      if (r.album_id && location.hash.includes('/library/album/') && !location.hash.endsWith(r.album_id)) location.hash = `#/library/album/${r.album_id}`;
      else window.dispatchEvent(new HashChangeEvent('hashchange'));
    } catch (err) { toast(err.message, 'err'); e.target.disabled = false; }
  };
}

const SORT_OPTS = {
  albums: [['added', 'Date added'], ['released', 'Release date'], ['album', 'Title A–Z'], ['artist', 'Artist A–Z'], ['duration', 'Length'], ['size', 'Size']],
  artists: [['name', 'Name A–Z'], ['added', 'Date added'], ['albums', 'Most albums'], ['tracks', 'Most songs']],
  tracks: [['added', 'Date added'], ['released', 'Release date'], ['title', 'Title A–Z'], ['artist', 'Artist A–Z'], ['album', 'Album A–Z'], ['duration', 'Length']],
};
SORT_OPTS.singles = SORT_OPTS.albums;
const DESC_FIRST = new Set(['added', 'released', 'duration', 'size', 'albums', 'tracks']);

export async function render(view, ctx) {
  const admin = ['admin', 'owner'].includes(ctx.user.role);
  const st = {
    tab: ctx.query.tab || 'albums', q: ctx.query.q || '', sort: ctx.query.sort || '', dir: ctx.query.dir || '',
    filter: ctx.query.filter || '', artist: ctx.query.artist || '', offset: 0,
  };
  view.innerHTML = `
    <div class="row wrap between" style="margin-bottom:18px">
      <div class="row" style="gap:12px;align-items:center"><div><h1>Library</h1><div class="muted small" id="lib-sub"></div></div>
        <button class="btn primary sm" id="shuffle-lib" title="Play everything in random order – no song twice this session">${icon.shuffle}Shuffle</button></div>
      ${admin ? `<div class="row wrap">
        <button class="btn sm" id="scan-btn">${icon.refresh}Scan</button>
        <button class="btn sm" id="autotag-btn">${icon.tag}Auto-tag untagged</button>
        <button class="btn sm" id="organize-btn" title="Move every file into Artist/Album/NN Title folders (your naming template), filling in missing tags first">${icon.disc}Organize files</button>
        <button class="btn sm" id="verify-btn">${icon.shield}Verify audio</button>
        <button class="btn sm" id="art-all">${icon.disc}Find missing artwork</button>
      </div>` : ''}
    </div>
    <div id="bulk" class="hidden card" style="margin-bottom:16px"></div>
    <div class="row wrap" style="margin-bottom:18px">
      <div class="tabs" id="lib-tabs">
        ${[['albums', 'Albums'], ['singles', 'Singles'], ['artists', 'Artists'], ['tracks', 'Songs'], ...(admin ? [['duplicates', 'Duplicates'], ['issues', 'Wrong audio']] : [])].map(([k, l]) => `<button data-t="${k}" class="${k === st.tab ? 'active' : ''}">${l}</button>`).join('')}
      </div>
      <div class="search grow" style="max-width:360px">${icon.search}<input class="input" id="lib-q" placeholder="Filter library…" value="${esc(st.q)}"></div>
      <div class="row" style="gap:6px"><select class="input" id="lib-sort" style="width:auto"></select>
        <button class="btn sm dir-btn" id="lib-dir" title="Ascending / descending"></button></div>
      <select class="input" id="lib-filter" style="width:auto">
        <option value="">All albums</option><option value="untagged">Needs tagging</option>
        <option value="tagged">MusicBrainz tagged</option><option value="lossless">Lossless only</option>
      </select>
      ${st.artist ? `<span class="badge accent">${esc(st.artist)} <a href="#/library" style="margin-left:4px">✕</a></span><button class="btn sm" id="artist-share" title="Share link">${icon.external}Share</button>` : ''}
    </div>
    <div id="lib-body"></div>
    <div class="row mt" style="justify-content:center"><button class="btn hidden" id="more">Load more</button></div>`;
  view.querySelector('#shuffle-lib').onclick = async () => { if (!await player.shuffleLibrary()) toast('Your library is empty', 'err'); };
  view.querySelector('#artist-share')?.addEventListener('click', () => shareArtist(st.artist));
  const body = view.querySelector('#lib-body');
  const more = view.querySelector('#more');
  let saved = {};
  try { saved = JSON.parse(localStorage.getItem('encore.libSort') || '{}'); } catch { /* ignore */ }
  const sortEl = view.querySelector('#lib-sort'), dirEl = view.querySelector('#lib-dir');
  const paintSort = () => {
    const opts = SORT_OPTS[st.tab];
    sortEl.parentElement.classList.toggle('hidden', !opts);
    if (!opts) return;
    const cur = (st.tab === ctx.query.tab && st.sort) || saved[st.tab]?.sort || opts[0][0];
    st.sort = opts.some(o => o[0] === cur) ? cur : opts[0][0];
    st.dir = saved[st.tab]?.sort === st.sort && saved[st.tab]?.dir ? saved[st.tab].dir : (DESC_FIRST.has(st.sort) ? 'desc' : 'asc');
    sortEl.innerHTML = opts.map(([k, l]) => `<option value="${k}">Sort: ${l}</option>`).join('');
    sortEl.value = st.sort;
    dirEl.textContent = st.dir === 'asc' ? '↑' : '↓';
  };
  const saveSort = () => {
    saved[st.tab] = { sort: st.sort, dir: st.dir };
    try { localStorage.setItem('encore.libSort', JSON.stringify(saved)); } catch { /* ignore */ }
  };
  paintSort();
  view.querySelector('#lib-filter').value = st.filter;
  bindTiles(body, ctx.navigate);
  // the side scrubber: sections of the list as it is sorted (none while filtering)
  const rowsOf = () => body.querySelectorAll(st.tab === 'tracks' ? 'tbody tr' : '.grid > *');
  const scrub = scrubber(view, {
    rows: rowsOf,
    long: () => !more.classList.contains('hidden') || document.documentElement.scrollHeight > 2 * window.innerHeight,
    jump: async (offset) => {
      while (rowsOf().length <= offset && !more.classList.contains('hidden')) {
        const n = rowsOf().length;
        await load(true, offset + 1 - n);
        if (rowsOf().length === n) break;
      }
      const r = rowsOf()[offset];
      if (r) window.scrollTo({ top: r.getBoundingClientRect().top + window.scrollY - 16 });
    },
    onLeave: ctx.onLeave,
  });
  let indexSeq = 0;
  const loadIndex = async () => {
    const seq = ++indexSeq;
    scrub.set([]);
    if (!['albums', 'singles', 'artists', 'tracks'].includes(st.tab) || st.q) return;
    const kind = st.tab === 'albums' ? 'albums&album_kind=albums' : st.tab;
    try {
      const sections = await api.get(`/library/index?kind=${kind}&sort=${st.sort}&dir=${st.dir}&filter=${st.filter}&artist=${encodeURIComponent(st.artist)}`);
      if (seq === indexSeq) scrub.set(sections);
    } catch { /* no scrubber */ }
  };

  let nextPage = null;   // {url, res}: the albums page after the last one shown, fetched ahead
  const load = async (append = false, want = 0) => {
    if (!append) { st.offset = 0; body.innerHTML = skeletonGrid(12); }
    const filtEl = view.querySelector('#lib-filter');
    filtEl.classList.toggle('hidden', !['albums', 'singles'].includes(st.tab));
    view.querySelector('#lib-q').parentElement.classList.toggle('hidden', ['duplicates', 'issues'].includes(st.tab));
    if (!append) loadIndex();
    if (st.tab === 'duplicates') return renderDuplicates(body, view, more, load);
    if (st.tab === 'issues') return renderIssues(body, view, more);
    if (st.tab === 'albums' || st.tab === 'singles') {
      const pageUrl = (offset, limit) => `/library/albums?q=${encodeURIComponent(st.q)}&sort=${st.sort}&dir=${st.dir}&kind=${st.tab}&filter=${st.filter}&artist=${encodeURIComponent(st.artist)}&limit=${limit}&offset=${offset}`;
      const url = pageUrl(st.offset, want ? Math.min(500, Math.max(PAGE, want)) : PAGE);
      // the next page was fetched ahead (with its covers) when this one showed
      const ahead = append && nextPage && nextPage.url === url ? nextPage.res : null;
      nextPage = null;
      const res = (ahead && await ahead.catch(() => null)) || await api.get(url);
      const html = res.items.map(libraryTile).join('');
      if (append) body.querySelector('.grid').insertAdjacentHTML('beforeend', html);
      else body.innerHTML = res.items.length ? `<div class="grid">${html}</div>` : emptyState(icon.library, 'No albums found', st.filter === 'untagged' ? 'Everything is tagged 🎉' : '');
      st.offset += res.items.length;
      more.classList.toggle('hidden', st.offset >= res.total);
      if (st.offset < res.total) {
        const u = pageUrl(st.offset, PAGE);
        nextPage = { url: u, res: api.get(u).then(r => { preloadCovers(r.items.map(a => coverUrl(a.id, a.cover_v, 400))); return r; }) };
        nextPage.res.catch(() => {});
      }
      view.querySelector('#lib-sub').textContent = `${fmt.num(res.total)} ${st.tab}`;
    } else if (st.tab === 'artists') {
      const rows = await api.get(`/library/artists?q=${encodeURIComponent(st.q)}&sort=${st.sort}&dir=${st.dir}`);
      more.classList.add('hidden');
      view.querySelector('#lib-sub').textContent = `${fmt.num(rows.length)} artists`;
      body.innerHTML = rows.length ? `<div class="grid">${rows.map(a => `
        <div class="tile round" data-href="${a.mbid ? `/artist/${esc(a.mbid)}` : `/artist-name/${encodeURIComponent(a.name)}`}">
          <div class="art">${art(`/api/library/albums/${a.cover_album}/cover${a.cover_v ? '?v=' + a.cover_v : ''}`, a.name)}
            ${admin ? `<div class="overlay"><button class="fab" data-rm-artist="${esc(a.name)}" title="Remove this artist from the library">${icon.trash}</button></div>` : ''}</div>
          <div class="t ellipsis" style="text-align:center">${esc(a.name)}</div>
          <div class="s" style="text-align:center">${a.albums} album${a.albums === 1 ? '' : 's'} · ${a.tracks} tracks</div>
        </div>`).join('')}</div>` : emptyState(icon.users, 'No artists found');
    } else {
      const lim = want ? Math.min(1000, Math.max(300, want)) : 300;
      const rows = await api.get(`/library/tracks?q=${encodeURIComponent(st.q)}&sort=${st.sort}&dir=${st.dir}&limit=${lim}&offset=${append ? st.offset : 0}`);
      const base = append ? (body._tracks || []).length : 0;
      const html = rows.map((t, i) => `<tr data-i="${base + i}">
        <td style="width:52px"><button class="thumb play-thumb" data-play-track="${t.id}" aria-label="Play">${art(`/api/library/albums/${t.album_id}/cover${t.cover_v ? '?v=' + t.cover_v : ''}`, t.album || t.title, '', 200)}<i>${icon.play}</i></button></td>
        <td><div style="font-weight:600" class="ellipsis">${esc(t.title)}</div><div class="small muted ellipsis">${esc(t.artist)}<span class="show-sm"> · ${esc(t.album || '')}</span></div></td>
        <td class="hide-sm"><a href="#/library/album/${esc(t.album_id)}">${esc(t.album || '')}</a></td>
        <td class="hide-sm"><span class="badge">${esc((t.format || '').toUpperCase())}${t.bitrate ? ' ' + t.bitrate + 'k' : ''}</span> ${verifyBadge(t)}</td>
        <td class="muted hide-sm">${fmt.dur(t.duration)}</td>
        <td style="text-align:right"><button class="btn sm ghost icon" data-tmenu="${base + i}" aria-label="More">${icon.more}</button></td></tr>`).join('');
      if (!append) {
        body.innerHTML = rows.length ? `<div class="card flat"><div class="table-wrap"><table class="table"><thead><tr><th></th><th>Title</th><th class="hide-sm">Album</th><th class="hide-sm">Format</th><th class="hide-sm">Time</th><th></th></tr></thead><tbody>${html}</tbody></table></div></div>` : emptyState(icon.music, 'No songs found');
        body._tracks = rows;
      } else {
        body.querySelector('tbody').insertAdjacentHTML('beforeend', html);
        body._tracks = body._tracks.concat(rows);
      }
      st.offset = (append ? st.offset : 0) + rows.length;
      more.classList.toggle('hidden', rows.length < lim);
      view.querySelector('#lib-sub').textContent = '';
    }
    scrub.refresh();
  };

  body.addEventListener('click', async (e) => {
    const rm = e.target.closest('[data-rm-artist]');
    if (rm) {
      e.stopPropagation();
      const name = rm.dataset.rmArtist;
      if (!await confirmDialog('Remove artist', `Delete every album by “${name}” from disk and cancel their pending downloads? Use this for artists that were added by a wrong download.`, 'Remove', true)) return;
      try { const r = await api.del(`/library/artists?name=${encodeURIComponent(name)}`); toast(`Removed ${r.removed} song(s)`, 'ok'); load(); } catch (err) { toast(err.message, 'err'); }
      return;
    }
    const mb = e.target.closest('[data-tmenu]');
    if (mb) return trackMenu(mb, body._tracks[+mb.dataset.tmenu], { admin, onDeleted: () => load() });
    const b = e.target.closest('[data-play-track]');
    if (!b) return;
    const tracks = body._tracks || [];
    const idx = tracks.findIndex(t => String(t.id) === b.dataset.playTrack);
    player.playQueue([tracks[idx]], 0, { context: 'single' });   // a single song: library shuffle continues after it
  });
  view.querySelector('#lib-tabs').onclick = (e) => {
    const t = e.target.closest('[data-t]');
    if (!t) return;
    st.tab = t.dataset.t;
    view.querySelectorAll('#lib-tabs button').forEach(b => b.classList.toggle('active', b === t));
    paintSort();
    load();
  };
  view.querySelector('#lib-q').addEventListener('input', debounce((e) => { st.q = e.target.value.trim(); load(); }, 300));
  sortEl.onchange = (e) => { st.sort = e.target.value; st.dir = DESC_FIRST.has(st.sort) ? 'desc' : 'asc'; dirEl.textContent = st.dir === 'asc' ? '↑' : '↓'; saveSort(); load(); };
  dirEl.onclick = () => { st.dir = st.dir === 'asc' ? 'desc' : 'asc'; dirEl.textContent = st.dir === 'asc' ? '↑' : '↓'; saveSort(); load(); };
  view.querySelector('#lib-filter').onchange = (e) => { st.filter = e.target.value; load(); };
  more.onclick = () => load(true);

  if (admin) {
    const bulk = view.querySelector('#bulk');
    const poll = async () => {
      const s = await api.get('/library/stats');
      const scanning = s.scan.scanning, b = s.bulk;
      const org = s.organize || {};
      bulk.classList.toggle('hidden', !scanning && !b.running && !s.verify.running && !(s.artwork && s.artwork.running) && !org.running);
      const vc = s.verify_counts || {};
      view.querySelector('#verify-btn').innerHTML = `${icon.shield}Verify audio${vc.unchecked ? ` <span class="badge">${fmt.num(vc.unchecked)} unchecked</span>` : ''}${vc.mismatch ? ` <span class="badge err">${vc.mismatch} wrong</span>` : ''}`;
      view.querySelector('#verify-btn').dataset.acoustid = s.acoustid ? '1' : '';
      if (scanning) bulk.innerHTML = `<div class="row"><span class="spinner"></span><b>Scanning library</b><span class="muted">${s.scan.progress} / ${s.scan.total} files</span></div><div class="progress mt"><i style="width:${s.scan.total ? s.scan.progress / s.scan.total * 100 : 0}%"></i></div>`;
      else if (s.artwork && s.artwork.running) bulk.innerHTML = `<div class="row"><span class="spinner"></span><b>Finding artwork</b><span class="muted">${s.artwork.done} / ${s.artwork.total} · ${s.artwork.found} found</span></div><div class="progress mt"><i style="width:${s.artwork.total ? s.artwork.done / s.artwork.total * 100 : 0}%"></i></div>`;
      else if (s.verify.running) bulk.innerHTML = `<div class="row"><span class="spinner"></span><b>Verifying audio fingerprints</b><span class="muted ellipsis">${s.verify.done} / ${s.verify.total} · ${s.verify.verified} ok · ${s.verify.mismatch} wrong · ${s.verify.unknown} unknown · ${esc(s.verify.current)}</span></div><div class="progress mt"><i style="width:${s.verify.total ? s.verify.done / s.verify.total * 100 : 0}%"></i></div>`;
      else if (org.running) bulk.innerHTML = `<div class="row"><span class="spinner"></span><b>Organizing files</b><span class="muted ellipsis">${org.done} / ${org.total} · ${org.moved} moved · ${org.fixed} re-tagged · ${esc(org.current)}</span></div><div class="progress mt"><i style="width:${org.total ? org.done / org.total * 100 : 0}%"></i></div>`;
      else if (b.running) bulk.innerHTML = `<div class="row"><span class="spinner"></span><b>Auto-tagging</b><span class="muted ellipsis">${b.done} / ${b.total} · ${b.tagged} tagged · ${b.skipped} skipped · ${esc(b.current)}</span></div><div class="progress mt"><i style="width:${b.total ? b.done / b.total * 100 : 0}%"></i></div>`;
      view.querySelector('#autotag-btn').innerHTML = `${icon.tag}Auto-tag untagged${s.untagged_albums ? ` <span class="badge warn">${s.untagged_albums}</span>` : ''}`;
      return scanning || b.running || s.verify.running || !!(s.artwork && s.artwork.running) || !!org.running;
    };
    let wasBusy = await poll();
    const timer = setInterval(async () => {
      const busy = await poll().catch(() => false);
      if (wasBusy && !busy) load();
      wasBusy = busy;
    }, 2500);
    ctx.onLeave(() => clearInterval(timer));
    view.querySelector('#organize-btn').onclick = async () => {
      if (!await confirmDialog('Organize files', 'Every song is moved into folders from its tags using your naming template (Settings → Apps & sources → Library & tagging), e.g. Artist/Album (Year)/01 - Title. Missing tags are filled in first from the file name, AcoustID and Deezer. Playlists and likes keep working.', 'Organize')) return;
      try { await api.post('/library/organize'); toast('Organizing…', 'ok'); setTimeout(() => window.dispatchEvent(new HashChangeEvent('hashchange')), 800); } catch (e) { toast(e.message, 'err'); }
    };
    view.querySelector('#scan-btn').onclick = async () => {
      try { await api.post('/library/scan'); toast('Library scan started', 'ok'); wasBusy = true; poll(); } catch (e) { toast(e.message, 'err'); }
    };
    view.querySelector('#art-all').onclick = async () => {
      try { await api.post('/library/artwork'); toast('Looking for missing covers (Cover Art Archive, Deezer, iTunes)…', 'ok'); wasBusy = true; poll(); } catch (err) { toast(err.message, 'err'); }
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
  if (t.verify_status === 'manual') return `<span class="badge ok" title="${esc(t.verify_detail || 'Confirmed by hand')}">✓ confirmed</span>`;
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
  const admin = ['admin', 'owner'].includes(ctx.user.role);
  const id = ctx.params[0];
  const a = await api.get(`/library/albums/${id}`);
  const multi = new Set(a.tracks.map(t => t.disc)).size > 1;
  const formats = [...new Set(a.tracks.map(t => (t.format || '').toUpperCase()))].join(' / ');
  const untagged = a.tracks.filter(t => !t.mb_albumid).length;
  const requesters = [...new Set(a.tracks.map(t => t.requested_by).filter(Boolean))].join(', ');
  const requestedAt = Math.min(...a.tracks.map(t => t.requested_at).filter(Boolean));
  const addedAt = Math.min(...a.tracks.map(t => t.added_at).filter(Boolean));
  const history = [requesters ? `Requested by ${esc(requesters)}${isFinite(requestedAt) ? ` on ${fmt.day(requestedAt)}` : ''}` : '', isFinite(addedAt) ? `${requesters ? 'added' : 'Added'} ${fmt.day(addedAt)}` : ''].filter(Boolean).join(' · ');
  view.innerHTML = `
    <div class="detail-head">
      <div class="cover">${art(`/api/library/albums/${id}/cover${a.cover_v ? '?v=' + a.cover_v : ''}`, a.album, "big")}</div>
      <div class="grow">
        <div class="kicker">Album · In your library</div>
        <h1>${esc(a.album || 'Unknown album')}</h1>
        <div class="muted"><a href="#/artist-name/${encodeURIComponent(a.albumartist)}" style="color:var(--text);font-weight:650">${esc(a.albumartist)}</a>
          ${a.year ? ' · ' + esc(a.year) : ''} · ${a.tracks.length} tracks · ${fmt.longDur(a.duration)} · ${fmt.size(a.size)}</div>
        ${history ? `<div class="small muted" style="margin-top:4px">${history}</div>` : ''}
        <div class="chips mt">
          <span class="badge">${esc(formats)}</span>
          ${a.genre ? `<span class="badge">${esc(a.genre)}</span>` : ''}
          ${untagged ? `<span class="badge warn">${untagged} track(s) not MusicBrainz-tagged</span>` : '<span class="badge ok">MusicBrainz tagged</span>'}
        </div>
        <div class="row wrap mt">
          <button class="btn primary" id="play-all">${icon.play}Play</button>
          <button class="btn" id="shuffle-all">${icon.shuffle}Shuffle</button>
          <button class="btn" id="album-share" title="Share link">${icon.external}Share</button>
          ${admin ? `<button class="btn" id="album-edit" title="Change the album, artist and song names">${icon.edit}Edit details</button>` : ''}
          <button class="btn icon" id="album-menu" aria-label="More options" title="More options">${icon.more}</button>
        </div>
      </div>
    </div>
    <div class="card flat"><div class="table-wrap"><table class="table tracklist">
      <thead><tr><th class="num">#</th><th>Title</th><th class="hide-sm">Artist</th><th class="hide-sm">Format</th><th>Time</th><th></th></tr></thead>
      <tbody>${a.tracks.map((t, i) => `<tr data-i="${i}">
        <td class="num">${multi ? t.disc + '-' : ''}${t.track || '–'}</td>
        <td><div style="font-weight:600">${esc(t.title)}</div>${admin ? `<div class="small faint ellipsis" style="max-width:520px">${esc(t.relpath)}</div>` : ''}${sourceLine(t)}</td>
        <td class="hide-sm muted">${esc(t.artist)}</td>
        <td class="hide-sm"><span class="badge">${esc((t.format || '').toUpperCase())}${t.bitrate ? ' ' + t.bitrate + 'k' : ''}</span>${t.mb_trackid ? '' : ' <span class="badge warn" title="No MusicBrainz id">?</span>'} ${verifyBadge(t)}</td>
        <td class="muted">${fmt.dur(t.duration)}</td>
        <td style="text-align:right;white-space:nowrap"><button class="btn sm ghost" data-like="${t.id}" title="Like (syncs to Finamp/Plexamp)">${icon.heart}</button><button class="btn sm ghost icon" data-tmenu="${i}" aria-label="More">${icon.more}</button></td></tr>`).join('')}</tbody>
    </table></div></div>`;
  const queueTracks = a.tracks.map(t => ({ ...t, album_id: id }));
  view.querySelector('#play-all').onclick = () => player.playQueue(queueTracks, 0, { context: 'album' });
  view.querySelector('#shuffle-all').onclick = () => player.playQueue([...queueTracks].sort(() => Math.random() - 0.5), 0, { context: 'album' });
  const reload = () => window.dispatchEvent(new HashChangeEvent('hashchange'));
  view.querySelector('#album-edit')?.addEventListener('click', () => editModal(id, a, ctx));
  view.querySelector('#album-share').onclick = () => shareAlbum(id, a);
  view.querySelector('#album-menu').onclick = (e) => popMenu(e.currentTarget, [
    { label: 'Add to queue', icon: icon.list, onClick: () => { player.enqueue(queueTracks); toast('Added to queue', 'ok'); } },
    { label: 'Play next', icon: icon.next, onClick: () => { player.playNext(queueTracks); toast('Plays next', 'ok'); } },
    { label: 'Add to playlist', icon: icon.plus, onClick: () => addToPlaylistModal(a.tracks.map(t => t.id)) },
    { label: 'Share link', icon: icon.external, onClick: () => shareAlbum(id, a) },
    player.canSaveOffline() ? { label: 'Save album for offline', icon: icon.download, onClick: () => { player.saveOffline(queueTracks); toast(`Saving ${queueTracks.length} song(s) to this phone…`, 'ok'); } } : null,
    admin ? { label: 'All songs are right (confirm)', icon: icon.check, onClick: async () => { await api.post('/library/verify-manual', { track_ids: a.tracks.map(t => t.id) }); toast('Album confirmed', 'ok'); reload(); } } : null,
    { label: 'Download album (zip)', icon: icon.download, href: `/api/library/albums/${id}/download` },
    { label: 'Lyrics for all songs', icon: icon.music, onClick: async () => (await import('./lyrics.js')).fetchAlbumLyrics(id) },
    a.mb_releasegroupid ? { label: 'Release info', icon: icon.disc, href: `#/album/${a.mb_releasegroupid}` } : null,
    ...(admin ? ['-',
      { label: 'Tag / match metadata', icon: icon.tag, onClick: () => matchModal(id, a, ctx) },
      { label: 'Edit details (album, artist, songs)', icon: icon.edit, onClick: () => editModal(id, a, ctx) },
      { label: 'Change artwork…', icon: icon.disc, onClick: async () => (await import('./artwork.js')).artworkModal(id, a, reload) },
      { label: 'Find artwork automatically', icon: icon.refresh, onClick: async () => {
        try { const r = await api.post(`/library/albums/${id}/artwork`); toast(`Artwork embedded in ${r.embedded} file(s)`, 'ok'); reload(); } catch (err) { toast(err.message, 'err'); }
      } },
      '-',
      { label: 'Delete album from disk', icon: icon.trash, danger: true, onClick: async () => {
        if (!await confirmDialog('Delete album', `Permanently delete "${a.album}" (${a.tracks.length} files) from disk?`, 'Delete', true)) return;
        await api.del(`/library/albums/${id}`);
        toast('Album deleted', 'ok');
        ctx.navigate('/library');
      } }] : []),
  ]);
  const highlight = (cur) => view.querySelectorAll('tbody tr').forEach((tr, i) => tr.classList.toggle('playing', !!cur && cur.id === a.tracks[i].id));
  ctx.onLeave(player.onChange(highlight));
  highlight(player.current());
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
    const tm = e.target.closest('[data-tmenu]');
    if (tm) return trackMenu(tm, queueTracks[+tm.dataset.tmenu], { admin, onDeleted: reload });
    if (!lb && !e.target.closest('a, button')) {
      const tr = e.target.closest('tr[data-i]');
      if (tr) player.playQueue(queueTracks, +tr.dataset.i, { context: 'album' });
    }
  });
}

function matchModal(id, a, ctx) {
  const m = modal({
    title: 'Tag this album',
    wide: true,
    body: `<div class="tabs" id="mprov" style="margin-bottom:12px;max-width:100%">${[['musicbrainz', 'MusicBrainz'], ['deezer', 'Deezer'], ['apple', 'Apple Music'], ['discogs', 'Discogs'], ['fingerprint', 'By audio (AcoustID)']].map(([k, l]) => `<button type="button" data-p="${k}" class="${k === 'musicbrainz' ? 'active' : ''}">${l}</button>`).join('')}</div>
      <p class="small muted" id="mprov-help" style="margin:-4px 0 10px"></p>
      <form class="row wrap" id="mq">
        <input class="input grow" name="artist" value="${esc(a.albumartist_orig ?? a.albumartist)}" placeholder="Artist" style="max-width:260px">
        <input class="input grow" name="album" value="${esc(a.album ?? '')}" placeholder="Album">
        <button class="btn" type="submit">${icon.search}Search</button>
        <input class="input" name="rid" placeholder="…or paste a MusicBrainz / Deezer / Apple / Discogs link" style="max-width:340px">
      </form>
      <div id="mres" class="mt"></div>`,
  });
  const res = m.el.querySelector('#mres');
  let provider = 'musicbrainz';
  const HELP = {
    musicbrainz: 'Full MusicBrainz tags (ids that Picard, Navidrome and Jellyfin use).',
    deezer: 'Titles, track numbers, ISRCs, label, barcode and cover from Deezer – for releases MusicBrainz lacks or gets wrong.',
    apple: 'Titles, track numbers, genre and a large cover from Apple Music.',
    discogs: 'Vinyl, cassettes, bootlegs and old releases (needs a Discogs token in Settings).',
    fingerprint: 'Ignores names: identifies each song by its audio, then finds the release most of them are on (needs an AcoustID key).',
  };
  const paintProv = () => {
    m.el.querySelectorAll('#mprov button').forEach(b => b.classList.toggle('active', b.dataset.p === provider));
    m.el.querySelector('#mprov-help').textContent = HELP[provider];
  };
  m.el.querySelector('#mprov').onclick = (e) => {
    const b = e.target.closest('[data-p]');
    if (!b) return;
    provider = b.dataset.p;
    paintProv();
    const f = new FormData(m.el.querySelector('#mq'));
    search(f.get('artist'), f.get('album'));
  };
  paintProv();
  const search = async (artist, album) => {
    res.innerHTML = `<div class="empty"><span class="spinner"></span>${provider === 'fingerprint' ? ' Listening to every song… this takes a few seconds per song' : ''}</div>`;
    try {
      const d = await api.get(`/library/albums/${id}/match?provider=${provider}&artist=${encodeURIComponent(artist)}&album=${encodeURIComponent(album)}`);
      res.innerHTML = d.candidates.length ? `<p class="small muted">Your files: ${d.track_count} tracks. Pick the matching release:</p>
        <div class="card flat"><div class="list">${d.candidates.map(c => `
        <div class="list-item" style="cursor:pointer" data-rid="${esc(c.id)}">
          <div class="thumb">${art(c.cover, c.title)}</div>
          <div class="grow"><div style="font-weight:600" class="ellipsis">${esc(c.title)} <span class="muted">· ${esc(c.artist)}</span></div>
            <div class="small muted">${esc([c.provider, c.date, c.country, c.formats, c.label, c.status, c.type].filter(Boolean).join(' · '))}</div></div>
          ${c.track_count ? `<span class="badge ${c.track_count === d.track_count ? 'ok' : 'warn'}">${c.track_count} tracks</span>` : ''}
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
          <thead><tr><th>Current</th><th>→ ${esc(r.provider || 'MusicBrainz')}</th><th class="hide-sm">New path</th></tr></thead>
          <tbody>${pv.rows.map(row => `<tr>
            <td><div>${esc(row.old_track ?? '–')}. ${esc(row.old_title)}</div><div class="small faint">${esc(row.old_artist)}</div></td>
            <td>${row.new_title ? `<div class="${row.new_title !== row.old_title ? 'diff-new' : ''}">${row.new_disc > 1 ? row.new_disc + '-' : ''}${row.new_track}. ${esc(row.new_title)}</div><div class="small faint">${esc(row.new_artist)}</div>` : '<span class="badge err">no match – left untouched</span>'}</td>
            <td class="small mono faint hide-sm">${esc(row.new_path || '')}</td></tr>`).join('')}</tbody>
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
          toast(`Album tagged from ${r.provider || 'MusicBrainz'}`, 'ok');
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
    const link = (f.get('rid') || '').trim();
    const mbid = link.match(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/i);
    const dz = link.match(/deezer\.com\/(?:\w+\/)?album\/(\d+)/);
    const ap = link.match(/apple\.com\/.*?\/album\/(?:[^/]+\/)?(?:id)?(\d+)/);
    const dc = link.match(/discogs\.com\/(?:[^/]+\/)?(release|master)\/(\d+)/);
    if (mbid) preview(mbid[0]);
    else if (dz) preview(`deezer:${dz[1]}`);
    else if (ap) preview(`apple:${ap[1]}`);
    else if (dc) preview(`discogs:${dc[1][0]}${dc[2]}`);
    else search(f.get('artist'), f.get('album'));
  };
  const artist0 = a.albumartist_orig ?? a.albumartist;
  search(artist0 === 'Unknown Artist' ? '' : artist0, a.album ?? '');
}

async function editModal(id, _shown, ctx) {
  const a = await api.get(`/library/albums/${id}`);
  const m = modal({
    title: 'Edit details',
    wide: true,
    body: `<p class="small muted" style="margin-top:0">Type what this album really is. The names are written into the files, and automatic tagging won't change them back.</p>
      <div class="form-grid">
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
      </tbody></table></div></div>
      <label class="switch mt"><input type="checkbox" id="ed-rename" checked><i></i>Move and rename the files to match (naming template)</label>`,
    footer: '<button class="btn" data-close>Cancel</button><button class="btn primary" id="save">Save</button>',
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
      if (m.el.querySelector('#ed-rename').checked) out = await api.post('/library/tags', { track_ids: ids, values: {}, rename: true });
      toast('Saved', 'ok');
      m.close();
      if (out.album_id && out.album_id !== id) ctx.navigate(`/library/album/${out.album_id}`);
      else window.dispatchEvent(new HashChangeEvent('hashchange'));
    } catch (err) { toast(err.message, 'err'); e.target.disabled = false; }
  };
}
