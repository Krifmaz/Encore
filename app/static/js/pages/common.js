import { api } from '../api.js';
import { art, confirmDialog, esc, fmt, icon, popMenu, statusBadge, toast } from '../ui.js';

export async function submitRequest(body, btn) {
  if (btn) btn.disabled = true;
  try {
    const r = await api.post('/requests', body);
    toast(r.status === 'pending' ? 'Request sent – waiting for approval' : 'Request queued', 'ok');
    window.dispatchEvent(new CustomEvent('encore:requests-changed'));
    if (btn) (btn.closest('.req-group') || btn).outerHTML = statusBadge(r.status);
    return r;
  } catch (e) {
    toast(e.message, 'err');
    if (btn) btn.disabled = false;
    return null;
  }
}

export function albumTile(a, { href, actions = '', sub } = {}) {
  return `<div class="tile" data-href="${esc(href)}">
    <div class="art">${art(a.cover, a.title)}
      <div class="corner">${a.in_library ? '<span class="badge ok">In library</span>' : a.request_status ? statusBadge(a.request_status) : ''}</div>
      <div class="overlay">${actions}</div>
    </div>
    <div class="t ellipsis" title="${esc(a.title)}">${esc(a.title)}</div>
    <div class="s ellipsis">${sub ?? esc([a.artist, a.year, a.type].filter(Boolean).join(' · '))}</div>
  </div>`;
}

export function libraryTile(a) {
  return `<div class="tile" data-href="/library/album/${esc(a.id)}">
    <div class="art">${art(`/api/library/albums/${a.id}/cover`, a.album)}
      ${a.untagged ? '<div class="corner"><span class="badge warn" title="Some tracks lack MusicBrainz tags">Untagged</span></div>' : ''}
      <div class="overlay"><button class="fab" data-play-album="${esc(a.id)}" aria-label="Play">${icon.play}</button></div>
    </div>
    <div class="t ellipsis" title="${esc(a.album)}">${esc(a.album || 'Unknown album')}</div>
    <div class="s ellipsis">${esc(a.albumartist)}${a.year ? ' · ' + esc(a.year) : ''}</div>
  </div>`;
}

export function bindTiles(root, navigate) {
  root.addEventListener('click', async (e) => {
    const play = e.target.closest('[data-play-album]');
    if (play) {
      e.stopPropagation();
      const { playAlbum } = await import('./library.js');
      playAlbum(play.dataset.playAlbum);
      return;
    }
    const song = e.target.closest('[data-play-song]');
    if (song) {
      e.stopPropagation();
      const player = await import('../player.js');
      player.playQueue([JSON.parse(song.dataset.playSong)], 0);
      return;
    }
    if (e.target.closest('button, a, input')) return;
    const tile = e.target.closest('[data-href]');
    if (tile) navigate(tile.dataset.href);
  });
}

export function requestRow(r, { admin = false } = {}) {
  const what = r.kind === 'album' ? `${esc(r.title || r.album)} <span class="muted">· ${esc(r.artist)}</span>`
    : r.kind === 'artist' ? `${esc(r.artist)} <span class="muted">· full discography</span>`
    : r.kind === 'url' ? `${esc(r.title || r.url)}`
    : r.kind === 'playlist' ? `${esc(r.title)} <span class="muted">· playlist import</span>`
    : `${esc(r.title)} <span class="muted">· ${esc(r.artist)}</span>`;
  const active = ['searching', 'downloading', 'importing', 'monitoring'].includes(r.status);
  return `<div class="list-item" data-id="${r.id}">
    <div class="thumb">${art(r.cover_url, r.title || r.artist)}</div>
    <div class="grow">
      <div class="ellipsis" style="font-weight:600">${what}</div>
      <div class="small muted ellipsis">
        <span class="badge" style="padding:0 7px">${esc(r.kind)}</span>
        ${r.source ? `<span class="faint">via ${esc(r.source)}</span> ·` : ''}
        ${esc(r.message || '')}
        ${admin && r.username ? ` · <span class="faint">by ${esc(r.username)}</span>` : ''}
        · <span class="faint">${fmt.ago(r.created_at)}</span>
      </div>
      ${active && r.progress ? `<div class="progress" style="margin-top:6px;max-width:420px"><i style="width:${r.progress}%"></i></div>` : ''}
    </div>
    <div class="row">
      ${statusBadge(r.status)}
      ${admin && r.status === 'pending' ? `<button class="btn sm primary" data-act="approve">${icon.check}Approve</button><button class="btn sm danger" data-act="deny">Deny</button>` : ''}
      ${['album', 'track'].includes(r.kind) && r.status !== 'completed' ? `<button class="btn sm" data-act="pick" data-req='${esc(JSON.stringify({ request_id: r.id, kind: r.kind, mbid: r.mbid, artist: r.artist, title: r.title, album: r.album, length: r.length }))}' title="Search the sources and pick the exact download">${icon.search}Pick</button>` : ''}
      ${['failed', 'cancelled'].includes(r.status) || (r.kind === 'playlist') ? `<button class="btn sm" data-act="retry" title="Retry">${icon.refresh}</button>` : ''}
      ${['pending', 'queued', 'searching', 'downloading', 'monitoring', 'importing'].includes(r.status) ? `<button class="btn sm" data-act="cancel" title="Cancel">${icon.x}</button>` : ''}
      ${!active ? `<button class="btn sm ghost icon" data-act="menu" data-status="${esc(r.status)}" aria-label="More">${icon.more}</button>` : ''}
    </div>
  </div>`;
}

export function bindRequestActions(root, reload) {
  root.addEventListener('click', async (e) => {
    const b = e.target.closest('[data-act]');
    if (!b) return;
    const id = b.closest('[data-id]').dataset.id;
    const act = b.dataset.act;
    if (act === 'menu') {
      const { session } = await import('../app.js');
      const st = b.dataset.status;
      const del = async (files) => {
        if (files && !await confirmDialog('Delete files', st === 'completed' ? 'Permanently delete the songs this request downloaded from the library?' : 'Delete the leftover/partial files of this download?', 'Delete', true)) return;
        try {
          const r = await api.del(`/requests/${id}${files ? '?files=true' : ''}`);
          if (files) toast(`${r.deleted_files} file(s) deleted`, 'ok');
          window.dispatchEvent(new CustomEvent('encore:requests-changed'));
          await reload();
        } catch (err) { toast(err.message, 'err'); }
      };
      popMenu(b, [
        { label: 'Remove from list', icon: icon.x, onClick: () => del(false) },
        ['admin', 'owner'].includes(session.user?.role) ? { label: st === 'completed' ? 'Delete downloaded files' : 'Delete leftover files', icon: icon.trash, danger: true, onClick: () => del(true) } : null,
      ]);
      return;
    }
    if (act === 'pick') {
      const { pickerModal } = await import('./picker.js');
      pickerModal({ ...JSON.parse(b.dataset.req), onDone: reload });
      return;
    }
    b.disabled = true;
    try {
      if (act === 'delete') await api.del(`/requests/${id}`);
      else await api.post(`/requests/${id}/${act}`);
      window.dispatchEvent(new CustomEvent('encore:requests-changed'));
      await reload();
    } catch (err) {
      toast(err.message, 'err');
      b.disabled = false;
    }
  });
}

/** "Clear" button menu for download lists: clear finished entries, optionally deleting their files. */
export function clearMenu(anchor, reload, { mine = true } = {}) {
  import('../app.js').then(({ session }) => {
    const admin = ['admin', 'owner'].includes(session.user?.role);
    const go = async (statuses, files, label) => {
      if (files && !await confirmDialog('Delete files', `${label}: this permanently deletes the files from disk.`, 'Delete', true)) return;
      try {
        const r = await api.post('/requests/clear', { statuses, files, mine });
        toast(`${r.removed} cleared${files ? ` · ${r.deleted_files} file(s) deleted` : ''}`, 'ok');
        window.dispatchEvent(new CustomEvent('encore:requests-changed'));
        reload();
      } catch (e) { toast(e.message, 'err'); }
    };
    popMenu(anchor, [
      { label: 'Clear completed', icon: icon.check, onClick: () => go(['completed'], false) },
      { label: 'Clear failed & cancelled', icon: icon.x, onClick: () => go(['failed', 'cancelled', 'denied'], false) },
      ...(admin ? ['-',
        { label: 'Clear failed & cancelled + delete their files', icon: icon.trash, danger: true, onClick: () => go(['failed', 'cancelled', 'denied'], true, 'Failed & cancelled') },
        { label: 'Clear completed + delete the songs', icon: icon.trash, danger: true, onClick: () => go(['completed'], true, 'Completed downloads') }] : []),
    ]);
  });
}
