import { api } from '../api.js';
import { art, esc, fmt, icon, toast } from '../ui.js';
import { bindRequestActions, requestRow, submitRequest } from './common.js';
import { importModal } from './playlists.js';

// links that only describe tracks (no downloadable audio) → resolved track-by-track
const METADATA_ONLY = /open\.spotify\.com|spotify\.link|^spotify:|music\.apple\.com|listenbrainz\.org/i;
// YouTube Music playlists are best matched track-by-track too (so albums can be upgraded to lossless)
const PREFER_IMPORT = /music\.youtube\.com\/(playlist|browse)/i;
const STREAMRIP = /qobuz\.com|tidal\.com|deezer\.com|deezer\.page\.link/i;

export async function render(view, ctx) {
  view.innerHTML = `
    <h1>Downloader</h1>
    <p class="muted" style="margin:6px 0 20px">Paste any link. <b>YouTube Music, Spotify, Apple Music, Deezer, Tidal and Qobuz</b> playlists/albums are matched track-by-track (exact ISRC when available) and each song is fetched from the best source: Qobuz/Tidal/Deezer lossless → Soulseek → YouTube.
      <b>YouTube, SoundCloud, Bandcamp, Mixcloud</b> and ~1,800 other sites download directly. Every file is fingerprint-verified and de-duplicated before it hits the library.</p>
    <form class="card stack" id="dl-form">
      <div class="row wrap">
        <div class="search grow">${icon.link}<input class="input" name="url" placeholder="https://…" required autocomplete="off"></div>
        <button class="btn" type="button" id="probe">Preview</button>
      </div>
      <div class="row wrap">
        <span class="small muted">If a song is on an album:</span>
        <div class="tabs" id="album-mode">
          <button type="button" data-m="song">Just the song</button>
          <button type="button" data-m="album">Get the whole album</button>
          <button type="button" data-m="">Admin default</button>
        </div>
      </div>
      <div class="row wrap" id="dl-actions">
        <button class="btn primary" type="button" id="get-songs" title="Every song behind the link is fetched from the best source (lossless first)">${icon.download}Get the song(s)</button>
        <button class="btn" type="button" id="as-playlist">${icon.list}Import as playlist</button>
        <button class="btn" type="submit" id="direct">${icon.link}Download link as-is</button>
      </div>
      <div id="probe-out"></div>
    </form>
    <div class="section-head"><h2>Recent downloads</h2></div>
    <div class="card flat" id="dl-list"></div>`;
  const form = view.querySelector('#dl-form');
  const out = view.querySelector('#probe-out');
  const urlIn = form.querySelector('input[name=url]');
  const direct = view.querySelector('#direct');
  const update = () => {
    const u = urlIn.value.trim();
    direct.disabled = METADATA_ONLY.test(u);
    direct.title = direct.disabled ? 'This service has no downloadable audio – use “Import as playlist”' : STREAMRIP.test(u) ? 'Rip this exact release with streamrip' : '';
  };
  urlIn.addEventListener('input', update);
  void PREFER_IMPORT;

  let albumMode = 'song';
  try { albumMode = localStorage.getItem('encore.albumMode') ?? 'song'; } catch { /* ignore */ }
  const paintMode = () => view.querySelectorAll('#album-mode button').forEach(b => b.classList.toggle('active', b.dataset.m === albumMode));
  paintMode();
  view.querySelector('#album-mode').onclick = (e) => {
    const b = e.target.closest('[data-m]');
    if (!b) return;
    albumMode = b.dataset.m;
    try { localStorage.setItem('encore.albumMode', albumMode); } catch { /* ignore */ }
    paintMode();
  };

  view.querySelector('#get-songs').onclick = async (e) => {
    const url = urlIn.value.trim();
    if (!url) return urlIn.focus();
    const btn = e.currentTarget;
    btn.disabled = true;
    btn.innerHTML = '<span class="spinner"></span> Looking up…';
    try {
      const r = await api.post('/requests/from-link', { url, album_mode: albumMode || null });
      toast(`${r.requested} song(s) requested${r.already_requested ? `, ${r.already_requested} already requested` : ''}${albumMode === 'album' ? ' – whole albums will be fetched' : ''}`, 'ok');
      urlIn.value = '';
      out.innerHTML = '';
      window.dispatchEvent(new CustomEvent('encore:requests-changed'));
      load();
    } catch (err) { toast(err.message, 'err'); }
    btn.disabled = false;
    btn.innerHTML = `${icon.download}Get the song(s)`;
  };

  view.querySelector('#as-playlist').onclick = () => {
    const url = urlIn.value.trim();
    if (!url) return urlIn.focus();
    importModal(ctx, { url });
    setTimeout(() => {
      const req = document.querySelector('#imp-req');
      if (req) req.checked = true;
      const sel = document.querySelector('#imp-album');
      if (sel) sel.value = albumMode;
      document.querySelector('#imp-preview')?.click();
    }, 50);
  };

  view.querySelector('#probe').onclick = async () => {
    const url = urlIn.value.trim();
    if (!url) return;
    if (METADATA_ONLY.test(url) || STREAMRIP.test(url)) return view.querySelector('#as-playlist').click();
    out.innerHTML = '<span class="spinner"></span>';
    try {
      const p = await api.get(`/discover/probe?url=${encodeURIComponent(url)}`);
      out.innerHTML = `<div class="row">
        <div class="thumb lg">${art(p.thumbnail, p.title)}</div>
        <div class="grow"><b>${esc(p.title || 'Untitled')}</b>
          <div class="small muted">${esc([p.extractor, p.uploader, p.is_playlist ? `${p.count} items` : (p.duration ? fmt.dur(p.duration) : '')].filter(Boolean).join(' · '))}</div>
        </div></div>`;
    } catch (e) { out.innerHTML = `<div class="error-box">${esc(e.message)}</div>`; }
  };

  form.onsubmit = async (e) => {
    e.preventDefault();
    const url = urlIn.value.trim();
    if (METADATA_ONLY.test(url)) return view.querySelector('#as-playlist').click();
    direct.disabled = true;
    try {
      await submitRequest({ kind: 'url', url, title: out.querySelector('b')?.textContent || null });
      urlIn.value = '';
      out.innerHTML = '';
      load();
    } catch (err) { toast(err.message, 'err'); }
    direct.disabled = false;
  };

  const list = view.querySelector('#dl-list');
  const load = async () => {
    const rows = (await api.get('/requests?mine=true&limit=100')).filter(r => r.kind === 'url' || r.kind === 'track' || r.kind === 'album').slice(0, 30);
    list.innerHTML = rows.length ? `<div class="list">${rows.map(r => requestRow(r)).join('')}</div>` : '<div class="empty muted">Nothing yet – paste a link above.</div>';
  };
  bindRequestActions(list, load);
  await load();
  const t = setInterval(load, 4000);
  ctx.onLeave(() => clearInterval(t));
}
