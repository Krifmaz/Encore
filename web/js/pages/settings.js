import { api } from '../api.js';
import { copyText, esc, EYE, EYE_OFF, icon, setSecretText, toast } from '../ui.js';
import * as accountPage from './account.js';
import * as connectionsPage from './connections.js';
import * as adminPage from './admin.js';
import * as autoPage from './autodownload.js';
import * as diagPage from './diagnostics.js';
import * as contributePage from './contribute.js';
import * as reportPage from './report.js';
import { bindYoutube, youtubeBlock } from './youtube.js';

// Settings → Playback → Cadenza engine, when there is no engine to show (the same words in web-rs/src/pages/settings.rs)
const ENGINE_GREYED = `<div class="stack" style="gap:8px;opacity:.45;pointer-events:none" aria-disabled="true"><label class="row" style="gap:8px;align-items:center"><input type="checkbox" disabled> Bit-perfect – send the file to the DAC untouched (no upsampling, dither or volume)</label><label class="stack" style="gap:4px"><span class="small muted">Upsampling filter</span><select class="input" disabled><option>–</option></select></label><label class="stack" style="gap:4px"><span class="small muted">Dither / noise shaping</span><select class="input" disabled><option>–</option></select></label><label class="stack" style="gap:4px"><span class="small muted">Output rate</span><select class="input" disabled><option>–</option></select></label></div><span class="small muted">Cadenza is Encore's own playback engine: long upsampling filters, DSD, dither and noise shaping, exclusive output straight into the DAC. It is built into the Encore desktop app – install the app on the computer connected to your DAC and this section comes alive on its own. Cadenza can also run on its own on a small box next to the DAC; link to it under Your devices. To have the server do the upsampling for a browser or phone instead, see Server processing above.</span>`;
const ENGINE_OFF_COPY = '<span class="small muted">Cadenza is off for this computer – music plays through the browser audio of this window. Link to This computer (Cadenza) under Your devices to turn it back on.</span>';

const S = (key, label, type = 'text', extra = {}) => ({ key, label, type, ...extra });

// Settings → Release types: MusicBrainz primary types, then the extras to skip (server: release_types.rs)
const RELEASE_KINDS = [['Album', 'Album'], ['EP', 'EP'], ['Single', 'Single'], ['Broadcast', 'Broadcast'], ['Other', 'Other']];
const RELEASE_EXTRAS = [['Compilation', 'Compilation'], ['Soundtrack', 'Soundtrack'], ['Spokenword', 'Spoken word'], ['Interview', 'Interview'], ['Audiobook', 'Audiobook'],
  ['Audio drama', 'Audio drama'], ['Live', 'Live'], ['Remix', 'Remix'], ['DJ-mix', 'DJ-mix'], ['Mixtape/Street', 'Mixtape / Street'], ['Demo', 'Demo'], ['Field recording', 'Field recording'],
  ['Instrumental', 'Instrumental'], ['Acoustic', 'Acoustic'], ['Sped up', 'Sped up / Slowed'], ['Karaoke', 'Karaoke'], ['Covers', 'Covers / Tributes']];
const typeKey = (s) => String(s).toLowerCase().replace(/[^a-z0-9]/g, '');

const SECTIONS = [
  { id: 'general', title: 'General', desc: "Your server's name and address, who can sign up, and how requests are approved.", fields: [
    S('app_name', 'App name'),
    S('base_url', 'Public URL', 'text', { help: 'e.g. https://music.example.com – used in links & notifications' }),
    S('share_preview', 'Shared link preview', 'select', { options: [['small', 'Small card (small cover)'], ['large', 'Big picture'], ['playable', 'Playable card (play button and seek bar)']], help: 'How shared links look in Discord, X and other chat apps' }),
    S('signup_mode', 'Sign-up mode', 'select', { options: [['invite', 'Invite only'], ['open', 'Open to anyone'], ['closed', 'Closed']] }),
    S('auto_approve', 'Auto-approve all requests', 'bool', { help: 'Otherwise only admins and users with auto-approve skip the approval queue' }),
    S('manual_search', 'Manual download picker', 'select', { options: [['everyone', 'Everyone'], ['admins', 'Admins only']], help: 'Who can search the sources and pick an exact download' }),
    S('default_request_limit', 'Default weekly request limit for new users', 'number', { help: '0 = unlimited' }),
    S('session_days', 'Stay signed in for (days)', 'number'),
    S('app_session_days', 'Keep the Encore app signed in for (days)', 'number', { help: 'Counted from the last time the app was used' }),
    S('contact_email', 'Contact email for MusicBrainz', 'text', { help: 'MusicBrainz asks API clients to identify themselves' }),
    S('musicbrainz_url', 'MusicBrainz server', 'text', { help: 'Use your own mirror for faster lookups' }),
    S('musicbrainz_rate', 'MusicBrainz requests / second', 'number', { step: '0.1', help: 'Keep at 1 for the public server' }),
  ] },
  { id: 'library', title: 'Library & tagging', desc: 'Where your music lives, and how downloaded files are tagged, named and organised.', fields: [
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
  { id: 'pipeline', title: 'Download pipeline', desc: 'Which sources are searched for a request, and in what order. The first one that has it wins.', fields: [
    S('source_order', 'Source order', 'text', { full: true, help: 'The one place to set what is searched, in order, comma-separated: qobuz, tidal, deezer (all through streamrip), slskd (Soulseek), torznab (torrents/Usenet, albums only), ytdlp (YouTube Music). Leave one out to never use it; sources you haven’t set up are skipped. Recommended (the default): qobuz, tidal, deezer, slskd, torznab, ytdlp – exact hi-res/lossless releases first, then Soulseek, then torrents/Usenet for whole albums, YouTube Music last.' }),
  ] },
  { id: 'releases', title: 'Release types', desc: 'Which releases Request discography, the artist pages and New releases count, like the metadata profiles in Lidarr. Song requests skip other versions (live, remix, sped up and so on) unless the song asked for is one.', fields: [
    S('release_types', 'Kinds to request', 'checks', { full: true, options: RELEASE_KINDS, help: 'Request discography gets these. Each request can change them in its own panel.' }),
    S('release_skip', 'Skip releases that are', 'checks', { full: true, options: RELEASE_EXTRAS, help: 'A release with any ticked extra is left out. Instrumental, acoustic, sped up, karaoke and covers are read from the titles.' }),
  ] },
  { id: 'verify', title: 'Verification & duplicates', desc: "Listens to each download and checks it's really the song you asked for, not a remix, live or wrong track.", test: 'acoustid', fields: [
    S('acoustid_api_key', 'AcoustID API key', 'password', { full: true, help: 'Free – register an application at acoustid.org/new-application. Used to fingerprint audio and confirm each file is the right song/version.' }),
    S('verify_on_import', 'Fingerprint every download before import', 'bool'),
    S('reject_mismatch', 'Reject downloads whose audio is a different song/version (and try the next source)', 'bool'),
    S('dedupe_on_import', 'Never import a second copy of a song (keep the better-quality file)', 'bool'),
    S('upgrade_check_audio', 'Before replacing a song with a better copy, compare the audio – a different recording is kept as its own song', 'bool'),
  ] },
  { id: 'streamrip', title: 'Qobuz / Tidal / Deezer (streamrip)', desc: 'Searches Qobuz, Tidal and Deezer with your own accounts and downloads lossless (FLAC). Usually the best quality. streamrip is only used for these three: its SoundCloud support would give the same quality as yt-dlp, so SoundCloud links go through yt-dlp.', test: 'streamrip', testLabel: 'Test & write config', fields: [
    S('streamrip_enabled', 'Enable streamrip source', 'bool', { help: 'Downloads lossless from your own paid accounts. Respect each service’s terms – use at your own risk.' }),
    S('streamrip_quality', 'Max quality', 'select', { options: [[1, 'MP3 320 / AAC'], [2, 'CD – 16-bit FLAC'], [3, 'Hi-res ≤ 24/96'], [4, 'Hi-res max']] }),
    S('streamrip_deezer_arl', 'Deezer ARL cookie', 'password', { full: true, help: 'From deezer.com → browser dev tools → Cookies → arl' }),
    S('streamrip_qobuz_user', 'Qobuz user id'),
    S('streamrip_qobuz_token', 'Qobuz auth token', 'password', { help: 'play.qobuz.com → dev tools → Network → user/login response' }),
    S('streamrip_bin', 'streamrip executable (optional)', 'text', { help: 'Leave empty to auto-detect' }),
    S('streamrip_command', 'Command template (advanced)', 'text', { full: true }),
  ] },
  { id: 'sync', title: 'Playlists & sync', desc: 'Keeps liked songs and playlists in step with Spotify, YouTube Music, Jellyfin, Plex and the rest. Runs as the “Sync likes & playlists” task above.', fields: [
    S('sync_interval_minutes', 'Sync likes & playlists every (minutes)', 'number', { help: '0 disables automatic sync – same as the task\u2019s “every” above' }),
    S('auto_request_followed', 'Automatically download new tracks in followed playlists (more rules under Admin → Auto-download)', 'bool'),
  ] },
  { id: 'torznab', title: 'Torrents & Usenet (Prowlarr)', desc: 'Searches your torrent and Usenet indexers (through Prowlarr) for whole albums and sends them to your torrent client (qBittorrent, Transmission, Deluge or rTorrent) or Usenet client (SABnzbd or NZBGet), or drops them into a watch folder.', test: 'torznab', fields: [
    S('torznab_enabled', 'Search torrent and Usenet indexers through Prowlarr', 'bool', { help: 'Albums only. Prowlarr searches every indexer you added there; torrents go to the torrent client below, Usenet to the Usenet client.' }),
    S('prowlarr_url', 'Prowlarr URL', 'text', { help: 'Internal address, e.g. http://prowlarr:9696' }),
    S('prowlarr_api_key', 'Prowlarr API key', 'password', { help: 'Prowlarr → Settings → General → API Key' }),
    S('torznab_min_seeders', 'Minimum seeders', 'number'),
    S('torznab_timeout_minutes', 'Try the next release after a stall of (minutes)', 'number'),
    S('torrent_client', 'Torrent client', 'select', { options: [['qbittorrent', 'qBittorrent'], ['transmission', 'Transmission'], ['deluge', 'Deluge'], ['rtorrent', 'rTorrent / ruTorrent'], ['blackhole', 'Blackhole folder (any client with a watch folder)']] }),
    S('qbit_url', 'qBittorrent URL', 'text', { help: 'e.g. http://qbittorrent:8080 (leave empty to skip torrents)', when: ['torrent_client', ['qbittorrent']] }),
    S('qbit_user', 'qBittorrent user', 'text', { when: ['torrent_client', ['qbittorrent']] }),
    S('qbit_password', 'qBittorrent password', 'password', { when: ['torrent_client', ['qbittorrent']] }),
    S('qbit_category', 'qBittorrent category', 'text', { when: ['torrent_client', ['qbittorrent']] }),
    S('qbit_download_path', 'Torrent download folder (same path in both containers)', 'text', { full: true, when: ['torrent_client', ['qbittorrent']] }),
    S('transmission_url', 'Transmission URL', 'text', { help: 'e.g. http://transmission:9091 (leave empty to skip torrents)', when: ['torrent_client', ['transmission']] }),
    S('transmission_user', 'Transmission user', 'text', { help: 'Leave empty when Transmission asks for no login', when: ['torrent_client', ['transmission']] }),
    S('transmission_password', 'Transmission password', 'password', { when: ['torrent_client', ['transmission']] }),
    S('transmission_label', 'Transmission label', 'text', { help: 'Transmission 4.0 or newer', when: ['torrent_client', ['transmission']] }),
    S('transmission_download_path', 'Torrent download folder (same path in both containers)', 'text', { full: true, when: ['torrent_client', ['transmission']] }),
    S('deluge_url', 'Deluge Web UI URL', 'text', { help: 'e.g. http://deluge:8112 (leave empty to skip torrents)', when: ['torrent_client', ['deluge']] }),
    S('deluge_password', 'Deluge Web UI password', 'password', { when: ['torrent_client', ['deluge']] }),
    S('deluge_label', 'Deluge label', 'text', { help: 'Needs the Label plugin; skipped when it is off', when: ['torrent_client', ['deluge']] }),
    S('deluge_download_path', 'Torrent download folder (same path in both containers)', 'text', { full: true, when: ['torrent_client', ['deluge']] }),
    S('rtorrent_url', 'rTorrent XML-RPC URL', 'text', { full: true, help: 'e.g. http://rutorrent:8080/RPC2 or http://rutorrent/plugins/httprpc/action.php (leave empty to skip torrents)', when: ['torrent_client', ['rtorrent']] }),
    S('rtorrent_user', 'rTorrent user', 'text', { help: 'Leave empty when the address asks for no login', when: ['torrent_client', ['rtorrent']] }),
    S('rtorrent_password', 'rTorrent password', 'password', { when: ['torrent_client', ['rtorrent']] }),
    S('rtorrent_label', 'rTorrent label (shown in ruTorrent)', 'text', { when: ['torrent_client', ['rtorrent']] }),
    S('rtorrent_download_path', 'Torrent download folder (same path in both containers)', 'text', { full: true, when: ['torrent_client', ['rtorrent']] }),
    S('torrent_blackhole_watch', 'Torrent watch folder', 'text', { full: true, help: 'Encore drops .torrent and .magnet files here for your client to pick up', when: ['torrent_client', ['blackhole']] }),
    S('torrent_blackhole_completed', 'Finished torrents folder', 'text', { full: true, help: 'Where your client puts finished downloads, as mounted in this container', when: ['torrent_client', ['blackhole']] }),
    S('nzb_client', 'Usenet client', 'select', { options: [['sabnzbd', 'SABnzbd'], ['nzbget', 'NZBGet'], ['blackhole', 'Blackhole folder (any client with a watch folder)']] }),
    S('sab_url', 'SABnzbd URL', 'text', { help: 'e.g. http://sabnzbd:8080 (leave empty to skip Usenet)', when: ['nzb_client', ['sabnzbd']] }),
    S('sab_api_key', 'SABnzbd API key', 'password', { when: ['nzb_client', ['sabnzbd']] }),
    S('sab_category', 'SABnzbd category', 'text', { when: ['nzb_client', ['sabnzbd']] }),
    S('sab_download_path', 'Usenet download folder (as mounted in this container)', 'text', { full: true, when: ['nzb_client', ['sabnzbd']] }),
    S('nzbget_url', 'NZBGet URL', 'text', { help: 'e.g. http://nzbget:6789 (leave empty to skip Usenet)', when: ['nzb_client', ['nzbget']] }),
    S('nzbget_user', 'NZBGet user', 'text', { when: ['nzb_client', ['nzbget']] }),
    S('nzbget_password', 'NZBGet password', 'password', { when: ['nzb_client', ['nzbget']] }),
    S('nzbget_category', 'NZBGet category', 'text', { when: ['nzb_client', ['nzbget']] }),
    S('nzbget_download_path', 'Usenet download folder (as mounted in this container)', 'text', { full: true, when: ['nzb_client', ['nzbget']] }),
    S('nzb_blackhole_watch', 'Usenet watch folder', 'text', { full: true, help: 'Encore drops .nzb files here for your client to pick up', when: ['nzb_client', ['blackhole']] }),
    S('nzb_blackhole_completed', 'Finished Usenet downloads folder', 'text', { full: true, help: 'Where your client puts finished downloads, as mounted in this container', when: ['nzb_client', ['blackhole']] }),
  ] },
  { id: 'slskd', title: 'Soulseek', desc: 'Searches the Soulseek file-sharing network – great for FLAC and rare releases. Encore runs Soulseek for you: enter any username and password and an account is created at the first sign-in. Forward the port for the best speeds.', test: 'slskd', fields: [
    S('slskd_enabled', 'Enable Soulseek', 'bool'),
    S('soulseek_username', 'Soulseek username', 'text', { help: 'New name + password = new account. Don’t use the same login in another Soulseek client at the same time.' }),
    S('soulseek_password', 'Soulseek password', 'password'),
    S('soulseek_port', 'Listen port', 'number', { help: 'TCP port Soulseek listens on – forward it on your router (and publish it in Docker) so users behind firewalls can send to you. While Send Soulseek through a VPN is on, the VPN’s forwarded port is used instead.' }),
    S('soulseek_share', 'Share your library on Soulseek (recommended)', 'bool', { help: 'The network runs on sharing; many users refuse downloads to people who share nothing. Only the library folder is shared, read-only.' }),
    S('soulseek_upload_slots', 'Upload slots', 'number'),
    S('slskd_download_path', 'Downloads folder (as mounted in this container)', 'text', { full: true }),
    S('slskd_preferred_format', 'Preferred format', 'select', { options: [['flac', 'FLAC / lossless'], ['mp3', 'MP3'], ['any', 'Best available']] }),
    S('slskd_min_bitrate', 'Minimum MP3 bitrate (kbps)', 'number'),
    S('slskd_search_timeout', 'Search time (seconds)', 'number'),
    S('slskd_stall_minutes', 'Give up on a stalled user after (minutes)', 'number'),
    S('slskd_max_candidates', 'Max users to try per request', 'number'),
    S('vpn_enabled', 'Send Soulseek through a VPN', 'bool', { help: 'Only Soulseek goes through the VPN; Tidal, Qobuz, Deezer and everything else keep your normal connection. No gluetun container needed.' }),
    S('vpn_wireguard_config', 'WireGuard config', 'textarea', { full: true, secret: true, help: 'Paste the whole .conf file from your VPN provider (ProtonVPN: Downloads → WireGuard configuration, with NAT-PMP port forwarding on). The private key is stored on this server only.' }),
    S('vpn_port_forwarding', 'Port forwarding', 'select', { options: [['natpmp', 'NAT-PMP (ProtonVPN)'], ['off', 'Off']] }),
    S('vpn_kill_switch', 'Keep Soulseek off while the VPN is down', 'bool'),
  ] },
  { id: 'ytdlp', title: 'YouTube Music & links (yt-dlp)', desc: 'Last-resort source: finds the song on YouTube Music. yt-dlp also downloads SoundCloud links you paste in the Downloader, along with YouTube, Bandcamp, Mixcloud and about 1,800 other sites.', fields: [
    S('ytdlp_enabled', 'Enable yt-dlp as a source', 'bool', { help: 'Searches YouTube Music for requests, and downloads links you paste in the Downloader from YouTube, SoundCloud, Bandcamp, Mixcloud and ~1,800 other sites.' }),
    S('ytdlp_last_resort', 'Always try YouTube Music last, so downloads (almost) never fail', 'bool'),
    S('ytdlp_format', 'Audio format', 'select', { options: [['best', 'Original stream – best quality, no re-encode (recommended)'], ['m4a', 'M4A (AAC, re-encoded)'], ['mp3', 'MP3 (re-encoded)'], ['opus', 'Opus']] }),
    S('ytdlp_quality', 'MP3/M4A quality', 'text', { help: 'Bitrate like 320, or 0 for best VBR' }),
  ] },
  { id: 'youtube', title: 'YouTube sign-in', desc: 'Signs yt-dlp in to YouTube: YouTube Music Premium unlocks 256 kbps audio, and signed-in downloads see fewer bot checks. Best: connect the Encore browser extension and hand over a sign-in from a private window – Encore then owns that session and keeps it alive by itself. Pasted cookies are the backup.', test: 'youtube', testLabel: 'Test cookies', fields: [
    S('ytdlp_cookies', 'Pasted cookies.txt (backup)', 'textarea', { full: true, secret: true, help: 'Netscape cookie file exported from music.youtube.com. Used only when there is no signed-in hand-off or extension sync. Export from a private window and close it right after, so your own browser does not rotate them.' }),
    S('youtube_light_refresh', 'Refresh pasted cookies every 4 hours', 'bool', { help: 'Only for pasted cookies while they are in use: Encore renews them the way an open YouTube tab does and saves the new ones. A handed-over sign-in is always refreshed every 4 hours; an everyday browser’s cookies never are (that would sign that browser out).' }),
  ] },
  { id: 'spotify', title: 'Integrations', desc: 'API keys that let users connect their Spotify, Tidal, Last.fm, Discogs and YouTube Music accounts, and power recommendations.', test: 'spotify', testLabel: 'Test all', fields: [
    S('spotify_client_id', 'Spotify Client ID', 'text', { help: 'developer.spotify.com – add <Public URL>/api/connections/spotify/callback as redirect URI. Development-mode apps allow 5 users and need a Premium owner (2026 rules); without it, public playlists still import via the embed fallback.' }),
    S('spotify_client_secret', 'Spotify Client secret', 'password'),
    S('tidal_client_id', 'Tidal Client ID', 'text', { help: 'Only for looking up Tidal albums/songs by ISRC and for the Tidal connection. Get one at developer.tidal.com → Dashboard → Create new app (any name), redirect URI <Public URL>/api/connections/tidal/callback, then copy the Client ID and Client secret here. Downloads from Tidal use your own account instead: Apps & sources → Qobuz / Tidal / Deezer (streamrip) → Sign in to Tidal.' }),
    S('tidal_client_secret', 'Tidal Client secret', 'password'),
    S('lastfm_api_key', 'Last.fm API key', 'password', { help: 'last.fm/api/account/create (free) – loved tracks, similar artists/songs, genre pages, For you' }),
    S('lastfm_shared_secret', 'Last.fm shared secret', 'password', { help: 'Shown next to the API key. Needed for scrobbling: users sign in with Last.fm on the Connections page.' }),
    S('discogs_token', 'Discogs personal access token', 'password', { help: 'discogs.com/settings/developers → “Generate new token” and paste that personal access token here (not an application’s Consumer Key/Secret – no app is needed). Finds vinyl, bootlegs and old releases MusicBrainz is missing.' }),
    S('ytmusic_client_id', 'YouTube Music OAuth Client ID', 'text', { help: 'Google Cloud → YouTube Data API v3 → OAuth client of type “TVs and Limited Input devices”. Lets users sign in to YouTube Music with a code (they can also paste browser headers instead).' }),
    S('ytmusic_client_secret', 'YouTube Music OAuth Client secret', 'password'),
  ] },
  { id: 'media', title: 'Media servers', desc: 'Tells Navidrome, Jellyfin or Plex to rescan after new music arrives, so it shows up there right away.', test: 'mediaservers', testLabel: 'Trigger refresh', fields: [
    S('navidrome_url', 'Navidrome URL', 'text', { help: 'URLs here let users link their own accounts on the Connections page; the admin credentials below are only used to trigger library rescans' }), S('navidrome_user', 'Navidrome user'), S('navidrome_password', 'Navidrome password', 'password'),
    S('jellyfin_url', 'Jellyfin URL'), S('jellyfin_api_key', 'Jellyfin API key', 'password'),
    S('plex_url', 'Plex URL'), S('plex_token', 'Plex token', 'password'),
  ] },
  { id: 'roon', title: 'Roon', desc: 'Encore as a Roon extension: your Roon zones show up under Your devices with what they play, transport and volume, and “Play in Roon” starts a search result in a zone. Enable it here, then approve Encore once in Roon → Settings → Extensions.', test: 'roon', testLabel: 'Test connection', fields: [
    S('roon_enabled', 'Enable Roon', 'bool', { help: 'Roon Core (or Nucleus / ROCK) on the same network. Roon 1.8 or newer.' }),
    S('roon_host', 'Roon Core address', 'text', { help: 'Leave empty to find the Core on the network. Running Encore in Docker? Multicast does not cross the bridge network – enter the Core’s IP here (add :port if it is not 9330).' }),
  ] },
  { id: 'cadenza', title: 'Server processing (Cadenza)', desc: 'Lets permitted users have their streams upsampled on this server by Cadenza (long-filter upsampling, dither and noise shaping, lossless 24-bit FLAC out). Off by default: it costs real processor time per stream. Allow users under People → Users & permissions.', test: 'cadenza', testLabel: 'Test', fields: [
    S('cadenza_server_enabled', 'Enable server processing', 'bool'),
    S('cadenza_max_streams', 'Streams at once', 'number', { help: 'Processed streams the whole server runs at the same time; more are played as the original.' }),
    S('cadenza_max_rate', 'Highest output rate', 'select', { options: [[96000, '96 kHz'], [192000, '192 kHz'], [384000, '384 kHz'], [768000, '768 kHz']] }),
    S('cadenza_max_taps', 'Longest filter', 'select', { options: [[16384, '16k taps (light)'], [65536, '64k taps'], [262144, '256k taps'], [1048576, '1M taps (heavy)']] }),
    S('cadenza_cache_mb', 'Cache size (MB)', 'number', { help: 'Each song is rendered once and kept on disk, so seeking and skipping need no new processing and the next song is ready before it plays. The least recently played renders make room. 0 = no cache (every stream is processed live).' }),
  ] },
  { id: 'email', title: 'Email', desc: 'Lets Encore send password-reset and sign-in emails.', test: 'email', testLabel: 'Send test email', fields: [
    S('smtp_host', 'SMTP server', 'text', { help: 'e.g. smtp.fastmail.com – used for password-reset emails' }),
    S('smtp_port', 'Port', 'number', { help: '587 (STARTTLS) or 465 (SSL)' }),
    S('smtp_user', 'Username'),
    S('smtp_password', 'Password', 'password'),
    S('smtp_from', 'From address', 'text', { help: 'e.g. Encore <music@example.com>' }),
    S('smtp_security', 'Security', 'select', { options: [['starttls', 'STARTTLS'], ['ssl', 'SSL/TLS'], ['none', 'None']] }),
  ] },
  { id: 'notify', title: 'Notifications', desc: 'Get a message on Discord, ntfy or any webhook when downloads finish or fail.', test: 'notify', testLabel: 'Send test', fields: [
    S('discord_webhook', 'Discord webhook URL', 'password', { full: true }),
    S('ntfy_url', 'ntfy topic URL', 'text', { help: 'e.g. https://ntfy.sh/my-music' }),
    S('ntfy_token', 'ntfy access token', 'password'),
    S('webhook_url', 'Generic webhook (JSON POST)', 'password', { full: true }),
  ] },
];

// Settings pages: related cards together, in an order that reads top to bottom.
// One nav entry per group; `parts` splits a long group into labelled runs of per-app cards.
// `aliases` keep old #/settings?s= links working.
const GROUPS = [
  { id: 'general', title: 'General', sections: ['general'] },
  { id: 'apps', title: 'Apps & sources', aliases: ['downloads', 'tagging', 'services'], parts: [
    ['Download sources', ['pipeline', 'releases', 'streamrip', 'torznab', 'slskd', 'ytdlp', 'youtube']],
    ['Library & tagging', ['library', 'verify']],
    ['Services', ['spotify', 'media', 'roon', 'cadenza']],
  ] },
  { id: 'accounts', title: 'Email & notifications', sections: ['email', 'notify'] },
  { id: 'autodownload', title: 'Auto-download', sections: [] },
  { id: 'contribute', title: 'Contribute', sections: [] },
  { id: 'blocked', title: 'Blocked', sections: [] },
  { id: 'tasks', title: 'Scheduled tasks', sections: ['sync'] },
];
GROUPS.forEach(g => { g.sections = g.sections || g.parts.flatMap(([, ids]) => ids); });
const groupOf = (id) => (GROUPS.find(g => g.id === id || (g.aliases || []).includes(id) || g.sections.includes(id)) || {}).id || id;

const MASK = '••••';
let REVEAL = false;   // server can hand a saved secret back to the eye/copy buttons
const secretOf = async (key) => (await api.get(`/admin/settings/secret/${encodeURIComponent(key)}`)).value;
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
  if (f.type === 'checks') {
    const on = new Set(String(v ?? '').split(',').map(typeKey));
    return `<div class="field ${cls}" data-k="${f.key}"><span>${esc(f.label)}</span><div class="row wrap" style="gap:6px 14px">${f.options.map(([val, lab]) => `<label class="row" style="gap:6px;align-items:center"><input type="checkbox" value="${esc(val)}" ${on.has(typeKey(val)) ? 'checked' : ''} ${dis}> ${esc(lab)}</label>`).join('')}</div>${help}</div>`;
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
  // also for values set by environment variables (read-only, but viewable); never the WireGuard private key
  const reveal = REVEAL && v && f.key !== 'vpn_wireguard_config';
  if (f.type === 'textarea') {
    const btns = reveal ? `<span class="row" style="gap:8px"><button type="button" class="btn sm" data-reveal="${f.key}">${EYE}Show saved</button><button type="button" class="btn sm" data-copy="${f.key}">${icon.copy}Copy</button></span>` : '';
    return `<label class="field ${cls}"><span>${esc(f.label)}</span><textarea class="input mono" data-k="${f.key}" ${dis} placeholder="${v ? 'Saved – paste to replace' : ''}"></textarea>${btns}${help}</label>`;
  }
  if (f.type === 'password') {
    // secrets are only sent to the browser when you press the eye/copy button: empty input = keep the saved value
    const input = `<input class="input" type="password" data-k="${f.key}" value="" ${dis} ${reveal ? 'data-eye="1"' : ''}
      autocomplete="new-password" placeholder="${locked ? 'set by environment' : v ? 'saved – paste a new value to replace' : 'not set'}">`;
    const wrapped = reveal ? `<span class="pw-wrap secret">${input}<button type="button" class="eye-btn" data-reveal="${f.key}" title="Show" aria-label="Show">${EYE}</button><button type="button" class="eye-btn" data-copy="${f.key}" title="Copy" aria-label="Copy">${icon.copy}</button></span>` : input;
    return `<label class="field ${cls}"><span>${esc(f.label)}</span>${wrapped}${help}</label>`;
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
  appearance: (el, ctx) => renderAppearance(el, ctx),
  language: async (el) => (await import('./translate.js')).renderLanguage(el),
  playback: (el, ctx) => renderPlayback(el, ctx),
  connections: (el, ctx) => connectionsPage.render(el, ctx),
  users: (el, ctx) => adminPage.renderUsers(el, ctx),
  invites: (el, ctx) => adminPage.renderInvites(el, ctx),
  autodownload: (el) => autoPage.render(el),
  contribute: (el, ctx) => contributePage.render(el, ctx),
  blocked: (el, ctx) => reportPage.render(el, ctx),
  diagnostics: (el) => diagPage.render(el),
  computer: async (el) => (await import('./computer.js')).render(el),
};

export async function render(view, ctx) {
  const admin = ['admin', 'owner'].includes(ctx.user?.role);
  let values = admin ? await api.get('/admin/settings') : {};
  const { session } = await import('../app.js');
  REVEAL = (session.info?.features || []).includes('reveal');
  let lidarrOpts = null;
  let current = ctx.query.s ? groupOf(ctx.query.s) : 'account';
  let focus = ctx.query.s && current !== ctx.query.s ? ctx.query.s : null;   // #/settings?s=<section> opens that card
  const nav = [
    ['You', [['account', 'Account'], ['appearance', 'Appearance'], ['language', 'Language'], ['playback', 'Playback'], ['connections', 'Connections']]],
    ...(admin ? [
      ['People', [['users', 'Users & permissions'], ['invites', 'Invites']]],
      ['Server', GROUPS.map(g => [g.id, g.title])],
      ['System', [['diagnostics', 'Diagnostics']]],
    ] : []),
  ];
  // the desktop app: its own settings (start with the computer, shortcuts, mini player, offline songs, updates)
  if (window.EncoreNative?.desktopSettings) nav[0][1].push(['computer', 'This computer']);
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
    if (id === 'about' || id === 'api' || id === 'keys') return show('general');
    const group = GROUPS.find(g => g.id === id) || GROUPS[0];
    // only what this server has: Encore has Prowlarr instead of Lidarr
    const secs = group.sections.map(sid => SECTIONS.find(x => x.id === sid))
      .map(sec => ({ ...sec, fields: sec.fields.filter(f => f.key in values) })).filter(sec => sec.fields.length);
    if (secs.some(x => x.id === 'lidarr') && values.lidarr_api_key && !lidarrOpts) {
      try { lidarrOpts = await api.get('/system/lidarr/options'); } catch { lidarrOpts = null; }
    }
    const meta = values._meta || {};
    const card = (sec, open) => `<details class="card settings-card" data-sec="${sec.id}" ${open ? 'open' : ''}>
      <summary class="row between" style="gap:10px;flex-wrap:nowrap;align-items:flex-start"><div style="min-width:0"><h2>${esc(sec.title)}</h2>${sec.desc ? `<div class="small muted" style="margin-top:4px;font-weight:400">${esc(sec.desc)}</div>` : ''}</div>
        <span class="row" style="gap:8px">${sec.test ? `<button class="btn sm" data-test>${icon.refresh}${esc(sec.testLabel || 'Test connection')}</button>` : ''}<span class="chev">${icon.chevron || '▾'}</span></span></summary>
      <div style="height:14px"></div>
      <div data-out>${flash && flash.sec === sec.id ? `<div class="test-result ${flash.ok ? 'ok' : 'err'}">${flash.ok ? icon.check : icon.x}<span>${esc(flash.message || '')}</span></div>` : ''}</div>
      ${sec.id === 'youtube' ? youtubeBlock() : ''}${fieldsHtml(sec.fields, (f) => showWhen(f, field(f, values[f.key], lidarrOpts, meta), values))}
      ${sec.id === 'streamrip' ? tidalBlock() : ''}
      ${sec.id === 'cadenza' ? cadenzaCacheBlock() + engineSettingsBlock() : ''}
      ${sec.id === 'slskd' ? soulseekStatusBlock() : ''}
      <div class="row mt" style="justify-content:flex-end"><button class="btn primary" data-save>${icon.check}Save</button></div>
    </details>`;
    // the first card starts open (unless a link asked for a specific one); every app is its own drop-down
    let first = true;
    const body = (group.parts || [['', group.sections]]).map(([label, ids]) => {
      const list = secs.filter(sec => ids.includes(sec.id));
      if (!list.length) return '';
      return (label ? `<div class="nav-label" style="padding:18px 4px 8px">${esc(label)}</div>` : '') + (label === 'Download sources' ? '<div id="tools-host"></div>' : '') + list.map(sec => {
        const open = (first && !focus) || secs.length === 1 || (flash && flash.sec === sec.id) || focus === sec.id;
        first = false;
        return card(sec, open);
      }).join('');
    }).join('');
    panel.innerHTML = (group.id === 'tasks' ? '<div id="tasks-host" style="margin-bottom:16px"></div>' : '') + body
      + (group.id === 'apps' ? pipelineHelp() : '') + (group.id === 'general' ? await apiCard() + await aboutCard() : '');
    if (group.id === 'tasks') renderTasks(panel.querySelector('#tasks-host'));
    if (panel.querySelector('#tools-host')) renderTools(panel.querySelector('#tools-host'), () => values, (v) => { values = v; });
    if (focus) { const c = panel.querySelector(`[data-sec="${focus}"]`); focus = null; if (c) c.scrollIntoView({ block: 'start' }); }
    panel.querySelectorAll('[data-sec]').forEach(card => {
      const sec = SECTIONS.find(x => x.id === card.dataset.sec);
      const out = card.querySelector('[data-out]');
      if (sec.id === 'streamrip') bindTidal(card);
      if (sec.id === 'cadenza') bindCadenzaCache(card, values);
      if (sec.id === 'slskd') { bindVpn(card, () => save(sec, true, card)); bindSoulseek(card); }
      if (sec.id === 'youtube') bindYoutube(card);
      bindShowWhen(card);
      const paint = (r) => { out.innerHTML = `<div class="test-result ${r.ok ? 'ok' : 'err'}">${r.ok ? icon.check : icon.x}<span>${esc(r.message || (r.ok ? 'Connected' : 'Failed'))}</span></div>`; };
      card.querySelector('[data-save]').onclick = () => save(sec, false, card);
      card.querySelectorAll('[data-clear]').forEach(a => a.addEventListener('click', async (e) => {
        e.preventDefault();
        await api.put('/admin/settings', { [a.dataset.clear]: '' }).then(v => { values = v; });
        toast('Cleared', 'ok');
        show(id);
      }));
      card.querySelectorAll('[data-reveal]').forEach(b => b.addEventListener('click', async (e) => {
        e.preventDefault();
        const input = card.querySelector(`[data-k="${b.dataset.reveal}"]`);
        const hidden = input.tagName === 'TEXTAREA' ? !input.dataset.shown : input.type === 'password';
        try {
          if (hidden) {
            if (!input.value) { input.value = await secretOf(b.dataset.reveal); input.dataset.revealed = '1'; }
            if (input.tagName !== 'TEXTAREA') input.type = 'text';
            input.dataset.shown = '1';
          } else {
            if (input.dataset.revealed) { input.value = ''; delete input.dataset.revealed; }
            if (input.tagName !== 'TEXTAREA') input.type = 'password';
            delete input.dataset.shown;
          }
          if (b.classList.contains('eye-btn')) { b.innerHTML = hidden ? EYE_OFF : EYE; b.title = hidden ? 'Hide' : 'Show'; }
          else b.innerHTML = hidden ? `${EYE_OFF}Hide` : `${EYE}Show saved`;
        } catch (err) { toast(err.message, 'err'); }
      }));
      card.querySelectorAll('[data-copy]').forEach(b => b.addEventListener('click', async (e) => {
        e.preventDefault();
        const input = card.querySelector(`[data-k="${b.dataset.copy}"]`);
        try { copyText(input.value || await secretOf(b.dataset.copy)); } catch (err) { toast(err.message, 'err'); }
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
      if (f.type === 'checks') body[f.key] = [...el.querySelectorAll('input:checked')].map(i => i.value).join(',');
      else if (el.type === 'checkbox') body[f.key] = el.checked;
      else if ((f.type === 'password' || f.secret) && el.value.trim() === '') { /* keep saved secret */ }
      else if (isMasked(el.value)) { /* never echo a mask back */ }
      else body[f.key] = f.type === 'password' || f.secret ? el.value.trim() : el.value;
    });
    if (sec.id === 'cadenza') {
      const engine = engineSettingsValue(root);
      if (engine !== null && (meta.cadenza_engine_settings || {}).source !== 'env') body.cadenza_engine_settings = engine;
    }
    try {
      values = await api.put('/admin/settings', body);
      if (body.cadenza_engine_settings && ENGINE) ENGINE.values = JSON.parse(body.cadenza_engine_settings);
      if (!quiet) toast('Settings saved', 'ok');
      if (sec.id === 'general' && body.app_name) document.title = body.app_name;
    } catch (e) {
      toast(e.message, 'err');
      if (quiet) throw e;
    }
  };

  // Encore's own API key, at the bottom of General (only the owner may see it)
  const apiCard = async () => {
    let key;
    try { key = (await api.get('/system/api-key')).api_key; } catch { return ''; }
    setTimeout(() => {
      const card = panel.querySelector('#api-card');
      if (!card) return;
      card.querySelector('#cp').onclick = () => copyText(card.querySelector('#key').dataset.value);
      card.querySelector('#regen').onclick = async () => {
        if (!confirm('Make a new API key? Scripts using the old one stop working.')) return;
        const r = await api.post('/admin/settings/api-key');
        setSecretText(card.querySelector('#key'), r.api_key);
        toast('New API key generated', 'ok');
      };
    });
    return `<div class="card stack mt" id="api-card">
      <h2>Encore API key</h2>
      <p class="muted small" style="margin:0">Send it in an <span class="kbd">X-Api-Key</span> header to automate Encore (scripts, Home Assistant…). It acts as the owner account – keep it private.</p>
      <div class="row wrap"><div class="code grow" id="key" data-secret data-value="${esc(key)}"></div><button class="btn" id="cp" title="Copy">${icon.copy}</button><button class="btn danger" id="regen">${icon.refresh}Regenerate</button></div>
    </div>`;
  };

  view.querySelector('.settings-nav').onclick = (e) => { const b = e.target.closest('[data-s]'); if (b) show(b.dataset.s); };
  view.querySelector('.settings-select').onchange = (e) => show(e.target.value);
  show(current);
}

// Tidal downloads need your Tidal account, not an API key: Encore signs in once through a link
// (device code, one step per streamrip "lane"). This block shows the link and waits for the approval;
// when Tidal blocks the server's address the tokens can be pasted from a computer that has access.
function tidalBlock() {
  return `<div class="card flat mt" id="tidal-login" style="padding:12px 14px">
    <div class="row between wrap" style="gap:8px">
      <div style="min-width:0"><b>Tidal account</b><div class="small muted" id="tidal-state">Checking…</div></div>
      <div class="row" style="gap:10px;align-items:center"><a href="#" class="small muted" id="tidal-signout" hidden>Sign out of Tidal</a><button class="btn sm" id="tidal-signin" type="button">${icon.link}Sign in to Tidal</button></div>
    </div>
    <div class="small mt" id="tidal-out" hidden></div>
    <details class="mt" id="tidal-paste">
      <summary class="small" style="cursor:pointer">Paste tokens instead</summary>
      <div class="small muted mt">On a computer with Tidal access: install streamrip (<code>pip install streamrip</code>), run <code>rip url https://tidal.com/browse/track/1</code>, approve the link it prints, then paste the [tidal] section of its config file here (<code>rip config path</code> shows where it is).</div>
      <textarea id="tidal-toml" class="mt" rows="6" spellcheck="false" style="width:100%;font-family:monospace" placeholder="[tidal]&#10;user_id = &quot;…&quot;&#10;access_token = &quot;…&quot;"></textarea>
      <div class="row wrap mt" style="gap:8px"><button class="btn sm" id="tidal-save-tokens" type="button">${icon.check}Save tokens</button></div>
    </details>
  </div>`;
}

// The settings grid of a card. The vpn_ fields (Soulseek) get a sub-block of their own, in place,
// with the tunnel's state and Test VPN under them.
// A field with `when: [key, [values]]` only shows while the select `key` has one of the values
// (Torrents & Usenet: only the chosen client's fields). The same markup in web-rs/src/pages/settings.rs.
function showWhen(f, html, values) {
  if (!f.when) return html;
  const [k, vals] = f.when;
  const on = vals.includes(String(values[k] ?? ''));
  return html.replace(' class="field ', ` data-when="${k}" data-when-in="${vals.join(' ')}"${on ? '' : ' hidden'} class="field `);
}
function bindShowWhen(card) {
  card.querySelectorAll('select[data-k]').forEach(sel => sel.addEventListener('change', () => {
    card.querySelectorAll(`[data-when="${sel.dataset.k}"]`).forEach(el => { el.hidden = !el.dataset.whenIn.split(' ').includes(sel.value); });
  }));
}

function fieldsHtml(fields, one) {
  const isVpn = (f) => f.key.startsWith('vpn_');
  const at = fields.findIndex(isVpn);
  if (at < 0) return `<div class="form-grid">${fields.map(one).join('')}</div>`;
  const rest = fields.filter(f => !isVpn(f));
  const after = rest.slice(at);
  return `<div class="form-grid">${rest.slice(0, at).map(one).join('')}</div>${vpnBlock(fields.filter(isVpn).map(one).join(''))}${after.length ? `<div class="form-grid mt">${after.map(one).join('')}</div>` : ''}`;
}

// Settings → Soulseek → VPN for Soulseek: the WireGuard fields, the tunnel's state (polled every 5 s while the card is open) and Test VPN.
function vpnBlock(inner) {
  return `<div class="card flat mt" id="vpn-block" style="padding:12px 14px">
    <b>VPN for Soulseek</b>
    <div class="form-grid mt">${inner}</div>
    <div class="row between wrap mt" style="gap:8px">
      <div class="small muted" id="vpn-state" style="min-width:0">Checking…</div>
      <button class="btn sm" id="vpn-test" type="button">${icon.refresh}Test VPN</button>
    </div>
  </div>`;
}

function bindVpn(card, saveSection) {
  const state = card.querySelector('#vpn-state'), btn = card.querySelector('#vpn-test');
  if (!state || !btn) return;
  const refresh = () => api.get('/system/vpn').then(r => { if (!btn.disabled) state.innerHTML = diagPage.vpnStateHtml(r); }).catch(() => { state.textContent = 'Status unavailable'; });
  refresh();
  const timer = setInterval(() => { if (!card.isConnected) return clearInterval(timer); if (card.open && !btn.disabled) refresh(); }, 5000);
  btn.onclick = async (e) => {
    e.preventDefault();
    btn.disabled = true;
    state.innerHTML = '<span class="spinner" style="width:10px;height:10px;border-width:2px"></span> Testing…';
    try {
      await saveSection();
      const r = await api.post('/system/vpn/test');
      const n = r.natpmp;
      state.innerHTML = diagPage.vpnStateHtml(r) + (n && n.ok === false && n.error ? ` · <span style="color:var(--err)">Port forwarding failed: ${esc(String(n.error))}</span>` : '');
    } catch (err) { state.innerHTML = `<span style="color:var(--err)">${esc(err.message || 'Test failed')}</span>`; }
    btn.disabled = false;
  };
}

// Server processing: how full the Cadenza pre-render cache is, how fast it renders, and a button to empty it.
function cadenzaCacheBlock() {
  return `<div class="card flat mt" id="cadenza-cache" style="padding:12px 14px">
    <div class="row between wrap" style="gap:8px">
      <div style="min-width:0"><b>Pre-render cache</b><div class="small muted" id="cadenza-cache-state">Checking…</div></div>
      <button class="btn sm" id="cadenza-cache-clear" type="button">${icon.trash}Clear cache</button>
    </div>
    <div class="small muted mt" id="cadenza-engine">Engine: …</div>
  </div>`;
}

/** "Cache: 812 MB of 4096 MB · 23 songs · renders at 6.2× real time" (the factor only once a render has finished). */
export function cadenzaCacheText(c) {
  if (!c || typeof c !== 'object') return 'Cache: not available on this server';
  const mb = (v) => String(Math.round(Number(v) || 0));
  const n = Number(c.files) || 0;
  const f = Number(c.last_factor);
  return `Cache: ${mb(c.used_mb)} MB of ${mb(c.max_mb)} MB · ${n} ${n === 1 ? 'song' : 'songs'}${f > 0 ? ` · renders at ${f.toFixed(1)}× real time` : ''}`;
}

/** "Engine: Cadenza 2.1.0 (updated)" / "Engine: Cadenza (built in)" – `engine` of GET /playback/dsp. */
export function cadenzaEngineText(e) {
  return `Engine: ${e && e.source === 'updated' && e.version ? `Cadenza ${e.version} (updated)` : 'Cadenza (built in)'}`;
}

// Server processing → Engine settings: drawn only from the list the active engine gives (engine.settings in
// GET /playback/dsp: `cadenza render --describe`, or the built-in engine's own), saved to cadenza_engine_settings.
let ENGINE = null;   // { settings, values } of the engine the block was drawn for

function engineSettingsBlock() {
  return `<div class="card flat mt" id="cadenza-engine-settings" style="padding:12px 14px">
    <b>Engine settings</b>
    <div class="small muted" id="cadenza-engine-hint">Checking…</div>
    <div class="form-grid mt" id="cadenza-engine-fields" hidden></div>
  </div>`;
}

const ENGINE_TYPES = ['select', 'bool', 'number'];

function engineDefaultText(s) {
  if (s.type === 'bool') return s.default ? 'on' : 'off';
  if (s.type === 'select') {
    const o = (Array.isArray(s.options) ? s.options : []).find(o => String(o[0]) === String(s.default));
    return o ? String(o[1]) : String(s.default ?? '');
  }
  return String(s.default ?? '');
}

/** One engine setting as a form field (data-ek = its key). A saved select value the engine no longer offers shows the default. */
function engineField(s, v, locked) {
  const dis = locked ? 'disabled' : '';
  const k = esc(s.key);
  const label = esc(s.label || s.key);
  const help = `<small>${s.help ? `${esc(s.help)} · ` : ''}Default: ${esc(engineDefaultText(s))}</small>`;
  if (s.type === 'bool') {
    return `<div class="field" style="justify-content:center"><label class="switch"><input type="checkbox" data-ek="${k}" ${v ? 'checked' : ''} ${dis}><i></i>${label}</label>${help}</div>`;
  }
  if (s.type === 'select') {
    const opts = Array.isArray(s.options) ? s.options : [];
    const pick = opts.some(o => String(o[0]) === String(v)) ? v : s.default;
    return `<label class="field"><span>${label}</span><select class="input" data-ek="${k}" ${dis}>${opts.map(([val, lab]) => `<option value="${esc(val)}" ${String(val) === String(pick) ? 'selected' : ''}>${esc(lab)}</option>`).join('')}</select>${help}</label>`;
  }
  const attr = (n) => (s[n] != null && Number.isFinite(Number(s[n])) ? ` ${n}="${esc(s[n])}"` : '');
  return `<label class="field"><span>${label}</span><input class="input" type="number" data-ek="${k}"${attr('min')}${attr('max')}${attr('step')} value="${esc(v ?? '')}" autocomplete="off" ${dis}>${help}</label>`;
}

function paintEngineSettings(card, engine, locked) {
  const hint = card.querySelector('#cadenza-engine-hint'), box = card.querySelector('#cadenza-engine-fields');
  if (!hint || !box || box.dataset.loaded) return;
  const settings = (engine && Array.isArray(engine.settings) ? engine.settings : []).filter(s => s && s.key && ENGINE_TYPES.includes(s.type));
  const values = engine && engine.values && typeof engine.values === 'object' && !Array.isArray(engine.values) ? engine.values : {};
  ENGINE = { settings, values };
  box.dataset.loaded = '1';
  if (!settings.length) { hint.textContent = 'This engine has no settings of its own.'; return; }
  hint.textContent = `The settings of the engine in use. People’s own filter and dither choices under Settings → Playback still win for their streams.${locked ? ' Set by an environment variable.' : ''}`;
  box.innerHTML = settings.map(s => engineField(s, Object.prototype.hasOwnProperty.call(values, s.key) ? values[s.key] : s.default, locked)).join('');
  box.hidden = false;
}

/** The engine settings as the JSON text for cadenza_engine_settings (only values that differ from the default;
 * keys this engine doesn't have stay as saved), or null when the block isn't drawn. */
function engineSettingsValue(root) {
  const box = root.querySelector('#cadenza-engine-fields');
  if (!box || !box.dataset.loaded || !ENGINE) return null;
  const out = { ...ENGINE.values };
  box.querySelectorAll('[data-ek]').forEach(el => {
    const s = ENGINE.settings.find(x => x.key === el.dataset.ek);
    if (!s) return;
    let v;
    if (s.type === 'bool') v = el.checked;
    else if (s.type === 'number') {
      v = el.value.trim() === '' ? NaN : Number(el.value);
      if (!Number.isFinite(v)) { delete out[s.key]; return; }
    } else {
      const o = (Array.isArray(s.options) ? s.options : []).find(o => String(o[0]) === el.value);
      v = o ? o[0] : el.value;
    }
    if (s.type === 'number' ? v === Number(s.default) : JSON.stringify(v) === JSON.stringify(s.default)) delete out[s.key];
    else out[s.key] = v;
  });
  return JSON.stringify(out);
}

function bindCadenzaCache(card, values) {
  const state = card.querySelector('#cadenza-cache-state'), btn = card.querySelector('#cadenza-cache-clear'), line = card.querySelector('#cadenza-engine');
  if (!state) return;
  const locked = (((values || {})._meta || {}).cadenza_engine_settings || {}).source === 'env';
  const refresh = () => api.get('/playback/dsp').then(d => {
    state.textContent = cadenzaCacheText(d?.cache);
    if (line) line.textContent = cadenzaEngineText(d?.engine);
    paintEngineSettings(card, d?.engine, locked);
  }).catch(() => { state.textContent = 'Status unavailable'; });
  refresh();
  btn.onclick = async (e) => {
    e.preventDefault();
    btn.disabled = true;
    try { await api.post('/playback/dsp/cache/clear'); toast('Cache cleared', 'ok'); } catch (err) { toast(err.message, 'err'); }
    btn.disabled = false;
    refresh();
  };
}

// Settings → Soulseek: how Encore's Soulseek engine is doing (GET /soulseek/status), asked again every 5 s while
// the card is open. The listen port is hidden while the VPN is on (the VPN's forwarded port is used).
function soulseekStatusBlock() {
  return `<div class="card flat mt" id="slsk-engine" style="padding:12px 14px">
    <b>Status</b>
    <div class="small muted" id="slsk-engine-state">Checking…</div>
  </div>`;
}

/** "Running v0.24.1 · signed in as alice" / "Not installed · Install or update it under Download tools" / "Waiting for the VPN" … */
export function soulseekStateHtml(r) {
  if (!r || typeof r !== 'object') return 'Status unavailable';
  const red = (t) => `<span style="color:var(--err)">${esc(t)}</span>`;
  const v = r.version ? ` v${esc(String(r.version).replace(/^v/, ''))}` : '';
  let line;
  if (r.state === 'not_installed') line = 'Not installed';
  else if (r.state === 'starting') line = `Starting…${r.error ? ` · ${red(r.error)}` : ''}`;
  else if (r.state === 'running') line = `Running${v}${r.logged_in ? ` · signed in as ${esc(r.username || '?')}` : r.error ? ` · ${red(r.error)}` : ' · signing in…'}`;
  else if (r.state === 'crashed') line = red(`Crashed${r.error ? ` – ${r.error}` : ''}`);
  else if (r.state === 'waiting_vpn') line = 'Waiting for the VPN';
  else line = r.enabled === false ? 'Off – switch on Enable Soulseek and save' : 'Off';
  return r.hint ? `${line} · ${esc(r.hint)}` : line;
}

function bindSoulseek(card) {
  const vpn = card.querySelector('[data-k="vpn_enabled"]'), port = card.querySelector('[data-k="soulseek_port"]');
  const field = port && port.closest('.field');
  if (vpn && field) {
    const sync = () => { field.hidden = vpn.checked; };
    sync();
    vpn.addEventListener('change', sync);
  }
  const state = card.querySelector('#slsk-engine-state');
  if (!state) return;
  const refresh = () => api.get('/soulseek/status').then(r => { state.innerHTML = soulseekStateHtml(r); }).catch(() => { state.textContent = 'Status unavailable'; });
  refresh();
  const timer = setInterval(() => { if (!card.isConnected) return clearInterval(timer); if (card.open) refresh(); }, 5000);
}

function bindTidal(card) {
  const state = card.querySelector('#tidal-state'), out = card.querySelector('#tidal-out'), btn = card.querySelector('#tidal-signin');
  const paste = card.querySelector('#tidal-paste'), toml = card.querySelector('#tidal-toml'), save = card.querySelector('#tidal-save-tokens'), signout = card.querySelector('#tidal-signout');
  if (!state) return;
  const spinner = '<span class="spinner" style="width:10px;height:10px;border-width:2px"></span>';
  const paint = (r) => {
    // partial (default lane only) and signed_in never come together: the server reports one or the other
    const signed = !!(r.signed_in || r.partial);
    state.textContent = r.partial ? 'Signed in for 16/44.1 FLAC only – sign in again for hi-res (step 2 was not finished).'
      : r.signed_in ? 'Signed in – Tidal downloads use this account.'
      : 'Not signed in – Tidal needs a one-time sign-in with your own Tidal account (no API key).';
    btn.innerHTML = `${icon.link}${signed ? 'Sign in again' : 'Sign in to Tidal'}`;
    signout.hidden = !signed;
    if (r.status === 'waiting' && r.url) {
      const steps = Number(r.steps) || 1, step = Number(r.step) || 1;
      out.hidden = false;
      out.innerHTML = `${steps > 1 ? `Step ${step} of ${steps} · ` : ''}Open <a href="${esc(r.url)}" target="_blank" rel="noopener" style="color:var(--accent)">${esc(r.url)}</a> on any device, approve in your Tidal account, then come back here. ${spinner} Waiting…`;
    } else if (r.status === 'done') { out.hidden = false; out.innerHTML = `<span class="badge ok">${icon.check} ${esc(r.message)}</span>`; }
    else if (r.status === 'failed') {
      out.hidden = false; out.innerHTML = `<span class="badge err">${esc(r.message || 'Sign-in failed')}</span>`;
      if ((r.message || '').includes('paste the tokens')) paste.open = true;
    }
  };
  let timer = null, opened = '';
  // Each step has its own link: open the new one when the server moves to step 2.
  const openLink = (r) => { if (r.status === 'waiting' && r.url && r.url !== opened) { opened = r.url; window.open(r.url, '_blank', 'noopener'); } };
  const poll = async () => {
    if (!card.isConnected) return clearInterval(timer);
    try {
      const r = await api.get('/system/streamrip/tidal-login'); paint(r); openLink(r);
      if (r.status !== 'waiting') { clearInterval(timer); timer = null; btn.disabled = false; }
    } catch { /* next tick */ }
  };
  api.get('/system/streamrip/tidal-login').then(paint).catch(() => { state.textContent = 'Status unavailable'; });
  btn.onclick = async () => {
    btn.disabled = true;
    out.hidden = false; out.innerHTML = `${spinner} Starting sign-in…`;
    try {
      const r = await api.post('/system/streamrip/tidal-login');
      paint(r);
      if (r.status === 'waiting') { openLink(r); if (timer) clearInterval(timer); timer = setInterval(poll, 3000); } else btn.disabled = false;
    } catch (e) { out.innerHTML = `<span class="badge err">${esc(e.message)}</span>`; btn.disabled = false; }
  };
  save.onclick = async () => {
    save.disabled = true;
    try {
      const r = await api.post('/system/streamrip/tidal-tokens', { toml: toml.value });
      paint(r); toml.value = ''; toast('Tidal tokens saved', 'ok');
    } catch (e) { toast(e.message, 'err'); }
    save.disabled = false;
  };
  signout.onclick = async (e) => {
    e.preventDefault();
    try { const r = await api.del('/system/streamrip/tidal-tokens'); paint(r); out.hidden = true; toast('Signed out of Tidal', 'ok'); } catch (err) { toast(err.message, 'err'); }
  };
}

// Settings → Apps & sources → Download tools: yt-dlp, streamrip, Cadenza (server processing) and slskd
// (Soulseek engine) update in place – kept in the config folder, so no new Encore image is needed.
const TOOL_SPIN = '<span class="spinner" style="width:10px;height:10px;border-width:2px"></span>';
const toolList = (r) => Array.isArray(r) ? r : (r && Array.isArray(r.tools) ? r.tools : []);
const toolUpdatable = (t) => !!t.updatable;
/** The reset link of a row: Cadenza goes back to the engine built into Encore, slskd is removed, the rest go back to the bundled copy. */
const TOOL_RESET = {
  cadenza: { link: 'Use the built-in engine', ask: 'Go back to the engine built into Encore? The downloaded Cadenza is removed.', done: 'Back to the built-in engine' },
  slskd: { link: 'Remove slskd', ask: 'Remove slskd? Soulseek stays off until you install it again.', done: 'slskd removed' },
  translator: { link: 'Remove the Translator', ask: 'Remove the Translator and its model? Translations made so far are kept; new titles only get official translations until you install it again.', done: 'Translator removed' },
};
const toolReset = (t) => TOOL_RESET[t.id] || { link: 'Use the bundled version', ask: `Go back to the ${t.name || t.id} version that came with Encore? The updated copy is removed.`, done: `Back to the bundled ${t.name || t.id}` };
/** slskd and the Translator are installed by Encore, not bundled: before that their button says Install. */
const toolInstall = (t) => (t.id === 'slskd' || t.id === 'translator') && !t.installed;

function toolRow(t, busy) {
  let action = '';
  if (busy === t.id) action = `<button class="btn sm" type="button" disabled>${TOOL_SPIN} ${toolInstall(t) ? 'Installing…' : 'Updating…'}</button>`;
  else if (toolInstall(t) && toolUpdatable(t)) action = `<button class="btn sm primary" type="button" data-tool-update="${esc(t.id)}" ${busy ? 'disabled' : ''}>${icon.download}Install${t.latest ? ` ${esc(t.latest)}` : ''}</button>`;
  else if (t.update_available && toolUpdatable(t)) action = `<button class="btn sm primary" type="button" data-tool-update="${esc(t.id)}" ${busy ? 'disabled' : ''}>${icon.download}Update to ${esc(t.latest || '')}</button>`;
  else if (t.update_available) action = `<span class="badge warn">${esc(t.latest || '')} available</span>`;
  else if (t.latest) action = `<span class="badge ok">${icon.check} Up to date</span>`;
  else if (toolUpdatable(t)) action = '<span class="small faint">Not checked yet</span>';
  const updated = t.source === 'updated' && t.id !== 'slskd' && t.id !== 'translator';
  return `<div class="list-item" style="align-items:flex-start;flex-wrap:wrap;gap:12px" data-tool="${esc(t.id)}">
      <div class="grow" style="min-width:200px"><div style="font-weight:650">${esc(t.name || t.id)} <span class="small muted mono">${t.installed ? esc(t.installed) : 'not installed'}</span>${updated ? ' <span class="small muted">(updated)</span>' : ''}</div>
        ${t.note ? `<div class="small muted">${esc(t.note)}</div>` : ''}
        ${t.error ? `<div class="small" style="color:var(--err)">${esc(t.error)}</div>` : ''}
        ${t.resettable && toolUpdatable(t) ? `<a href="#" class="small muted" data-tool-reset="${esc(t.id)}" ${busy ? 'hidden' : ''}>${esc(toolReset(t).link)}</a>` : ''}</div>
      <div class="row" style="gap:8px;align-items:center">${action}</div></div>`;
}

async function renderTools(host, getValues, setValues) {
  let tools;
  try { tools = toolList(await api.get('/system/tools')); } catch { host.innerHTML = ''; return; }
  let busy = null, checking = false;
  const values = getValues();
  const meta = values._meta || {};
  const toggle = (key, label) => {
    if (!(key in values)) return '';
    const locked = (meta[key] || {}).source === 'env';
    return `<label class="switch"><input type="checkbox" data-tool-auto="${key}" ${values[key] ? 'checked' : ''} ${locked ? 'disabled' : ''}><i></i>${esc(label)}</label>`;
  };
  const toggles = toggle('tools_auto_ytdlp', 'Update yt-dlp automatically (recommended – sites change often)')
    + toggle('tools_auto_streamrip', 'Update streamrip automatically when a new release is out')
    + toggle('tools_auto_cadenza', 'Update Cadenza automatically when a new release is out')
    + toggle('tools_auto_slskd', 'Update slskd automatically when a new release is out')
    + toggle('tools_auto_translator', 'Update the Translator (llama.cpp) automatically')
    + toggle('translate_background', 'Translate the whole library in the background (titles on screen are always translated first)');
  // the Translator's model: changing it downloads the other one on the next Install / Update
  const modelPick = 'translator_model' in values ? `<label class="stack" style="gap:4px;max-width:420px"><span class="small muted">Translator model</span>
    <select class="input" id="tools-model" ${(meta.translator_model || {}).source === 'env' ? 'disabled' : ''}>${[['small', 'Small – Qwen3 4B Instruct (about 2.5 GB)'], ['best', 'Best – Qwen3 8B (about 5 GB)']].map(([v, l]) => `<option value="${v}" ${values.translator_model === v ? 'selected' : ''}>${l}</option>`).join('')}</select>
    <span class="small muted">Small runs on most servers (about 4 GB of memory while it works); Best translates better and needs about 7 GB. It only runs while there is something to translate.</span></label>` : '';
  host.innerHTML = `<div class="card" id="tools-card" style="margin-bottom:12px">
    <div class="row between" style="margin-bottom:8px;gap:10px"><h2>Download tools</h2><button class="btn sm" type="button" id="tools-check">${icon.refresh}Check for updates</button></div>
    <p class="muted small" style="margin:0 0 14px">yt-dlp, streamrip, Cadenza, slskd and the Translator update in place – no new Encore image needed. Updates are kept in the config folder.</p>
    <div class="list" id="tools-list"></div>
    <div class="stack mt" style="gap:8px">${modelPick}${toggles}<div class="small muted">Every tool is checked daily; updates install by themselves only where the switch is on. Change the schedule under <a href="#/settings?s=tasks">Scheduled tasks</a>.</div></div>
  </div>`;
  const list = host.querySelector('#tools-list'), check = host.querySelector('#tools-check');
  const paint = () => {
    list.innerHTML = tools.map(t => toolRow(t, busy)).join('');
    check.disabled = checking || !!busy;
    check.innerHTML = checking ? `${TOOL_SPIN} Checking…` : `${icon.refresh}Check for updates`;
  };
  const reload = async () => { try { tools = toolList(await api.get('/system/tools')); } catch { /* keep the last list */ } };
  paint();
  check.onclick = async () => {
    checking = true; paint();
    try { tools = toolList(await api.post('/system/tools/check')); } catch (e) { toast(e.message, 'err'); }
    checking = false; paint();
  };
  list.onclick = async (e) => {
    const up = e.target.closest('[data-tool-update]'), reset = e.target.closest('[data-tool-reset]');
    if (up && !busy) {
      const id = up.dataset.toolUpdate;
      const tool = tools.find(t => t.id === id) || {};
      const name = tool.name || id, install = toolInstall(tool);
      busy = id; paint();
      try {
        const r = await api.post(`/system/tools/${id}/update`);
        toast(`${name} ${install ? 'installed' : 'updated to'} ${r.installed || r.latest || ''}`, 'ok');
      } catch (err) { toast(err.message, 'err'); }
      busy = null;
      await reload(); paint();
    } else if (reset && !busy) {
      e.preventDefault();
      const id = reset.dataset.toolReset;
      const how = toolReset(tools.find(t => t.id === id) || { id });
      if (!confirm(how.ask)) return;
      try { await api.post(`/system/tools/${id}/reset`); toast(how.done, 'ok'); } catch (err) { toast(err.message, 'err'); }
      await reload(); paint();
    }
  };
  host.querySelector('#tools-model')?.addEventListener('change', async (e) => {
    try { setValues(await api.put('/admin/settings', { translator_model: e.target.value })); toast('Saved – Install or Update the Translator to download this model', 'ok'); await reload(); paint(); }
    catch (err) { toast(err.message, 'err'); }
  });
  host.querySelectorAll('[data-tool-auto]').forEach(box => box.addEventListener('change', async () => {
    try { setValues(await api.put('/admin/settings', { [box.dataset.toolAuto]: box.checked })); toast('Saved', 'ok'); }
    catch (err) { box.checked = !box.checked; toast(err.message, 'err'); }
  }));
}

/** Version, build and app downloads – shown at the bottom of General (and of Account for members). */
export async function aboutCard() {
  const info = (await import('../app.js')).session.info || {};
  const server = 'Encore';
  return `<div class="card stack mt"><h2>About Encore</h2>
    <div class="row wrap" style="gap:24px"><div><div class="small muted">Version</div><div style="font-weight:700;font-size:20px">v${esc(info.version || '?')}</div></div>
      <div><div class="small muted">Server</div><div>${esc(server)}</div></div>
      <div><div class="small muted">Build</div><div class="mono">${esc(info.build || 'dev')}</div></div></div>
    <div class="small muted">Apps for Android, Windows, macOS and Linux: <a href="https://github.com/Krifmaz/Encore/releases" target="_blank" rel="noopener">github.com/Krifmaz/Encore/releases</a></div>
    <div class="small muted">Interface: JavaScript · <a href="/next/${esc(location.hash || '#/')}">Try the new Rust (WebAssembly) interface</a> – a preview of the same app rewritten in Rust; it uses your current sign-in and this link brings you back.</div></div>`;
}

/** Settings → Playback: stream quality, buffering, what happens when the queue ends, server-side Cadenza processing of streams
 * (where an admin switched it on and allowed the account; greyed out otherwise) and the switches of a linked Cadenza device. */
async function renderPlayback(el) {
  const playback = await import('../playback.js');
  const player = await import('../player.js');
  const { session } = await import('../app.js');
  const remote = await import('../remote.js');
  const serverPrefs = (session.info?.features || []).includes('theme');
  let p = playback.current();
  // server-side Cadenza: on for this server, and allowed for this account? (fetched once per visit to the tab)
  let dsp;
  try { dsp = await api.get('/playback/dsp'); } catch { dsp = { enabled: false }; }
  // the desktop app carries Cadenza: its { running, device_id, name, error } – asked again every couple of seconds
  const native = !!window.EncoreNative?.engine;
  const engineStatus = async () => { try { let s = await window.EncoreNative.engineStatus(); if (typeof s === 'string') s = JSON.parse(s); return s || null; } catch { return null; } };
  let engineSt = native ? await engineStatus() : null;
  const khz = (hz) => String(Math.round(hz / 100) / 10);
  const DSP_RATES = [88200, 96000, 176400, 192000, 352800, 384000, 705600, 768000];
  const opt = (v, l, cur) => `<option value="${esc(v)}" ${String(v) === String(cur) ? 'selected' : ''}>${esc(l)}</option>`;
  const dspHtml = () => {
    const greyed = `<div class="stack" style="gap:8px;opacity:.45;pointer-events:none" aria-disabled="true"><label class="row" style="gap:8px;align-items:center"><input type="checkbox" disabled> Upsample my streams on the server</label><label class="stack" style="gap:4px"><span class="small muted">Upsampling filter</span><select class="input" disabled><option>–</option></select></label><label class="stack" style="gap:4px"><span class="small muted">Dither / noise shaping</span><select class="input" disabled><option>–</option></select></label><label class="stack" style="gap:4px"><span class="small muted">Output rate</span><select class="input" disabled><option>–</option></select></label></div>`;
    if (!dsp?.enabled) return `${greyed}<span class="small muted">Turned off on this server. Cadenza upsampling for streams uses the server's processor, so an admin has to switch it on (Settings → Apps & sources → Server processing) and allow your account.</span>`;
    if (!dsp.allowed) return `${greyed}<span class="small muted">Your account isn't allowed to use the server's processing. Ask an admin.</span>`;
    const dev = player.deviceRate();
    const off = p.dsp ? '' : 'disabled';
    const rates = [['device', dev ? `Match this device (${khz(dev)} kHz)` : 'Match this device'], ['auto', 'Highest the server allows'],
      ...DSP_RATES.filter(r => r <= (dsp.max_rate || 0)).map(r => [r, `${khz(r)} kHz`])];
    return `<label class="row" style="gap:8px;align-items:center"><input type="checkbox" id="pb-dsp" ${p.dsp ? 'checked' : ''}> Upsample my streams on the server</label>
        <label class="stack" style="gap:4px"><span class="small muted">Upsampling filter</span><select class="input" id="pb-dsp-filter" ${off}>${opt('', 'Server default', p.dsp_filter)}${(dsp.filters || []).map(([v, l]) => opt(v, l, p.dsp_filter)).join('')}</select></label>
        <label class="stack" style="gap:4px"><span class="small muted">Dither / noise shaping</span><select class="input" id="pb-dsp-dither" ${off}>${(dsp.dithers || []).map(([v, l]) => opt(v, l, p.dsp_dither)).join('')}</select></label>
        <label class="stack" style="gap:4px"><span class="small muted">Output rate</span><select class="input" id="pb-dsp-rate" ${off}>${rates.map(([v, l]) => opt(v, l, p.dsp_rate)).join('')}</select></label>
        <span class="small muted">The server's Cadenza upsamples the original file and sends lossless 24-bit FLAC. It uses more data (about 4.6 Mbit/s at 96 kHz, 9.2 Mbit/s at 192 kHz) and the server's processor; Data saver and Auto still step down to a lighter stream when the connection can't keep up.</span>`;
  };
  const seg = (key, opts) => `<div class="seg" data-key="${key}">${opts.map(([v, l]) => `<button type="button" data-v="${v}" class="${String(p[key]) === String(v) ? 'on' : ''}">${l}</button>`).join('')}</div>`;
  const QUALITY_HELP = {
    auto: 'The original file. Encore measures your connection while it plays and steps the stream down as far as it needs to keep the music playing – down to 96 kbps (64 kbps only on a very poor connection) – then back up as soon as the connection allows.',
    original: "Always the original file, even if it stutters on a slow connection. Formats a browser can't play are converted to FLAC (lossless).",
    saver: 'Always a small 96 kbps stream (AAC, or Opus in browsers without AAC) – for mobile data and slow links.',
  };
  // sound effects on this device: the bands on screen are the preset's own until you edit one (then the preset is Custom)
  const bands = () => playback.eqBands(p).map(b => ({ ...b }));
  const sign = (v) => `${v > 0 ? '+' : ''}${v} dB`;
  const fadeText = (v) => v > 0 ? `${v} s` : 'Off';
  const bandRow = (b, i) => `<tr data-i="${i}">
      <td><select class="input" data-f="type" aria-label="Band ${i + 1} type">${playback.EQ_TYPES.map(([v, l]) => opt(v, l, b.type)).join('')}</select></td>
      <td><input class="input" type="number" data-f="freq" min="20" max="20000" step="1" value="${b.freq}" aria-label="Band ${i + 1} frequency (Hz)"></td>
      <td><input class="input" type="number" data-f="gain" min="-24" max="24" step="0.5" value="${b.gain}" aria-label="Band ${i + 1} gain (dB)"></td>
      <td><input class="input" type="number" data-f="q" min="0.1" max="18" step="0.1" value="${b.q}" aria-label="Band ${i + 1} Q"></td>
      <td><button type="button" class="btn ghost icon sm" data-rm-band="${i}" aria-label="Remove band" title="Remove band">✕</button></td></tr>`;
  const soundHtml = () => {
    const list = bands();
    return `<label class="row" style="gap:8px;align-items:center"><input type="checkbox" id="pb-fx" ${p.fx_on ? 'checked' : ''}> Use sound effects</label>
      <fieldset class="stack" id="pb-fx-box" style="gap:8px;border:0;padding:0;margin:0;min-width:0;${p.fx_on ? '' : 'opacity:.45'}" ${p.fx_on ? '' : 'disabled'}>
        <label class="stack" style="gap:4px"><span class="small muted">Equalizer preset</span><select class="input" id="pb-eq-preset">${playback.EQ_PRESETS.map(x => opt(x.id, x.label, p.eq_preset)).join('')}${p.eq_preset === 'custom' ? opt('custom', 'Custom', p.eq_preset) : ''}</select></label>
        <label class="stack" style="gap:4px"><span class="small muted">Preamp <b id="pb-eq-preamp-v">${sign(p.eq_preamp)}</b></span><input type="range" id="pb-eq-preamp" min="-24" max="12" step="0.5" value="${p.eq_preamp}"></label>
        <div style="overflow-x:auto"><table class="eq-bands" style="width:100%;border-collapse:collapse">
          <thead><tr class="small muted" style="text-align:left"><th>Type</th><th>Frequency (Hz)</th><th>Gain (dB)</th><th>Q</th><th></th></tr></thead>
          <tbody>${list.length ? list.map(bandRow).join('') : '<tr><td colspan="5" class="small muted">No bands.</td></tr>'}</tbody></table></div>
        <div class="row"><button type="button" class="btn sm" id="pb-eq-add" ${list.length >= 10 ? 'disabled' : ''}>${icon.plus}Add band</button></div>
        <label class="stack" style="gap:4px"><span class="small muted">Crossfeed</span><select class="input" id="pb-crossfeed">${playback.CROSSFEED.map(([v, l]) => opt(v, l, p.crossfeed)).join('')}</select></label>
        <span class="small muted">For headphones: mixes a little of each channel into the other, like speakers in a room.</span>
        <label class="stack" style="gap:4px"><span class="small muted">Balance <b id="pb-balance-v">${playback.balanceText(p.balance)}</b></span><input type="range" id="pb-balance" min="-100" max="100" step="1" value="${p.balance}"></label>
        <label class="row" style="gap:8px;align-items:center"><input type="checkbox" id="pb-mono" ${p.mono ? 'checked' : ''}> Mono</label>
      </fieldset>
      <span class="small muted">Runs in this browser or app – it costs your server nothing. Works with every quality setting. Turning it on means the sound is no longer the untouched file (the Signal path says so).</span>`;
  };
  // Song transitions: how one song becomes the next (every mode uses the seconds and the album rule)
  const TRANSITION_HELP = {
    off: 'Each song plays to its end, then the next one starts.',
    crossfade: 'The end of one song fades out while the next one fades in.',
    overlap: 'The next song starts at full volume a few seconds early while the old one fades out under it.',
    smart: 'Skips the silence at the end of one song and the start of the next, then overlaps them from the last audible moment, with a short fade.',
  };
  const transitionsHtml = (cadenza) => {
    const off = p.transition === 'off';
    const secs = Math.max(1, p.crossfade || playback.TRANSITION_DEFAULT_S);
    return `${seg('transition', playback.TRANSITIONS)}
      <span class="small muted">${TRANSITION_HELP[p.transition] || ''}</span>
      <label class="stack" style="gap:4px;${off ? 'opacity:.45' : ''}"><span class="small muted">Length <b id="pb-crossfade-v">${fadeText(secs)}</b></span><input type="range" id="pb-crossfade" min="1" max="12" step="1" value="${secs}" ${off ? 'disabled' : ''}></label>
      <label class="row" style="gap:8px;align-items:center"><input type="checkbox" id="pb-crossfade-gapless" ${p.crossfade_album_gapless ? 'checked' : ''}> Keep albums gapless (no transition between songs from the same album)</label>
      ${cadenza ? '<span class="small muted">Song transitions don\'t apply while playing through Cadenza</span>' : ''}`;
  };
  const bindSound = () => {
    el.querySelector('#pb-fx').onchange = (e) => set({ fx_on: e.target.checked });
    el.querySelector('#pb-eq-preset').onchange = (e) => {
      const x = playback.EQ_PRESETS.find(y => y.id === e.target.value);
      if (x) set({ eq_preset: x.id, eq_bands: x.bands.map(b => ({ ...b })), eq_preamp: x.preamp });
    };
    const preamp = el.querySelector('#pb-eq-preamp');
    preamp.oninput = () => { el.querySelector('#pb-eq-preamp-v').textContent = sign(+preamp.value); };
    preamp.onchange = () => set({ eq_preamp: +preamp.value });
    el.querySelectorAll('.eq-bands tr[data-i] [data-f]').forEach(c => c.onchange = () => {
      const list = bands();
      const i = +c.closest('tr').dataset.i;
      list[i] = { ...list[i], [c.dataset.f]: c.dataset.f === 'type' ? c.value : +c.value };
      set({ eq_preset: 'custom', eq_bands: list });
    });
    el.querySelectorAll('[data-rm-band]').forEach(b => b.onclick = () => {
      const list = bands();
      list.splice(+b.dataset.rmBand, 1);
      set({ eq_preset: 'custom', eq_bands: list });
    });
    el.querySelector('#pb-eq-add').onclick = () => {
      const list = bands();
      if (list.length < 10) set({ eq_preset: 'custom', eq_bands: [...list, { type: 'peaking', freq: 1000, gain: 0, q: 1 }] });
    };
    el.querySelector('#pb-crossfeed').onchange = (e) => set({ crossfeed: e.target.value });
    const bal = el.querySelector('#pb-balance');
    bal.oninput = () => { el.querySelector('#pb-balance-v').textContent = playback.balanceText(+bal.value); };
    bal.onchange = () => set({ balance: +bal.value });
    el.querySelector('#pb-mono').onchange = (e) => set({ mono: e.target.checked });
  };
  const bindTransitions = () => {
    const cf = el.querySelector('#pb-crossfade');
    cf.oninput = () => { el.querySelector('#pb-crossfade-v').textContent = fadeText(+cf.value); };
    cf.onchange = () => set({ crossfade: +cf.value });
    el.querySelector('#pb-crossfade-gapless').onchange = (e) => set({ crossfade_album_gapless: e.target.checked });
  };
  // what the Cadenza engine section is painted from – a repaint only when it changes
  const engineKey = () => JSON.stringify([player.linked()?.id, player.linked()?.name, player.linked()?.state?.engine ?? null, engineSt]);
  let painted = '';
  const tick = setInterval(async () => {
    if (!el.isConnected) return clearInterval(tick);
    if (native) engineSt = await engineStatus();
    if (el.isConnected && engineKey() !== painted) paint();
  }, 2000);
  const paint = () => {
    const link = player.linked();
    const eng = link?.state?.engine;
    // the engine inside this very app, or a Cadenza somewhere else?
    const local = !!eng?.embedded && remote.isEngine(link.id);
    painted = engineKey();
    const engTitle = eng ? (local ? ' · this computer' : ` · ${esc(link.name)}`) : native ? ' · this computer' : '';
    const engBlock = eng ? engineHtml(eng) : native ? (remote.engineOff() ? ENGINE_OFF_COPY : engineStartingHtml()) : ENGINE_GREYED;
    el.innerHTML = `<div class="card stack">
      <h2>Playback</h2>
      <p class="small muted" style="margin:0">${serverPrefs ? 'Saved to your account, so it follows you to every device.' : 'Saved on this device.'}</p>
      <div class="stack" style="gap:6px"><h3>Quality</h3>${seg('quality', [['auto', 'Auto'], ['original', 'Original'], ['saver', 'Data saver']])}
        <span class="small muted">${QUALITY_HELP[p.quality]}</span></div>
      <div class="stack" style="gap:6px"><h3>Buffering</h3>
        <label class="row" style="gap:8px;align-items:center"><input type="checkbox" id="pb-preload" ${p.preload ? 'checked' : ''}> Pre-load the next song while this one plays</label>
        <span class="small muted">The next song is fetched once the current one is safely buffered, so it starts without a gap. Turn it off on a very slow connection to leave all the bandwidth to the song playing.</span></div>
      <div class="stack" style="gap:6px"><h3>When the queue ends</h3>
        <label class="row" style="gap:8px;align-items:center"><input type="checkbox" id="pb-autoplay" ${p.autoplay ? 'checked' : ''}> Keep going with a shuffle of the whole library</label>
        <label class="row" style="gap:8px;align-items:center"><input type="checkbox" id="pb-resume" ${p.resume ? 'checked' : ''}> Resume where I left off when the app opens</label></div>
      <div class="stack" style="gap:6px"><h3>Sound (on this device)</h3>
        ${soundHtml()}</div>
      <div class="stack" style="gap:6px"><h3>Song transitions</h3>
        ${transitionsHtml(!!eng)}</div>
      <div class="stack" style="gap:6px"><h3>Pure playback</h3>
        <span class="small muted">Encore streams the original file untouched – the quality badge on the player opens the <b>Signal path</b>, which shows what the file is, what was sent and what this device does with it. A browser hands the sound to the operating system, which mixes it with other sounds and may resample it; bit-perfect output needs an exclusive-mode engine.</span></div>
      <div class="stack" style="gap:6px"><h3>Server processing (Cadenza)</h3>
        ${dspHtml()}</div>
      <div class="stack" style="gap:6px"><h3>Cadenza engine${engTitle}</h3>
        ${engBlock}</div>
    </div>`;
    el.querySelectorAll('.seg[data-key] button').forEach(b => b.onclick = () => set({ [b.closest('.seg').dataset.key]: b.dataset.v }));
    el.querySelector('#pb-preload').onchange = (e) => set({ preload: e.target.checked });
    el.querySelector('#pb-autoplay').onchange = (e) => set({ autoplay: e.target.checked });
    el.querySelector('#pb-resume').onchange = (e) => set({ resume: e.target.checked });
    bindSound();
    bindTransitions();
    const dspBox = el.querySelector('#pb-dsp');
    if (dspBox) {
      dspBox.onchange = (e) => set({ dsp: e.target.checked });
      el.querySelector('#pb-dsp-filter').onchange = (e) => set({ dsp_filter: e.target.value });
      el.querySelector('#pb-dsp-dither').onchange = (e) => set({ dsp_dither: e.target.value });
      el.querySelector('#pb-dsp-rate').onchange = (e) => set({ dsp_rate: /^\d+$/.test(e.target.value) ? Number(e.target.value) : e.target.value });
    }
    // every engine switch goes to the linked device as { action: 'engine', set: { key: value } }
    const sendEng = (set) => { link.send({ action: 'engine', set }); if (!local) toast('Sent to ' + link.name, 'ok'); };
    const num = (v) => /^-?\d+(\.\d+)?$/.test(v) ? Number(v) : v;
    el.querySelectorAll('[data-eng]').forEach(c => {
      if (c.type === 'range') c.oninput = () => { const v = el.querySelector('#' + c.dataset.v); if (v) v.textContent = dbText(+c.value); };
      c.onchange = (e) => {
        const k = c.dataset.eng;
        const v = c.type === 'checkbox' ? e.target.checked : c.type === 'range' ? Number(c.value) : ['rate', 'bits'].includes(k) ? num(c.value) : c.value;
        sendEng({ [k]: v });
      };
    });
    el.querySelectorAll('[data-eng-seg] button').forEach(b => b.onclick = () => sendEng({ [b.closest('.seg').dataset.engSeg]: b.dataset.v }));
    const ha = el.querySelector('#eng-headroom-auto');
    if (ha) ha.onchange = (e) => sendEng({ headroom_db: e.target.checked ? null : -3 });
  };
  const dbText = (v) => `${(Number(v) || 0).toFixed(1)} dB`;
  /** A linked Cadenza describes every switch in its state (see Cadenza's encore/state.rs): outputs, exclusive, bit-perfect, PCM and DSD groups, headroom, soft clipper, GPU, matrix. */
  const engineHtml = (e) => {
    const sel = (key, label, list, cur, extra = '') => Array.isArray(list) ? `<label class="stack" style="gap:4px"><span class="small muted">${label}</span><select class="input" data-eng="${key}" ${extra}>${list.map(([v, l]) => `<option value="${esc(v)}" ${String(v) === String(cur) ? 'selected' : ''}>${esc(l)}</option>`).join('')}</select></label>` : '';
    const segE = (key, list, cur) => `<div class="seg" data-eng-seg="${key}">${list.map(([v, l]) => `<button type="button" data-v="${esc(v)}" class="${String(v) === String(cur) ? 'on' : ''}">${esc(l)}</button>`).join('')}</div>`;
    const range = (key, label, id, v, min) => `<label class="stack" style="gap:4px"><span class="small muted">${label}: <span id="${id}">${dbText(v)}</span></span><input type="range" min="${min}" max="0" step="0.5" value="${Number(v) || 0}" data-eng="${key}" data-v="${id}" style="accent-color:var(--accent)"></label>`;
    const bp = e.bitperfect ? 'disabled' : '';
    const dsd = e.dsd || {};
    const manual = e.headroom_db !== null && e.headroom_db !== undefined;
    const gpuLine = e.gpu_in_use ? `In use: ${esc(e.gpu_name || 'NVIDIA GPU')}` : e.gpu_available ? `${esc(e.gpu_name || 'NVIDIA GPU')} found, not in use` : 'No NVIDIA GPU found';
    return `<div class="stack" style="gap:8px">
      ${sel('output', 'Output device', e.outputs, e.output_id)}
      ${'exclusive' in e ? `<label class="row" style="gap:8px;align-items:center"><input type="checkbox" data-eng="exclusive" ${(e.exclusive_wanted ?? e.exclusive) ? 'checked' : ''}> Exclusive – Cadenza alone uses the DAC, nothing else is mixed in</label>` : ''}
      <label class="row" style="gap:8px;align-items:center"><input type="checkbox" data-eng="bitperfect" ${e.bitperfect ? 'checked' : ''}> Bit-perfect – send the file to the DAC untouched (no upsampling, dither or volume)</label>
      ${Array.isArray(e.output_modes) ? `<div class="stack" style="gap:4px"><span class="small muted">Output mode</span>${segE('output_mode', e.output_modes, e.output_mode)}</div>` : ''}
      ${e.output_mode === 'dsd' ? `${sel('dsd_rate', 'DSD rate', dsd.rates, dsd.rate, bp)}
      ${sel('dsd_modulator', 'DSD modulator', dsd.modulators, dsd.modulator, bp)}
      ${sel('dsd_packing', 'DSD packing', dsd.packings, dsd.packing, bp)}
      ${Array.isArray(dsd.oversamplings) ? `<div class="stack" style="gap:4px"><span class="small muted">DSD oversampling</span>${segE('dsd_oversampling', dsd.oversamplings, dsd.oversampling)}</div>` : ''}
      ${'gain_db' in dsd ? range('dsd_gain_db', 'DSD gain', 'eng-dsd-gain-v', dsd.gain_db, -6) : ''}`
      : `${sel('filter', 'Upsampling filter', e.filters, e.filter, bp)}
      ${sel('dither', 'Dither / noise shaping', e.dithers, e.dither, bp)}
      ${sel('rate', 'Output rate', e.rates, e.rate, bp)}
      ${sel('bits', 'Bit depth', Array.isArray(e.bits_list) ? e.bits_list.map(b => [b, `${b}-bit`]) : null, e.bits, bp)}`}
      ${'headroom_db' in e ? `<label class="row" style="gap:8px;align-items:center"><input type="checkbox" id="eng-headroom-auto" ${manual ? '' : 'checked'}> Automatic headroom – room for the filters, so loud peaks do not clip</label>
      ${manual ? range('headroom_db', 'Headroom', 'eng-headroom-v', e.headroom_db, -12) : ''}` : ''}
      ${'soft_clip' in e ? `<label class="row" style="gap:8px;align-items:center"><input type="checkbox" data-eng="soft_clip" ${e.soft_clip ? 'checked' : ''}> Soft clipper – round off peaks that would clip instead of cutting them</label>` : ''}
      ${'gpu' in e ? `<div class="stack" style="gap:4px"><span class="small muted">GPU</span>${segE('gpu', [['auto', 'Auto'], ['on', 'On'], ['off', 'Off']], e.gpu)}<span class="small muted">${gpuLine}</span></div>` : ''}
      ${sel('matrix', 'Matrix (channel mixing)', e.matrix_presets, e.matrix ?? 'off')}
      <span class="small muted">Output: ${esc(e.output || '–')} · ${e.exclusive ? 'Exclusive' : 'Shared'}</span>
      ${e.error ? `<span class="small" style="color:var(--err)">${esc(e.error)}</span>` : ''}
    </div>`;
  };
  /** Desktop app: the engine is signing in but its device has not shown up yet (or says why not). */
  const engineStartingHtml = () => engineSt?.error
    ? `<span class="small" style="color:var(--err)">${esc(engineSt.error)}</span><span class="small muted">Encore keeps trying.</span>`
    : '<span class="small muted">Starting the engine…</span>';
  const set = (patch) => { playback.save({ ...p, ...patch }, { serverPrefs }).then(c => { p = c; paint(); }); p = { ...p, ...patch }; paint(); };
  paint();
}

/** Settings → Appearance: theme, accent colour, faded/solid background. */
async function renderAppearance(el) {
  const appearance = await import('../appearance.js');
  const { session } = await import('../app.js');
  const serverPrefs = (session.info?.features || []).includes('theme');
  let p = appearance.current();
  const seg = (key, opts) => `<div class="seg" data-key="${key}">${opts.map(([v, l]) => `<button type="button" data-v="${v}" class="${p[key] === v ? 'on' : ''}">${l}</button>`).join('')}</div>`;
  const paint = () => {
    el.innerHTML = `<div class="card stack">
      <h2>Appearance</h2>
      <p class="small muted" style="margin:0">${serverPrefs ? 'Saved to your account, so it follows you to every device.' : 'Saved on this device.'}</p>
      <div class="stack" style="gap:8px"><h3>Presets</h3>
        <div class="presets">${appearance.PRESETS.map(x => {
          const bg = { encore: '#161617', oled: '#000', light: '#f6f6fb', dark: '#0b0b12' }[x.theme];
          const ac = appearance.ACCENTS.find(a => a.id === x.accent) || { a: x.custom, b: x.custom };
          const on = p.theme === x.theme && p.accent === x.accent && p.background === x.background && (x.accent !== 'custom' || p.custom === x.custom);
          return `<button type="button" class="preset ${on ? 'on' : ''}" data-preset="${x.id}"><i style="background:linear-gradient(135deg, ${ac.a} 0 22%, ${ac.b} 22% 34%, ${bg} 34%)"></i>${esc(x.name)}</button>`;
        }).join('')}</div></div>
      <div class="stack" style="gap:6px"><h3>Theme</h3>${seg('theme', [['encore', 'Encore'], ['dark', 'Dark'], ['oled', 'OLED black'], ['light', 'Light'], ['system', 'Match device']])}</div>
      <div class="stack" style="gap:8px"><h3>Accent colour</h3>
        <div class="swatches">${appearance.ACCENTS.map(a => `<button type="button" class="swatch ${p.accent === a.id ? 'on' : ''}" data-accent="${a.id}" title="${esc(a.name)}" aria-label="${esc(a.name)}" style="background:linear-gradient(135deg, ${a.a}, ${a.b})"></button>`).join('')}</div>
        <div class="custom-colour"><span class="small muted">Any colour:</span><input type="color" id="custom" value="${esc(p.custom)}" title="Pick any colour">
          <input class="input" id="custom-hex" value="${esc(p.custom)}" maxlength="7" spellcheck="false" aria-label="Hex colour">
          ${p.accent === 'custom' ? '<span class="badge accent">in use</span>' : ''}</div></div>
      <div class="stack" style="gap:6px"><h3>Background</h3>${seg('background', [['fade', 'Faded colour'], ['solid', 'Solid']])}</div>
      <div class="row"><button class="btn primary" type="button">${icon.check}Example button</button><span class="badge accent">Example badge</span></div>
    </div>`;
    el.querySelectorAll('.seg button').forEach(b => b.onclick = () => set({ [b.closest('.seg').dataset.key]: b.dataset.v }));
    el.querySelectorAll('[data-accent]').forEach(b => b.onclick = () => set({ accent: b.dataset.accent }));
    el.querySelectorAll('[data-preset]').forEach(b => b.onclick = () => {
      const { id, name, ...look } = appearance.PRESETS.find(x => x.id === b.dataset.preset);
      set(look);
    });
    el.querySelector('#custom').oninput = (e) => { p = { ...p, accent: 'custom', custom: e.target.value }; el.querySelector('#custom-hex').value = e.target.value; appearance.apply(p); };
    el.querySelector('#custom').onchange = (e) => set({ accent: 'custom', custom: e.target.value });
    el.querySelector('#custom-hex').onchange = (e) => {
      let v = e.target.value.trim();
      if (!v.startsWith('#')) v = '#' + v;
      if (/^#([0-9a-f]{3}|[0-9a-f]{6})$/i.test(v)) set({ accent: 'custom', custom: v.length === 4 ? '#' + [...v.slice(1)].map(c => c + c).join('') : v.toLowerCase() });
      else toast('Use a hex colour like #ff7a00', 'err');
    };
  };
  const set = (patch) => { p = { ...p, ...patch }; appearance.save(p, { serverPrefs }); paint(); };
  paint();
}

function pipelineHelp() {
  return `<div class="card mt"><h3>How requests are fulfilled</h3>
    <ol class="muted" style="line-height:1.9;padding-left:18px;margin:10px 0 0">
      <li><b>Library check</b> – if the album/track is already in your library (by MusicBrainz id or name) it's marked available instantly.</li>
      <li><b>streamrip</b> – lossless from Qobuz, Tidal or Deezer with your own accounts, matched by ISRC.</li>
      <li><b>Torrents & Usenet</b> – Prowlarr searches your indexers; the best album release goes to your torrent or Usenet client and is imported when it finishes. If it stalls past the timeout, the next release or source is tried.</li>
      <li><b>Soulseek</b> – Encore searches the network, results are ranked by track count, format, bitrate, folder name and the uploader's free slots/speed. Stalled or failed users are skipped automatically.</li>
      <li><b>yt-dlp</b> – each track from the MusicBrainz tracklist is matched on YouTube (preferring official "Topic" uploads with matching length).</li>
      <li><b>Import</b> – files are matched to the MusicBrainz release, tagged (incl. MBIDs, ISRCs, cover art), renamed with your template, and your media servers are refreshed.</li>
    </ol></div>`;
}
