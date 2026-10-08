// Manual download picker: search every source, see quality/size/uploader, download the exact result you choose.
import { api } from '../api.js';
import { esc, fmt, icon, modal, toast } from '../ui.js';

const NAMES = { slskd: 'Soulseek', deezer: 'Deezer', ytmusic: 'YouTube Music', lidarr: 'Lidarr' };

/**
 * opts: { kind: 'album'|'track', artist, title, album, mbid, artist_mbid, track_count, length, year, cover_url, isrc, request_id }
 */
export async function pickerModal(opts) {
  let info;
  try { info = await api.get('/picker/sources'); } catch (e) { return toast(e.message, 'err'); }
  if (!info.allowed) return toast('Manual search is limited to admins', 'err');
  const kind = opts.kind === 'track' ? 'track' : 'album';
  const state = { tab: 'all', lossless: false, results: {}, errors: {}, loading: new Set() };
  const m = modal({
    title: `Pick a download · ${esc(opts.title || opts.album)}`,
    wide: true,
    body: `
      <form class="row wrap" id="pk-form">
        <div class="search grow">${icon.search}<input class="input" name="q" value="${esc([opts.artist, opts.title || opts.album].filter(Boolean).join(' '))}" autocomplete="off"></div>
        <button class="btn primary">Search</button>
      </form>
      <div class="row wrap mt" style="justify-content:space-between">
        <div class="tabs" id="pk-tabs"></div>
        <label class="row small" style="gap:6px"><input type="checkbox" id="pk-lossless"> Lossless only</label>
      </div>
      <div id="pk-out" class="mt" style="min-height:200px"></div>
      <form class="row wrap mt" id="pk-link">
        <div class="search grow">${icon.link}<input class="input" name="url" placeholder="…or paste a link: Qobuz, Tidal, Deezer, YouTube, Bandcamp, SoundCloud…" autocomplete="off"></div>
        <button class="btn">${icon.download}Download link</button>
      </form>
      <p class="small faint" style="margin:8px 0 0">The file you pick is still fingerprint-checked, tagged with MusicBrainz data and de-duplicated. If it fails, the request fails instead of trying other sources.</p>`,
  });
  const el = m.el;
  const out = el.querySelector('#pk-out');
  if (!info.sources.length) {
    out.innerHTML = '<div class="empty muted">No download sources are enabled – set them up in Settings.</div>';
  }

  const tabs = () => {
    const all = Object.values(state.results).reduce((n, r) => n + r.length, 0);
    el.querySelector('#pk-tabs').innerHTML = [['all', 'All', all], ...info.sources.map(s => [s.id, s.name, (state.results[s.id] || []).length])]
      .map(([k, l, n]) => `<button data-t="${k}" class="${state.tab === k ? 'active' : ''}">${esc(l)} ${state.loading.has(k) || (k === 'all' && state.loading.size) ? '<span class="spinner" style="width:12px;height:12px"></span>' : `<span class="faint">${n}</span>`}</button>`).join('');
  };

  const row = (x, i) => `<tr>
      <td><div style="font-weight:600;word-break:break-word">${esc(x.title)}</div>
        <div class="small muted" style="word-break:break-word">${esc(x.subtitle || '')}</div>
        ${x.warning ? `<div class="small" style="color:var(--warn)">${esc(x.warning)}</div>` : ''}</td>
      <td><span class="badge ${x.lossless ? 'ok' : ''}">${esc(x.quality || '?')}</span></td>
      <td class="hide-sm muted small">${x.tracks ? `${x.tracks} file${x.tracks === 1 ? '' : 's'}` : ''}${x.duration ? ` · ${fmt.dur(x.duration)}` : ''}${x.size ? `<br>${fmt.size(x.size)}` : ''}</td>
      <td class="hide-sm small muted">${esc(x.info || '')}</td>
      <td style="text-align:right"><button class="btn sm primary" data-pick="${esc(x.source)}:${i}">${icon.download}Get</button></td>
    </tr>`;

  const section = (src) => {
    let list = state.results[src] || [];
    if (state.lossless) list = list.filter(x => x.lossless);
    const head = state.tab === 'all' ? `<div class="section-head" style="margin:14px 0 8px"><h3>${esc(NAMES[src] || src)} <span class="faint small">${list.length}</span></h3></div>` : '';
    if (state.errors[src]) return head + `<div class="error-box small">${esc(state.errors[src])}</div>`;
    if (state.loading.has(src)) return head + `<div class="muted small"><span class="spinner"></span> Searching ${esc(NAMES[src])}…${src === 'slskd' ? ' (Soulseek takes a few seconds)' : ''}</div>`;
    if (!list.length) return head + '<div class="muted small">No results.</div>';
    const shown = state.tab === 'all' ? list.slice(0, 8) : list.slice(0, 150);
    return head + `<div class="table-wrap"><table class="table"><thead><tr><th>Result</th><th>Quality</th><th class="hide-sm">Files</th><th class="hide-sm">Details</th><th></th></tr></thead>
      <tbody>${shown.map(x => row(x, state.results[src].indexOf(x))).join('')}</tbody></table></div>
      ${state.tab === 'all' && list.length > shown.length ? `<button class="btn sm ghost mt" data-t="${src}">Show all ${list.length} from ${esc(NAMES[src])}</button>` : ''}`;
  };

  const paint = () => {
    tabs();
    if (!info.sources.length) return;
    out.innerHTML = (state.tab === 'all' ? info.sources.map(s => s.id) : [state.tab]).map(section).join('');
  };

  const run = (q) => {
    state.results = {}; state.errors = {};
    for (const s of info.sources) {
      if (s.id === 'lidarr' && kind !== 'album') continue;
      state.loading.add(s.id);
      api.post('/picker/search', { source: s.id, kind, artist: opts.artist || '', title: opts.title || opts.album || '', query: q, mbid: opts.mbid, artist_mbid: opts.artist_mbid, track_count: opts.track_count, length: opts.length })
        .then(r => { state.results[s.id] = r; })
        .catch(e => { state.errors[s.id] = e.message; })
        .finally(() => { state.loading.delete(s.id); if (el.isConnected) paint(); });
    }
    paint();
  };

  const download = async (source, payload, title, btn) => {
    btn.disabled = true;
    const req = { kind, mbid: opts.mbid || null, artist: opts.artist, title: opts.title || opts.album, album: opts.album || (kind === 'album' ? opts.title : null), year: opts.year, cover_url: opts.cover_url, length: opts.length, isrc: opts.isrc, album_mode: 'song' };
    try {
      const r = await api.post('/picker/pick', { source, payload, title, ...(opts.request_id ? { request_id: opts.request_id } : { request: req }) });
      toast(r.status === 'pending' ? 'Sent for approval – your pick will be used' : 'Downloading your pick', 'ok');
      window.dispatchEvent(new CustomEvent('encore:requests-changed'));
      m.close();
      opts.onDone?.(r);
    } catch (e) { toast(e.message, 'err'); btn.disabled = false; }
  };

  el.addEventListener('click', (e) => {
    const t = e.target.closest('[data-t]');
    if (t) { state.tab = t.dataset.t; paint(); return; }
    const b = e.target.closest('[data-pick]');
    if (b) {
      const [src, i] = b.dataset.pick.split(':');
      const x = state.results[src][+i];
      download(src, x.payload, x.title, b);
    }
  });
  el.querySelector('#pk-lossless').onchange = (e) => { state.lossless = e.target.checked; paint(); };
  el.querySelector('#pk-form').onsubmit = (e) => { e.preventDefault(); run(new FormData(e.target).get('q')); };
  el.querySelector('#pk-link').onsubmit = (e) => {
    e.preventDefault();
    const url = new FormData(e.target).get('url').trim();
    if (url) download('link', { url }, url, e.target.querySelector('button'));
  };
  run(el.querySelector('input[name=q]').value);
}
