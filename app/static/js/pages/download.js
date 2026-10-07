import { api } from '../api.js';
import { art, esc, fmt, icon, toast } from '../ui.js';
import { bindRequestActions, requestRow, submitRequest } from './common.js';

export async function render(view, ctx) {
  view.innerHTML = `
    <h1>Downloader</h1>
    <p class="muted" style="margin:6px 0 20px">Paste a link from YouTube, YouTube Music, SoundCloud, Bandcamp, Mixcloud, Audiomack, Vimeo and ~1,800 other sites – or a Spotify playlist/album to request every track.</p>
    <form class="card stack" id="dl-form">
      <div class="row wrap">
        <div class="search grow">${icon.link}<input class="input" name="url" placeholder="https://…" required autocomplete="off"></div>
        <button class="btn" type="button" id="probe">Preview</button>
        <button class="btn primary" type="submit">${icon.download}Download</button>
      </div>
      <div id="probe-out"></div>
    </form>
    <div class="section-head"><h2>Recent downloads</h2></div>
    <div class="card flat" id="dl-list"></div>`;
  const form = view.querySelector('#dl-form');
  const out = view.querySelector('#probe-out');
  const urlIn = form.querySelector('input[name=url]');
  const isSpotify = (u) => /open\.spotify\.com|^spotify:/.test(u);

  view.querySelector('#probe').onclick = async () => {
    const url = urlIn.value.trim();
    if (!url) return;
    out.innerHTML = '<span class="spinner"></span>';
    try {
      const p = await api.get(`/discover/probe?url=${encodeURIComponent(url)}`);
      out.innerHTML = `<div class="row">
        <div class="thumb lg">${art(p.thumbnail || p.tracks?.[0]?.cover, p.title)}</div>
        <div class="grow"><b>${esc(p.title || 'Untitled')}</b>
          <div class="small muted">${esc([p.source === 'spotify' ? 'Spotify' : p.extractor, p.uploader, p.is_playlist ? `${p.count} items` : (p.duration ? fmt.dur(p.duration) : '')].filter(Boolean).join(' · '))}</div>
          ${p.source === 'spotify' ? '<div class="small faint">Each track becomes a request (Soulseek first, then YouTube).</div>' : ''}
        </div></div>
        ${p.tracks ? `<div class="small muted mt" style="max-height:180px;overflow:auto">${p.tracks.slice(0, 100).map((t, i) => `${i + 1}. ${esc(t.artist)} – ${esc(t.title)}`).join('<br>')}</div>` : ''}`;
    } catch (e) { out.innerHTML = `<div class="error-box">${esc(e.message)}</div>`; }
  };

  form.onsubmit = async (e) => {
    e.preventDefault();
    const url = urlIn.value.trim();
    const btn = form.querySelector('button[type=submit]');
    btn.disabled = true;
    try {
      if (isSpotify(url)) {
        const r = await api.post('/requests/import', { url });
        toast(`Imported ${r.count} tracks as requests`, 'ok');
      } else {
        await submitRequest({ kind: 'url', url, title: out.querySelector('b')?.textContent || null });
      }
      urlIn.value = '';
      out.innerHTML = '';
      load();
    } catch (err) {
      toast(err.message, 'err');
    }
    btn.disabled = false;
  };

  const list = view.querySelector('#dl-list');
  const load = async () => {
    const rows = (await api.get('/requests?mine=true&limit=100')).filter(r => r.kind === 'url' || r.kind === 'playlist');
    list.innerHTML = rows.length ? `<div class="list">${rows.map(r => requestRow(r)).join('')}</div>` : '<div class="empty muted">No downloads yet</div>';
  };
  bindRequestActions(list, load);
  await load();
  const t = setInterval(load, 4000);
  ctx.onLeave(() => clearInterval(t));
}
