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
    <form class="card stack mt" id="email-form" style="max-width:720px">
      <h3>Email</h3>
      <p class="small muted" style="margin:0">Used only to send you a link if you forget your password.</p>
      <div class="row wrap" style="gap:10px"><input class="input grow" type="email" name="email" value="${esc(me.email || '')}" placeholder="you@example.com" autocomplete="email" style="min-width:220px">
        <button class="btn primary" type="submit">Save</button></div>
    </form>
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
  view.querySelector('#email-form').onsubmit = async (e) => {
    e.preventDefault();
    try { await api.put('/auth/profile', { email: e.target.email.value }); toast('Email saved', 'ok'); } catch (err) { toast(err.message, 'err'); }
  };
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
