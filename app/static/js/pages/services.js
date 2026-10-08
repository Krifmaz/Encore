// Admin → Built-in services: slskd, Lidarr, Prowlarr, qBittorrent and SABnzbd running inside Encore (all-in-one image).
import { api } from '../api.js';
import { esc, icon, modal, toast } from '../ui.js';

const up = (s) => s < 90 ? `${Math.round(s)} s` : s < 5400 ? `${Math.round(s / 60)} min` : s < 172800 ? `${Math.round(s / 3600)} h` : `${Math.round(s / 86400)} days`;
const NEXT = {
  slskd: 'Enter your Soulseek login below.',
  prowlarr: 'Add your indexers (torrent trackers / Usenet indexers) – they sync to Lidarr automatically.',
  sabnzbd: 'Run the wizard once and add your Usenet server.',
  qbittorrent: 'Ready – Lidarr sends torrents here.',
  lidarr: 'Ready – Encore uses it for album monitoring.',
};

export async function render(view) {
  view.innerHTML = '<h1>Built-in services</h1><div class="skeleton mt" style="height:240px"></div>';
  const d = await api.get('/admin/services');
  if (!d.enabled) {
    view.innerHTML = `<h1>Built-in services</h1><div class="card mt stack" style="max-width:760px">
      <p>Encore can run <b>slskd, Lidarr, Prowlarr, qBittorrent and SABnzbd</b> inside its own container – configured and connected to each other automatically, with their web UIs here behind your Encore login. No separate containers needed.</p>
      <p class="muted small">Use the <code>ghcr.io/krifmaz/encore:aio</code> image and add <code>ENCORE_EMBEDDED=all</code> (or a list such as <code>slskd,lidarr,prowlarr,qbittorrent</code>) to your compose file. See the README for a VPN (gluetun) example.</p></div>`;
    return;
  }
  view.innerHTML = `<div class="row between wrap"><div><h1>Built-in services</h1><p class="muted" style="margin:6px 0 0">Running inside Encore and connected automatically. Web UIs open behind your admin login.</p></div>
      <button class="btn" id="rewire">${icon.refresh}Re-run setup</button></div>
    <div class="list card flat mt">${d.services.filter(s => s.enabled).map(s => `<div class="list-item" style="flex-wrap:wrap;gap:12px" data-k="${s.key}">
      <span class="dot ${s.status === 'running' ? 'ok' : s.status === 'starting' || s.status === 'restarting' ? 'warn' : 'err'}"></span>
      <div class="grow" style="min-width:220px"><div style="font-weight:650">${esc(s.name)} <span class="muted small">· ${esc(s.what)}</span></div>
        <div class="small muted">${esc(s.status)}${s.uptime ? ` · up ${up(s.uptime)}` : ''}${s.restarts ? ` · ${s.restarts} restart(s)` : ''}${s.error ? ` · <span style="color:var(--err)">${esc(s.error)}</span>` : ''}</div>
        <div class="small faint">${esc(NEXT[s.key] || '')}</div></div>
      <div class="row" style="gap:6px">
        ${s.installed ? `<a class="btn sm primary" href="${esc(s.ui)}" target="_blank" rel="noopener">${icon.external}Open</a>` : ''}
        <button class="btn sm" data-logs>Logs</button>
        ${s.installed ? `<button class="btn sm" data-restart>${icon.refresh}Restart</button>` : ''}</div>
    </div>`).join('')}</div>
    ${d.services.some(s => s.key === 'slskd' && s.enabled) ? `<form class="card stack mt" id="slsk" style="max-width:560px"><h3>Soulseek login</h3>
      <p class="small muted" style="margin:0">Any username works – Soulseek creates the account on first login. Saving restarts slskd.</p>
      <div class="row wrap"><input class="input grow" name="username" placeholder="Soulseek username" value="${esc(d.soulseek.username || '')}" ${d.soulseek.locked ? 'disabled' : ''} autocomplete="off">
        <input class="input grow" type="password" name="password" placeholder="${d.soulseek.password_set ? '•••••••• (unchanged)' : 'Password'}" ${d.soulseek.locked ? 'disabled' : ''} autocomplete="new-password">
        <button class="btn primary" ${d.soulseek.locked ? 'disabled title="Set by SOULSEEK_USERNAME / SOULSEEK_PASSWORD"' : ''}>Save</button></div></form>` : ''}`;
  view.querySelector('#rewire').onclick = async (e) => {
    e.currentTarget.disabled = true;
    try { const r = await api.post('/admin/services/wire'); toast(Object.keys(r.done).length ? `Set up: ${Object.keys(r.done).join(', ')}` : 'Everything is already connected', 'ok'); } catch (err) { toast(err.message, 'err'); }
    e.currentTarget.disabled = false;
  };
  view.querySelectorAll('[data-k]').forEach(row => {
    const k = row.dataset.k;
    row.querySelector('[data-restart]')?.addEventListener('click', async () => { await api.post(`/admin/services/${k}/restart`); toast('Restarting…', 'ok'); setTimeout(() => render(view), 3000); });
    row.querySelector('[data-logs]').addEventListener('click', async () => {
      const r = await api.get(`/admin/services/${k}/logs`);
      modal({ title: `${esc(k)} log`, wide: true, body: `<pre class="small mono" style="white-space:pre-wrap;max-height:60vh;overflow:auto;margin:0">${esc(r.log || '(empty)')}</pre>` });
    });
  });
  view.querySelector('#slsk')?.addEventListener('submit', async (e) => {
    e.preventDefault();
    const f = Object.fromEntries(new FormData(e.target));
    try { await api.put('/admin/services/soulseek', f); toast('Saved – slskd is restarting', 'ok'); setTimeout(() => render(view), 4000); } catch (err) { toast(err.message, 'err'); }
  });
}
