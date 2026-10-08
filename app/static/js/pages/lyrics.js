// Lyrics for one song (shown, fetched and saved as .lrc/.txt next to the file) or a whole album.
import { api } from '../api.js';
import { esc, modal, toast } from '../ui.js';

export async function lyricsModal(t) {
  const m = modal({ title: `Lyrics · ${esc(t.title)}`, body: '<div class="muted"><span class="spinner"></span> Looking for lyrics…</div>',
    footer: '<button class="btn" id="ly-refetch">Search again</button><button class="btn primary" data-close>Close</button>' });
  const show = async (force) => {
    const body = m.el.querySelector('.modal-body');
    try {
      const r = await api.get(`/library/tracks/${t.id}/lyrics${force ? '?force=true' : ''}`);
      const text = r.synced ? r.lyrics.replace(/\[[^\]]*\]/g, '').replace(/\n{3,}/g, '\n\n').trim() : r.lyrics;
      body.innerHTML = r.instrumental ? '<div class="muted">Instrumental – no lyrics.</div>' : text
        ? `<div class="small faint" style="margin-bottom:10px">${r.synced ? 'Time-synced' : 'Plain'} lyrics · ${r.source === 'file' ? 'saved next to the song' : 'from LRCLIB, saved next to the song'}</div><div style="white-space:pre-wrap;line-height:1.6">${esc(text)}</div>`
        : '<div class="muted">No lyrics found on LRCLIB.</div>';
    } catch (e) { body.innerHTML = `<div class="error-box">${esc(e.message)}</div>`; }
  };
  m.el.querySelector('#ly-refetch').onclick = () => show(true);
  show(false);
}

export async function fetchAlbumLyrics(albumId) {
  toast('Looking for lyrics…');
  try {
    const r = await api.post(`/library/albums/${albumId}/lyrics`);
    toast(r.checked ? `Lyrics found for ${r.found} of ${r.checked} song(s)` : 'Every song already has lyrics', 'ok');
  } catch (e) { toast(e.message, 'err'); }
}
