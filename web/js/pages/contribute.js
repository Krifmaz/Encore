// Settings → Contribute: send AcoustID fingerprints and MusicBrainz ISRCs for songs the audio confirmed,
// and how albums missing from MusicBrainz get added. The same page in web-rs/src/pages/contribute.rs.
import { api } from '../api.js';
import { esc, fmt, icon, toast } from '../ui.js';

const MASK = '••••';

function secretInput(key, label, v, meta, help) {
  const locked = (meta[key] || {}).source === 'env';
  return `<label class="field"><span>${esc(label)}</span><input class="input" type="password" data-k="${key}" data-secret="1" value="" ${locked ? 'disabled' : ''}
    autocomplete="new-password" placeholder="${locked ? 'set by environment' : v ? 'saved – paste a new value to replace' : 'not set'}">${help ? `<small>${help}</small>` : ''}</label>`;
}

function textInput(key, label, v, meta, help) {
  const locked = (meta[key] || {}).source === 'env';
  return `<label class="field"><span>${esc(label)}</span><input class="input" type="text" data-k="${key}" value="${esc(v ?? '')}" autocomplete="off" ${locked ? 'disabled' : ''}>${help ? `<small>${help}</small>` : ''}</label>`;
}

function toggle(key, label, v, meta) {
  const locked = (meta[key] || {}).source === 'env';
  return `<div class="field" style="justify-content:center"><label class="switch"><input type="checkbox" data-k="${key}" ${v ? 'checked' : ''} ${locked ? 'disabled' : ''}><i></i>${esc(label)}</label></div>`;
}

function stat(n, label) {
  return `<div class="stat"><b>${fmt.num(n)}</b><span>${esc(label)}</span></div>`;
}

function lastLine(l) {
  if (!l || !l.at) return '';
  return `<div class="small ${l.ok ? 'muted' : ''}" style="${l.ok ? '' : 'color:var(--err)'}">Last run ${esc(fmt.ago(l.at))}: ${esc(l.message || '')}</div>`;
}

export async function render(view, ctx) {
  const [values, d] = await Promise.all([api.get('/admin/settings'), api.get('/contribute')]);
  const meta = values._meta || {};
  const a = d.acoustid, m = d.isrc;
  const flash = ctx?.query?.error ? `<div class="test-result err">${icon.x}<span>${esc(ctx.query.error)}</span></div>`
    : ctx?.query?.mb ? `<div class="test-result ok">${icon.check}<span>MusicBrainz account connected</span></div>` : '';
  view.innerHTML = `
    <div class="card stack" style="margin-bottom:16px">
      <h2>Contribute</h2>
      <p class="muted small" style="margin:0">Encore leans on AcoustID and MusicBrainz to know what a file is. Here it can give back: fingerprints of songs whose recording the audio confirmed, ISRCs MusicBrainz is missing, and albums that are only on Deezer or Apple Music. Only songs Encore is sure about are sent; nothing is sent until you add your own keys.</p>
      ${flash}
    </div>
    <div class="card stack" data-part="acoustid" style="margin-bottom:16px">
      <div class="row between wrap" style="gap:8px"><h2>AcoustID fingerprints</h2><button class="btn sm" data-run="acoustid" ${a.ready ? '' : 'disabled'}>${icon.refresh}Send now</button></div>
      <p class="muted small" style="margin:0">When a download's audio confirms the MusicBrainz recording it was asked for, its fingerprint is sent to AcoustID in batches, once per song. Your user key says who sent it; the API key under Apps &amp; sources → Verification is Encore's application key.</p>
      <div class="stats">${stat(a.queued, 'Waiting to send')}${stat(a.submitted, 'Sent, being processed')}${stat(a.imported, 'In AcoustID')}${stat(a.failed, 'Failed')}</div>
      ${a.why_not && a.enabled ? `<div class="small muted">${esc(a.why_not)}${a.can_fingerprint ? '' : '. fpcalc is not installed on this server'}</div>` : ''}
      ${lastLine(a.last)}
      <div class="form-grid">
        ${toggle('contribute_acoustid', 'Send fingerprints of confirmed songs to AcoustID', values.contribute_acoustid, meta)}
        ${secretInput('acoustid_user_key', 'AcoustID user key', values.acoustid_user_key, meta, 'Sign in at <a href="https://acoustid.org/new-application" target="_blank" rel="noopener">acoustid.org</a> and copy the user API key shown on your profile. It is not the application key: that one identifies Encore, this one identifies you.')}
      </div>
      <div class="row" style="justify-content:flex-end"><button class="btn primary" data-save="acoustid">${icon.check}Save</button></div>
    </div>
    <div class="card stack" data-part="isrc" style="margin-bottom:16px">
      <div class="row between wrap" style="gap:8px"><h2>MusicBrainz ISRCs</h2>
        <span class="row" style="gap:8px">${m.connected ? `<button class="btn sm" data-run="isrc">${icon.refresh}Send now</button><button class="btn sm" data-mb="disconnect">Disconnect</button>` : `<button class="btn sm" data-mb="connect" ${m.has_client ? '' : 'disabled'}>${icon.link}Connect MusicBrainz</button>`}</span></div>
      <p class="muted small" style="margin:0">Songs from Deezer, Tidal and Qobuz carry the ISRC of the recording. When the audio confirmed the MusicBrainz recording and MusicBrainz does not list that ISRC yet, Encore adds it under your MusicBrainz account. ISRCs that MusicBrainz has on another recording are left for a person to look at.</p>
      <div class="small">${m.connected ? `Connected as <b>${esc(m.account || 'your account')}</b>` : 'Not connected'}</div>
      <div class="stats">${stat(m.queued, 'Waiting to send')}${stat(m.submitted, 'Added to MusicBrainz')}${stat(m.skipped, 'Skipped')}${stat(m.failed, 'Failed')}</div>
      ${lastLine(m.last)}
      <div class="form-grid">
        ${toggle('contribute_isrc', 'Add missing ISRCs to MusicBrainz', values.contribute_isrc, meta)}
        ${textInput('mb_client_id', 'MusicBrainz application client ID', values.mb_client_id, meta, `Register an application at <a href="https://musicbrainz.org/account/applications" target="_blank" rel="noopener">musicbrainz.org/account/applications</a> (type: web application) with the callback URL ${m.redirect_uri ? `<span class="mono">${esc(m.redirect_uri)}</span>` : '&lt;Public URL&gt;/api/contribute/mb/callback (set the Public URL under General first)'}.`)}
        ${secretInput('mb_client_secret', 'MusicBrainz application client secret', values.mb_client_secret, meta, '')}
      </div>
      <div class="row" style="justify-content:flex-end"><button class="btn primary" data-save="isrc">${icon.check}Save</button></div>
    </div>
    <div class="card stack">
      <h2>Albums missing from MusicBrainz</h2>
      <p class="muted small" style="margin:0">Artist pages list albums found on Deezer and Apple Music that MusicBrainz does not have. Their Add to MusicBrainz link opens the MusicBrainz release editor with the title, artists, date, label, barcode, track list and store links filled in. Check it, then enter the edit under your own MusicBrainz account. Once the album is in MusicBrainz, Encore re-tags your copy on its own.</p>
    </div>`;

  const save = async (part) => {
    const body = {};
    view.querySelector(`[data-part="${part}"]`).querySelectorAll('[data-k]').forEach(el => {
      if (el.disabled) return;
      if (el.type === 'checkbox') body[el.dataset.k] = el.checked;
      else if (el.dataset.secret && (el.value.trim() === '' || el.value.startsWith(MASK))) { /* keep the saved secret */ }
      else body[el.dataset.k] = el.value.trim();
    });
    await api.put('/admin/settings', body);
  };
  view.querySelectorAll('[data-save]').forEach(b => b.onclick = async () => {
    try { await save(b.dataset.save); toast('Settings saved', 'ok'); render(view, { ...ctx, query: {} }); } catch (e) { toast(e.message, 'err'); }
  });
  view.querySelectorAll('[data-run]').forEach(b => b.onclick = async () => {
    b.disabled = true;
    try { const r = await api.post(`/contribute/${b.dataset.run}/run`); toast(r.message || 'Done', 'ok'); } catch (e) { toast(e.message, 'err'); }
    render(view, { ...ctx, query: {} });
  });
  view.querySelectorAll('[data-mb]').forEach(b => b.onclick = async () => {
    b.disabled = true;
    try {
      if (b.dataset.mb === 'connect') {
        await save('isrc');
        const r = await api.post('/contribute/mb/connect');
        location.href = r.url;
        return;
      }
      await api.post('/contribute/mb/disconnect');
      toast('Disconnected', 'ok');
    } catch (e) { toast(e.message, 'err'); }
    render(view, { ...ctx, query: {} });
  });
}
