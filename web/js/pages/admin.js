import { api } from '../api.js';
import { confirmDialog, copyText, emptyState, esc, fmt, icon, modal, toast } from '../ui.js';

export async function renderUsers(view, ctx) {
  view.innerHTML = `
    <div class="row wrap between" style="margin-bottom:18px">
      <div><h1>Users</h1><div class="small muted">Manage who can access your library and what they can request.</div></div>
      <div class="row"><a class="btn" href="#/settings?s=invites">${icon.ticket}Invites</a><button class="btn primary" id="add">${icon.plus}Add user</button></div>
    </div>
    <div class="card flat" id="users"></div>
    <div class="section-head"><h2>Permissions for everyone</h2><span class="small muted">applies to every non-admin user; fine-tune one user with the gear in their row</span></div>
    <div class="card flat" id="bulk-perms"></div>
    <div class="section-head"><h2>New members start with</h2><span class="small muted">copied onto every new account (sign-up, invite or added here); change one person later with their gear</span></div>
    <div class="card" id="new-defaults"></div>`;
  const box = view.querySelector('#users');
  const bulkBox = view.querySelector('#bulk-perms');
  const loadBulk = async () => {
    const d = await api.get('/admin/permissions');
    bulkBox.innerHTML = `<div class="list">${d.perms.map(p => `<div class="list-item" style="flex-wrap:wrap">
      <div class="grow" style="min-width:200px"><div style="font-weight:600">${esc(p.label)}</div><div class="small muted">${p.on} of ${p.of} user(s) have it</div></div>
      <div class="row" style="gap:6px;flex:none"><button class="btn sm" data-bulk="${p.key}" data-v="1">All on</button><button class="btn sm" data-bulk="${p.key}" data-v="0">All off</button></div></div>`).join('')}</div>`;
  };
  bulkBox.onclick = async (e) => {
    const b = e.target.closest('[data-bulk]');
    if (!b) return;
    try { await api.post('/admin/users/bulk', { perm: b.dataset.bulk, value: b.dataset.v === '1' }); toast('Updated for everyone', 'ok'); loadBulk(); load(); } catch (err) { toast(err.message, 'err'); }
  };
  loadBulk();
  const NEW = [['new_user_auto_approve', 'Requests skip the approval queue'], ['new_user_auto_approve_playlists', 'Playlist downloads skip the approval queue'],
    ['new_user_pick', 'Pick the exact version to download'], ['new_user_fix', 'Fix their own downloads'], ['new_user_storage', 'See server storage'],
    ['new_user_auto_download', 'Auto-download (likes and followed playlists)']];
  const defBox = view.querySelector('#new-defaults');
  api.get('/admin/settings').then(v => {
    defBox.innerHTML = `<div class="stack">${NEW.map(([k, l]) => `<label class="switch"><input type="checkbox" data-nd="${k}" ${v[k] ? 'checked' : ''}><i></i>${esc(l)}</label>`).join('')}
      <label class="field" style="max-width:260px"><span>Weekly request limit (0 = unlimited)</span><input class="input" type="number" min="0" data-nd="default_request_limit" value="${esc(v.default_request_limit ?? 0)}"></label></div>`;
    defBox.onchange = async (e) => {
      const k = e.target.dataset.nd;
      if (!k) return;
      try { await api.put('/admin/settings', { [k]: e.target.type === 'checkbox' ? e.target.checked : +e.target.value }); toast('Saved for new members', 'ok'); } catch (err) { toast(err.message, 'err'); }
    };
  }).catch(() => { defBox.textContent = ''; });
  const load = async () => {
    const users = await api.get('/admin/users');
    const { avatarHtml } = await import('../app.js');
    box.innerHTML = `<div class="table-wrap"><table class="table"><thead><tr>
      <th>User</th><th>Role</th><th class="hide-sm">Requests</th><th class="hide-sm">Weekly limit</th><th>Auto-approve</th><th class="hide-sm">Last seen</th><th></th></tr></thead><tbody>
      ${users.map(u => `<tr data-id="${u.id}">
        <td><div class="row">${avatarHtml(u)}<div><div style="font-weight:650">${esc(u.username)} ${u.id === ctx.user.id ? '<span class="badge">you</span>' : ''} ${u.disabled ? '<span class="badge err">disabled</span>' : ''}</div>
          <div class="small faint">${[u.email && esc(u.email), u.invite_code && `invite ${esc(u.invite_code)}`, `joined ${fmt.ago(u.created_at)}`].filter(Boolean).join(' · ')}</div></div></div></td>
        <td>${u.role === 'owner' ? '<span class="badge accent">Owner</span>' : `<select class="input" data-f="role" style="width:auto;padding:6px 10px" ${u.id === ctx.user.id ? 'disabled' : ''}>
          <option value="user" ${u.role === 'user' ? 'selected' : ''}>User</option><option value="admin" ${u.role === 'admin' ? 'selected' : ''}>Admin</option></select>`}</td>
        <td class="hide-sm">${u.request_count}</td>
        <td class="hide-sm"><input class="input" data-f="request_limit" type="number" min="0" value="${u.request_limit || ''}" placeholder="∞" style="width:80px;padding:6px 10px"></td>
        <td><label class="switch"><input type="checkbox" data-f="auto_approve" ${u.auto_approve ? 'checked' : ''}><i></i></label></td>
        <td class="hide-sm small muted">${fmt.ago(u.last_login)}</td>
        <td style="text-align:right;white-space:nowrap">
          ${!['admin', 'owner'].includes(u.role) ? `<button class="btn sm" data-a="perms" title="Permissions">${icon.settings}</button>` : ''}
          <button class="btn sm" data-a="pw" title="Reset password">${icon.shield}</button>
          ${u.id !== ctx.user.id ? `<button class="btn sm" data-a="toggle">${u.disabled ? 'Enable' : 'Disable'}</button>
          <button class="btn sm danger ghost" data-a="del" title="Delete">${icon.trash}</button>` : ''}
        </td></tr>`).join('')}
      </tbody></table></div>`;
  };
  const patch = async (id, body) => {
    try { await api.patch(`/admin/users/${id}`, body); toast('Saved', 'ok'); } catch (e) { toast(e.message, 'err'); }
    load();
  };
  box.addEventListener('change', (e) => {
    const f = e.target.dataset.f;
    if (!f) return;
    const id = e.target.closest('[data-id]').dataset.id;
    const val = e.target.type === 'checkbox' ? e.target.checked : f === 'request_limit' ? (parseInt(e.target.value, 10) || 0) : e.target.value;
    patch(id, { [f]: val });
  });
  box.addEventListener('click', async (e) => {
    const b = e.target.closest('[data-a]');
    if (!b) return;
    const tr = b.closest('[data-id]');
    const id = tr.dataset.id;
    const name = tr.querySelector('[style*="font-weight:650"]').textContent.trim().split(' ')[0];
    if (b.dataset.a === 'perms') {
      const [users, d] = await Promise.all([api.get('/admin/users'), api.get('/admin/permissions')]);
      const u = users.find(x => String(x.id) === String(id));
      const m = modal({
        title: `Permissions – ${esc(name)}`,
        body: `<div class="stack">${d.perms.map(p => `<label class="switch"><input type="checkbox" data-p="${p.key}" ${u.perms[p.key] ? 'checked' : ''}><i></i>${esc(p.label)}</label>`).join('')}</div>`,
        footer: '<button class="btn" data-close>Cancel</button><button class="btn primary" id="ok">Save</button>',
      });
      m.el.querySelector('#ok').onclick = async () => {
        const perms = Object.fromEntries([...m.el.querySelectorAll('[data-p]')].map(x => [x.dataset.p, x.checked]));
        await patch(id, { perms }); m.close(); loadBulk();
      };
      return;
    }
    if (b.dataset.a === 'toggle') patch(id, { disabled: b.textContent.trim() === 'Disable' });
    if (b.dataset.a === 'del' && await confirmDialog('Delete user', `Delete ${name}? Their requests stay in history.`, 'Delete', true)) {
      await api.del(`/admin/users/${id}`); load();
    }
    if (b.dataset.a === 'pw') {
      const m = modal({
        title: `Reset password for ${esc(name)}`,
        body: '<label class="field"><span>New password</span><input class="input" type="password" id="np" minlength="8" autocomplete="new-password"></label><p class="small muted">All of their sessions will be signed out.</p>',
        footer: '<button class="btn" data-close>Cancel</button><button class="btn primary" id="ok">Set password</button>',
      });
      m.el.querySelector('#ok').onclick = async () => { await patch(id, { password: m.el.querySelector('#np').value }); m.close(); };
    }
  });
  view.querySelector('#add').onclick = () => {
    const m = modal({
      title: 'Add user',
      body: `<form class="stack" id="nu">
        <label class="field"><span>Username</span><input class="input" name="username" required minlength="3"></label>
        <label class="field"><span>Email (optional)</span><input class="input" name="email" type="email"></label>
        <label class="field"><span>Password</span><input class="input" type="password" name="password" required minlength="8" autocomplete="new-password"></label>
        <label class="field"><span>Role</span><select class="input" name="role"><option value="user">User</option><option value="admin">Admin</option></select></label>
      </form>`,
      footer: '<button class="btn" data-close>Cancel</button><button class="btn primary" id="ok">Create</button>',
    });
    m.el.querySelector('#ok').onclick = async () => {
      const f = m.el.querySelector('#nu');
      if (!f.reportValidity()) return;
      try { await api.post('/admin/users', Object.fromEntries(new FormData(f))); m.close(); toast('User created', 'ok'); load(); } catch (e) { toast(e.message, 'err'); }
    };
  };
  await load();
}

export async function renderInvites(view, ctx) {
  const settings = await api.get('/admin/settings');
  view.innerHTML = `
    <div class="row wrap between" style="margin-bottom:18px">
      <div><h1>Invites</h1><div class="small muted">Share an invite link so friends can create an account.</div></div>
      <button class="btn primary" id="new">${icon.plus}New invite</button>
    </div>
    <div class="card row wrap between" style="margin-bottom:18px">
      <div><b>Sign-up mode</b><div class="small muted">Who is allowed to create an account on the login page.</div></div>
      <div class="tabs" id="mode">${[['invite', 'Invite only'], ['open', 'Open'], ['closed', 'Closed']].map(([k, l]) => `<button data-m="${k}" class="${settings.signup_mode === k ? 'active' : ''}">${l}</button>`).join('')}</div>
    </div>
    <div class="card flat" id="inv"></div>`;
  view.querySelector('#mode').onclick = async (e) => {
    const b = e.target.closest('[data-m]');
    if (!b) return;
    await api.put('/admin/settings', { signup_mode: b.dataset.m });
    view.querySelectorAll('#mode button').forEach(x => x.classList.toggle('active', x === b));
    toast(`Sign-ups: ${b.textContent}`, 'ok');
  };
  const base = (settings.base_url || location.origin).replace(/\/+$/, '');
  const link = (code) => `${base}/#/signup?invite=${encodeURIComponent(code)}`;
  const box = view.querySelector('#inv');
  const load = async () => {
    const rows = await api.get('/admin/invites');
    const now = Date.now() / 1000;
    box.innerHTML = rows.length ? `<div class="table-wrap"><table class="table"><thead><tr><th>Code</th><th>Uses</th><th class="hide-sm">Role</th><th class="hide-sm">Expires</th><th class="hide-sm">Used by</th><th></th></tr></thead><tbody>
      ${rows.map(i => {
        const dead = i.uses >= i.max_uses || (i.expires_at && i.expires_at < now);
        return `<tr data-id="${i.id}" data-code="${esc(i.code)}">
          <td><span class="mono" style="font-weight:700;${dead ? 'text-decoration:line-through;opacity:.5' : ''}">${esc(i.code)}</span>${i.note ? `<div class="small faint">${esc(i.note)}</div>` : ''}</td>
          <td>${i.uses} / ${i.max_uses}</td>
          <td class="hide-sm">${['admin', 'owner'].includes(i.role) ? '<span class="badge warn">admin</span>' : 'user'}</td>
          <td class="hide-sm small muted">${i.expires_at ? (i.expires_at < now ? 'expired' : new Date(i.expires_at * 1000).toLocaleDateString()) : 'never'}</td>
          <td class="hide-sm small muted">${esc(i.used_by || '—')}</td>
          <td style="text-align:right;white-space:nowrap">${dead ? '' : `<button class="btn sm" data-a="copy">${icon.copy}Copy link</button>`}
            <button class="btn sm danger ghost" data-a="del">${icon.trash}</button></td></tr>`;
      }).join('')}</tbody></table></div>` : emptyState(icon.ticket, 'No invites yet', 'Create one and send the link to a friend.');
  };
  box.onclick = async (e) => {
    const b = e.target.closest('[data-a]');
    if (!b) return;
    const tr = b.closest('[data-id]');
    if (b.dataset.a === 'copy') copyText(link(tr.dataset.code));
    if (b.dataset.a === 'del') { await api.del(`/admin/invites/${tr.dataset.id}`); load(); }
  };
  view.querySelector('#new').onclick = () => {
    const m = modal({
      title: 'New invite',
      body: `<form class="stack" id="ni">
        <div class="form-grid">
          <label class="field"><span>Max uses</span><input class="input" type="number" name="max_uses" value="1" min="1"></label>
          <label class="field"><span>Expires after (days)</span><input class="input" type="number" name="expires_days" value="7" min="0" step="0.5"><small>0 = never</small></label>
        </div>
        <label class="field"><span>Role</span><select class="input" name="role"><option value="user">User</option><option value="admin">Admin</option></select></label>
        <label class="field"><span>Note</span><input class="input" name="note" placeholder="e.g. for Sam"></label>
      </form><div id="made"></div>`,
      footer: '<button class="btn" data-close>Close</button><button class="btn primary" id="ok">Create invite</button>',
    });
    m.el.querySelector('#ok').onclick = async (ev) => {
      const f = Object.fromEntries(new FormData(m.el.querySelector('#ni')));
      try {
        const inv = await api.post('/admin/invites', { max_uses: +f.max_uses || 1, expires_days: +f.expires_days || null, role: f.role, note: f.note || null });
        m.el.querySelector('#ni').classList.add('hidden');
        ev.target.classList.add('hidden');
        m.el.querySelector('#made').innerHTML = `<p class="muted" style="margin-top:0">Send this link – it signs them up with the invite filled in:</p>
          <div class="code">${esc(link(inv.code))}</div>
          <div class="row mt"><button class="btn primary" id="cp">${icon.copy}Copy link</button><span class="muted">Code: <b class="mono">${esc(inv.code)}</b></span></div>`;
        m.el.querySelector('#cp').onclick = () => copyText(link(inv.code));
        load();
      } catch (e) { toast(e.message, 'err'); }
    };
  };
  await load();
}
