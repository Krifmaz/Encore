import { api } from '../api.js';
import { esc, fmt, icon, toast } from '../ui.js';

export async function render(view, ctx) {
  const me = await api.get('/auth/me');
  const { session, avatarHtml, refreshUser } = await import('../app.js');
  const features = session.info?.features || [];
  const has = (f) => features.includes(f);
  const admin = ['admin', 'owner'].includes(me.role);
  view.innerHTML = `
    <h1 style="margin-bottom:18px">Account</h1>
    <div class="stack" style="max-width:720px;gap:16px">
      <div class="card stack">
        <div class="row">${avatarHtml(me, 52)}
          <div class="grow"><div class="row" style="gap:6px;align-items:center"><h2>${esc(me.username)}</h2>${has('rename') ? `<button class="btn ghost icon sm" type="button" id="name-edit" title="Change username" aria-label="Change username">${icon.edit}</button>` : ''}</div><div class="muted small">${me.role === 'owner' ? 'Owner' : admin ? 'Administrator' : 'Member'} · joined ${fmt.ago(me.created_at)}</div></div></div>
        ${has('rename') ? `<form class="stack" id="name-form" hidden>
          <div class="row wrap" style="gap:10px"><input class="input grow" name="username" value="${esc(me.username)}" autocomplete="username" minlength="3" maxlength="32" required style="min-width:220px">
            <button class="btn primary" type="submit">Save</button><button class="btn ghost" type="button" id="name-cancel">Cancel</button></div>
          <p class="small muted" style="margin:0">What you sign in with and what others see. 3–32 letters, numbers, . _ -</p>
          ${me.email ? '' : `<p class="small" id="name-warn" style="margin:0;color:var(--warn)">⚠ There's no email on this account, so your username is the only way you'll be able to sign in – make it one you'll remember. Add an email below to be able to reset your password.</p>`}
        </form>` : ''}
        ${has('avatar') ? `<div class="row wrap" style="gap:8px"><label class="btn sm">${icon.upload || icon.plus}Change picture<input type="file" id="avatar-file" accept="image/*" hidden></label>
          ${me.avatar_at ? '<button class="btn sm ghost" id="avatar-remove">Remove</button>' : ''}</div>` : ''}
        <div class="small muted">Requests this week: <b>${me.requests_this_week}</b>${me.request_limit && !['admin', 'owner'].includes(me.role) ? ` of ${me.request_limit}` : ''}
          ${me.auto_approve || ['admin', 'owner'].includes(me.role) ? ' · <span class="badge ok">auto-approved</span>' : ''}</div>
      </div>
      <div class="card stack" id="pw-card">
        <div class="row" style="align-items:center"><h3 class="grow">Password</h3><button class="btn sm" type="button" id="pw-toggle">Change password</button></div>
        <form class="stack" id="pw" hidden>
          <label class="field"><span>Current password</span><input class="input" type="password" name="current" autocomplete="current-password" required></label>
          <label class="field"><span>New password</span><input class="input" type="password" name="new" autocomplete="new-password" minlength="8" required></label>
          <button class="btn primary" type="submit">Update password</button>
        </form>
      </div>
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
    ${has('totp') ? `<div class="card stack mt" style="max-width:720px" id="twofa"><h3>Two-step sign-in</h3>
      <p class="small muted" style="margin:0">${me.totp_enabled ? '✅ On – signing in asks for a code from your authenticator app.' : 'Ask for a 6-digit code from an authenticator app (Google Authenticator, 1Password, Aegis…) when signing in.'}</p>
      <div id="twofa-body">${me.totp_enabled ? '<button class="btn danger" id="twofa-off" style="width:fit-content">Turn off</button>' : '<button class="btn primary" id="twofa-on" style="width:fit-content">Set up</button>'}</div></div>` : ''}
    <div class="section-head"><h2>Active sessions</h2>${has('logout_all') ? `<button class="btn sm danger" id="logout-all">${icon.logout}Sign out everywhere</button>` : ''}</div>
    <div class="card flat" id="sessions"></div>
    ${admin ? '' : await (await import('./settings.js')).aboutCard()}`;
  // username: the pencil opens the editor (with a warning when there's no email to fall back on)
  const nameForm = view.querySelector('#name-form');
  view.querySelector('#name-edit')?.addEventListener('click', () => { nameForm.hidden = !nameForm.hidden; if (!nameForm.hidden) nameForm.username.focus(); });
  view.querySelector('#name-cancel')?.addEventListener('click', () => { nameForm.hidden = true; nameForm.username.value = me.username; });
  // password: a button that opens the form
  const pwForm = view.querySelector('#pw'), pwToggle = view.querySelector('#pw-toggle');
  const pwShow = (on) => { pwForm.hidden = !on; pwToggle.textContent = on ? 'Cancel' : 'Change password'; if (on) pwForm.current.focus(); else pwForm.reset(); };
  pwToggle.onclick = () => pwShow(pwForm.hidden);
  view.querySelector('#pw').onsubmit = async (e) => {
    e.preventDefault();
    try {
      await api.post('/auth/password', Object.fromEntries(new FormData(e.target)));
      toast('Password updated – other sessions signed out', 'ok');
      pwShow(false);
      loadSessions();
    } catch (err) { toast(err.message, 'err'); }
  };
  view.querySelector('#change-server')?.addEventListener('click', () => window.EncoreNative.changeServer());
  view.querySelector('#name-form')?.addEventListener('submit', async (e) => {
    e.preventDefault();
    try {
      const r = await api.put('/auth/profile', { username: e.target.username.value.trim() });
      toast(`You're now ${r.username}`, 'ok');
      await refreshUser?.();
    } catch (err) { toast(err.message, 'err'); }
  });
  view.querySelector('#email-form').onsubmit = async (e) => {
    e.preventDefault();
    try { await api.put('/auth/profile', { email: e.target.email.value }); toast('Email saved', 'ok'); } catch (err) { toast(err.message, 'err'); }
  };
  // profile picture (Encore 2)
  view.querySelector('#avatar-file')?.addEventListener('change', async (e) => {
    const f = e.target.files[0];
    if (!f) return;
    const fd = new FormData();
    fd.append('file', f);
    const r = await fetch('/api/auth/avatar', { method: 'POST', body: fd, headers: { 'X-Requested-With': 'encore' }, credentials: 'same-origin' });
    if (!r.ok) return toast((await r.json().catch(() => ({}))).detail || 'Upload failed', 'err');
    toast('Picture updated', 'ok');
    refreshUser();
  });
  view.querySelector('#avatar-remove')?.addEventListener('click', async () => { await api.del('/auth/avatar'); refreshUser(); });
  // two-step sign-in (Encore 2)
  view.querySelector('#twofa-on')?.addEventListener('click', async () => {
    let s;
    try { s = await api.post('/auth/2fa/setup'); } catch (e) { return toast(e.message, 'err'); }
    view.querySelector('#twofa-body').innerHTML = `<div class="row wrap" style="gap:18px;align-items:flex-start">
        ${s.qr ? `<img src="data:image/png;base64,${s.qr}" alt="QR code" style="width:180px;height:180px;border-radius:12px;background:#fff;padding:8px">` : ''}
        <div class="stack" style="flex:1;min-width:220px"><span class="small muted">Scan the code with your authenticator app, or enter this key:</span>
          <span class="code mono" style="word-break:break-all">${esc(s.secret)}</span>
          <form class="row" id="twofa-confirm"><input class="input" name="code" inputmode="numeric" autocomplete="one-time-code" placeholder="6-digit code" maxlength="8" required style="max-width:160px">
            <button class="btn primary" type="submit">Turn on</button></form></div></div>`;
    view.querySelector('#twofa-confirm').onsubmit = async (e) => {
      e.preventDefault();
      try { await api.post('/auth/2fa/enable', { code: e.target.code.value }); toast('Two-step sign-in is on', 'ok'); render(view, ctx); } catch (err) { toast(err.message, 'err'); }
    };
  });
  view.querySelector('#twofa-off')?.addEventListener('click', async () => {
    view.querySelector('#twofa-body').innerHTML = `<form class="row wrap" id="twofa-off-form"><input class="input" type="password" name="password" placeholder="Your password" autocomplete="current-password" required style="max-width:240px">
      <button class="btn danger" type="submit">Turn off two-step sign-in</button></form>`;
    view.querySelector('#twofa-off-form').onsubmit = async (e) => {
      e.preventDefault();
      try { await api.post('/auth/2fa/disable', { password: e.target.password.value }); toast('Two-step sign-in is off', 'ok'); render(view, ctx); } catch (err) { toast(err.message, 'err'); }
    };
  });
  view.querySelector('#logout-all')?.addEventListener('click', async () => {
    const { confirmDialog } = await import('../ui.js');
    if (!await confirmDialog('Sign out everywhere?', 'Every phone, computer and app signed in to your account is signed out – including this one.', 'Sign out everywhere', true)) return;
    await api.post('/auth/logout-all');
    location.reload();
  });
  const box = view.querySelector('#sessions');
  const loadSessions = async () => {
    const rows = await api.get('/auth/sessions');
    box.innerHTML = `<div class="list">${rows.map(s => `<div class="list-item" data-sid="${esc(s.id)}">
      <div class="grow"><div class="ellipsis" style="font-weight:600">${esc(s.device_name || s.user_agent || 'Unknown device')}</div>
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
