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
  listenbrainz: { what: 'Two-way loves; playlists pushed (needs MusicBrainz-tagged tracks)', form: 'token' },
  lastfm: { what: 'Import loved tracks', form: 'username' },
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
    if (act === 'connect') return connect(svc);
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
        <label class="switch"><input type="checkbox" data-flag="sync_likes" ${c.sync_likes ? 'checked' : ''}><i></i>Sync likes</label>
        ${s.kind !== 'stream' ? `<label class="switch"><input type="checkbox" data-flag="sync_playlists" ${c.sync_playlists ? 'checked' : ''}><i></i>Sync playlists</label>` : ''}
      </div>
      <div class="row wrap">${k !== 'lastfm' ? `<button class="btn sm" data-act="playlists">${icon.download}Import playlists</button>` : ''}
        <button class="btn sm danger ghost" data-act="disconnect">Disconnect</button></div>`
      : available ? `<button class="btn" data-act="connect">${icon.link}Connect</button>`
        : '<div class="small faint">Not set up yet – an admin needs to add it in Settings.</div>'}
  </div>`;
}

async function connect(svc) {
  const form = (INFO[svc] || {}).form;
  if (form === 'oauth') {
    try { const r = await api.post(`/connections/${svc}`, {}); location.href = r.url; } catch (e) { toast(e.message, 'err'); }
    return;
  }
  if (form === 'ytmusic') return connectYTMusic();
  if (form === 'plex') {
    try {
      const r = await api.post('/connections/plex', {});
      const w = window.open(r.url, 'plexauth', 'width=600,height=750');
      toast('Sign in to Plex in the popup…');
      for (let i = 0; i < 120; i++) {
        await new Promise(res => setTimeout(res, 2000));
        const c = await api.post('/connections/plex/check', { pin_id: r.pin_id });
        if (c.done) { w?.close(); toast(`Plex connected (${c.account})`, 'ok'); window.dispatchEvent(new HashChangeEvent('hashchange')); return; }
      }
      toast('Plex sign-in timed out', 'err');
    } catch (e) { toast(e.message, 'err'); }
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

function connectYTMusic() {
  const m = modal({
    title: 'Connect YouTube Music',
    wide: true,
    body: `<div class="tabs" id="ytm-tabs"><button data-m="code" class="active">Sign in with a code</button><button data-m="headers">Paste browser headers</button></div>
      <div id="ytm-code" class="mt"><p class="muted" style="margin-top:0">You'll get a short code to enter at google.com/device with the Google account you use for YouTube Music.</p>
        <button class="btn primary" id="ytm-start">${icon.link}Get code</button><div id="ytm-out" class="mt"></div></div>
      <div id="ytm-headers" class="mt hidden">
        <ol class="small muted" style="margin-top:0;line-height:1.7"><li>Open music.youtube.com in a desktop browser while signed in, press F12 → Network.</li>
          <li>Click around, find a <span class="mono">browse</span> request (POST), right-click → Copy → <b>Copy request headers</b>.</li><li>Paste below.</li></ol>
        <textarea class="input mono" id="ytm-raw" rows="8" placeholder="accept: */*\ncookie: …"></textarea>
        <button class="btn primary mt" id="ytm-save">Connect</button></div>`,
  });
  const el = (s) => m.el.querySelector(s);
  el('#ytm-tabs').onclick = (e) => {
    const b = e.target.closest('[data-m]');
    if (!b) return;
    m.el.querySelectorAll('#ytm-tabs button').forEach(x => x.classList.toggle('active', x === b));
    el('#ytm-code').classList.toggle('hidden', b.dataset.m !== 'code');
    el('#ytm-headers').classList.toggle('hidden', b.dataset.m !== 'headers');
  };
  el('#ytm-save').onclick = async () => {
    try {
      const r = await api.post('/connections/ytmusic', { headers: el('#ytm-raw').value });
      toast(`YouTube Music connected (${r.account})`, 'ok'); m.close(); window.dispatchEvent(new HashChangeEvent('hashchange'));
    } catch (e) { toast(e.message, 'err'); }
  };
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
