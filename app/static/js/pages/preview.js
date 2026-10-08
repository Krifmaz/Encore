// 30-second previews of songs and albums that aren't in the library – hear them before requesting.
import { api } from '../api.js';
import * as player from '../player.js';
import { toast } from '../ui.js';

export async function previewTrack(t) {
  try {
    const p = await api.get(`/discover/preview?artist=${encodeURIComponent(t.artist)}&title=${encodeURIComponent(t.title)}${t.isrc ? '&isrc=' + encodeURIComponent(t.isrc) : ''}${t.length ? '&length=' + t.length : ''}`);
    player.preview({ title: t.title, artist: t.artist, album: t.album || '', cover: t.cover || p.cover, preview_url: p.preview_url });
  } catch (e) { toast(/404|No preview/.test(e.message) ? 'No preview available for this song' : e.message, 'err'); }
}

/** key = "dz:<deezer album id>", or pass artist + title. */
export async function previewAlbum(key, artist = '', title = '') {
  try {
    const d = await api.get(`/discover/preview-album?${key ? 'key=' + encodeURIComponent(key) : `artist=${encodeURIComponent(artist)}&title=${encodeURIComponent(title)}`}`);
    if (!d.tracks.length) return toast('No previews available for this album', 'err');
    player.playQueue(d.tracks.map(t => ({ ...t, preview: true, id: `pv-${t.preview_url}` })), 0, { context: 'preview' });
    toast(`Previewing ${d.album || title} – 30 s per song`, 'ok');
  } catch (e) { toast(e.message, 'err'); }
}
