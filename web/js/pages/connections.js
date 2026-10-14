import { api } from '../api.js';
import { confirmDialog, esc, fmt, icon, modal, toast } from '../ui.js';
import { importModal } from './playlists.js';

const INFO = {
  jellyfin: { what: 'Two-way favourites (Finamp ♥) and playlist sync', form: 'userpass' },
  plex: { what: 'Two-way loves (Plexamp ♥) and playlist sync', form: 'plex' },
  navidrome: { what: 'Two-way stars and playlist sync', form: 'userpass' },
  ytmusic: { what: 'Two-way likes (👍) and playlist sync; import your library & Liked Music', form: 'ytmusic' },
  spotify: { what: 'Import private playlists & Liked Songs; likes flow into Encore', form: 'oauth' },
  tidal: { what: 'Import playlists & collection; likes flow into Encore', form: 'oauth' },
  deezer: { what: 'Import your public favourites & playlists', form: 'profile' },
  listenbrainz: { what: 'Scrobbles what you play here; two-way loves; playlists pushed (needs MusicBrainz-tagged tracks); For you recommendations', form: 'token' },
  lastfm: { what: 'Scrobbles what you play here; imports loved tracks; For you recommendations', form: 'lastfm' },
};

export async function render(view, ctx) {
  if (ctx.query.connected) toast(`${ctx.query.connected} connected`, 'ok');
  if (ctx.query.error) toast(ctx.query.error, 'err');
  const d = await api.get('/connections');
  view.innerHTML = `
    <div class="row wrap between" style="margin-bottom:18px">
      <div><h1>Connections</h1><div class="small muted">Link your accounts so likes and playlists stay the same in Finamp, Plexamp, Navidrome and here.${d.sync.last ? ` Last sync ${fmt.ago(d.sync.last)}.` : ''}</div></div>
      <button class="btn primary" id="sync-now">${icon.refresh}Sync now</button>
    </div>
    <div class="grid" style="grid-template-columns:repeat(auto-fill,minmax(300px,1fr))">
      ${Object.entries(d.services).map(([k, s]) => card(k, s, d.connections[k], d.available[k])).join('')}
    </div>`;
  view.querySelector('#sync-now').onclick = async (e) => {
    e.target.disabled = true;
    try {
      const r = await api.post('/sync');
      toast(`Synced: +${r.liked_added} / −${r.liked_removed} likes, ${r.pushed} changes pushed, ${r.playlists} playlist(s)${r.errors ? ` · ${r.errors} error(s)` : ''}`, r.errors ? 'err' : 'ok');
      window.dispatchEvent(new HashChangeEvent('hashchange'));
    } catch (err) { toast(err.message, 'err'); }
    e.target.disabled = false;
  };
  view.querySelector('.grid').addEventListener('click', async (e) => {
    const b = e.target.closest('[data-act]');
    if (!b) return;
    const svc = b.closest('[data-svc]').dataset.svc;
    const act = b.dataset.act;
    if (act === 'connect') return connect(svc, d, ['admin', 'owner'].includes(ctx.user?.role));
    if (act === 'disconnect') {
      if (!await confirmDialog('Disconnect', `Disconnect ${d.services[svc].label}?`, 'Disconnect', true)) return;
      await api.del(`/connections/${svc}`);
      window.dispatchEvent(new HashChangeEvent('hashchange'));
    }
    if (act === 'playlists') return remotePlaylists(svc, d.services[svc].label, ctx);
  });
  view.querySelector('.grid').addEventListener('change', async (e) => {
    const f = e.target.dataset.flag;
    if (!f) return;
    await api.patch(`/connections/${e.target.closest('[data-svc]').dataset.svc}`, { [f]: e.target.checked });
  });
}

function card(k, s, c, available) {
  const info = INFO[k] || {};
  return `<div class="card stack" data-svc="${k}">
    <div class="row between"><h3>${esc(s.label)}</h3>${c ? (c.last_error ? `<span class="badge err" title="${esc(c.last_error)}">error</span>` : '<span class="badge ok">connected</span>') : ''}</div>
    <div class="small muted">${esc(info.what || '')}</div>
    ${c ? `<div class="small">Signed in as <b>${esc(c.account || '')}</b>${c.last_sync ? ` · synced ${fmt.ago(c.last_sync)}` : ''}</div>
      ${c.last_error ? `<div class="small" style="color:var(--err)">${esc(c.last_error)}</div>` : ''}
      <div class="row wrap">
        ${c.can_scrobble ? `<label class="switch"><input type="checkbox" data-flag="scrobble" ${c.scrobble ? 'checked' : ''}><i></i>Scrobble what I play</label>` : ''}
        <label class="switch"><input type="checkbox" data-flag="sync_likes" ${c.sync_likes ? 'checked' : ''}><i></i>Sync likes</label>
        ${s.kind !== 'stream' ? `<label class="switch"><input type="checkbox" data-flag="sync_playlists" ${c.sync_playlists ? 'checked' : ''}><i></i>Sync playlists</label>` : ''}
      </div>
      ${k === 'lastfm' && !c.can_scrobble && c.can_scrobble !== undefined ? '<div class="small muted">Connected by username only – <button class="btn sm ghost" data-act="connect" style="padding:2px 6px">Sign in with Last.fm</button> to scrobble.</div>' : ''}
      <div class="row wrap">${k !== 'lastfm' ? `<button class="btn sm" data-act="playlists">${icon.download}Import playlists</button>` : ''}
        <button class="btn sm danger ghost" data-act="disconnect">Disconnect</button></div>`
      : available ? `<button class="btn" data-act="connect">${icon.link}Connect</button>`
        : '<div class="small faint">Not set up yet – an admin needs to add it in Settings.</div>'}
  </div>`;
}

async function connect(svc, d, admin) {
  const form = (INFO[svc] || {}).form;
  const label = d.services[svc]?.label || svc;
  if (form === 'oauth') {
    try { const r = await api.post(`/connections/${svc}`, {}); location.href = r.url; } catch (e) {
      // the admin side is not ready (no Public URL / no client id): a modal the user can read, not a passing toast
      if (!/Public URL|Client ID/.test(e.message)) return toast(e.message, 'err');
      const base = (d.base_url || location.origin).replace(/\/+$/, '');
      modal({
        title: `Connect ${esc(label)}`,
        body: `<p class="muted" style="margin-top:0">${esc(e.message)}</p>
      ${admin ? `<div class="small">Redirect URI to register at the service: <span class="mono">${esc(`${base}/api/connections/${svc}/callback`)}</span></div>` : ''}`,
        footer: '<button class="btn" data-close>Close</button>',
      });
    }
    return;
  }
  if (form === 'ytmusic') return connectYTMusic(!!d.available_oauth?.ytmusic);
  if (form === 'lastfm') return connectLastfm();
  if (form === 'plex') {
    let r;
    try { r = await api.post('/connections/plex', {}); } catch (e) { return toast(e.message, 'err'); }
    // a popup is blocked on phones (window.open returns null) – the modal always shows the link and the progress
    const w = window.open(r.url, 'plexauth', 'width=600,height=750');
    const m = modal({
      title: 'Sign in to Plex',
      body: `<a class="btn primary" target="_blank" rel="noopener" href="${esc(r.url)}">${icon.link}Open Plex sign-in</a>
      <p class="small muted" style="margin-bottom:0"><span class="spinner"></span> Approve Encore in the Plex page that opens, then come back here. Waiting…</p>`,
    });
    try {
      for (let i = 0; i < 120 && document.body.contains(m.el); i++) {
        await new Promise(res => setTimeout(res, 2000));
        if (!document.body.contains(m.el)) return;
        const c = await api.post('/connections/plex/check', { pin_id: r.pin_id });
        if (c.done) { w?.close(); m.close(); toast(`Plex connected (${c.account})`, 'ok'); window.dispatchEvent(new HashChangeEvent('hashchange')); return; }
      }
      if (document.body.contains(m.el)) { m.close(); toast('Plex sign-in timed out', 'err'); }
    } catch (e) { m.close(); toast(e.message, 'err'); }
    return;
  }
  const fields = {
    userpass: '<label class="field"><span>Username</span><input class="input" name="username" autocomplete="off"></label><label class="field"><span>Password</span><input class="input" type="password" name="password" autocomplete="off"></label>',
    token: '<label class="field"><span>User token</span><input class="input" name="token"><small>listenbrainz.org → Settings → User token</small></label>',
    username: '<label class="field"><span>Last.fm username</span><input class="input" name="username"></label>',
    profile: '<label class="field"><span>Deezer profile link or user id</span><input class="input" name="profile" placeholder="https://www.deezer.com/profile/123456"><small>Your favourites must be public</small></label>',
  }[form];
  const m = modal({
    title: 'Connect',
    body: `<form class="stack" id="cf">${fields}</form>`,
    footer: '<button class="btn" data-close>Cancel</button><button class="btn primary" id="ok">Connect</button>',
  });
  m.el.querySelector('#ok').onclick = async () => {
    try {
      const r = await api.post(`/connections/${svc}`, Object.fromEntries(new FormData(m.el.querySelector('#cf'))));
      toast(`Connected as ${r.account}`, 'ok');
      m.close();
      window.dispatchEvent(new HashChangeEvent('hashchange'));
    } catch (e) { toast(e.message, 'err'); }
  };
}

function connectLastfm() {
  const m = modal({
    title: 'Connect Last.fm',
    body: `<p class="muted" style="margin-top:0">Sign in on last.fm and allow Encore – then everything you play here is scrobbled, and For you suggests artists from your listening.</p>
      <button class="btn primary" id="lf-auth">${icon.link}Sign in with Last.fm</button>
      <details class="mt"><summary class="small muted" style="cursor:pointer">Or just enter a username (loved tracks and recommendations, no scrobbling)</summary>
        <div class="row mt" style="gap:8px"><input class="input grow" id="lf-user" placeholder="Last.fm username"><button class="btn" id="lf-ok">Connect</button></div></details>`,
  });
  m.el.querySelector('#lf-auth').onclick = async () => {
    try { const r = await api.post('/connections/lastfm', {}); location.href = r.url; } catch (e) { toast(e.message, 'err'); }
  };
  m.el.querySelector('#lf-ok').onclick = async () => {
    try {
      const r = await api.post('/connections/lastfm', { username: m.el.querySelector('#lf-user').value.trim() });
      toast(`Connected as ${r.account}`, 'ok'); m.close(); window.dispatchEvent(new HashChangeEvent('hashchange'));
    } catch (e) { toast(e.message, 'err'); }
  };
}

// `oauth`: an admin added a Google OAuth client, so the code sign-in can work. Without one the modal opens straight
// on the pasted-headers path instead of asking the server for a code and showing its error.
function connectYTMusic(oauth = true) {
  const m = modal({
    title: 'Connect YouTube Music',
    wide: true,
    body: `${oauth ? `<div class="tabs" id="ytm-tabs"><button data-m="code" class="active">Sign in with a code</button><button data-m="headers">Paste browser headers</button></div>
      <div id="ytm-code" class="mt"><p class="muted" style="margin-top:0">You'll get a short code to enter at google.com/device with the Google account you use for YouTube Music.</p>
        <button class="btn primary" id="ytm-start">${icon.link}Get code</button><div id="ytm-out" class="mt"></div></div>` : ''}
      <div id="ytm-headers" class="${oauth ? 'mt hidden' : 'mt'}">
        ${oauth ? '' : '<p class="muted" style="margin-top:0">Signing in with a code needs a Google OAuth client, which no admin has added in Settings → Integrations yet. Paste the headers of a signed-in music.youtube.com request instead – it works the same.</p>'}
        <ol class="small muted" style="margin-top:0;line-height:1.7"><li>Open music.youtube.com in a desktop browser while signed in, press F12 → Network.</li>
          <li>Click around, find a <span class="mono">browse</span> request (POST), right-click → Copy → <b>Copy request headers</b>.</li><li>Paste below.</li></ol>
        <textarea class="input mono" id="ytm-raw" rows="8" placeholder="accept: */*\ncookie: …"></textarea>
        <button class="btn primary mt" id="ytm-save">Connect</button></div>`,
  });
  const el = (s) => m.el.querySelector(s);
  if (!oauth) {
    el('#ytm-save').onclick = () => saveHeaders(m, el('#ytm-raw').value);
    return;
  }
  el('#ytm-tabs').onclick = (e) => {
    const b = e.target.closest('[data-m]');
    if (!b) return;
    m.el.querySelectorAll('#ytm-tabs button').forEach(x => x.classList.toggle('active', x === b));
    el('#ytm-code').classList.toggle('hidden', b.dataset.m !== 'code');
    el('#ytm-headers').classList.toggle('hidden', b.dataset.m !== 'headers');
  };
  el('#ytm-save').onclick = () => saveHeaders(m, el('#ytm-raw').value);
  el('#ytm-start').onclick = async (ev) => {
    ev.target.disabled = true;
    try {
      const c = await api.post('/connections/ytmusic', {});
      el('#ytm-out').innerHTML = `<div class="row wrap"><span>Go to</span><a class="btn sm" target="_blank" rel="noopener" href="${esc(c.url)}">${esc(c.url)}</a><span>and enter</span>
        <span class="code" style="font-size:20px;letter-spacing:.15em">${esc(c.user_code)}</span></div><p class="small muted"><span class="spinner"></span> Waiting for you to approve…</p>`;
      for (let i = 0; i < 180 && document.body.contains(m.el); i++) {
        await new Promise(res => setTimeout(res, (c.interval || 5) * 1000));
        const r = await api.post('/connections/ytmusic/check', { device_code: c.device_code }).catch(() => ({ done: false }));
        if (r.done) { toast(`YouTube Music connected (${r.account})`, 'ok'); m.close(); window.dispatchEvent(new HashChangeEvent('hashchange')); return; }
      }
    } catch (e) { toast(e.message, 'err'); ev.target.disabled = false; }
  };
}

async function saveHeaders(m, headers) {
  try {
    const r = await api.post('/connections/ytmusic', { headers });
    toast(`YouTube Music connected (${r.account})`, 'ok'); m.close(); window.dispatchEvent(new HashChangeEvent('hashchange'));
  } catch (e) { toast(e.message, 'err'); }
}

async function remotePlaylists(svc, label, ctx) {
  const m = modal({ title: `Playlists on ${esc(label)}`, wide: true, body: '<div class="empty"><span class="spinner"></span></div>' });
  try {
    const rows = await api.get(`/connections/${svc}/playlists`);
    const liked = { spotify: 'https://open.spotify.com/collection/tracks', ytmusic: 'https://music.youtube.com/playlist?list=LM', deezer: null };
    m.el.querySelector('.modal-body').innerHTML = `
      ${liked[svc] ? `<button class="btn mt" id="liked" style="margin:0 0 12px">♥ Import Liked Songs</button>` : ''}
      <div class="card flat"><div class="list">${rows.map(p => `<div class="list-item" data-rid="${esc(p.id)}" data-url="${esc(p.url || '')}" data-name="${esc(p.name)}">
        <div class="grow ellipsis"><b>${esc(p.name)}</b></div>
        <label class="switch small"><input type="checkbox" data-follow checked><i></i>Follow</label>
        <button class="btn sm primary" data-import>${icon.download}Import</button></div>`).join('') || '<div class="empty muted">No playlists found</div>'}</div></div>`;
    m.el.querySelector('#liked')?.addEventListener('click', () => { m.close(); importModal(ctx, { url: liked[svc] }); });
    m.el.querySelector('.list').onclick = async (e) => {
      const b = e.target.closest('[data-import]');
      if (!b) return;
      const row = b.closest('[data-rid]');
      b.disabled = true;
      try {
        const r = await api.post(`/connections/${svc}/playlists/${encodeURIComponent(row.dataset.rid)}/import`, {
          name: row.dataset.name, url: row.dataset.url || null, follow: row.querySelector('[data-follow]').checked });
        toast(`Imported ${row.dataset.name}: ${r.added} tracks${r.duplicates_skipped ? `, ${r.duplicates_skipped} duplicates skipped` : ''}`, 'ok');
        b.outerHTML = '<span class="badge ok">imported</span>';
      } catch (err) { toast(err.message, 'err'); b.disabled = false; }
    };
  } catch (e) {
    m.el.querySelector('.modal-body').innerHTML = `<div class="error-box">${esc(e.message)}</div>`;
  }
}
