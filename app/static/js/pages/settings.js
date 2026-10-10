import { api } from '../api.js';
import { copyText, esc, icon, setSecretText, toast } from '../ui.js';
import * as accountPage from './account.js';
import * as connectionsPage from './connections.js';
import * as adminPage from './admin.js';
import * as autoPage from './autodownload.js';
import * as diagPage from './diagnostics.js';

const S = (key, label, type = 'text', extra = {}) => ({ key, label, type, ...extra });

const SECTIONS = [
  { id: 'general', title: 'General', fields: [
    S('app_name', 'App name'),
    S('base_url', 'Public URL', 'text', { help: 'e.g. https://music.example.com – used in links & notifications' }),
    S('signup_mode', 'Sign-up mode', 'select', { options: [['invite', 'Invite only'], ['open', 'Open to anyone'], ['closed', 'Closed']] }),
    S('auto_approve', 'Auto-approve all requests', 'bool', { help: 'Otherwise only admins and users with auto-approve skip the approval queue' }),
    S('manual_search', 'Manual download picker', 'select', { options: [['everyone', 'Everyone'], ['admins', 'Admins only']], help: 'Who can search the sources and pick an exact download' }),
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
    S('discography_singles', 'Requesting a whole discography includes singles (not just albums and EPs)', 'bool'),
    S('source_order', 'Source order', 'text', { full: true, help: 'Comma-separated: lidarr, streamrip, hifi, slskd, ytdlp. Each source is tried in turn until one succeeds. Lidarr only handles albums.' }),
  ] },
  { id: 'verify', title: 'Verification & duplicates', test: 'acoustid', fields: [
    S('acoustid_api_key', 'AcoustID API key', 'password', { full: true, help: 'Free – register an application at acoustid.org/new-application. Used to fingerprint audio and confirm each file is the right song/version.' }),
    S('verify_on_import', 'Fingerprint every download before import', 'bool'),
    S('reject_mismatch', 'Reject downloads whose audio is a different song/version (and try the next source)', 'bool'),
    S('dedupe_on_import', 'Never import a second copy of a song (keep the better-quality file)', 'bool'),
    S('upgrade_check_audio', 'Before replacing a song with a better copy, compare the audio – a different recording is kept as its own song', 'bool'),
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
  { id: 'hifi', title: 'TIDAL via HiFi API (Monochrome)', test: 'hifi', fields: [
    S('hifi_enabled', 'Enable HiFi API source', 'bool', { help: 'Lossless TIDAL downloads through a HiFi API instance – the backend Monochrome uses. Run your own instance with your own TIDAL subscription.' }),
    S('hifi_api_url', 'HiFi API URL(s)', 'text', { full: true, help: 'e.g. http://hifi-api:8000 – several can be given, comma-separated; they are tried in order' }),
    S('hifi_quality', 'Quality', 'select', { options: [['LOSSLESS', 'Lossless – 16-bit FLAC'], ['HI_RES_LOSSLESS', 'Hi-res FLAC (up to 24/192)'], ['HIGH', 'AAC 320']] }),
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
    S('ytdlp_enabled', 'Enable yt-dlp as a source', 'bool'),
    S('ytdlp_last_resort', 'Always try YouTube Music last, so downloads (almost) never fail', 'bool'),
    S('ytdlp_format', 'Audio format', 'select', { options: [['best', 'Original stream – best quality, no re-encode (recommended)'], ['m4a', 'M4A (AAC, re-encoded)'], ['mp3', 'MP3 (re-encoded)'], ['opus', 'Opus']] }),
    S('ytdlp_quality', 'MP3/M4A quality', 'text', { help: 'Bitrate like 320, or 0 for best VBR' }),
    S('ytdlp_cookies', 'cookies.txt contents (optional)', 'textarea', { full: true, secret: true, help: 'Netscape cookie file from music.youtube.com. With a YouTube Music Premium account this unlocks 256 kbps audio. Export from a private window so the cookies don’t rotate.' }),
  ] },
  { id: 'spotify', title: 'Integrations', test: 'spotify', testLabel: 'Test Spotify', fields: [
    S('spotify_client_id', 'Spotify Client ID', 'text', { help: 'developer.spotify.com – add <Public URL>/api/connections/spotify/callback as redirect URI. Development-mode apps allow 5 users and need a Premium owner (2026 rules); without it, public playlists still import via the embed fallback.' }),
    S('spotify_client_secret', 'Spotify Client secret', 'password'),
    S('tidal_client_id', 'Tidal Client ID', 'text', { help: 'developer.tidal.com – redirect URI <Public URL>/api/connections/tidal/callback' }),
    S('tidal_client_secret', 'Tidal Client secret', 'password'),
    S('lastfm_api_key', 'Last.fm API key', 'password', { help: 'last.fm/api/account/create (free) – loved tracks, similar artists/songs, genre pages' }),
    S('discogs_token', 'Discogs personal access token', 'password', { help: 'discogs.com/settings/developers (free) – finds vinyl, bootlegs and old releases MusicBrainz is missing' }),
    S('ytmusic_client_id', 'YouTube Music OAuth Client ID', 'text', { help: 'Google Cloud → YouTube Data API v3 → OAuth client of type “TVs and Limited Input devices”. Lets users sign in to YouTube Music with a code (they can also paste browser headers instead).' }),
    S('ytmusic_client_secret', 'YouTube Music OAuth Client secret', 'password'),
  ] },
  { id: 'media', title: 'Media servers', test: 'mediaservers', testLabel: 'Trigger refresh', fields: [
    S('navidrome_url', 'Navidrome URL', 'text', { help: 'URLs here let users link their own accounts on the Connections page; the admin credentials below are only used to trigger library rescans' }), S('navidrome_user', 'Navidrome user'), S('navidrome_password', 'Navidrome password', 'password'),
    S('jellyfin_url', 'Jellyfin URL'), S('jellyfin_api_key', 'Jellyfin API key', 'password'),
    S('plex_url', 'Plex URL'), S('plex_token', 'Plex token', 'password'),
  ] },
  { id: 'email', title: 'Email', test: 'email', testLabel: 'Send test email', fields: [
    S('smtp_host', 'SMTP server', 'text', { help: 'e.g. smtp.fastmail.com – used for password-reset emails' }),
    S('smtp_port', 'Port', 'number', { help: '587 (STARTTLS) or 465 (SSL)' }),
    S('smtp_user', 'Username'),
    S('smtp_password', 'Password', 'password'),
    S('smtp_from', 'From address', 'text', { help: 'e.g. Encore <music@example.com>' }),
    S('smtp_security', 'Security', 'select', { options: [['starttls', 'STARTTLS'], ['ssl', 'SSL/TLS'], ['none', 'None']] }),
  ] },
  { id: 'notify', title: 'Notifications', test: 'notify', testLabel: 'Send test', fields: [
    S('discord_webhook', 'Discord webhook URL', 'password', { full: true }),
    S('ntfy_url', 'ntfy topic URL', 'text', { help: 'e.g. https://ntfy.sh/my-music' }),
    S('ntfy_token', 'ntfy access token', 'password'),
    S('webhook_url', 'Generic webhook (JSON POST)', 'password', { full: true }),
  ] },
];

// Every key, token and password lives on one page; the other pages keep the non-secret options.
const KEY_FIELD = (f) => f.type === 'password' || f.secret || /client_id$|qobuz_user$/.test(f.key);
const KEYS = { id: 'keys', title: 'API keys & passwords', fields: [] };
for (const sec of SECTIONS) {
  const keys = sec.fields.filter(KEY_FIELD);
  keys.forEach(f => KEYS.fields.push({ ...f, label: `${f.label} – ${sec.title}`, full: true }));
  sec.fields = sec.fields.filter(f => !KEY_FIELD(f));
}
SECTIONS.push(KEYS);

// Settings pages: related cards together, in an order that reads top to bottom.
const GROUPS = [
  { id: 'general', title: 'General', sections: ['general'] },
  { id: 'keys', title: 'API keys & passwords', sections: ['keys'] },
  { id: 'downloads', title: 'Downloads & sources', sections: ['pipeline', 'slskd', 'ytdlp', 'hifi', 'streamrip', 'lidarr'] },
  { id: 'tagging', title: 'Library, tagging & IDs', sections: ['library', 'verify'] },
  { id: 'accounts', title: 'Email & notifications', sections: ['email', 'notify'] },
  { id: 'connections', title: 'Services & sync', sections: ['spotify', 'sync', 'media'] },
];
const groupOf = (id) => (GROUPS.find(g => g.id === id || g.sections.includes(id)) || {}).id || id;

const MASK = '••••';
const isMasked = (v) => typeof v === 'string' && v.startsWith(MASK);

function sourceNote(f, v, meta) {
  const m = meta[f.key] || {};
  const secret = f.type === 'password' || f.secret;
  if (m.source === 'env') {
    return `<small class="src env">🔒 Set by <span class="mono">${esc(m.env)}</span> in your compose/env – change it there (environment always wins)</small>`;
  }
  if (secret && v) return `<small class="src">Saved ✓ (hidden) · <a href="#" data-clear="${f.key}">clear</a></small>`;
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

const EVERY = [[15, '15 minutes'], [30, '30 minutes'], [60, 'hour'], [180, '3 hours'], [360, '6 hours'], [720, '12 hours'], [1440, 'day'], [10080, 'week']];
const ago = (t) => { if (!t) return 'never'; const s = Date.now() / 1000 - t; return s < 90 ? 'just now' : s < 5400 ? `${Math.round(s / 60)} min ago` : s < 129600 ? `${Math.round(s / 3600)} h ago` : `${Math.round(s / 86400)} days ago`; };
const until = (t) => { if (!t) return ''; const s = t - Date.now() / 1000; return s <= 60 ? 'any moment' : s < 5400 ? `in ${Math.round(s / 60)} min` : s < 129600 ? `in ${Math.round(s / 3600)} h` : `in ${Math.round(s / 86400)} days`; };

/** Settings → Scheduled tasks: every background job, with on/off, interval, last result and Run now. */
async function renderTasks(panel) {
  const d = await api.get('/admin/tasks');
  panel.innerHTML = `<div class="card"><div class="row between" style="margin-bottom:8px"><h2>Scheduled tasks</h2><button class="btn sm" id="t-refresh">${icon.refresh}Refresh</button></div>
    <p class="muted small" style="margin:0 0 14px">Background jobs Encore runs on its own. Switch them off, change how often they run, or run one now.</p>
    <div class="list">${d.tasks.map(t => `<div class="list-item" style="align-items:flex-start;flex-wrap:wrap;gap:12px" data-k="${t.key}">
      <label class="switch" style="margin-top:2px" title="${t.locked ? 'Set by an environment variable' : ''}"><input type="checkbox" data-on ${t.enabled ? 'checked' : ''} ${t.locked ? 'disabled' : ''}><i></i></label>
      <div class="grow" style="min-width:220px"><div style="font-weight:650">${esc(t.name)}${t.locked ? ' <span title="Set by an environment variable">🔒</span>' : ''}</div>
        <div class="small muted">${esc(t.desc)}</div>
        <div class="small faint" style="margin-top:4px">Last run ${ago(t.last_run)}${t.duration ? ` · took ${t.duration < 60 ? Math.round(t.duration) + ' s' : Math.round(t.duration / 60) + ' min'}` : ''}${t.running ? ' · <b style="color:var(--accent)">running…</b>' : ''}
          ${t.error ? ` · <span style="color:var(--err)">${esc(t.error)}</span>` : t.result && !t.running ? ` · ${esc(t.result)}` : ''}${t.enabled && t.next_run && !t.running ? ` · next ${until(t.next_run)}` : ''}</div></div>
      <div class="row" style="gap:8px"><span class="small muted">every</span>
        <select class="input" data-every style="width:auto" ${t.locked ? 'disabled' : ''}>${[...new Set([...EVERY.map(e => e[0]), t.minutes])].sort((a, b) => a - b).map(m => `<option value="${m}" ${m === t.minutes ? 'selected' : ''}>${(EVERY.find(e => e[0] === m) || [0, m + ' minutes'])[1]}</option>`).join('')}</select>
        <button class="btn sm" data-run ${t.running ? 'disabled' : ''}>${icon.play}Run now</button></div>
    </div>`).join('')}</div>
    <div class="row mt" style="gap:8px"><span class="small muted">Clean up finished downloads older than</span>
      <input class="input" type="number" min="1" id="t-days" value="${d.cleanup_days}" style="width:90px"><span class="small muted">days</span></div></div>`;
  panel.querySelector('#t-refresh').onclick = () => renderTasks(panel);
  panel.querySelector('#t-days').onchange = async (e) => { await api.put('/admin/settings', { task_cleanup_days: +e.target.value }); toast('Saved', 'ok'); };
  panel.querySelectorAll('[data-k]').forEach(row => {
    const k = row.dataset.k;
    row.querySelector('[data-on]').onchange = async (e) => {
      try { await api.patch(`/admin/tasks/${k}`, { enabled: e.target.checked }); toast(e.target.checked ? 'Enabled' : 'Disabled', 'ok'); renderTasks(panel); } catch (err) { toast(err.message, 'err'); }
    };
    row.querySelector('[data-every]').onchange = async (e) => {
      try { await api.patch(`/admin/tasks/${k}`, { minutes: +e.target.value }); toast('Saved', 'ok'); renderTasks(panel); } catch (err) { toast(err.message, 'err'); }
    };
    row.querySelector('[data-run]').onclick = async (e) => {
      e.currentTarget.disabled = true;
      try { await api.post(`/admin/tasks/${k}/run`); toast('Started', 'ok'); setTimeout(() => renderTasks(panel), 1500); } catch (err) { toast(err.message, 'err'); }
    };
  });
}

// Pages that are whole screens of their own, shown inside Settings
const EMBEDDED = {
  account: (el, ctx) => accountPage.render(el, ctx),
  connections: (el, ctx) => connectionsPage.render(el, ctx),
  users: (el, ctx) => adminPage.renderUsers(el, ctx),
  invites: (el, ctx) => adminPage.renderInvites(el, ctx),
  autodownload: (el) => autoPage.render(el),
  diagnostics: (el) => diagPage.render(el),
};

export async function render(view, ctx) {
  const admin = ctx.user?.role === 'admin';
  let values = admin ? await api.get('/admin/settings') : {};
  let lidarrOpts = null;
  let current = ctx.query.s ? groupOf(ctx.query.s) : 'account';
  const nav = [
    ['You', [['account', 'Account'], ['connections', 'Connections']]],
    ...(admin ? [
      ['People', [['users', 'Users & permissions'], ['invites', 'Invites']]],
      ['Server', [...GROUPS.map(g => [g.id, g.title]), ['autodownload', 'Auto-download'], ['tasks', 'Scheduled tasks']]],
      ['System', [['diagnostics', 'Diagnostics'], ['api', 'API & security']]],
    ] : []),
    ['', [['about', 'About']]],
  ];
  const pages = nav.flatMap(([, items]) => items);
  if (!pages.some(([id]) => id === current)) current = 'account';
  view.innerHTML = `
    <h1 style="margin-bottom:18px">Settings</h1>
    <select class="input settings-select" aria-label="Settings page">${nav.map(([label, items]) => `<optgroup label="${esc(label || 'Encore')}">${items.map(([id, t]) => `<option value="${id}">${esc(t)}</option>`).join('')}</optgroup>`).join('')}</select>
    <div class="settings-grid">
      <nav class="settings-nav">${nav.map(([label, items]) => `${label ? `<div class="nav-label">${esc(label)}</div>` : '<div style="height:10px"></div>'}${items.map(([id, t]) => `<button data-s="${id}">${esc(t)}</button>`).join('')}`).join('')}</nav>
      <div id="panel"></div>
    </div>`;
  const panel = view.querySelector('#panel');

  const show = async (id, flash = null) => {
    current = id;
    view.querySelectorAll('.settings-nav button').forEach(b => b.classList.toggle('active', b.dataset.s === id));
    view.querySelector('.settings-select').value = id;
    history.replaceState(null, '', `#/settings?s=${id}`);
    if (EMBEDDED[id]) { panel.innerHTML = '<div class="skeleton" style="height:200px"></div>'; return EMBEDDED[id](panel, ctx); }
    if (id === 'about') return showAbout();
    if (id === 'api') return showApi();
    if (id === 'tasks') return renderTasks(panel);
    const group = GROUPS.find(g => g.id === id) || GROUPS[0];
    const secs = group.sections.map(sid => SECTIONS.find(x => x.id === sid));
    if (secs.some(x => x.id === 'lidarr') && values.lidarr_api_key && !lidarrOpts) {
      try { lidarrOpts = await api.get('/system/lidarr/options'); } catch { lidarrOpts = null; }
    }
    const meta = values._meta || {};
    panel.innerHTML = secs.map((sec, i) => `<details class="card settings-card" data-sec="${sec.id}" ${i === 0 || secs.length === 1 || (flash && flash.sec === sec.id) ? 'open' : ''}>
      <summary class="row between" style="gap:10px;flex-wrap:wrap"><h2>${esc(sec.title)}</h2>
        <span class="row" style="gap:8px">${sec.test ? `<button class="btn sm" data-test>${icon.refresh}${esc(sec.testLabel || 'Test connection')}</button>` : ''}<span class="chev">${icon.chevron || '▾'}</span></span></summary>
      <div style="height:14px"></div>
      <div data-out>${flash && flash.sec === sec.id ? `<div class="test-result ${flash.ok ? 'ok' : 'err'}">${flash.ok ? icon.check : icon.x}<span>${esc(flash.message || '')}</span></div>` : ''}</div>
      <div class="form-grid">${sec.fields.map(f => field(f, values[f.key], lidarrOpts, meta)).join('')}</div>
      <div class="row mt" style="justify-content:flex-end"><button class="btn primary" data-save>${icon.check}Save</button></div>
    </details>`).join('') + (group.id === 'downloads' ? pipelineHelp() : '');
    panel.querySelectorAll('[data-sec]').forEach(card => {
      const sec = SECTIONS.find(x => x.id === card.dataset.sec);
      const out = card.querySelector('[data-out]');
      const paint = (r) => { out.innerHTML = `<div class="test-result ${r.ok ? 'ok' : 'err'}">${r.ok ? icon.check : icon.x}<span>${esc(r.message || (r.ok ? 'Connected' : 'Failed'))}</span></div>`; };
      card.querySelector('[data-save]').onclick = () => save(sec, false, card);
      card.querySelectorAll('[data-clear]').forEach(a => a.addEventListener('click', async (e) => {
        e.preventDefault();
        await api.put('/admin/settings', { [a.dataset.clear]: '' }).then(v => { values = v; });
        toast('Cleared', 'ok');
        show(id);
      }));
      const t = card.querySelector('[data-test]');
      if (t) t.onclick = async (ev) => {
        ev.preventDefault();
        card.open = true;
        t.disabled = true;
        out.innerHTML = '<div class="test-result"><span class="spinner"></span><span>Testing…</span></div>';
        let r;
        try {
          await save(sec, true, card);
          r = await api.post(`/system/test/${sec.test}`);
        } catch (e) {
          r = { ok: false, message: e.message || 'Test failed' };
        }
        if (r.options) { lidarrOpts = r.options; await show(id, { ...r, sec: sec.id }); } else paint(r);
        t.disabled = false;
      };
    });
  };

  const save = async (sec, quiet = false, root = panel) => {
    const body = {};
    const meta = values._meta || {};
    root.querySelectorAll('[data-k]').forEach(el => {
      const f = sec.fields.find(x => x.key === el.dataset.k);
      if (!f) return;
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

  const showAbout = async () => {
    const info = (await import('../app.js')).session.info || {};
    panel.innerHTML = `<div class="card stack"><h2>About Encore</h2>
      <div class="row wrap" style="gap:24px"><div><div class="small muted">Version</div><div style="font-weight:700;font-size:20px">v${esc(info.version || '?')}</div></div>
        <div><div class="small muted">Build</div><div class="mono">${esc(info.build || 'dev')}</div></div></div>
      <div class="small muted">Apps for Android, Windows, macOS and Linux: <a href="https://github.com/Krifmaz/Encore/releases" target="_blank" rel="noopener">github.com/Krifmaz/Encore/releases</a></div></div>`;
  };

  const showApi = async () => {
    const { api_key } = await api.get('/system/api-key');
    panel.innerHTML = `<div class="card stack">
      <h2>API access</h2>
      <p class="muted" style="margin:0">Use this key in an <span class="kbd">X-Api-Key</span> header to automate requests (e.g. from scripts or Home Assistant). It acts as the first admin account. Full API docs at <a href="/api/docs" target="_blank" style="color:var(--accent)">/api/docs</a>.</p>
      <div class="row"><div class="code grow" id="key" data-secret data-value="${esc(api_key)}"></div><button class="btn" id="cp">${icon.copy}</button><button class="btn danger" id="regen">${icon.refresh}Regenerate</button></div>
    </div>
    <div class="card stack mt">
      <h2>Security notes</h2>
      <ul class="muted" style="margin:0;padding-left:18px;line-height:1.8">
        <li>Only expose <b>this app</b> publicly (ideally behind HTTPS via a reverse proxy). Keep Lidarr and slskd on your internal Docker network – Encore talks to them for your users.</li>
        <li>Sign-ups are <b>${esc(values.signup_mode)}</b>. Manage access under <a href="#/admin/invites" style="color:var(--accent)">Invites</a> and <a href="#/admin/users" style="color:var(--accent)">Users</a>.</li>
        <li>Sessions are HTTP-only SameSite cookies, every change needs a CSRF header, and failed logins are rate-limited per IP.</li>
      </ul>
    </div>`;
    panel.querySelector('#cp').onclick = () => copyText(panel.querySelector('#key').dataset.value);
    panel.querySelector('#regen').onclick = async () => {
      const r = await api.post('/admin/settings/api-key');
      setSecretText(panel.querySelector('#key'), r.api_key);
      toast('New API key generated', 'ok');
    };
  };

  view.querySelector('.settings-nav').onclick = (e) => { const b = e.target.closest('[data-s]'); if (b) show(b.dataset.s); };
  view.querySelector('.settings-select').onchange = (e) => show(e.target.value);
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
