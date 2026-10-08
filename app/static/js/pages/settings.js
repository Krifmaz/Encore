import { api } from '../api.js';
import { copyText, esc, icon, toast } from '../ui.js';

const S = (key, label, type = 'text', extra = {}) => ({ key, label, type, ...extra });

const SECTIONS = [
  { id: 'general', title: 'General', fields: [
    S('app_name', 'App name'),
    S('base_url', 'Public URL', 'text', { help: 'e.g. https://music.example.com – used in links & notifications' }),
    S('signup_mode', 'Sign-up mode', 'select', { options: [['invite', 'Invite only'], ['open', 'Open to anyone'], ['closed', 'Closed']] }),
    S('auto_approve', 'Auto-approve all requests', 'bool', { help: 'Otherwise only admins and users with auto-approve skip the approval queue' }),
    S('default_request_limit', 'Default weekly request limit for new users', 'number', { help: '0 = unlimited' }),
    S('session_days', 'Stay signed in for (days)', 'number'),
    S('contact_email', 'Contact email for MusicBrainz', 'text', { help: 'MusicBrainz asks API clients to identify themselves' }),
    S('musicbrainz_url', 'MusicBrainz server', 'text', { help: 'Use your own mirror for faster lookups' }),
    S('musicbrainz_rate', 'MusicBrainz requests / second', 'number', { step: '0.1', help: 'Keep at 1 for the public server' }),
  ] },
  { id: 'library', title: 'Library & tagging', fields: [
    S('library_path', 'Music library path', 'text', { help: 'Inside the container (mount your music folder here)' }),
    S('staging_path', 'Staging path for yt-dlp downloads'),
    S('scan_interval_minutes', 'Rescan library every (minutes)', 'number', { help: '0 disables periodic scans' }),
    S('autotag_on_import', 'Tag downloads from MusicBrainz on import', 'bool'),
    S('autotag_threshold', 'Auto-tag confidence threshold', 'number', { step: '0.05', help: '0–1. Matches below this are imported with their existing tags' }),
    S('rename_on_import', 'Rename & organise files', 'bool'),
    S('naming_template', 'Album naming template', 'text', { full: true, help: 'Fields: {albumartist} {artist} {album} {year} {disc} {discprefix} {track:02d} {title} {genre} {label} {type} {format}' }),
    S('single_template', 'Single-track naming template', 'text', { full: true }),
    S('save_cover', 'Save cover.jpg in album folders', 'bool'),
    S('embed_cover', 'Embed cover art in files', 'bool'),
  ] },
  { id: 'pipeline', title: 'Download pipeline', fields: [
    S('source_order', 'Source order', 'text', { full: true, help: 'Comma-separated: lidarr, streamrip, slskd, ytdlp. Each source is tried in turn until one succeeds. Lidarr only handles albums.' }),
  ] },
  { id: 'verify', title: 'Verification & duplicates', test: 'acoustid', fields: [
    S('acoustid_api_key', 'AcoustID API key', 'password', { full: true, help: 'Free – register an application at acoustid.org/new-application. Used to fingerprint audio and confirm each file is the right song/version.' }),
    S('verify_on_import', 'Fingerprint every download before import', 'bool'),
    S('reject_mismatch', 'Reject downloads whose audio is a different song/version (and try the next source)', 'bool'),
    S('dedupe_on_import', 'Never import a second copy of a song (keep the better-quality file)', 'bool'),
  ] },
  { id: 'streamrip', title: 'Qobuz / Tidal / Deezer (streamrip)', test: 'streamrip', testLabel: 'Test & write config', fields: [
    S('streamrip_enabled', 'Enable streamrip source', 'bool', { help: 'Downloads lossless from your own paid accounts. Respect each service’s terms – use at your own risk.' }),
    S('streamrip_sources', 'Services to search (in order)', 'text', { help: 'deezer, tidal (Qobuz links work in the Downloader)' }),
    S('streamrip_quality', 'Max quality', 'select', { options: [[1, 'MP3 320 / AAC'], [2, 'CD – 16-bit FLAC'], [3, 'Hi-res ≤ 24/96'], [4, 'Hi-res max']] }),
    S('streamrip_deezer_arl', 'Deezer ARL cookie', 'password', { full: true, help: 'From deezer.com → browser dev tools → Cookies → arl' }),
    S('streamrip_qobuz_user', 'Qobuz user id'),
    S('streamrip_qobuz_token', 'Qobuz auth token', 'password', { help: 'play.qobuz.com → dev tools → Network → user/login response' }),
    S('streamrip_bin', 'streamrip executable (optional)', 'text', { help: 'Leave empty to auto-detect' }),
    S('streamrip_command', 'Command template (advanced)', 'text', { full: true }),
  ] },
  { id: 'sync', title: 'Playlists & sync', fields: [
    S('sync_interval_minutes', 'Sync likes & playlists every (minutes)', 'number', { help: '0 disables automatic sync' }),
    S('auto_request_followed', 'Automatically download new tracks in followed playlists (more rules under Admin → Auto-download)', 'bool'),
  ] },
  { id: 'lidarr', title: 'Lidarr', test: 'lidarr', fields: [
    S('lidarr_enabled', 'Enable Lidarr', 'bool'),
    S('lidarr_url', 'URL', 'text', { help: 'Internal address, e.g. http://lidarr:8686' }),
    S('lidarr_api_key', 'API key', 'password'),
    S('lidarr_root_folder', 'Root folder', 'select', { options: [], dynamic: 'root_folders' }),
    S('lidarr_quality_profile', 'Quality profile', 'select', { options: [], dynamic: 'quality_profiles' }),
    S('lidarr_metadata_profile', 'Metadata profile', 'select', { options: [], dynamic: 'metadata_profiles' }),
    S('lidarr_timeout_minutes', 'Fall back to next source after (minutes)', 'number'),
  ] },
  { id: 'slskd', title: 'Soulseek (slskd)', test: 'slskd', fields: [
    S('slskd_enabled', 'Enable slskd', 'bool'),
    S('slskd_url', 'URL', 'text', { help: 'Internal address, e.g. http://slskd:5030' }),
    S('slskd_api_key', 'API key', 'password', { help: 'Set in slskd.yml under web.authentication.api_keys' }),
    S('slskd_download_path', 'slskd downloads folder (as mounted in this container)', 'text', { full: true }),
    S('slskd_preferred_format', 'Preferred format', 'select', { options: [['flac', 'FLAC / lossless'], ['mp3', 'MP3'], ['any', 'Best available']] }),
    S('slskd_min_bitrate', 'Minimum MP3 bitrate (kbps)', 'number'),
    S('slskd_search_timeout', 'Search time (seconds)', 'number'),
    S('slskd_stall_minutes', 'Give up on a stalled user after (minutes)', 'number'),
    S('slskd_max_candidates', 'Max users to try per request', 'number'),
  ] },
  { id: 'ytdlp', title: 'YouTube Music / SoundCloud (yt-dlp)', fields: [
    S('ytdlp_enabled', 'Enable yt-dlp as a fallback source', 'bool'),
    S('ytdlp_format', 'Audio format', 'select', { options: [['best', 'Original stream – best quality, no re-encode (recommended)'], ['m4a', 'M4A (AAC, re-encoded)'], ['mp3', 'MP3 (re-encoded)'], ['opus', 'Opus']] }),
    S('ytdlp_quality', 'MP3/M4A quality', 'text', { help: 'Bitrate like 320, or 0 for best VBR' }),
    S('ytdlp_cookies', 'cookies.txt contents (optional)', 'textarea', { full: true, secret: true, help: 'Netscape cookie file from music.youtube.com. With a YouTube Music Premium account this unlocks 256 kbps audio. Export from a private window so the cookies don’t rotate.' }),
  ] },
  { id: 'spotify', title: 'Integrations', test: 'spotify', testLabel: 'Test Spotify', fields: [
    S('spotify_client_id', 'Spotify Client ID', 'text', { help: 'developer.spotify.com – add <Public URL>/api/connections/spotify/callback as redirect URI. Development-mode apps allow 5 users and need a Premium owner (2026 rules); without it, public playlists still import via the embed fallback.' }),
    S('spotify_client_secret', 'Spotify Client secret', 'password'),
    S('tidal_client_id', 'Tidal Client ID', 'text', { help: 'developer.tidal.com – redirect URI <Public URL>/api/connections/tidal/callback' }),
    S('tidal_client_secret', 'Tidal Client secret', 'password'),
    S('lastfm_api_key', 'Last.fm API key', 'text', { help: 'last.fm/api/account/create – for importing loved tracks' }),
    S('ytmusic_client_id', 'YouTube Music OAuth Client ID', 'text', { help: 'Google Cloud → YouTube Data API v3 → OAuth client of type “TVs and Limited Input devices”. Lets users sign in to YouTube Music with a code (they can also paste browser headers instead).' }),
    S('ytmusic_client_secret', 'YouTube Music OAuth Client secret', 'password'),
  ] },
  { id: 'media', title: 'Media servers', test: 'mediaservers', testLabel: 'Trigger refresh', fields: [
    S('navidrome_url', 'Navidrome URL', 'text', { help: 'URLs here let users link their own accounts on the Connections page; the admin credentials below are only used to trigger library rescans' }), S('navidrome_user', 'Navidrome user'), S('navidrome_password', 'Navidrome password', 'password'),
    S('jellyfin_url', 'Jellyfin URL'), S('jellyfin_api_key', 'Jellyfin API key', 'password'),
    S('plex_url', 'Plex URL'), S('plex_token', 'Plex token', 'password'),
  ] },
  { id: 'notify', title: 'Notifications', test: 'notify', testLabel: 'Send test', fields: [
    S('discord_webhook', 'Discord webhook URL', 'password', { full: true }),
    S('ntfy_url', 'ntfy topic URL', 'text', { help: 'e.g. https://ntfy.sh/my-music' }),
    S('ntfy_token', 'ntfy access token', 'password'),
    S('webhook_url', 'Generic webhook (JSON POST)', 'password', { full: true }),
  ] },
];

const MASK = '••••';
const isMasked = (v) => typeof v === 'string' && v.startsWith(MASK);

function sourceNote(f, v, meta) {
  const m = meta[f.key] || {};
  const secret = f.type === 'password' || f.secret;
  if (m.source === 'env') {
    return `<small class="src env">🔒 Set by <span class="mono">${esc(m.env)}</span> in your compose/env${secret && v ? ` · ${esc(v)}` : ''} – change it there (environment always wins)</small>`;
  }
  if (secret && v) return `<small class="src">Saved in Encore · ${esc(v)} · <a href="#" data-clear="${f.key}">clear</a></small>`;
  if (secret) return `<small class="src">Not set${m.env ? ` – or set <span class="mono">${esc(m.env)}</span> in your compose file` : ''}</small>`;
  if (m.source === 'ui') return '';
  return m.env ? `<small class="src faint">Default · env var <span class="mono">${esc(m.env)}</span></small>` : '';
}

function field(f, v, opts, meta = {}) {
  const id = `f-${f.key}`;
  const locked = (meta[f.key] || {}).source === 'env';
  const dis = locked ? 'disabled' : '';
  const help = (f.help ? `<small>${esc(f.help)}</small>` : '') + sourceNote(f, v, meta);
  const cls = f.full || f.type === 'textarea' ? 'full' : '';
  if (f.type === 'bool') {
    return `<div class="field ${cls}" style="justify-content:center"><label class="switch"><input type="checkbox" data-k="${f.key}" ${v ? 'checked' : ''} ${dis}><i></i>${esc(f.label)}</label>${help}</div>`;
  }
  if (f.type === 'select') {
    let options = f.options;
    if (f.dynamic) {
      const list = (opts && opts[f.dynamic]) || [];
      options = list.map(o => [o.path ?? o.id, o.path ?? o.name]);
      if (v && !options.some(o => String(o[0]) === String(v))) options.unshift([v, `${v}`]);
      options.unshift([f.key === 'lidarr_root_folder' ? '' : 0, 'Lidarr default (first)']);
    }
    return `<label class="field ${cls}"><span>${esc(f.label)}</span><select class="input" data-k="${f.key}" id="${id}" ${dis}>
      ${options.map(([val, lab]) => `<option value="${esc(val)}" ${String(val) === String(v) ? 'selected' : ''}>${esc(lab)}</option>`).join('')}</select>${help}</label>`;
  }
  if (f.type === 'textarea') {
    return `<label class="field ${cls}"><span>${esc(f.label)}</span><textarea class="input mono" data-k="${f.key}" ${dis} placeholder="${v ? 'Saved – paste to replace' : ''}"></textarea>${help}</label>`;
  }
  if (f.type === 'password') {
    // secrets are never sent to the browser: empty input = keep the saved value
    return `<label class="field ${cls}"><span>${esc(f.label)}</span><input class="input" type="password" data-k="${f.key}" value="" ${dis}
      autocomplete="new-password" placeholder="${locked ? 'set by environment' : v ? 'saved – paste a new value to replace' : 'not set'}">${help}</label>`;
  }
  const type = f.type === 'number' ? 'number' : 'text';
  return `<label class="field ${cls}"><span>${esc(f.label)}</span><input class="input" type="${type}" data-k="${f.key}" ${f.step ? `step="${f.step}"` : ''}
    value="${esc(v ?? '')}" autocomplete="off" ${dis}>${help}</label>`;
}

export async function render(view, ctx) {
  let values = await api.get('/admin/settings');
  let lidarrOpts = null;
  let current = ctx.query.s || 'general';
  view.innerHTML = `
    <h1 style="margin-bottom:18px">Settings</h1>
    <div class="settings-grid">
      <nav class="settings-nav">${SECTIONS.map(s => `<button data-s="${s.id}">${esc(s.title)}</button>`).join('')}<button data-s="api">API & security</button></nav>
      <div id="panel"></div>
    </div>`;
  const panel = view.querySelector('#panel');

  const show = async (id, flash = null) => {
    current = id;
    view.querySelectorAll('.settings-nav button').forEach(b => b.classList.toggle('active', b.dataset.s === id));
    if (id === 'api') return showApi();
    const sec = SECTIONS.find(s => s.id === id);
    if (id === 'lidarr' && values.lidarr_api_key && !lidarrOpts) {
      try { lidarrOpts = await api.get('/system/lidarr/options'); } catch { lidarrOpts = null; }
    }
    const meta = values._meta || {};
    panel.innerHTML = `<div class="card">
      <div class="row between" style="margin-bottom:18px"><h2>${esc(sec.title)}</h2>
        ${sec.test ? `<button class="btn sm" id="test">${icon.refresh}${esc(sec.testLabel || 'Test connection')}</button>` : ''}</div>
      <div id="test-out"></div>
      <div class="form-grid">${sec.fields.map(f => field(f, values[f.key], lidarrOpts, meta)).join('')}</div>
      <div class="row mt-lg" style="justify-content:flex-end"><button class="btn primary" id="save">${icon.check}Save</button></div>
    </div>
    ${id === 'pipeline' ? pipelineHelp() : ''}`;
    const out = panel.querySelector('#test-out');
    const paint = (r) => {
      out.innerHTML = `<div class="test-result ${r.ok ? 'ok' : 'err'}">${r.ok ? icon.check : icon.x}<span>${esc(r.message || (r.ok ? 'Connected' : 'Failed'))}</span></div>`;
    };
    if (flash) paint(flash);
    panel.querySelector('#save').onclick = () => save(sec);
    panel.querySelectorAll('[data-clear]').forEach(a => a.addEventListener('click', async (e) => {
      e.preventDefault();
      await api.put('/admin/settings', { [a.dataset.clear]: '' }).then(v => { values = v; });
      toast('Cleared', 'ok');
      show(id);
    }));
    const t = panel.querySelector('#test');
    if (t) t.onclick = async () => {
      t.disabled = true;
      out.innerHTML = '<div class="test-result"><span class="spinner"></span><span>Testing…</span></div>';
      let r;
      try {
        await save(sec, true);
        r = await api.post(`/system/test/${sec.test}`);
      } catch (e) {
        r = { ok: false, message: e.message || 'Test failed' };
      }
      if (r.options) {
        // Lidarr returned its profiles/root folders: redraw the dropdowns, keep the result visible
        lidarrOpts = r.options;
        await show(id, r);
      } else {
        paint(r);
      }
      t.disabled = false;
    };
  };

  const save = async (sec, quiet = false) => {
    const body = {};
    const meta = values._meta || {};
    panel.querySelectorAll('[data-k]').forEach(el => {
      const f = sec.fields.find(x => x.key === el.dataset.k);
      if ((meta[f.key] || {}).source === 'env') return;            // locked by environment
      if (el.type === 'checkbox') body[f.key] = el.checked;
      else if ((f.type === 'password' || f.secret) && el.value.trim() === '') { /* keep saved secret */ }
      else if (isMasked(el.value)) { /* never echo a mask back */ }
      else body[f.key] = f.type === 'password' || f.secret ? el.value.trim() : el.value;
    });
    try {
      values = await api.put('/admin/settings', body);
      if (!quiet) toast('Settings saved', 'ok');
      if (sec.id === 'general' && body.app_name) document.title = body.app_name;
    } catch (e) {
      toast(e.message, 'err');
      if (quiet) throw e;
    }
  };

  const showApi = async () => {
    const { api_key } = await api.get('/system/api-key');
    panel.innerHTML = `<div class="card stack">
      <h2>API access</h2>
      <p class="muted" style="margin:0">Use this key in an <span class="kbd">X-Api-Key</span> header to automate requests (e.g. from scripts or Home Assistant). It acts as the first admin account. Full API docs at <a href="/api/docs" target="_blank" style="color:var(--accent)">/api/docs</a>.</p>
      <div class="row"><div class="code grow" id="key">${esc(api_key)}</div><button class="btn" id="cp">${icon.copy}</button><button class="btn danger" id="regen">${icon.refresh}Regenerate</button></div>
    </div>
    <div class="card stack mt">
      <h2>Security notes</h2>
      <ul class="muted" style="margin:0;padding-left:18px;line-height:1.8">
        <li>Only expose <b>this app</b> publicly (ideally behind HTTPS via a reverse proxy). Keep Lidarr and slskd on your internal Docker network – Encore talks to them for your users.</li>
        <li>Sign-ups are <b>${esc(values.signup_mode)}</b>. Manage access under <a href="#/admin/invites" style="color:var(--accent)">Invites</a> and <a href="#/admin/users" style="color:var(--accent)">Users</a>.</li>
        <li>Sessions are HTTP-only SameSite cookies, every change needs a CSRF header, and failed logins are rate-limited per IP.</li>
      </ul>
    </div>`;
    panel.querySelector('#cp').onclick = () => copyText(panel.querySelector('#key').textContent);
    panel.querySelector('#regen').onclick = async () => {
      const r = await api.post('/admin/settings/api-key');
      panel.querySelector('#key').textContent = r.api_key;
      toast('New API key generated', 'ok');
    };
  };

  view.querySelector('.settings-nav').onclick = (e) => { const b = e.target.closest('[data-s]'); if (b) show(b.dataset.s); };
  show(current);
}

function pipelineHelp() {
  return `<div class="card mt"><h3>How requests are fulfilled</h3>
    <ol class="muted" style="line-height:1.9;padding-left:18px;margin:10px 0 0">
      <li><b>Library check</b> – if the album/track is already in your library (by MusicBrainz id or name) it's marked available instantly.</li>
      <li><b>Lidarr</b> – the artist is added (unmonitored), the requested album is monitored and searched on your indexers. If nothing is grabbed before the timeout, the next source is tried (the album stays monitored).</li>
      <li><b>Soulseek</b> – slskd searches the network, results are ranked by track count, format, bitrate, folder name and the uploader's free slots/speed. Stalled or failed users are skipped automatically.</li>
      <li><b>yt-dlp</b> – each track from the MusicBrainz tracklist is matched on YouTube (preferring official "Topic" uploads with matching length).</li>
      <li><b>Import</b> – files are matched to the MusicBrainz release, tagged (incl. MBIDs, ISRCs, cover art), renamed with your template, and your media servers are refreshed.</li>
    </ol></div>`;
}
