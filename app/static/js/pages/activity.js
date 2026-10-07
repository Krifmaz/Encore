import { api } from '../api.js';
import { emptyState, esc, fmt, icon, toast } from '../ui.js';

export async function render(view, ctx) {
  view.innerHTML = `
    <h1 style="margin-bottom:18px">Activity</h1>
    <div class="section-head"><h2>Soulseek transfers</h2><button class="btn sm" id="clear-slskd">${icon.trash}Clear completed</button></div>
    <div class="card flat" id="slskd"><div class="empty"><span class="spinner"></span></div></div>
    <div class="section-head"><h2>Lidarr queue</h2></div>
    <div class="card flat" id="lidarr"></div>
    <div class="section-head"><h2>Log</h2>
      <select class="input" id="lvl" style="width:auto"><option value="">All levels</option><option value="info">Info</option><option value="warning">Warnings</option><option value="error">Errors</option></select></div>
    <div class="card flat" id="log"></div>`;
  let level = '';
  const load = async () => {
    const [d, logs] = await Promise.all([api.get('/system/downloads'), api.get('/system/activity')]);
    view.querySelector('#slskd').innerHTML = d.errors.slskd ? `<div class="error-box" style="margin:14px">${esc(d.errors.slskd)}</div>`
      : d.slskd.length ? `<div class="table-wrap"><table class="table"><thead><tr><th>File</th><th class="hide-sm">User</th><th>State</th><th style="width:180px">Progress</th></tr></thead><tbody>
        ${d.slskd.slice(0, 200).map(f => `<tr><td><div class="ellipsis" style="max-width:420px;font-weight:600">${esc(f.filename)}</div><div class="small faint ellipsis" style="max-width:420px">${esc(f.directory)} · ${fmt.size(f.size)}</div></td>
          <td class="hide-sm muted">${esc(f.username)}</td>
          <td><span class="badge ${/Succeeded/.test(f.state) ? 'ok' : /Errored|Rejected|TimedOut|Cancelled/.test(f.state) ? 'err' : 'info'}">${esc(f.state)}</span></td>
          <td><div class="progress"><i style="width:${f.progress}%"></i></div><div class="small faint">${f.progress}%${f.speed ? ' · ' + fmt.size(f.speed) + '/s' : ''}</div></td></tr>`).join('')}
        </tbody></table></div>` : emptyState(icon.download, 'No Soulseek transfers', 'slskd may not be configured.');
    view.querySelector('#lidarr').innerHTML = d.errors.lidarr ? `<div class="error-box" style="margin:14px">${esc(d.errors.lidarr)}</div>`
      : d.lidarr.length ? `<div class="list">${d.lidarr.map(q => `<div class="list-item"><div class="grow"><b>${esc(q.artist)} – ${esc(q.album)}</b>
          <div class="small muted">${esc([q.state || q.status, q.client, q.protocol, q.timeleft].filter(Boolean).join(' · '))}</div>
          ${q.messages.length ? `<div class="small" style="color:var(--warn)">${esc(q.messages.join(' · '))}</div>` : ''}
          <div class="progress" style="margin-top:6px;max-width:420px"><i style="width:${q.progress}%"></i></div></div></div>`).join('')}</div>`
      : emptyState(icon.download, 'Lidarr queue is empty');
    const rows = logs.filter(l => !level || l.level === level);
    view.querySelector('#log').innerHTML = rows.length ? `<div class="list">${rows.slice(0, 200).map(l => `<div class="list-item" style="padding:8px 16px">
      <span class="badge ${l.level === 'error' ? 'err' : l.level === 'warning' ? 'warn' : ''}" style="width:68px;justify-content:center">${esc(l.level)}</span>
      <span class="grow small">${esc(l.message)}</span><span class="small faint" title="${fmt.date(l.ts)}">${fmt.ago(l.ts)}</span></div>`).join('')}</div>` : '<div class="empty muted">No log entries</div>';
  };
  view.querySelector('#lvl').onchange = (e) => { level = e.target.value; load(); };
  view.querySelector('#clear-slskd').onclick = async () => {
    try { await api.post('/system/downloads/slskd/clear'); load(); } catch (e) { toast(e.message, 'err'); }
  };
  await load();
  const t = setInterval(load, 5000);
  ctx.onLeave(() => clearInterval(t));
}
