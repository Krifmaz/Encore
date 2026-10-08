import { api } from '../api.js';
import { esc, fmt, icon, toast } from '../ui.js';

export async function render(view, ctx) {
  const me = await api.get('/auth/me');
  view.innerHTML = `
    <h1 style="margin-bottom:18px">Account</h1>
    <div class="row wrap gap-lg" style="align-items:flex-start">
      <div class="card stack" style="flex:1;min-width:280px">
        <div class="row"><div class="avatar" style="width:52px;height:52px;font-size:22px">${esc(me.username[0])}</div>
          <div><h2>${esc(me.username)}</h2><div class="muted small">${me.role === 'admin' ? 'Administrator' : 'Member'} · joined ${fmt.ago(me.created_at)}</div></div></div>
        <div class="small muted">Requests this week: <b>${me.requests_this_week}</b>${me.request_limit && me.role !== 'admin' ? ` of ${me.request_limit}` : ''}
          ${me.auto_approve || me.role === 'admin' ? ' · <span class="badge ok">auto-approved</span>' : ''}</div>
      </div>
      <form class="card stack" id="pw" style="flex:1;min-width:280px">
        <h3>Change password</h3>
        <label class="field"><span>Current password</span><input class="input" type="password" name="current" autocomplete="current-password" required></label>
        <label class="field"><span>New password</span><input class="input" type="password" name="new" autocomplete="new-password" minlength="8" required></label>
        <button class="btn primary" type="submit">Update password</button>
      </form>
    </div>
    <div class="card stack mt" id="loc" style="max-width:720px"></div>
    <div class="card stack mt" style="max-width:720px"><h3>Apps</h3>
      <p class="small muted" style="margin:0">Android app and desktop apps for Windows, macOS and Linux: <a href="https://github.com/Krifmaz/Encore/releases" target="_blank" rel="noopener">download from the Releases page</a>. On a phone you can also use your browser's <b>Add to Home screen</b>.</p>
      ${window.EncoreNative?.changeServer ? '<button class="btn" id="change-server" style="width:fit-content">Change server…</button>' : ''}</div>
    <div class="section-head"><h2>Active sessions</h2></div>
    <div class="card flat" id="sessions"></div>`;
  view.querySelector('#pw').onsubmit = async (e) => {
    e.preventDefault();
    try {
      await api.post('/auth/password', Object.fromEntries(new FormData(e.target)));
      toast('Password updated – other sessions signed out', 'ok');
      e.target.reset();
      loadSessions();
    } catch (err) { toast(err.message, 'err'); }
  };
  view.querySelector('#change-server')?.addEventListener('click', () => window.EncoreNative.changeServer());
  const loc = view.querySelector('#loc');
  const paintLoc = async () => {
    const p = await api.get('/auth/prefs');
    loc.innerHTML = `<h3>Song & artist names</h3>
      <p class="small muted" style="margin:0">Show names in your language where a translation or transliteration exists (MusicBrainz aliases and official translated tracklists), otherwise romanized. Only changes what <b>you</b> see – files and other users are untouched.</p>
      <label class="switch"><input type="checkbox" id="loc-on" ${p.localize ? 'checked' : ''}><i></i>Localize song and artist names</label>
      <div class="row wrap ${p.localize ? '' : 'hidden'}" id="loc-opts" style="gap:12px">
        <label class="field" style="margin:0"><span>Language</span><select class="input" id="loc-lang">${Object.entries(p.languages).map(([k, l]) => `<option value="${k}" ${k === p.lang ? 'selected' : ''}>${esc(l)}</option>`).join('')}</select></label>
        <label class="switch" style="margin-top:18px"><input type="checkbox" id="loc-latin" ${p.latin ? 'checked' : ''}><i></i>Latin letters (romanize Japanese, Korean, Cyrillic…)</label>
      </div>
      ${p.job.running ? `<div class="small muted"><span class="spinner"></span> Looking up names… ${p.job.done}/${p.job.total}</div>` : ''}`;
    const save = async (body) => {
      try { await api.put('/auth/prefs', body); toast('Saved – names update as they are looked up', 'ok'); setTimeout(paintLoc, 400); } catch (err) { toast(err.message, 'err'); }
    };
    loc.querySelector('#loc-on').onchange = (e) => save({ localize: e.target.checked });
    loc.querySelector('#loc-lang').onchange = (e) => save({ lang: e.target.value });
    loc.querySelector('#loc-latin').onchange = (e) => save({ latin: e.target.checked });
    if (p.job.running) setTimeout(() => view.isConnected && paintLoc(), 2000);
  };
  paintLoc();
  const box = view.querySelector('#sessions');
  const loadSessions = async () => {
    const rows = await api.get('/auth/sessions');
    box.innerHTML = `<div class="list">${rows.map(s => `<div class="list-item" data-sid="${esc(s.id)}">
      <div class="grow"><div class="ellipsis" style="font-weight:600">${esc(s.user_agent || 'Unknown device')}</div>
      <div class="small muted">${esc(s.ip || '')} · signed in ${fmt.ago(s.created_at)}</div></div>
      ${s.current ? '<span class="badge ok">This device</span>' : `<button class="btn sm danger">${icon.logout}Revoke</button>`}</div>`).join('')}</div>`;
  };
  box.onclick = async (e) => {
    const b = e.target.closest('button');
    if (!b) return;
    await api.del(`/auth/sessions/${b.closest('[data-sid]').dataset.sid}`);
    loadSessions();
  };
  loadSessions();
}
