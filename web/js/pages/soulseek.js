// Soulseek: search the network, browse what a user shares, download files or whole folders, watch transfers.
// Everything goes through /api/soulseek; a download becomes an ordinary request the worker tags and imports.
import { api } from '../api.js';
import { debounce, emptyState, esc, fmt, icon, modal, qsa, toast } from '../ui.js';

const LOSSLESS = ['flac', 'alac', 'wav', 'aiff', 'aif', 'ape', 'wv'];
const MAX_USERS = 150;     // result cards rendered per search – the network can answer with far more
const MAX_FOLDERS = 400;   // folders listed in the browse modal before asking for a filter

// slskd transfer states → badge class + short label (anything else is prettified by stateBadge)
const STATES = {
  'Requested': ['', 'Requested'],
  'Queued, Remotely': ['info', 'In their queue'],
  'Queued, Locally': ['info', 'Queued'],
  'Initializing': ['accent', 'Starting'],
  'InProgress': ['accent', 'Downloading'],
  'Completed, Succeeded': ['ok', 'Done'],
  'Completed, Errored': ['err', 'Failed'],
  'Completed, Cancelled': ['', 'Cancelled'],
  'Completed, TimedOut': ['err', 'Timed out'],
  'Completed, Rejected': ['err', 'Rejected'],
  'Completed, Aborted': ['err', 'Aborted'],
};

// ---- small helpers -------------------------------------------------------------------------------

/** Last component of a Soulseek path (backslashes from Windows clients, slashes from others). */
const baseName = (p) => String(p || '').split(/[\\/]/).filter(Boolean).pop() || '';
const extOf = (f) => String(f.extension || (f.name || baseName(f.filename)).split('.').pop() || '').replace(/^\./, '').toLowerCase();
const fileKey = (u, f) => `${u.username}\n${f.filename}`;
const isFinished = (t) => /^Completed/.test(t.state || '');
const isActive = (t) => !isFinished(t);
const plural = (n, w) => `${fmt.num(n)} ${w}${n === 1 ? '' : 's'}`;
const speedText = (b) => b >= 1e6 ? `${(b / 1e6).toFixed(1)} MB/s` : `${Math.round(b / 1e3)} kB/s`;
const pct = (t) => Math.max(0, Math.min(100, Math.round(t.progress ?? (t.size ? (t.bytes || 0) / t.size * 100 : 0))));
const sortUsers = (users) => users.slice().sort((a, b) => (b.hasFreeUploadSlot ? 1 : 0) - (a.hasFreeUploadSlot ? 1 : 0) || (b.uploadSpeed || 0) - (a.uploadSpeed || 0));

/** Same text as the picker's quality badge: "FLAC 24-bit/96kHz", "MP3 320k". */
function quality(f) {
  const ext = extOf(f);
  const up = ext.toUpperCase() || '?';
  if (LOSSLESS.includes(ext)) {
    const bits = f.bitDepth ? ` ${f.bitDepth}-bit` : '';
    const khz = f.sampleRate && f.sampleRate !== 44100 ? `/${f.sampleRate / 1000}kHz` : '';
    return up + bits + khz;
  }
  return f.bitRate ? `${up} ${f.bitRate}k` : up;
}

/** Humanised slskd state as a badge; uploads say "Uploading" instead of "Downloading". */
function stateBadge(state, upload = false) {
  let [cls, label] = STATES[state] || ['', String(state || 'Unknown').replace(/([a-z])([A-Z])/g, '$1 $2').replace(/,\s*/g, ' · ')];
  if (upload && state === 'InProgress') label = 'Uploading';
  const spin = state === 'InProgress' || state === 'Initializing' ? '<span class="spinner" style="width:10px;height:10px;border-width:2px"></span>' : '';
  return `<span class="badge ${cls}">${spin}${esc(label)}</span>`;
}

/** ui.toast() takes plain text; this one can carry a link. */
function toastHtml(html, type = 'ok') {
  const el = document.createElement('div');
  el.className = `toast ${type}`;
  el.innerHTML = html;
  document.getElementById('toasts').appendChild(el);
  setTimeout(() => { el.style.opacity = '0'; el.style.transition = 'opacity .3s'; }, 5000);
  setTimeout(() => el.remove(), 5400);
}

/** "Signed in as X · sharing N files · port P" – or why it isn't working, linking to the settings. */
function statusHtml(s, error) {
  const settings = '<a href="#/settings?s=slskd" style="color:var(--accent)">Settings → Soulseek</a>';
  if (error) return `<span class="dot err"></span> ${esc(error)} · ${settings}`;
  if (!s.enabled) return `<span class="dot"></span> Soulseek is not enabled – turn it on in ${settings}`;
  if (!s.logged_in) return `<span class="dot err"></span> <span style="color:var(--warn)">Not signed in${s.error ? ` – ${esc(s.error)}` : ''}</span> · ${settings}`;
  const parts = [`<span class="dot ok"></span> Signed in as <b>${esc(s.username || '?')}</b>${s.client === 'slskd' ? ' <span class="faint">via slskd</span>' : ''}`];
  parts.push(s.shares?.files ? `sharing ${plural(s.shares.files, 'file')}` : 'sharing nothing');
  if (s.port) parts.push(`port ${esc(s.port)}`);
  if (s.error) parts.push(`<span style="color:var(--warn)">${esc(s.error)}</span>`);
  return parts.join(' · ');
}

// ---- downloading -----------------------------------------------------------------------------------

/** POST /download: the worker fetches, tags and imports the files exactly like a picker pick. */
async function download(u, folder, files, btn) {
  if (!files.length) return false;
  btn.disabled = true;
  try {
    await api.post('/soulseek/download', {
      username: u.username,
      directory: folder.directory,
      files: files.map(f => ({ filename: f.filename, size: f.size })),
      kind: files.length > 1 ? 'album' : 'track',
    });
    toastHtml(`Added to your requests · <a href="#/requests" style="color:var(--accent);font-weight:600">View requests</a>`, 'ok');
    window.dispatchEvent(new CustomEvent('encore:requests-changed'));
    return true;
  } catch (e) {
    toast(e.message, 'err');
    return false;
  } finally { btn.disabled = false; }
}

// ---- result markup (shared by search results and the browse modal) -----------------------------------

function userBadges(u) {
  return (u.hasFreeUploadSlot ? '<span class="badge ok">Free slot</span>' : `<span class="badge warn">${fmt.num(u.queueLength || 0)} queued</span>`)
    + (u.uploadSpeed ? ` <span class="badge">${esc(speedText(u.uploadSpeed))}</span>` : '');
}

function fileRow(f, k, checked) {
  return `<div class="slsk-file">
    <label><input type="checkbox" data-i="${k}" ${checked ? 'checked' : ''}><span class="name">${esc(f.name || baseName(f.filename))}</span></label>
    <span class="meta"><span class="badge ${LOSSLESS.includes(extOf(f)) ? 'ok' : ''}">${esc(quality(f))}</span>${f.length ? `<span class="len">${fmt.dur(f.length)}</span>` : ''}<span>${fmt.size(f.size)}</span></span>
  </div>`;
}

const fileRows = (u, f, checked) => f.files.map((x, k) => fileRow(x, k, checked.has(fileKey(u, x)))).join('');

function selectedBtn(u, f, checked) {
  const n = f.files.filter(x => checked.has(fileKey(u, x))).length;
  return `<button class="btn sm" data-dl="selected" ${n ? '' : 'disabled'}>${icon.check}Download selected${n ? ` (${n})` : ''}</button>`;
}

/** One folder: name, full path, download buttons, file rows. Collapsible in the browse modal, open in results. */
function folderHtml(u, i, f, j, checked, { collapsible = false, open = true } = {}) {
  const size = f.files.reduce((n, x) => n + (x.size || 0), 0);
  const head = `<div class="slsk-folder-head">
      ${collapsible ? `<span class="chev">${icon.chevron}</span>` : ''}
      <div class="grow"><div class="name">${esc(baseName(f.directory) || f.directory)} <span class="faint small">· ${plural(f.files.length, 'file')} · ${fmt.size(size)}</span></div>
        <div class="path">${esc(f.directory)}</div></div>
      <div class="row" style="gap:6px;flex:none">${selectedBtn(u, f, checked)}<button class="btn sm primary" data-dl="folder">${icon.download}Download folder</button></div>
    </div>`;
  const files = `<div class="slsk-files" ${open ? '' : 'data-lazy'}>${open ? fileRows(u, f, checked) : ''}</div>`;
  return collapsible
    ? `<details class="slsk-folder" data-u="${i}" data-f="${j}" ${open ? 'open' : ''}><summary>${head}</summary>${files}</details>`
    : `<div class="slsk-folder" data-u="${i}" data-f="${j}">${head}${files}</div>`;
}

/** One search result: a collapsible card per user; its folders are rendered when it is first opened. */
function userCard(u, i, checked, open) {
  const files = u.folders.reduce((n, f) => n + f.files.length, 0);
  return `<details class="card flat slsk-user" data-u="${i}" data-user="${esc(u.username)}" ${open ? 'open' : ''}>
    <summary>
      <span class="chev">${icon.chevron}</span>
      <b class="ellipsis" style="max-width:240px">${esc(u.username)}</b>
      ${userBadges(u)}
      <span class="faint small">${plural(u.folders.length, 'folder')} · ${plural(files, 'file')}</span>
      <span class="grow"></span>
      <button class="btn sm" data-browse="${esc(u.username)}">${icon.user}Browse</button>
    </summary>
    <div class="slsk-body" ${open ? '' : 'data-lazy'}>${open ? u.folders.map((f, j) => folderHtml(u, i, f, j, checked)).join('') : ''}</div>
  </details>`;
}

/**
 * Wires a results container: lazy rendering of collapsed cards/folders, the checkboxes, the download
 * and Browse buttons. `root._users` holds the data the data-u / data-f indexes point into.
 */
function bindTree(root, checked) {
  const lookup = (el) => {
    const box = el.closest('[data-f]');
    const u = root._users[+box.dataset.u];
    return { box, u, f: u.folders[+box.dataset.f] };
  };
  const syncSel = (box, u, f) => { box.querySelector('[data-dl=selected]').outerHTML = selectedBtn(u, f, checked); };
  // <details> toggle events don't bubble, but they can be captured
  root.addEventListener('toggle', (e) => {
    const d = e.target;
    const body = d.open && d.querySelector(':scope > [data-lazy]');
    if (!body) return;
    body.removeAttribute('data-lazy');
    const i = +d.dataset.u, u = root._users[i];
    body.innerHTML = d.dataset.f !== undefined ? fileRows(u, u.folders[+d.dataset.f], checked)
      : u.folders.map((f, j) => folderHtml(u, i, f, j, checked)).join('');
  }, true);
  root.addEventListener('change', (e) => {
    const cb = e.target.closest('input[data-i]');
    if (!cb) return;
    const { box, u, f } = lookup(cb);
    checked[cb.checked ? 'add' : 'delete'](fileKey(u, f.files[+cb.dataset.i]));
    syncSel(box, u, f);
  });
  root.addEventListener('click', async (e) => {
    const b = e.target.closest('[data-dl], [data-browse]');
    if (!b) return;
    e.preventDefault();   // the buttons sit inside <summary>: don't fold the card
    if (b.dataset.browse !== undefined) return browseModal(b.dataset.browse);
    const { box, u, f } = lookup(b);
    const files = b.dataset.dl === 'folder' ? f.files : f.files.filter(x => checked.has(fileKey(u, x)));
    if (await download(u, f, files, b) && b.dataset.dl === 'selected') {
      files.forEach(x => checked.delete(fileKey(u, x)));
      // a poll may have repainted the results meanwhile: untick the live folder, not a detached one
      const live = root.querySelector(`[data-u="${box.dataset.u}"][data-f="${box.dataset.f}"]`) || box;
      qsa('input[data-i]', live).forEach(cb => { cb.checked = false; });
      syncSel(live, u, f);
    }
  });
}

// ---- browse a user ----------------------------------------------------------------------------------

/** Wide modal with everything a user shares: GET /browse/{user}, then a filter box over the folders. */
async function browseModal(username) {
  const checked = new Set();
  const m = modal({
    title: `${esc(username)} <span class="faint" style="font-weight:400">· shared files</span>`,
    wide: true,
    body: `<div class="empty"><span class="spinner"></span><p class="small muted">Asking ${esc(username)} for their file list – this can take a while if they are slow to answer.</p></div>`,
  });
  const body = m.el.querySelector('.modal-body');
  let d;
  try { d = await api.get(`/soulseek/browse/${encodeURIComponent(username)}`); }
  catch (e) { if (m.el.isConnected) body.innerHTML = `<div class="error-box">${esc(e.message)}</div>`; return; }
  if (!m.el.isConnected) return;
  const user = { username, folders: d.folders || [] };
  const total = user.folders.reduce((n, f) => n + f.files.length, 0);
  body.innerHTML = `<div class="row wrap" style="margin-bottom:12px">
      <div class="search grow">${icon.search}<input class="input" id="br-q" placeholder="Filter folders and files…" autocomplete="off"></div>
      <span class="small muted">${plural(user.folders.length, 'folder')} · ${plural(total, 'file')}</span></div>
    <div id="br-list"></div>`;
  const list = body.querySelector('#br-list');
  list._users = [user];
  bindTree(list, checked);
  const paint = (q) => {
    const needle = q.toLowerCase();
    const hit = (s) => String(s || '').toLowerCase().includes(needle);
    const shown = user.folders.map((f, j) => [f, j]).filter(([f]) => !needle || hit(f.directory) || f.files.some(x => hit(x.name)));
    const open = !!needle && shown.length <= 6;   // a narrow filter: show the files right away
    list.innerHTML = shown.length
      ? shown.slice(0, MAX_FOLDERS).map(([f, j]) => folderHtml(user, 0, f, j, checked, { collapsible: true, open })).join('')
        + (shown.length > MAX_FOLDERS ? `<div class="muted small mt">Showing ${MAX_FOLDERS} of ${plural(shown.length, 'folder')} – narrow the filter to see the rest.</div>` : '')
      : emptyState(icon.search, needle ? 'No folders match' : 'Nothing shared');
  };
  paint('');
  body.querySelector('#br-q').addEventListener('input', debounce((e) => paint(e.target.value.trim()), 200));
}

// ---- transfers ----------------------------------------------------------------------------------------

function downloadsTable(rows) {
  return `<div class="table-wrap"><table class="table"><thead><tr><th class="hide-sm">User</th><th>File</th><th>State</th><th style="width:160px">Progress</th><th class="hide-sm">Speed / size</th><th></th></tr></thead><tbody>
    ${rows.map((x, i) => `<tr>
      <td class="hide-sm muted">${esc(x.username)}</td>
      <td><div class="ellipsis" style="max-width:420px;font-weight:600" title="${esc(x.filename)}">${esc(x.basename || baseName(x.filename))}</div>
        <div class="small faint ellipsis" style="max-width:420px">${esc(x.directory || '')}</div>
        ${x.error ? `<div class="small" style="color:var(--err)">${esc(x.error)}</div>` : ''}</td>
      <td>${stateBadge(x.state)}</td>
      <td><div class="progress"><i style="width:${pct(x)}%"></i></div><div class="small faint">${pct(x)}%</div></td>
      <td class="hide-sm small muted" style="white-space:nowrap">${isActive(x) && x.speed ? `${esc(speedText(x.speed))}<br>` : ''}${fmt.size(x.bytes)} / ${fmt.size(x.size)}</td>
      <td><button class="btn sm ghost icon" data-cancel="${i}" title="${isActive(x) ? 'Cancel' : 'Remove'}" aria-label="${isActive(x) ? 'Cancel' : 'Remove'}">${icon.x}</button></td>
    </tr>`).join('')}</tbody></table></div>`;
}

function uploadsTable(rows) {
  return `<div class="table-wrap"><table class="table"><thead><tr><th class="hide-sm">User</th><th>File</th><th>State</th><th style="width:160px">Progress</th></tr></thead><tbody>
    ${rows.map(x => `<tr>
      <td class="hide-sm muted">${esc(x.username)}</td>
      <td><div class="ellipsis" style="max-width:420px;font-weight:600" title="${esc(x.filename)}">${esc(x.basename || baseName(x.filename))}</div><div class="small faint">${fmt.size(x.size)}</div></td>
      <td>${stateBadge(x.state, true)}</td>
      <td><div class="progress"><i style="width:${pct(x)}%"></i></div><div class="small faint">${fmt.size(x.bytes)} / ${fmt.size(x.size)}</div></td>
    </tr>`).join('')}</tbody></table></div>`;
}

// ---- page -----------------------------------------------------------------------------------------------

export async function render(view, ctx) {
  const state = { tab: ctx.query.tab === 'transfers' ? 'transfers' : 'search', status: null, enabled: true, results: [], transfers: { downloads: [], uploads: [] } };
  const checked = new Set();   // "user\nfilename" of every ticked file in the search results
  view.innerHTML = `
    <div class="row wrap between" style="margin-bottom:18px">
      <div><h1>Soulseek</h1><div class="small muted slsk-status" id="slsk-status"><span class="spinner" style="width:12px;height:12px"></span></div></div>
      <div class="tabs" id="slsk-tabs">
        <button data-t="search" class="${state.tab === 'search' ? 'active' : ''}">Search</button>
        <button data-t="transfers" class="${state.tab === 'transfers' ? 'active' : ''}">Transfers <span class="faint" id="slsk-tcount"></span></button>
      </div>
    </div>
    <div id="slsk-search" class="${state.tab === 'search' ? '' : 'hidden'}">
      <form class="row wrap" id="slsk-form">
        <div class="search grow">${icon.search}<input class="input" name="q" type="search" enterkeyhint="search" placeholder="Artist, album, song – the way the uploader probably named it" value="${esc(ctx.query.q || '')}" autocomplete="off"></div>
        <button class="btn primary" type="submit">${icon.search}Search</button>
      </form>
      <div class="small muted mt" id="slsk-sstatus"></div>
      <div class="stack" id="slsk-results" style="margin-top:10px"></div>
    </div>
    <div id="slsk-transfers" class="${state.tab === 'transfers' ? '' : 'hidden'}"></div>`;
  const statusEl = view.querySelector('#slsk-status');
  const form = view.querySelector('#slsk-form');
  const sStatus = view.querySelector('#slsk-sstatus');
  const results = view.querySelector('#slsk-results');
  const trEl = view.querySelector('#slsk-transfers');
  results._users = state.results;
  bindTree(results, checked);

  // status line
  const loadStatus = async () => {
    try {
      state.status = await api.get('/soulseek/status');
      state.enabled = !!state.status.enabled;
      statusEl.innerHTML = statusHtml(state.status);
    } catch (e) {
      state.enabled = ![400, 403].includes(e.status);   // 400 "not enabled", 403 may not pick downloads
      state.forbidden = e.status === 403;
      statusEl.innerHTML = statusHtml(null, e.message);
    }
    form.classList.toggle('hidden', !state.enabled);
    if (!state.enabled) {
      sStatus.innerHTML = '';
      results.innerHTML = state.forbidden ? emptyState(icon.users, 'Soulseek is for people who may pick downloads', 'Ask an admin to allow manual download picking for your account.')
        : emptyState(icon.users, 'Soulseek is unavailable', 'An admin can turn it on under <a href="#/settings?s=slskd" style="color:var(--accent)">Settings → Soulseek</a>.');
    }
  };

  // search: POST /search, then GET /search/{id} once a second until done
  let gen = 0, pollT = null;
  const paintResults = (done) => {
    const open = new Set(qsa('details.slsk-user[open]', results).map(d => d.dataset.user));
    if (done && !open.size && state.results.length) open.add(state.results[0].username);
    results._users = state.results;
    const shown = state.results.slice(0, MAX_USERS);
    results.innerHTML = shown.map((u, i) => userCard(u, i, checked, open.has(u.username))).join('')
      + (state.results.length > shown.length ? `<div class="muted small">Showing the first ${MAX_USERS} of ${plural(state.results.length, 'user')} – free slots and fast uploaders come first.</div>` : '');
  };
  const search = async (q) => {
    const my = ++gen;
    clearTimeout(pollT);
    state.results = [];
    checked.clear();
    results.innerHTML = '';
    sStatus.innerHTML = '<span class="spinner" style="width:12px;height:12px"></span> Starting the search…';
    let s;
    try { s = await api.post('/soulseek/search', { q }); }
    catch (e) { if (my === gen) sStatus.innerHTML = `<span style="color:var(--err)">${esc(e.message)}</span>`; return; }
    const poll = async () => {
      if (my !== gen || !view.isConnected) return;
      let r;
      try { r = await api.get(`/soulseek/search/${encodeURIComponent(s.id)}`); }
      catch (e) { if (my === gen) sStatus.innerHTML = `<span style="color:var(--err)">${esc(e.status === 404 ? 'That search has expired – run it again.' : e.message)}</span>`; return; }
      if (my !== gen) return;
      state.results = sortUsers(r.results || []);
      const files = state.results.reduce((n, u) => n + u.folders.reduce((m, f) => m + f.files.length, 0), 0);
      paintResults(r.done);
      if (r.done) {
        sStatus.innerHTML = state.results.length ? `${plural(state.results.length, 'user')} · ${plural(files, 'file')} for “${esc(r.q || q)}”` : '';
        if (!state.results.length) results.innerHTML = emptyState(icon.search, 'Nothing found', 'Try fewer words, or just the artist and album – Soulseek matches file and folder names.');
      } else {
        sStatus.innerHTML = `<span class="spinner" style="width:12px;height:12px"></span> Searching for up to ${esc(s.seconds || '?')} s… ${plural(state.results.length, 'user')} so far`;
        pollT = setTimeout(poll, 1000);
      }
    };
    poll();
  };
  ctx.onLeave(() => { gen++; clearTimeout(pollT); });
  form.onsubmit = (e) => {
    e.preventDefault();
    const q = new FormData(form).get('q').trim();
    if (!q) return form.querySelector('input').focus();
    search(q);
  };

  // transfers: repainted from state every 2 s while the tab is open; the tab label counts active downloads
  const paintTransfers = () => {
    const { downloads: dl = [], uploads: ul = [] } = state.transfers;
    trEl.innerHTML = `
      <div class="section-head"><h2>Downloads <span class="faint small">${dl.length}</span></h2>
        <button class="btn sm" id="slsk-clear" ${dl.some(isFinished) ? '' : 'disabled'}>${icon.trash}Clear finished</button></div>
      <div class="card flat">${dl.length ? downloadsTable(dl) : emptyState(icon.download, 'No downloads', 'Files you download from Soulseek show up here while they transfer.')}</div>
      <div class="section-head"><h2>Uploads <span class="faint small">${ul.length}</span></h2></div>
      <div class="card flat slsk-ul">${ul.length ? uploadsTable(ul) : emptyState(icon.users, 'No uploads', state.status?.client === 'slskd' ? 'Uploads are only listed for the built-in client.' : 'Other people fetching from your shares show up here.')}</div>`;
  };
  const loadTransfers = async () => {
    try { state.transfers = await api.get('/soulseek/transfers'); }
    catch (e) { if (state.tab === 'transfers' && !trEl.querySelector('.table')) trEl.innerHTML = `<div class="error-box">${esc(e.message)}</div>`; return; }
    const active = (state.transfers.downloads || []).filter(isActive).length;
    view.querySelector('#slsk-tcount').textContent = active ? String(active) : '';
    if (state.tab === 'transfers') paintTransfers();
  };
  trEl.addEventListener('click', async (e) => {
    const b = e.target.closest('[data-cancel], #slsk-clear');
    if (!b) return;
    b.disabled = true;
    try {
      if (b.dataset.cancel !== undefined) {
        const x = state.transfers.downloads[+b.dataset.cancel];
        await api.del(`/soulseek/transfers/${encodeURIComponent(x.username)}/${encodeURIComponent(x.id)}`);
      } else {
        await api.post('/soulseek/transfers/clear');
      }
      await loadTransfers();
    } catch (err) { toast(err.message, 'err'); b.disabled = false; }
  });

  // tabs
  const panels = { search: view.querySelector('#slsk-search'), transfers: trEl };
  const showTab = (t) => {
    state.tab = t;
    qsa('#slsk-tabs button', view).forEach(b => b.classList.toggle('active', b.dataset.t === t));
    Object.entries(panels).forEach(([k, el]) => el.classList.toggle('hidden', k !== t));
    if (t === 'transfers') { paintTransfers(); if (state.enabled) loadTransfers(); }
  };
  view.querySelector('#slsk-tabs').onclick = (e) => { const b = e.target.closest('[data-t]'); if (b) showTab(b.dataset.t); };

  await loadStatus();
  if (state.enabled) {
    await loadTransfers();
    if (ctx.query.q) search(ctx.query.q);
  }
  if (state.tab === 'transfers') paintTransfers();
  let n = 0;
  const tick = setInterval(() => {
    if (!view.isConnected) return;
    n++;
    if (n % 15 === 0) loadStatus();                                                    // every 30 s
    if (state.enabled && (state.tab === 'transfers' || n % 5 === 0)) loadTransfers();   // 2 s on the tab, 10 s for the badge
  }, 2000);
  ctx.onLeave(() => clearInterval(tick));
}
