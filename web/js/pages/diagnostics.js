import { api } from '../api.js';
import { confirmDialog, copyText, esc, fmt, icon, toast } from '../ui.js';

const KIND = { service: 'Connections', path: 'Folders & permissions', tool: 'Tools in the container' };

export async function render(view) {
  view.innerHTML = `
    <div class="row wrap between" style="margin-bottom:18px">
      <div><h1>Diagnostics</h1><div class="small muted">Runs every connection, folder and tool check from inside the Encore container and shows the raw answers. Keys are hidden.</div></div>
      <div class="row"><button class="btn" id="copy" disabled>${icon.copy}Copy report</button><button class="btn primary" id="run">${icon.refresh}Run checks</button></div>
    </div>
    <div id="diag"><div class="empty"><span class="spinner"></span></div></div>
    <div class="section-head"><h2>Backups</h2>
      <div class="row"><label class="btn sm">${icon.upload || icon.plus}Upload a backup…<input type="file" id="bk-upload" accept=".db,.sqlite,.sqlite3,application/vnd.sqlite3,application/x-sqlite3" hidden></label><button class="btn sm primary" id="bk-now">Back up now</button></div>
    </div>
    <div class="card flat">
      <div class="small muted" style="margin-bottom:10px">Encore keeps copies of its database here (daily, and before every upgrade). Download one now and then to keep it somewhere else.</div>
      <div id="backups"><div class="empty"><span class="spinner"></span></div></div>
    </div>`;
  const box = view.querySelector('#diag');
  const bk = view.querySelector('#backups');
  const drawBackups = (d) => {
    bk.innerHTML = d.backups.length ? `<div class="table-wrap"><table class="table"><thead><tr><th>Name</th><th>Size</th><th>Date</th><th>Kind</th><th></th></tr></thead><tbody>
      ${d.backups.map(b => `<tr><td class="mono small">${esc(b.name)}</td><td class="small">${fmt.size(b.size)}</td><td class="small">${fmt.date(b.time)}</td><td><span class="badge">${esc(b.kind)}</span></td>
        <td><div class="row" style="justify-content:flex-end;gap:6px"><a class="btn sm" href="/api/system/backups/${encodeURIComponent(b.name)}" download="${esc(b.name)}">${icon.download}Download</a><button class="btn sm" data-restore="${esc(b.name)}">Restore…</button></div></td></tr>`).join('')}
    </tbody></table></div>` : '<div class="empty muted small">No backups yet – the first one is taken a day after the first start, or press Back up now.</div>';
    bk.querySelectorAll('[data-restore]').forEach(b => b.onclick = async () => {
      const name = b.dataset.restore;
      const ok = await confirmDialog('Restore this backup?', `Encore replaces its current database with ${name}. A safety copy of the current data is taken first, then the server reopens the database: everything saved since that backup (library index, requests, playlists, settings and sign-ins) is replaced, and this page reloads – you may have to sign in again.`, 'Restore', true);
      if (!ok) return;
      bk.innerHTML = '<div class="empty"><span class="spinner"></span><p class="muted">Restoring – the server reopens its database, this page reloads in a moment…</p></div>';
      try {
        await api.post(`/system/backups/${encodeURIComponent(name)}/restore`);
      } catch (e) {
        toast(e.message, 'err');
        return loadBackups();
      }
      toast('Backup restored', 'ok');
      setTimeout(() => location.reload(), 2500);
    });
  };
  const loadBackups = async () => {
    try { drawBackups(await api.get('/system/backups')); } catch (e) { bk.innerHTML = `<div class="empty muted small">${esc(e.message)}</div>`; }
  };
  view.querySelector('#bk-now').onclick = async (e) => {
    e.target.disabled = true;
    try {
      const d = await api.post('/system/backups');
      toast(`Backed up to backups/${d.created.name}`, 'ok');
      drawBackups(d);
    } catch (err) { toast(err.message, 'err'); }
    e.target.disabled = false;
  };
  view.querySelector('#bk-upload').addEventListener('change', async (e) => {
    const f = e.target.files[0];
    if (!f) return;
    e.target.value = '';
    bk.innerHTML = '<div class="empty"><span class="spinner"></span><p class="muted">Uploading and checking…</p></div>';
    const fd = new FormData();
    fd.append('file', f);
    const r = await fetch('/api/system/backups/upload', { method: 'POST', body: fd, headers: { 'X-Requested-With': 'encore' }, credentials: 'same-origin' });
    if (!r.ok) { toast((await r.json().catch(() => ({}))).detail || 'Upload failed', 'err'); return loadBackups(); }
    const d = await r.json();
    toast(`Saved as backups/${d.created.name}`, 'ok');
    drawBackups(d);
  });
  loadBackups();
  let last = null;
  const run = async () => {
    box.innerHTML = '<div class="empty"><span class="spinner"></span><p class="muted">Checking…</p></div>';
    last = await api.get('/system/diagnostics');
    view.querySelector('#copy').disabled = false;
    const groups = {};
    last.checks.forEach(c => (groups[c.kind] ||= []).push(c));
    const bad = last.checks.filter(c => !c.ok).length;
    box.innerHTML = `
      <div class="card row" style="margin-bottom:16px">
        <span class="badge ${bad ? 'err' : 'ok'}">${bad ? `${bad} problem${bad > 1 ? 's' : ''}` : 'All good'}</span>
        <span class="muted small">Encore ${esc(last.version)} · running as uid ${last.uid} · ${fmt.date(last.time)}</span>
      </div>
      ${Object.entries(KIND).map(([k, title]) => groups[k] ? `
        <div class="section-head"><h2>${title}</h2></div>
        <div class="card flat"><div class="list">${groups[k].map(c => `
          <div class="list-item" style="align-items:flex-start">
            <span class="dot ${c.ok ? 'ok' : 'err'}" style="margin-top:7px"></span>
            <div class="grow">
              <div class="row wrap"><b>${esc(c.name)}</b>${c.status ? `<span class="badge ${c.ok ? 'ok' : 'err'}">HTTP ${c.status}</span>` : ''}${c.ms != null ? `<span class="small faint">${c.ms} ms</span>` : ''}</div>
              <div class="small mono faint ellipsis">${esc(c.request || '')}</div>
              <div class="small" style="color:${c.ok ? 'var(--muted)' : 'var(--err)'}">${esc(c.detail || '')}</div>
              ${c.body ? `<details><summary class="small muted" style="cursor:pointer">Raw response</summary><div class="code diag-raw">${esc(c.body)}</div></details>` : ''}
            </div>
          </div>`).join('')}</div></div>` : '').join('')}
      <div class="section-head"><h2>Where settings come from</h2><span class="small muted">Environment variables always win over values saved in Encore</span></div>
      <div class="card flat"><div class="table-wrap"><table class="table"><thead><tr><th>Setting</th><th>Source</th><th>Value</th></tr></thead><tbody>
        ${last.settings.map(s => `<tr><td class="mono small">${esc(s.key)}</td>
          <td>${s.source === 'env' ? `<span class="badge info">env ${esc(s.env)}</span>` : '<span class="badge">saved in Encore</span>'}</td>
          <td class="small mono ellipsis" style="max-width:360px">${esc(String(s.value))}</td></tr>`).join('')}
      </tbody></table></div></div>`;
  };
  view.querySelector('#run').onclick = run;
  view.querySelector('#copy').onclick = () => {
    const lines = [`Encore ${last.version} diagnostics (uid ${last.uid})`];
    last.checks.forEach(c => lines.push(`${c.ok ? 'OK  ' : 'FAIL'} ${c.name} – ${c.request || ''}${c.status ? ` → HTTP ${c.status}` : ''}${c.ms != null ? ` (${c.ms} ms)` : ''}\n     ${c.detail || ''}${c.body && !c.ok ? `\n     body: ${c.body.slice(0, 300)}` : ''}`));
    lines.push('', 'Settings:');
    last.settings.forEach(s => lines.push(`  ${s.key} = ${s.value}  [${s.source === 'env' ? 'env ' + s.env : 'saved'}]`));
    copyText(lines.join('\n'));
  };
  await run();
}
