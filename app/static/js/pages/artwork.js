// Pick an album's artwork by hand: search every catalogue or upload a picture; it's embedded in the files.
import { api } from '../api.js';
import { esc, modal, toast } from '../ui.js';

export function artworkModal(albumId, a, onDone) {
  const m = modal({
    title: 'Change artwork', wide: true,
    body: `<form class="row wrap" id="aw-form"><input class="input grow" name="q" value="${esc(a.album || '')}" placeholder="Album title to search for" style="min-width:0">
        <button class="btn primary">Search</button>
        <label class="btn">Upload…<input type="file" accept="image/jpeg,image/png" id="aw-file" hidden></label></form>
      <div class="small muted mt">Cover Art Archive, Deezer, Apple Music, YouTube Music${''} and Discogs. The picture is embedded in every song and saved as cover.jpg.</div>
      <div id="aw-out" class="mt"></div>`,
  });
  const out = m.el.querySelector('#aw-out');
  const apply = async (body, el) => {
    el?.classList.add('busy');
    try {
      const r = await api.post(`/library/albums/${albumId}/art`, body);
      toast(`Artwork embedded in ${r.embedded} file(s)`, 'ok');
      m.close();
      // show the new picture everywhere on the page right away
      const v = Date.now();
      document.querySelectorAll(`img[src*="/albums/${albumId}/cover"]`).forEach(img => { img.src = img.src.replace(/[?&]v=\d+/, '') + (img.src.includes('?') ? '&' : '?') + 'v=' + v; });
      onDone?.();
    } catch (e) { toast(e.message, 'err'); el?.classList.remove('busy'); }
  };
  const search = async (q) => {
    out.innerHTML = '<div class="muted"><span class="spinner"></span> Searching every catalogue…</div>';
    try {
      const c = await api.get(`/library/albums/${albumId}/art-candidates?q=${encodeURIComponent(q)}`);
      out.innerHTML = c.length ? `<div class="grid art-grid">${c.map((x, i) => `<button class="tile art-pick" data-i="${i}" title="${esc(x.label || '')}">
          <div class="art"><img src="${esc(x.thumb)}" alt="" loading="lazy"></div>
          <div class="s ellipsis">${esc(x.source)}${x.size ? ' · ' + esc(x.size) : ''}</div><div class="s ellipsis faint">${esc(x.label || '')}</div></button>`).join('')}</div>`
        : '<div class="muted">No artwork found – try another title, or upload a picture.</div>';
      out.onclick = (e) => { const b = e.target.closest('[data-i]'); if (b) apply({ url: c[+b.dataset.i].url }, b); };
    } catch (e) { out.innerHTML = `<div class="error-box">${esc(e.message)}</div>`; }
  };
  m.el.querySelector('#aw-form').onsubmit = (e) => { e.preventDefault(); search(new FormData(e.target).get('q')); };
  m.el.querySelector('#aw-file').onchange = (e) => {
    const f = e.target.files[0];
    if (!f) return;
    if (f.size > 15 * 1024 * 1024) return toast('Picture is larger than 15 MB', 'err');
    const rd = new FileReader();
    rd.onload = () => apply({ data: rd.result });
    rd.readAsDataURL(f);
  };
  search(a.album || '');
}
