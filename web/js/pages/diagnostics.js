import { api } from '../api.js';
import { copyText, esc, fmt, icon } from '../ui.js';

const KIND = { service: 'Connections', path: 'Folders & permissions', tool: 'Tools in the container' };

export async function render(view) {
  view.innerHTML = `
    <div class="row wrap between" style="margin-bottom:18px">
      <div><h1>Diagnostics</h1><div class="small muted">Runs every connection, folder and tool check from inside the Encore container and shows the raw answers. Keys are hidden.</div></div>
      <div class="row"><button class="btn" id="copy" disabled>${icon.copy}Copy report</button><button class="btn primary" id="run">${icon.refresh}Run checks</button></div>
    </div>
    <div id="diag"><div class="empty"><span class="spinner"></span></div></div>`;
  const box = view.querySelector('#diag');
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
