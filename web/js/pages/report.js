// ⋯ → Report a problem (songs and albums), and Settings → Blocked: reports, blocked sources with Undo,
// and the songs the quality checks flagged. The same page in web-rs/src/pages/report.rs.
import { api } from '../api.js';
import { esc, fmt, icon, modal, toast } from '../ui.js';

export const REASONS = [
  ['wrong_song', 'Wrong song'],
  ['wrong_version', 'Wrong version (live, remix, edit, missing parts)'],
  ['bad_audio', 'Bad audio quality'],
  ['loudness', 'Too quiet or too loud'],
  ['fake_lossless', 'Fake lossless'],
  ['glitches', 'Skips or glitches'],
  ['other', 'Other'],
];

/** The reason a quality-check flag suggests. */
export function reasonFor(codes) {
  const c = codes || [];
  if (c.includes('fake_lossless') || c.includes('upsampled') || c.includes('low_bitrate')) return 'fake_lossless';
  if (c.includes('quiet')) return 'loudness';
  if (c.includes('wrong_length')) return 'wrong_version';
  return 'bad_audio';
}

/**
 * Report a problem with a song ({ kind: 'track', id, title }) or an album ({ kind: 'album', id, title }).
 * Encore blocks the source the file came from, moves it to the trash and looks for another copy.
 */
export function reportModal({ kind, id, title, reason = '', onDone } = {}) {
  const what = kind === 'album' ? 'album' : 'song';
  const m = modal({
    title: 'Report a problem',
    body: `<p class="small muted" style="margin-top:0">What is wrong with “${esc(title || '')}”? Encore stops using the source this ${what} came from, moves the file${kind === 'album' ? 's' : ''} to the trash for 7 days and gets the ${what} again from somewhere else. Plays, likes and playlists stay with the ${what}.</p>
      <div class="stack" style="gap:8px" id="rp-reasons">${REASONS.map(([k, l]) => `<label class="row" style="gap:8px;align-items:center"><input type="radio" name="rp-reason" value="${k}" ${k === reason ? 'checked' : ''}> ${esc(l)}</label>`).join('')}</div>
      <label class="field mt"><span>Note</span><textarea class="input" id="rp-note" rows="2" placeholder="Optional – e.g. the intro is missing"></textarea></label>`,
    footer: '<button class="btn" data-close>Cancel</button><button class="btn primary" id="rp-send">Report</button>',
  });
  m.el.querySelector('#rp-send').onclick = async (e) => {
    const r = m.el.querySelector('input[name=rp-reason]:checked');
    const note = m.el.querySelector('#rp-note').value.trim();
    if (!r) { toast('Pick what is wrong', 'err'); return; }
    if (r.value === 'other' && !note) { toast('Say what is wrong', 'err'); return; }
    e.currentTarget.disabled = true;
    try {
      const body = { kind, reason: r.value, note };
      if (kind === 'album') body.album_id = id; else body.track_id = id;
      const res = await api.post('/report', body);
      m.close();
      toast(res.status === 'replacing' ? 'Reported – looking for a better copy' : (res.message || 'Reported'), 'ok');
      onDone?.(res);
    } catch (err) {
      toast(err.message, 'err');
      e.currentTarget.disabled = false;
    }
  };
}

const STATUS = { replacing: ['Looking for a replacement', 'warn'], replaced: ['Replaced', 'ok'], no_replacement: ['No replacement yet', 'err'], dismissed: ['Dismissed', ''] };

function reportRow(r, admin) {
  const [label, cls] = STATUS[r.status] || [r.status, ''];
  const href = r.kind === 'album' && r.album_id ? `#/library/album/${encodeURIComponent(r.album_id)}` : '';
  const name = `${esc(r.artist || '')} – ${esc(r.title || '')}`;
  return `<div class="list-item" style="align-items:flex-start;flex-wrap:wrap;gap:10px" data-report="${r.id}">
    <div class="grow" style="min-width:220px"><div style="font-weight:650">${href ? `<a href="${href}">${name}</a>` : name} <span class="small muted">${r.kind === 'album' ? 'album' : 'song'}</span></div>
      <div class="small">${esc(r.reason_label || r.reason)}${r.note ? ` · “${esc(r.note)}”` : ''}</div>
      <div class="small faint">${admin && r.username ? `${esc(r.username)} · ` : ''}${fmt.ago(r.created_at)}${r.source ? ` · from ${esc(r.source)}` : ''}${r.message ? ` · ${esc(r.message)}` : ''}</div></div>
    <span class="badge ${cls}">${esc(label)}</span>
    <span class="row" style="gap:6px">${r.status === 'no_replacement' ? `<button class="btn sm" data-retry="${r.id}">${icon.refresh}Try again</button>` : ''}${admin && ['replacing', 'no_replacement'].includes(r.status) ? `<button class="btn sm ghost" data-dismiss="${r.id}">Dismiss</button>` : ''}</span>
  </div>`;
}

/** Settings → Blocked */
export async function render(view, ctx) {
  const admin = ['admin', 'owner'].includes(ctx?.user?.role);
  const [values, reps, blocks, sus] = await Promise.all([
    admin ? api.get('/admin/settings') : Promise.resolve({}),
    api.get('/reports'),
    admin ? api.get('/blocks') : Promise.resolve({ blocks: [] }),
    admin ? api.get('/quality/suspect') : Promise.resolve({ tracks: [] }),
  ]);
  const meta = values._meta || {};
  const lockedQ = (meta.quality_check_on_import || {}).source === 'env';
  view.innerHTML = `
    <div class="card stack" style="margin-bottom:16px">
      <h2>Blocked</h2>
      <p class="muted small" style="margin:0">When someone reports a song or album (⋯ → Report a problem), the exact source it came from is blocked – that Soulseek file or folder, torrent, Usenet post, streaming item or YouTube video – and the music is fetched again from somewhere else. Undo a block to allow that source again.</p>
      ${admin ? `<div class="field" style="justify-content:center"><label class="switch"><input type="checkbox" id="q-import" ${values.quality_check_on_import ? 'checked' : ''} ${lockedQ ? 'disabled' : ''}><i></i>Check every download before it enters the library</label>
        <small>Fake lossless (a lossy file saved as FLAC), upsampled hi-res, very quiet audio and the wrong length are turned away and the next copy is tried. The Quality scan under Scheduled tasks checks the songs already in the library.</small></div>` : ''}
    </div>
    <div class="card stack" style="margin-bottom:16px">
      <h2>Reports</h2>
      ${reps.reports.length ? `<div class="list">${reps.reports.map(r => reportRow(r, admin)).join('')}</div>` : '<p class="muted small" style="margin:0">Nothing reported yet.</p>'}
    </div>
    ${admin ? `<div class="card stack" style="margin-bottom:16px">
      <h2>Blocked sources</h2>
      ${blocks.blocks.length ? `<div class="list">${blocks.blocks.map(b => `<div class="list-item" style="flex-wrap:wrap;gap:10px" data-block="${b.id}">
        <div class="grow" style="min-width:220px"><div style="font-weight:600" class="ellipsis">${esc(b.label || b.key)}</div>
          <div class="small faint">${esc(b.kind)}${b.reason_label ? ` · ${esc(b.reason_label)}` : ''}${b.title ? ` · ${esc(b.artist || '')} – ${esc(b.title)}` : ''}${b.username ? ` · ${esc(b.username)}` : ''} · ${fmt.ago(b.created_at)}</div></div>
        <button class="btn sm" data-undo="${b.id}">Undo</button></div>`).join('')}</div>` : '<p class="muted small" style="margin:0">No blocked sources.</p>'}
    </div>
    <div class="card stack">
      <h2>Suspect quality</h2>
      <p class="muted small" style="margin:0">Songs the quality checks flagged. Replace one to block where it came from and get it again.</p>
      ${sus.tracks.length ? `<div class="list">${sus.tracks.map((t, i) => `<div class="list-item" style="flex-wrap:wrap;gap:10px">
        <div class="grow" style="min-width:220px"><div style="font-weight:600"><a href="#/library/album/${encodeURIComponent(t.album_id || '')}">${esc(t.artist || '')} – ${esc(t.title || '')}</a></div>
          <div class="small" style="color:var(--warn)">${esc(t.summary || '')}</div>
          <div class="small faint">${esc((t.format || '').toUpperCase())}${t.bitrate ? ` ${t.bitrate}k` : ''}${t.album ? ` · ${esc(t.album)}` : ''}</div></div>
        <span class="row" style="gap:6px"><button class="btn sm ghost" data-check="${i}">${icon.refresh}Check again</button><button class="btn sm primary" data-replace="${i}">Replace</button></span></div>`).join('')}</div>` : '<p class="muted small" style="margin:0">Nothing flagged.</p>'}
    </div>` : ''}`;
  const again = () => render(view, ctx);
  view.querySelector('#q-import')?.addEventListener('change', async (e) => {
    try { await api.put('/admin/settings', { quality_check_on_import: e.target.checked }); toast('Saved', 'ok'); } catch (err) { toast(err.message, 'err'); }
  });
  view.onclick = async (e) => {
    const b = e.target.closest('[data-retry],[data-dismiss],[data-undo],[data-check],[data-replace]');
    if (!b) return;
    const d = b.dataset;
    if (d.replace !== undefined) {
      const t = sus.tracks[+d.replace];
      reportModal({ kind: 'track', id: t.id, title: t.title, reason: reasonFor((t.issues || []).map(x => x.code)), onDone: again });
      return;
    }
    b.disabled = true;
    try {
      if (d.retry) { await api.post(`/reports/${d.retry}/retry`); toast('Looking for a replacement', 'ok'); }
      else if (d.dismiss) { await api.post(`/reports/${d.dismiss}/dismiss`); toast('Dismissed', 'ok'); }
      else if (d.undo) { await api.del(`/blocks/${d.undo}`); toast('Unblocked – that source may be used again', 'ok'); }
      else if (d.check !== undefined) {
        const q = await api.post(`/quality/tracks/${sus.tracks[+d.check].id}/check`);
        toast(q && q.status === 'suspect' ? q.summary : 'Looks fine now', q && q.status === 'suspect' ? 'err' : 'ok');
      }
      await again();
    } catch (err) {
      toast(err.message, 'err');
      b.disabled = false;
    }
  };
}
