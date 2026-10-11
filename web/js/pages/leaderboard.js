// Leaderboard: who requests, plays and likes the most, plus fun library facts.
import { api } from '../api.js';
import { art, emptyState, esc, fmt, icon } from '../ui.js';

const MEDAL = ['🥇', '🥈', '🥉'];

export async function render(view) {
  view.innerHTML = '<h1>Leaderboard</h1><div class="skeleton mt" style="height:240px"></div>';
  const d = await api.get('/stats/leaderboard');
  const fmtV = (k, v) => k === 'listened' ? fmt.longDur(v) : fmt.num(v);
  const board = (k, b) => `<div class="card stack" style="flex:1;min-width:260px"><h2>${esc(b.label)}</h2>
    ${b.rows.length ? b.rows.map((r, i) => `<div class="row between" style="${r.me ? 'color:var(--accent);font-weight:700' : ''}">
      <span>${MEDAL[i] || `<span class="faint" style="display:inline-block;width:22px">${i + 1}.</span>`} ${esc(r.username)}${r.me ? ' (you)' : ''}</span><b>${fmtV(k, r.value)}</b></div>`).join('')
      : '<div class="muted small">Nobody yet – be the first!</div>'}</div>`;
  view.innerHTML = `
    <h1>Leaderboard</h1><p class="muted" style="margin:6px 0 20px">Plays count after 30 seconds in the web player. Bragging rights only.</p>
    <div class="row wrap gap-lg" style="align-items:flex-start">${Object.entries(d.boards).map(([k, b]) => board(k, b)).join('')}</div>
    ${d.this_week.length ? `<div class="section-head"><h2>This week's top listener</h2></div><div class="card"><b style="font-size:20px">👑 ${esc(d.this_week[0].username)}</b> <span class="muted">· ${fmt.num(d.this_week[0].plays)} songs played this week</span></div>` : ''}
    <div class="row wrap gap-lg mt" style="align-items:flex-start">
      <div class="card stack" style="flex:1;min-width:280px"><h2>Most played songs</h2>
        ${d.top_songs.length ? d.top_songs.map((s, i) => `<div class="row" style="gap:10px"><span class="faint" style="width:20px">${i + 1}</span>
          <div class="thumb" style="width:36px;height:36px">${art(s.album_id ? `/api/library/albums/${s.album_id}/cover` : null, s.title)}</div>
          <div class="grow" style="min-width:0"><div class="ellipsis" style="font-weight:600">${esc(s.title)}</div><div class="small muted ellipsis">${esc(s.artist)}</div></div><b>${fmt.num(s.plays)}</b></div>`).join('') : emptyState(icon.play, 'No plays yet')}</div>
      <div class="card stack" style="flex:1;min-width:280px"><h2>Most played artists</h2>
        ${d.top_artists.length ? d.top_artists.map((a, i) => `<div class="row between"><span>${MEDAL[i] || (i + 1) + '.'} <a href="#/artist-name/${encodeURIComponent(a.artist)}">${esc(a.artist)}</a> <span class="faint small">· ${a.listeners} listener${a.listeners === 1 ? '' : 's'}</span></span><b>${fmt.num(a.plays)}</b></div>`).join('') : '<div class="muted small">No plays yet.</div>'}</div>
      <div class="card stack" style="flex:1;min-width:280px"><h2>Biggest in the library</h2>
        ${d.library_artists.map((a, i) => `<div class="row between"><span>${MEDAL[i] || (i + 1) + '.'} <a href="#/artist-name/${encodeURIComponent(a.artist)}">${esc(a.artist)}</a></span><span class="muted small">${a.albums} albums · <b>${fmt.num(a.songs)}</b> songs</span></div>`).join('')}</div>
    </div>
    <div class="section-head"><h2>Fun facts</h2></div>
    <div class="stats">${d.facts.map(f => `<div class="stat"><b>${esc(f.value)}</b><span>${esc(f.label)}</span></div>`).join('')}</div>`;
}
