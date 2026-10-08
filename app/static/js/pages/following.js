import { api } from '../api.js';
import { confirmDialog, emptyState, esc, fmt, icon, modal, toast } from '../ui.js';

const TYPE_LABEL = { Album: 'Albums', EP: 'EPs', Single: 'Singles' };

export function followModal(artist, onDone) {
  const cur = artist.follow;
  const types = cur ? cur.types : ['Album', 'EP'];
  const m = modal({
    title: `${cur ? 'Following' : 'Follow'} ${esc(artist.name)}`,
    body: `<p class="muted" style="margin-top:0">Encore checks MusicBrainz for new releases and downloads them automatically, in the best quality available.</p>
      <div class="stack">
        <div class="row wrap"><b class="small">Get new:</b>
          ${Object.entries(TYPE_LABEL).map(([k, l]) => `<label class="switch"><input type="checkbox" data-t="${k}" ${types.includes(k) ? 'checked' : ''}><i></i>${l}</label>`).join('')}
        </div>
        ${cur ? '' : `<label class="switch"><input type="checkbox" id="backfill"><i></i>Also download the ones I'm missing now</label>`}
        <p class="small faint" style="margin:0">Compilations, live albums, soundtracks and remix albums are never downloaded automatically.</p>
      </div>`,
    footer: `${cur ? `<button class="btn danger" id="unfollow">Unfollow</button>` : ''}<button class="btn" data-close>Cancel</button>
      <button class="btn primary" id="ok">${icon.check}${cur ? 'Save' : 'Follow'}</button>`,
  });
  const picked = () => [...m.el.querySelectorAll('[data-t]:checked')].map(i => i.dataset.t);
  m.el.querySelector('#ok').onclick = async (e) => {
    e.target.disabled = true;
    try {
      if (cur) {
        await api.patch(`/follows/${cur.id}`, { types: picked() });
        toast('Saved', 'ok');
      } else {
        const r = await api.post('/follows', { mbid: artist.id, name: artist.name, types: picked(), backfill: m.el.querySelector('#backfill').checked });
        toast(`Following ${artist.name}${r.requested ? ` – requested ${r.requested} missing release(s)` : ''}`, 'ok');
      }
      m.close();
      onDone && onDone();
    } catch (err) { toast(err.message, 'err'); e.target.disabled = false; }
  };
  m.el.querySelector('#unfollow')?.addEventListener('click', async () => {
    await api.del(`/follows/${cur.id}`);
    toast(`Unfollowed ${artist.name}`, 'ok');
    m.close();
    onDone && onDone();
  });
}

export async function render(view) {
  view.innerHTML = `
    <div class="row wrap between" style="margin-bottom:18px">
      <div><h1>Following</h1><div class="small muted">New albums and EPs from these artists are downloaded automatically when they come out.</div></div>
      <div class="row wrap">
        <button class="btn" id="lib">${icon.library}Follow all artists in my library</button>
        <button class="btn primary" id="check">${icon.refresh}Check for new releases</button>
      </div>
    </div>
    <div id="lib-status"></div>
    <div class="card flat" id="list"></div>`;
  const list = view.querySelector('#list');
  const load = async () => {
    const rows = await api.get('/follows');
    list.innerHTML = rows.length ? `<div class="table-wrap"><table class="table"><thead><tr>
        <th>Artist</th><th>Gets</th><th class="hide-sm">Known releases</th><th class="hide-sm">Downloaded new</th><th class="hide-sm">Last checked</th><th></th></tr></thead><tbody>
      ${rows.map(f => `<tr data-id="${f.id}">
        <td><a href="#/artist/${esc(f.mbid)}" style="font-weight:650">${esc(f.name)}</a>${f.last_error ? `<div class="small" style="color:var(--err)">${esc(f.last_error)}</div>` : ''}</td>
        <td>${f.types.map(t => `<span class="badge">${esc(TYPE_LABEL[t] || t)}</span>`).join(' ')}</td>
        <td class="hide-sm muted">${f.known_releases}</td>
        <td class="hide-sm">${f.found_count ? `<span class="badge ok">${f.found_count}</span> <span class="small faint">${fmt.ago(f.last_found)}</span>` : '<span class="faint">–</span>'}</td>
        <td class="hide-sm small muted">${fmt.ago(f.last_checked)}</td>
        <td style="text-align:right"><button class="btn sm ghost" data-unfollow title="Unfollow">${icon.x}</button></td></tr>`).join('')}
      </tbody></table></div>`
      : emptyState(icon.users, 'Not following anyone yet', 'Open an artist in Discover and press Follow – or follow everyone already in your library.');
  };
  list.onclick = async (e) => {
    const b = e.target.closest('[data-unfollow]');
    if (!b) return;
    const tr = b.closest('[data-id]');
    if (!await confirmDialog('Unfollow', `Stop watching ${tr.querySelector('a').textContent} for new releases?`, 'Unfollow')) return;
    await api.del(`/follows/${tr.dataset.id}`);
    load();
  };
  view.querySelector('#check').onclick = async (e) => {
    const btn = e.currentTarget;
    btn.disabled = true;
    btn.innerHTML = '<span class="spinner"></span> Checking MusicBrainz…';
    try {
      const r = await api.post('/follows/check');
      toast(r.new_releases ? `${r.new_releases} new release(s) found and requested` : 'No new releases', 'ok');
      load();
    } catch (err) { toast(err.message, 'err'); }
    btn.disabled = false;
    btn.innerHTML = `${icon.refresh}Check for new releases`;
  };
  const status = view.querySelector('#lib-status');
  const pollLib = async () => {
    const s = await api.get('/follows/library');
    if (s.running) {
      status.innerHTML = '<div class="card row" style="margin-bottom:16px"><span class="spinner"></span><span>Matching your library artists on MusicBrainz… (about 1 artist per second)</span></div>';
      setTimeout(() => view.isConnected && pollLib(), 2000);
    } else if (s.followed !== undefined) {
      status.innerHTML = `<div class="card" style="margin-bottom:16px"><b>Followed ${s.followed} artist(s) from your library.</b>
        ${s.unmatched_count ? `<div class="small muted">${s.unmatched_count} couldn't be matched to MusicBrainz – tag those albums with “Match MusicBrainz” first: ${esc(s.unmatched.slice(0, 12).join(', '))}${s.unmatched_count > 12 ? '…' : ''}</div>` : ''}</div>`;
      load();
    }
  };
  view.querySelector('#lib').onclick = async () => {
    try { await api.post('/follows/library'); pollLib(); } catch (err) { toast(err.message, 'err'); }
  };
  await load();
  pollLib();
}
