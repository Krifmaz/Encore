// Storage at a glance: disk, library breakdown, biggest artists/albums, download folders.
import { api } from '../api.js';
import { esc, fmt } from '../ui.js';

const bar = (part, whole, color = 'var(--accent)') => `<div class="progress" style="height:8px"><i style="width:${whole ? Math.min(100, part / whole * 100) : 0}%;background:${color}"></i></div>`;

export async function render(view) {
  view.innerHTML = '<h1>Storage</h1><div class="skeleton mt" style="height:200px"></div>';
  const s = await api.get('/library/storage');
  const maxA = Math.max(1, ...s.artists.map(a => a.size));
  const maxF = Math.max(1, ...s.formats.map(f => f.size));
  view.innerHTML = `
    <h1>Storage</h1>
    ${s.disk ? `<div class="card mt"><div class="row between wrap"><b>Disk</b><span class="muted small">${fmt.size(s.disk.used)} used · ${fmt.size(s.disk.free)} free of ${fmt.size(s.disk.total)}</span></div>
      <div class="progress mt" style="height:14px;display:flex"><i style="width:${s.library.size / s.disk.total * 100}%;background:var(--accent)" title="Music"></i><i style="width:${Math.max(0, s.disk.used - s.library.size) / s.disk.total * 100}%;background:var(--muted);opacity:.5" title="Other files"></i></div>
      <div class="row small muted mt" style="gap:16px"><span>■ <b style="color:var(--accent)">Music</b> ${fmt.size(s.library.size)}</span><span>■ Other ${fmt.size(Math.max(0, s.disk.used - s.library.size))}</span><span>□ Free ${fmt.size(s.disk.free)}</span></div></div>` : ''}
    <div class="stats mt">
      <div class="stat"><b>${fmt.size(s.library.size)}</b><span>Library</span></div>
      <div class="stat"><b>${fmt.num(s.library.albums)}</b><span>Albums</span></div>
      <div class="stat"><b>${fmt.num(s.library.tracks)}</b><span>Songs</span></div>
      ${s.downloads.map(d => `<div class="stat"><b>${fmt.size(d.size)}</b><span>${d.name === 'soulseek' ? 'Soulseek downloads folder' : 'Staging (in progress)'}</span></div>`).join('')}
    </div>
    <div class="row wrap gap-lg mt" style="align-items:flex-start">
      <div class="card stack" style="flex:1;min-width:280px"><h2>By format</h2>
        ${s.formats.map(f => `<div><div class="row between small"><b>${esc(f.format || '?')}</b><span class="muted">${fmt.num(f.tracks)} songs · ${fmt.size(f.size)}</span></div>${bar(f.size, maxF)}</div>`).join('')}</div>
      <div class="card stack" style="flex:1;min-width:280px"><h2>Biggest artists</h2>
        ${s.artists.map(a => `<div><div class="row between small"><a href="#/artist-name/${encodeURIComponent(a.name)}"><b>${esc(a.name)}</b></a><span class="muted">${fmt.size(a.size)}</span></div>${bar(a.size, maxA, 'var(--accent-2)')}</div>`).join('')}</div>
    </div>
    <div class="section-head"><h2>Biggest albums</h2></div>
    <div class="card flat"><div class="list">${s.albums.map(a => `<a class="list-item" href="#/library/album/${esc(a.id)}">
      <div class="grow"><div style="font-weight:600" class="ellipsis">${esc(a.album || 'Unknown album')}</div><div class="small muted">${esc(a.albumartist)} · ${a.tracks} songs</div></div><b>${fmt.size(a.size)}</b></a>`).join('')}</div></div>`;
}

/** Tiny meter in the sidebar; refreshes every few minutes. */
export async function sidebarMeter(el) {
  try {
    const s = await api.get('/library/storage');
    if (!s.disk) { el.innerHTML = `<a href="#/storage" class="storage-mini"><span>Library ${fmt.size(s.library.size)}</span></a>`; return; }
    const pct = s.disk.used / s.disk.total * 100;
    el.innerHTML = `<a href="#/storage" class="storage-mini" title="Storage">
      <div class="row between small"><span>Storage</span><span class="faint">${fmt.size(s.disk.free)} free</span></div>
      <div class="progress" style="height:6px;margin-top:6px"><i style="width:${pct}%;${pct > 90 ? 'background:var(--err)' : ''}"></i></div>
      <div class="faint" style="font-size:11px;margin-top:4px">Music ${fmt.size(s.library.size)} · ${fmt.size(s.disk.total)} disk</div></a>`;
  } catch { el.innerHTML = ''; }
}
