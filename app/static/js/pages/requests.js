import { api } from '../api.js';
import { emptyState, esc, icon } from '../ui.js';
import { bindRequestActions, requestRow } from './common.js';

export async function render(view, ctx) {
  const admin = ctx.user.role === 'admin';
  let filter = ctx.query.status || (admin ? 'all' : 'all');
  let mine = !admin;
  view.innerHTML = `
    <div class="row wrap between" style="margin-bottom:18px">
      <div><h1>Requests</h1><div class="small muted" id="quota"></div></div>
      <div class="row">
        ${admin ? `<label class="switch"><input type="checkbox" id="mine"><i></i>Only mine</label>
          <button class="btn sm" id="clear">${icon.trash}Clear finished</button>` : ''}
      </div>
    </div>
    <div class="tabs" id="r-tabs">
      ${[['all', 'All'], ['active', 'Active'], ['pending', 'Awaiting approval'], ['completed', 'Available'], ['failed', 'Failed']].map(([k, l]) => `<button data-s="${k}" class="${k === filter ? 'active' : ''}">${l}</button>`).join('')}
    </div>
    <div class="card flat mt" id="r-list"><div class="empty"><span class="spinner"></span></div></div>`;
  const list = view.querySelector('#r-list');
  const load = async () => {
    const q = new URLSearchParams({ limit: '300' });
    if (filter !== 'all') q.set('status', filter);
    if (mine) q.set('mine', 'true');
    const rows = await api.get('/requests?' + q);
    const top = rows.filter(r => !r.parent_id || !rows.some(p => p.id === r.parent_id));
    list.innerHTML = top.length ? `<div class="list">${top.map(r => {
      const kids = rows.filter(c => c.parent_id === r.id);
      return requestRow(r, { admin }) + (kids.length ? `<details style="padding:0 16px 10px 78px"><summary class="small muted" style="cursor:pointer">${kids.length} child request(s) · ${kids.filter(k => k.status === 'completed').length} available</summary><div class="list">${kids.map(k => requestRow(k, { admin })).join('')}</div></details>` : '');
    }).join('')}</div>`
      : emptyState(icon.inbox, 'Nothing here', 'Head to <a href="#/search" style="color:var(--accent)">Discover</a> to request music.');
  };
  bindRequestActions(list, load);
  view.querySelector('#r-tabs').onclick = (e) => {
    const t = e.target.closest('[data-s]');
    if (!t) return;
    filter = t.dataset.s;
    view.querySelectorAll('#r-tabs button').forEach(b => b.classList.toggle('active', b === t));
    load();
  };
  if (admin) {
    view.querySelector('#mine').onchange = (e) => { mine = e.target.checked; load(); };
    view.querySelector('#clear').onclick = async () => { await api.post('/requests/clear'); load(); };
  }
  const me = await api.get('/auth/me');
  if (me.request_limit && me.role !== 'admin') {
    view.querySelector('#quota').textContent = `${me.requests_this_week} of ${me.request_limit} requests used this week`;
  }
  await load();
  const timer = setInterval(() => { if (!list.querySelector('details[open]')) load(); }, 4000);
  ctx.onLeave(() => clearInterval(timer));
}
