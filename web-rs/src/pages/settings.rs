//! Port of web/js/pages/settings.js – see docs/ui-rewrite/CONTRACT.md
//! The settings hub: the nav groups, the admin settings form (every `S(...)` field with its help text),
//! secrets with the eye/copy buttons, Save and Test, scheduled tasks, appearance, the API-key and About
//! cards, and the embedded sub-pages (account, connections, users, invites, auto-download, diagnostics).

use crate::api::{self, ApiError};
use crate::app::{self, Ctx, PageFuture};
use crate::appearance;
use crate::dom;
use crate::json::{js_string, json, truthy, J};
use crate::pages::{account, admin, autodownload, connections, diagnostics};
use crate::ui::{copy_text, esc, escv, icon, set_secret_text, toast, EYE, EYE_OFF};
use serde_json::{Map, Value};
use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Element, HtmlInputElement};

/// One `S(key, label, type, extra)` of the JS.
#[derive(Clone, Copy)]
struct Field {
    key: &'static str,
    label: &'static str,
    kind: &'static str,
    help: Option<&'static str>,
    full: bool,
    secret: bool,
    step: Option<&'static str>,
    options: &'static [(&'static str, &'static str)],
    dynamic: Option<&'static str>,
}

/// `S(key, label, type = 'text', extra = {})` – the `extra` keys are the builder methods below.
const fn s(key: &'static str, label: &'static str, kind: &'static str) -> Field {
    Field { key, label, kind, help: None, full: false, secret: false, step: None, options: &[], dynamic: None }
}
impl Field {
    const fn help(mut self, h: &'static str) -> Self { self.help = Some(h); self }
    const fn full(mut self) -> Self { self.full = true; self }
    const fn secret(mut self) -> Self { self.secret = true; self }
    const fn step(mut self, st: &'static str) -> Self { self.step = Some(st); self }
    const fn options(mut self, o: &'static [(&'static str, &'static str)]) -> Self { self.options = o; self }
}

struct Section {
    id: &'static str,
    title: &'static str,
    desc: &'static str,
    test: Option<&'static str>,
    test_label: Option<&'static str>,
    fields: &'static [Field],
}

const SECTIONS: &[Section] = &[
    Section { id: "general", title: "General", desc: "Your server's name and address, who can sign up, and how requests are approved.", test: None, test_label: None, fields: &[
        s("app_name", "App name", "text"),
        s("base_url", "Public URL", "text").help("e.g. https://music.example.com – used in links & notifications"),
        s("signup_mode", "Sign-up mode", "select").options(&[("invite", "Invite only"), ("open", "Open to anyone"), ("closed", "Closed")]),
        s("auto_approve", "Auto-approve all requests", "bool").help("Otherwise only admins and users with auto-approve skip the approval queue"),
        s("manual_search", "Manual download picker", "select").options(&[("everyone", "Everyone"), ("admins", "Admins only")]).help("Who can search the sources and pick an exact download"),
        s("default_request_limit", "Default weekly request limit for new users", "number").help("0 = unlimited"),
        s("session_days", "Stay signed in for (days)", "number"),
        s("app_session_days", "Keep the Encore app signed in for (days)", "number").help("Counted from the last time the app was used"),
        s("contact_email", "Contact email for MusicBrainz", "text").help("MusicBrainz asks API clients to identify themselves"),
        s("musicbrainz_url", "MusicBrainz server", "text").help("Use your own mirror for faster lookups"),
        s("musicbrainz_rate", "MusicBrainz requests / second", "number").step("0.1").help("Keep at 1 for the public server"),
    ] },
    Section { id: "library", title: "Library & tagging", desc: "Where your music lives, and how downloaded files are tagged, named and organised.", test: None, test_label: None, fields: &[
        s("library_path", "Music library path", "text").help("Inside the container (mount your music folder here)"),
        s("staging_path", "Staging path for yt-dlp downloads", "text"),
        s("scan_interval_minutes", "Rescan library every (minutes)", "number").help("0 disables periodic scans"),
        s("autotag_on_import", "Tag downloads from MusicBrainz on import", "bool"),
        s("autotag_threshold", "Auto-tag confidence threshold", "number").step("0.05").help("0–1. Matches below this are imported with their existing tags"),
        s("rename_on_import", "Rename & organise files", "bool"),
        s("naming_template", "Album naming template", "text").full().help("Fields: {albumartist} {artist} {album} {year} {disc} {discprefix} {track:02d} {title} {genre} {label} {type} {format}"),
        s("single_template", "Single-track naming template", "text").full(),
        s("save_cover", "Save cover.jpg in album folders", "bool"),
        s("embed_cover", "Embed cover art in files", "bool"),
    ] },
    Section { id: "pipeline", title: "Download pipeline", desc: "Which sources are searched for a request, and in what order. The first one that has it wins.", test: None, test_label: None, fields: &[
        s("discography_singles", "Requesting a whole discography includes singles (not just albums and EPs)", "bool"),
        s("source_order", "Source order", "text").full().help("The one place to set what is searched, in order, comma-separated: qobuz, tidal, deezer (all through streamrip), slskd (Soulseek), torznab (torrents/Usenet, albums only), ytdlp (YouTube Music). Leave one out to never use it; sources you haven’t set up are skipped. Recommended (the default): qobuz, tidal, deezer, slskd, torznab, ytdlp – exact hi-res/lossless releases first, then Soulseek, then torrents/Usenet for whole albums, YouTube Music last."),
    ] },
    Section { id: "verify", title: "Verification & duplicates", desc: "Listens to each download and checks it's really the song you asked for, not a remix, live or wrong track.", test: Some("acoustid"), test_label: None, fields: &[
        s("acoustid_api_key", "AcoustID API key", "password").full().help("Free – register an application at acoustid.org/new-application. Used to fingerprint audio and confirm each file is the right song/version."),
        s("verify_on_import", "Fingerprint every download before import", "bool"),
        s("reject_mismatch", "Reject downloads whose audio is a different song/version (and try the next source)", "bool"),
        s("dedupe_on_import", "Never import a second copy of a song (keep the better-quality file)", "bool"),
        s("upgrade_check_audio", "Before replacing a song with a better copy, compare the audio – a different recording is kept as its own song", "bool"),
    ] },
    Section { id: "streamrip", title: "Qobuz / Tidal / Deezer (streamrip)", desc: "Searches Qobuz, Tidal and Deezer with your own accounts and downloads lossless (FLAC). Usually the best quality. streamrip is only used for these three: its SoundCloud support would give the same quality as yt-dlp, so SoundCloud links go through yt-dlp.", test: Some("streamrip"), test_label: Some("Test & write config"), fields: &[
        s("streamrip_enabled", "Enable streamrip source", "bool").help("Downloads lossless from your own paid accounts. Respect each service’s terms – use at your own risk."),
        s("streamrip_quality", "Max quality", "select").options(&[("1", "MP3 320 / AAC"), ("2", "CD – 16-bit FLAC"), ("3", "Hi-res ≤ 24/96"), ("4", "Hi-res max")]),
        s("streamrip_deezer_arl", "Deezer ARL cookie", "password").full().help("From deezer.com → browser dev tools → Cookies → arl"),
        s("streamrip_qobuz_user", "Qobuz user id", "text"),
        s("streamrip_qobuz_token", "Qobuz auth token", "password").help("play.qobuz.com → dev tools → Network → user/login response"),
        s("streamrip_bin", "streamrip executable (optional)", "text").help("Leave empty to auto-detect"),
        s("streamrip_command", "Command template (advanced)", "text").full(),
    ] },
    Section { id: "sync", title: "Playlists & sync", desc: "Keeps liked songs and playlists in step with Spotify, YouTube Music, Jellyfin, Plex and the rest. Runs as the “Sync likes & playlists” task above.", test: None, test_label: None, fields: &[
        s("sync_interval_minutes", "Sync likes & playlists every (minutes)", "number").help("0 disables automatic sync – same as the task’s “every” above"),
        s("auto_request_followed", "Automatically download new tracks in followed playlists (more rules under Admin → Auto-download)", "bool"),
    ] },
    Section { id: "torznab", title: "Torrents & Usenet (Prowlarr)", desc: "Searches your torrent and Usenet indexers (through Prowlarr) for whole albums and sends them to qBittorrent or SABnzbd.", test: Some("torznab"), test_label: None, fields: &[
        s("torznab_enabled", "Search torrent and Usenet indexers through Prowlarr", "bool").help("Albums only. Prowlarr searches every indexer you added there; torrents go to qBittorrent, Usenet to SABnzbd."),
        s("prowlarr_url", "Prowlarr URL", "text").help("Internal address, e.g. http://prowlarr:9696"),
        s("prowlarr_api_key", "Prowlarr API key", "password").help("Prowlarr → Settings → General → API Key"),
        s("torznab_min_seeders", "Minimum seeders", "number"),
        s("torznab_timeout_minutes", "Try the next release after a stall of (minutes)", "number"),
        s("qbit_url", "qBittorrent URL", "text").help("e.g. http://qbittorrent:8080 (leave empty to skip torrents)"),
        s("qbit_user", "qBittorrent user", "text"),
        s("qbit_password", "qBittorrent password", "password"),
        s("qbit_category", "qBittorrent category", "text"),
        s("qbit_download_path", "Torrent download folder (same path in both containers)", "text").full(),
        s("sab_url", "SABnzbd URL", "text").help("e.g. http://sabnzbd:8080 (leave empty to skip Usenet)"),
        s("sab_api_key", "SABnzbd API key", "password"),
        s("sab_category", "SABnzbd category", "text"),
        s("sab_download_path", "Usenet download folder (as mounted in this container)", "text").full(),
    ] },
    Section { id: "slskd", title: "Soulseek", desc: "Searches the Soulseek file-sharing network – great for FLAC and rare releases. Encore runs Soulseek for you: enter any username and password and an account is created at the first sign-in. Forward the port for the best speeds.", test: Some("slskd"), test_label: None, fields: &[
        s("slskd_enabled", "Enable Soulseek", "bool"),
        s("soulseek_username", "Soulseek username", "text").help("New name + password = new account. Don’t use the same login in another Soulseek client at the same time."),
        s("soulseek_password", "Soulseek password", "password"),
        s("soulseek_port", "Listen port", "number").help("TCP port Soulseek listens on – forward it on your router (and publish it in Docker) so users behind firewalls can send to you. While Send Soulseek through a VPN is on, the VPN’s forwarded port is used instead."),
        s("soulseek_share", "Share your library on Soulseek (recommended)", "bool").help("The network runs on sharing; many users refuse downloads to people who share nothing. Only the library folder is shared, read-only."),
        s("soulseek_upload_slots", "Upload slots", "number"),
        s("slskd_download_path", "Downloads folder (as mounted in this container)", "text").full(),
        s("slskd_preferred_format", "Preferred format", "select").options(&[("flac", "FLAC / lossless"), ("mp3", "MP3"), ("any", "Best available")]),
        s("slskd_min_bitrate", "Minimum MP3 bitrate (kbps)", "number"),
        s("slskd_search_timeout", "Search time (seconds)", "number"),
        s("slskd_stall_minutes", "Give up on a stalled user after (minutes)", "number"),
        s("slskd_max_candidates", "Max users to try per request", "number"),
        s("vpn_enabled", "Send Soulseek through a VPN", "bool").help("Only Soulseek goes through the VPN; Tidal, Qobuz, Deezer and everything else keep your normal connection. No gluetun container needed."),
        s("vpn_wireguard_config", "WireGuard config", "textarea").full().secret().help("Paste the whole .conf file from your VPN provider (ProtonVPN: Downloads → WireGuard configuration, with NAT-PMP port forwarding on). The private key is stored on this server only."),
        s("vpn_port_forwarding", "Port forwarding", "select").options(&[("natpmp", "NAT-PMP (ProtonVPN)"), ("off", "Off")]),
        s("vpn_kill_switch", "Keep Soulseek off while the VPN is down", "bool"),
    ] },
    Section { id: "ytdlp", title: "YouTube Music & links (yt-dlp)", desc: "Last-resort source: finds the song on YouTube Music. yt-dlp also downloads SoundCloud links you paste in the Downloader, along with YouTube, Bandcamp, Mixcloud and about 1,800 other sites.", test: Some("ytdlp"), test_label: Some("Test cookies"), fields: &[
        s("ytdlp_enabled", "Enable yt-dlp as a source", "bool").help("Searches YouTube Music for requests, and downloads links you paste in the Downloader from YouTube, SoundCloud, Bandcamp, Mixcloud and ~1,800 other sites."),
        s("ytdlp_last_resort", "Always try YouTube Music last, so downloads (almost) never fail", "bool"),
        s("ytdlp_format", "Audio format", "select").options(&[("best", "Original stream – best quality, no re-encode (recommended)"), ("m4a", "M4A (AAC, re-encoded)"), ("mp3", "MP3 (re-encoded)"), ("opus", "Opus")]),
        s("ytdlp_quality", "MP3/M4A quality", "text").help("Bitrate like 320, or 0 for best VBR"),
        s("ytdlp_cookies", "cookies.txt contents (optional)", "textarea").full().secret().help("Netscape cookie file from music.youtube.com. With a YouTube Music Premium account this unlocks 256 kbps audio. Export from a private window so the cookies don’t rotate."),
    ] },
    Section { id: "spotify", title: "Integrations", desc: "API keys that let users connect their Spotify, Tidal, Last.fm, Discogs and YouTube Music accounts, and power recommendations.", test: Some("spotify"), test_label: Some("Test all"), fields: &[
        s("spotify_client_id", "Spotify Client ID", "text").help("developer.spotify.com – add <Public URL>/api/connections/spotify/callback as redirect URI. Development-mode apps allow 5 users and need a Premium owner (2026 rules); without it, public playlists still import via the embed fallback."),
        s("spotify_client_secret", "Spotify Client secret", "password"),
        s("tidal_client_id", "Tidal Client ID", "text").help("Only for looking up Tidal albums/songs by ISRC and for the Tidal connection. Get one at developer.tidal.com → Dashboard → Create new app (any name), redirect URI <Public URL>/api/connections/tidal/callback, then copy the Client ID and Client secret here. Downloads from Tidal use your own account instead: Apps & sources → Qobuz / Tidal / Deezer (streamrip) → Sign in to Tidal."),
        s("tidal_client_secret", "Tidal Client secret", "password"),
        s("lastfm_api_key", "Last.fm API key", "password").help("last.fm/api/account/create (free) – loved tracks, similar artists/songs, genre pages, For you"),
        s("lastfm_shared_secret", "Last.fm shared secret", "password").help("Shown next to the API key. Needed for scrobbling: users sign in with Last.fm on the Connections page."),
        s("discogs_token", "Discogs personal access token", "password").help("discogs.com/settings/developers → “Generate new token” and paste that personal access token here (not an application’s Consumer Key/Secret – no app is needed). Finds vinyl, bootlegs and old releases MusicBrainz is missing."),
        s("ytmusic_client_id", "YouTube Music OAuth Client ID", "text").help("Google Cloud → YouTube Data API v3 → OAuth client of type “TVs and Limited Input devices”. Lets users sign in to YouTube Music with a code (they can also paste browser headers instead)."),
        s("ytmusic_client_secret", "YouTube Music OAuth Client secret", "password"),
    ] },
    Section { id: "media", title: "Media servers", desc: "Tells Navidrome, Jellyfin or Plex to rescan after new music arrives, so it shows up there right away.", test: Some("mediaservers"), test_label: Some("Trigger refresh"), fields: &[
        s("navidrome_url", "Navidrome URL", "text").help("URLs here let users link their own accounts on the Connections page; the admin credentials below are only used to trigger library rescans"), s("navidrome_user", "Navidrome user", "text"), s("navidrome_password", "Navidrome password", "password"),
        s("jellyfin_url", "Jellyfin URL", "text"), s("jellyfin_api_key", "Jellyfin API key", "password"),
        s("plex_url", "Plex URL", "text"), s("plex_token", "Plex token", "password"),
    ] },
    Section { id: "roon", title: "Roon", desc: "Encore as a Roon extension: your Roon zones show up under Your devices with what they play, transport and volume, and “Play in Roon” starts a search result in a zone. Enable it here, then approve Encore once in Roon → Settings → Extensions.", test: Some("roon"), test_label: Some("Test connection"), fields: &[
        s("roon_enabled", "Enable Roon", "bool").help("Roon Core (or Nucleus / ROCK) on the same network. Roon 1.8 or newer."),
        s("roon_host", "Roon Core address", "text").help("Leave empty to find the Core on the network. Running Encore in Docker? Multicast does not cross the bridge network – enter the Core’s IP here (add :port if it is not 9330)."),
    ] },
    Section { id: "cadenza", title: "Server processing (Cadenza)", desc: "Lets permitted users have their streams upsampled on this server by Cadenza (long-filter upsampling, dither and noise shaping, lossless 24-bit FLAC out). Off by default: it costs real processor time per stream. Allow users under People → Users & permissions.", test: Some("cadenza"), test_label: Some("Test"), fields: &[
        s("cadenza_server_enabled", "Enable server processing", "bool"),
        s("cadenza_max_streams", "Streams at once", "number").help("Processed streams the whole server runs at the same time; more are played as the original."),
        s("cadenza_max_rate", "Highest output rate", "select").options(&[("96000", "96 kHz"), ("192000", "192 kHz"), ("384000", "384 kHz"), ("768000", "768 kHz")]),
        s("cadenza_max_taps", "Longest filter", "select").options(&[("16384", "16k taps (light)"), ("65536", "64k taps"), ("262144", "256k taps"), ("1048576", "1M taps (heavy)")]),
        s("cadenza_cache_mb", "Cache size (MB)", "number").help("Each song is rendered once and kept on disk, so seeking and skipping need no new processing and the next song is ready before it plays. The least recently played renders make room. 0 = no cache (every stream is processed live)."),
    ] },
    Section { id: "email", title: "Email", desc: "Lets Encore send password-reset and sign-in emails.", test: Some("email"), test_label: Some("Send test email"), fields: &[
        s("smtp_host", "SMTP server", "text").help("e.g. smtp.fastmail.com – used for password-reset emails"),
        s("smtp_port", "Port", "number").help("587 (STARTTLS) or 465 (SSL)"),
        s("smtp_user", "Username", "text"),
        s("smtp_password", "Password", "password"),
        s("smtp_from", "From address", "text").help("e.g. Encore <music@example.com>"),
        s("smtp_security", "Security", "select").options(&[("starttls", "STARTTLS"), ("ssl", "SSL/TLS"), ("none", "None")]),
    ] },
    Section { id: "notify", title: "Notifications", desc: "Get a message on Discord, ntfy or any webhook when downloads finish or fail.", test: Some("notify"), test_label: Some("Send test"), fields: &[
        s("discord_webhook", "Discord webhook URL", "password").full(),
        s("ntfy_url", "ntfy topic URL", "text").help("e.g. https://ntfy.sh/my-music"),
        s("ntfy_token", "ntfy access token", "password"),
        s("webhook_url", "Generic webhook (JSON POST)", "password").full(),
    ] },
];

// One nav entry per group; `parts` splits a long group into labelled runs of per-app cards.
// `aliases` keep old #/settings?s= links working.
struct Group { id: &'static str, title: &'static str, aliases: &'static [&'static str], parts: &'static [(&'static str, &'static [&'static str])] }
impl Group {
    fn sections(&self) -> impl Iterator<Item = &'static str> + '_ { self.parts.iter().flat_map(|(_, ids)| ids.iter().copied()) }
}
const GROUPS: &[Group] = &[
    Group { id: "general", title: "General", aliases: &[], parts: &[("", &["general"])] },
    Group { id: "apps", title: "Apps & sources", aliases: &["downloads", "tagging", "services"], parts: &[
        ("Download sources", &["pipeline", "streamrip", "torznab", "slskd", "ytdlp"]),
        ("Library & tagging", &["library", "verify"]),
        ("Services", &["spotify", "media", "roon", "cadenza"]),
    ] },
    Group { id: "accounts", title: "Email & notifications", aliases: &[], parts: &[("", &["email", "notify"])] },
    Group { id: "autodownload", title: "Auto-download", aliases: &[], parts: &[] },
    Group { id: "tasks", title: "Scheduled tasks", aliases: &[], parts: &[("", &["sync"])] },
];
fn group_of(id: &str) -> String {
    GROUPS.iter().find(|g| g.id == id || g.aliases.contains(&id) || g.sections().any(|s| s == id)).map(|g| g.id.to_string()).unwrap_or_else(|| id.to_string())
}

const MASK: &str = "••••";
thread_local! {
    /// server can hand a saved secret back to the eye/copy buttons
    static REVEAL: Cell<bool> = const { Cell::new(false) };
}
fn reveal() -> bool { REVEAL.with(|r| r.get()) }
async fn secret_of(key: &str) -> Result<String, ApiError> {
    let enc: String = js_sys::encode_uri_component(key).into();
    Ok(api::get(&format!("/admin/settings/secret/{enc}")).await?.s("value"))
}
fn is_masked(v: &str) -> bool { v.starts_with(MASK) }

fn source_note(f: &Field, v: &Value, meta: &Value) -> String {
    let m = meta.o(f.key);
    let secret = f.kind == "password" || f.secret;
    if m.s("source") == "env" {
        return format!(r##"<small class="src env">🔒 Set by <span class="mono">{}</span> in your compose/env – change it there (environment always wins)</small>"##, esc(&m.s("env")));
    }
    if secret && truthy(v) { return format!(r##"<small class="src">Saved ✓ (hidden) · <a href="#" data-clear="{}">clear</a></small>"##, f.key); }
    if secret {
        let env = if m.truthy("env") { format!(r##" – or set <span class="mono">{}</span> in your compose file"##, esc(&m.s("env"))) } else { String::new() };
        return format!(r##"<small class="src">Not set{env}</small>"##);
    }
    if m.s("source") == "ui" { return String::new(); }
    if m.truthy("env") { format!(r##"<small class="src faint">Default · env var <span class="mono">{}</span></small>"##, esc(&m.s("env"))) } else { String::new() }
}

fn field(f: &Field, v: &Value, opts: Option<&Value>, meta: &Value) -> String {
    let id = format!("f-{}", f.key);
    let locked = meta.o(f.key).s("source") == "env";
    let dis = if locked { "disabled" } else { "" };
    let help = format!("{}{}", f.help.map(|h| format!("<small>{}</small>", esc(h))).unwrap_or_default(), source_note(f, v, meta));
    let cls = if f.full || f.kind == "textarea" { "full" } else { "" };
    let label = esc(f.label);
    let key = f.key;
    if f.kind == "bool" {
        return format!(r##"<div class="field {cls}" style="justify-content:center"><label class="switch"><input type="checkbox" data-k="{key}" {checked} {dis}><i></i>{label}</label>{help}</div>"##,
            checked = if truthy(v) { "checked" } else { "" });
    }
    if f.kind == "select" {
        let mut options: Vec<(String, String)> = f.options.iter().map(|(val, lab)| (val.to_string(), lab.to_string())).collect();
        if let Some(dynamic) = f.dynamic {
            let list = opts.map(|o| o.a(dynamic)).unwrap_or_default();
            options = list.iter().map(|o| {
                let val = if !o.is_nullish("path") { o.s("path") } else { o.s("id") };
                let lab = if !o.is_nullish("path") { o.s("path") } else { o.s("name") };
                (val, lab)
            }).collect();
            let vs = js_string(v);
            if truthy(v) && !options.iter().any(|o| o.0 == vs) { options.insert(0, (vs.clone(), vs)); }
            options.insert(0, (if f.key == "lidarr_root_folder" { String::new() } else { "0".to_string() }, "Lidarr default (first)".to_string()));
        }
        let vs = js_string(v);
        let opts_html: String = options.iter().map(|(val, lab)| format!(r##"<option value="{}" {}>{}</option>"##, esc(val), if *val == vs { "selected" } else { "" }, esc(lab))).collect();
        return format!(r##"<label class="field {cls}"><span>{label}</span><select class="input" data-k="{key}" id="{id}" {dis}>
      {opts_html}</select>{help}</label>"##);
    }
    // also for values set by environment variables (read-only, but viewable); never the WireGuard private key
    let reveal = reveal() && truthy(v) && f.key != "vpn_wireguard_config";
    if f.kind == "textarea" {
        let btns = if reveal {
            format!(r##"<span class="row" style="gap:8px"><button type="button" class="btn sm" data-reveal="{key}">{eye}Show saved</button><button type="button" class="btn sm" data-copy="{key}">{copy}Copy</button></span>"##, eye = EYE, copy = icon("copy"))
        } else { String::new() };
        return format!(r##"<label class="field {cls}"><span>{label}</span><textarea class="input mono" data-k="{key}" {dis} placeholder="{ph}"></textarea>{btns}{help}</label>"##,
            ph = if truthy(v) { "Saved – paste to replace" } else { "" });
    }
    if f.kind == "password" {
        // secrets are only sent to the browser when you press the eye/copy button: empty input = keep the saved value
        let input = format!(r##"<input class="input" type="password" data-k="{key}" value="" {dis} {eye}
      autocomplete="new-password" placeholder="{ph}">"##,
            eye = if reveal { r#"data-eye="1""# } else { "" },
            ph = if locked { "set by environment" } else if truthy(v) { "saved – paste a new value to replace" } else { "not set" });
        let wrapped = if reveal {
            format!(r##"<span class="pw-wrap secret">{input}<button type="button" class="eye-btn" data-reveal="{key}" title="Show" aria-label="Show">{eye}</button><button type="button" class="eye-btn" data-copy="{key}" title="Copy" aria-label="Copy">{copy}</button></span>"##, eye = EYE, copy = icon("copy"))
        } else { input };
        return format!(r##"<label class="field {cls}"><span>{label}</span>{wrapped}{help}</label>"##);
    }
    let kind = if f.kind == "number" { "number" } else { "text" };
    format!(r##"<label class="field {cls}"><span>{label}</span><input class="input" type="{kind}" data-k="{key}" {step}
    value="{val}" autocomplete="off" {dis}>{help}</label>"##,
        step = f.step.map(|st| format!(r#"step="{st}""#)).unwrap_or_default(), val = escv(v))
}

const EVERY: &[(i64, &str)] = &[(15, "15 minutes"), (30, "30 minutes"), (60, "hour"), (180, "3 hours"), (360, "6 hours"), (720, "12 hours"), (1440, "day"), (10080, "week")];
/// `Math.round(x)` (halves round towards +∞).
fn js_round(x: f64) -> i64 { (x + 0.5).floor() as i64 }
/// `${n}` of a JS number (15 → "15", 2.5 → "2.5").
fn js_num(n: f64) -> String { js_string(&Value::from(n)) }
/// JS unary `+value` on an input's string, as JSON: a number, or null for NaN.
fn js_plus(s: &str) -> Value {
    let t = s.trim();
    if t.is_empty() { return Value::from(0); }
    match t.parse::<f64>() {
        Ok(f) if f.is_finite() => if f.fract() == 0.0 && f.abs() < 9.0e15 { Value::from(f as i64) } else { Value::from(f) },
        _ => Value::Null,
    }
}
fn ago(t: f64) -> String {
    if t == 0.0 { return "never".into(); }
    let s = dom::now_s() - t;
    if s < 90.0 { "just now".into() } else if s < 5400.0 { format!("{} min ago", js_round(s / 60.0)) } else if s < 129600.0 { format!("{} h ago", js_round(s / 3600.0)) } else { format!("{} days ago", js_round(s / 86400.0)) }
}
fn until(t: f64) -> String {
    if t == 0.0 { return String::new(); }
    let s = t - dom::now_s();
    if s <= 60.0 { "any moment".into() } else if s < 5400.0 { format!("in {} min", js_round(s / 60.0)) } else if s < 129600.0 { format!("in {} h", js_round(s / 3600.0)) } else { format!("in {} days", js_round(s / 86400.0)) }
}

fn task_row(t: &Value) -> String {
    let locked = t.truthy("locked");
    let enabled = t.truthy("enabled");
    let running = t.truthy("running");
    let minutes = t.f("minutes");
    // [...new Set([...EVERY.map(e => e[0]), t.minutes])].sort((a, b) => a - b)
    let mut ms: Vec<f64> = EVERY.iter().map(|e| e.0 as f64).collect();
    if !ms.contains(&minutes) { ms.push(minutes); }
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let every: String = ms.iter().map(|m| {
        let lab = EVERY.iter().find(|e| e.0 as f64 == *m).map(|e| e.1.to_string()).unwrap_or_else(|| format!("{} minutes", js_num(*m)));
        format!(r##"<option value="{}" {}>{}</option>"##, js_num(*m), if *m == minutes { "selected" } else { "" }, lab)
    }).collect();
    let took = if t.truthy("duration") {
        let d = t.f("duration");
        format!(" · took {}", if d < 60.0 { format!("{} s", js_round(d)) } else { format!("{} min", js_round(d / 60.0)) })
    } else { String::new() };
    let run_note = if running { r##" · <b style="color:var(--accent)">running…</b>"## } else { "" };
    let result = if t.truthy("error") { format!(r##" · <span style="color:var(--err)">{}</span>"##, esc(&t.s("error"))) }
        else if t.truthy("result") && !running { format!(" · {}", esc(&t.s("result"))) } else { String::new() };
    let next = if enabled && t.truthy("next_run") && !running { format!(" · next {}", until(t.f("next_run"))) } else { String::new() };
    format!(r##"<div class="list-item" style="align-items:flex-start;flex-wrap:wrap;gap:12px" data-k="{key}">
      <label class="switch" style="margin-top:2px" title="{ltitle}"><input type="checkbox" data-on {checked} {ldis}><i></i></label>
      <div class="grow" style="min-width:220px"><div style="font-weight:650">{name}{lock}</div>
        <div class="small muted">{desc}</div>
        <div class="small faint" style="margin-top:4px">Last run {ago}{took}{run_note}
          {result}{next}</div></div>
      <div class="row" style="gap:8px"><span class="small muted">every</span>
        <select class="input" data-every style="width:auto" {ldis}>{every}</select>
        <button class="btn sm" data-run {rdis}>{play}Run now</button></div>
    </div>"##,
        key = t.s("key"),
        ltitle = if locked { "Set by an environment variable" } else { "" },
        checked = if enabled { "checked" } else { "" },
        ldis = if locked { "disabled" } else { "" },
        name = esc(&t.s("name")),
        lock = if locked { r##" <span title="Set by an environment variable">🔒</span>"## } else { "" },
        desc = esc(&t.s("desc")),
        ago = ago(t.f("last_run")),
        rdis = if running { "disabled" } else { "" },
        play = icon("play"))
}

/// Settings → Scheduled tasks: every background job, with on/off, interval, last result and Run now.
async fn render_tasks(panel: Element) {
    let d = match api::get("/admin/tasks").await { Ok(d) => d, Err(e) => { dom::console_error(&e.message); return; } };
    let rows: String = d.a("tasks").iter().map(task_row).collect();
    dom::set_html(&panel, &format!(r##"<div class="card"><div class="row between" style="margin-bottom:8px"><h2>Scheduled tasks</h2><button class="btn sm" id="t-refresh">{refresh}Refresh</button></div>
    <p class="muted small" style="margin:0 0 14px">Background jobs Encore runs on its own. Switch them off, change how often they run, or run one now.</p>
    <div class="list">{rows}</div>
    <div class="row mt" style="gap:8px"><span class="small muted">Clean up finished downloads older than</span>
      <input class="input" type="number" min="1" id="t-days" value="{days}" style="width:90px"><span class="small muted">days</span></div></div>"##,
        refresh = icon("refresh"), days = js_string(d.g("cleanup_days"))));
    if let Some(b) = dom::qs_in(&panel, "#t-refresh") {
        let p = panel.clone();
        dom::on(&b, "click", move |_| dom::spawn(render_tasks(p.clone())));
    }
    if let Some(i) = dom::qs_in(&panel, "#t-days") {
        let i2 = i.clone();
        dom::on(&i, "change", move |_| {
            let days = js_plus(&dom::value(&i2));
            dom::spawn(async move {
                match api::put("/admin/settings", json!({ "task_cleanup_days": days })).await {
                    Ok(_) => toast("Saved", "ok"),
                    Err(e) => dom::console_error(&e.message),
                }
            });
        });
    }
    for row in dom::qsa_in(&panel, "[data-k]") {
        let k = dom::data(&row, "k");
        if let Some(on) = dom::qs_in(&row, "[data-on]") {
            let (on2, k2, p) = (on.clone(), k.clone(), panel.clone());
            dom::on(&on, "change", move |_| {
                let checked = dom::checked(&on2);
                let (k, p) = (k2.clone(), p.clone());
                dom::spawn(async move {
                    match api::patch(&format!("/admin/tasks/{k}"), json!({ "enabled": checked })).await {
                        Ok(_) => { toast(if checked { "Enabled" } else { "Disabled" }, "ok"); dom::spawn(render_tasks(p)); }
                        Err(err) => toast(&err.message, "err"),
                    }
                });
            });
        }
        if let Some(sel) = dom::qs_in(&row, "[data-every]") {
            let (sel2, k2, p) = (sel.clone(), k.clone(), panel.clone());
            dom::on(&sel, "change", move |_| {
                let minutes = js_plus(&dom::value(&sel2));
                let (k, p) = (k2.clone(), p.clone());
                dom::spawn(async move {
                    match api::patch(&format!("/admin/tasks/{k}"), json!({ "minutes": minutes })).await {
                        Ok(_) => { toast("Saved", "ok"); dom::spawn(render_tasks(p)); }
                        Err(err) => toast(&err.message, "err"),
                    }
                });
            });
        }
        if let Some(run) = dom::qs_in(&row, "[data-run]") {
            let (run2, k2, p) = (run.clone(), k.clone(), panel.clone());
            dom::on(&run, "click", move |_| {
                dom::set_disabled(&run2, true);
                let (k, p) = (k2.clone(), p.clone());
                dom::spawn(async move {
                    match api::post_empty(&format!("/admin/tasks/{k}/run")).await {
                        Ok(_) => { toast("Started", "ok"); dom::timeout(1500, move || dom::spawn(render_tasks(p))); }
                        Err(err) => toast(&err.message, "err"),
                    }
                });
            });
        }
    }
}

// Pages that are whole screens of their own, shown inside Settings
fn embedded(id: &str) -> Option<app::PageFn> {
    Some(match id {
        "account" => account::render,
        "appearance" => render_appearance,
        "playback" => render_playback,
        "connections" => connections::render,
        "users" => admin::render_users,
        "invites" => admin::render_invites,
        "autodownload" => autodownload::render,
        "diagnostics" => diagnostics::render,
        _ => return None,
    })
}

/// `history.replaceState(null, '', url)`.
fn replace_state(url: &str) {
    if let Ok(h) = dom::window().history() { let _ = h.replace_state_with_url(&JsValue::NULL, "", Some(url)); }
}
fn input_type(el: &Element) -> String { el.dyn_ref::<HtmlInputElement>().map(|i| i.type_()).unwrap_or_default() }
fn set_input_type(el: &Element, t: &str) { if let Some(i) = el.dyn_ref::<HtmlInputElement>() { i.set_type(t); } }

/// `<div class="test-result …">` for a /system/test result (the `paint` of the JS).
fn test_result(r: &Value) -> String {
    let ok = r.truthy("ok");
    let msg = { let m = r.g("message"); if truthy(m) { js_string(m) } else if ok { "Connected".to_string() } else { "Failed".to_string() } };
    format!(r##"<div class="test-result {}">{}<span>{}</span></div>"##, if ok { "ok" } else { "err" }, if ok { icon("check") } else { icon("x") }, esc(&msg))
}

/// The state the JS `render` closed over: `values`, `lidarrOpts`, the panel and the ctx.
struct Hub {
    view: Element,
    panel: Element,
    ctx: Ctx,
    values: RefCell<Value>,
    lidarr_opts: RefCell<Option<Value>>,
    focus: RefCell<Option<String>>,   // #/settings?s=<section> opens that card once
}

impl Hub {
    async fn save(&self, sec: &'static Section, quiet: bool, root: &Element) -> Result<(), ApiError> {
        let mut body = Map::new();
        let meta = self.values.borrow().o("_meta");
        for el in dom::qsa_in(root, "[data-k]") {
            let k = dom::data(&el, "k");
            let Some(f) = sec.fields.iter().find(|x| x.key == k) else { continue };
            if meta.o(f.key).s("source") == "env" { continue; }            // locked by environment
            let secret = f.kind == "password" || f.secret;
            if el.get_attribute("type").as_deref() == Some("checkbox") { body.insert(f.key.to_string(), Value::Bool(dom::checked(&el))); }
            else {
                let v = dom::value(&el);
                if secret && v.trim().is_empty() { /* keep saved secret */ }
                else if is_masked(&v) { /* never echo a mask back */ }
                else { body.insert(f.key.to_string(), Value::String(if secret { v.trim().to_string() } else { v })); }
            }
        }
        if sec.id == "cadenza" && meta.o("cadenza_engine_settings").s("source") != "env" {
            if let Some(engine) = engine_settings_value(root) { body.insert("cadenza_engine_settings".to_string(), Value::String(engine)); }
        }
        let engine_saved = body.get("cadenza_engine_settings").and_then(|v| v.as_str()).map(String::from);
        let app_name = body.get("app_name").cloned().unwrap_or(Value::Null);
        match api::put("/admin/settings", Value::Object(body)).await {
            Ok(v) => {
                *self.values.borrow_mut() = v;
                if let Some(e) = engine_saved.filter(|e| !e.is_empty()) {
                    ENGINE.with(|en| if let Some(en) = en.borrow_mut().as_mut() { en.values = serde_json::from_str(&e).unwrap_or_else(|_| json!({})); });
                }
                if !quiet { toast("Settings saved", "ok"); }
                if sec.id == "general" && truthy(&app_name) { dom::set_title(&js_string(&app_name)); }
                Ok(())
            }
            Err(e) => {
                toast(&e.message, "err");
                Err(e)   // `if (quiet) throw e` – the non-quiet callers ignore it
            }
        }
    }

    // Encore's own API key, at the bottom of General (only the owner may see it)
    async fn api_card(&self) -> String {
        let key = match api::get("/system/api-key").await { Ok(r) => r.s("api_key"), Err(_) => return String::new() };
        let panel = self.panel.clone();
        dom::timeout(0, move || {
            let Some(card) = dom::qs_in(&panel, "#api-card") else { return };
            if let Some(cp) = dom::qs_in(&card, "#cp") {
                let card2 = card.clone();
                dom::on(&cp, "click", move |_| { if let Some(k) = dom::qs_in(&card2, "#key") { copy_text(&dom::data(&k, "value")); } });
            }
            if let Some(regen) = dom::qs_in(&card, "#regen") {
                let card2 = card.clone();
                dom::on(&regen, "click", move |_| {
                    if !dom::confirm("Make a new API key? Scripts using the old one stop working.") { return; }
                    let card = card2.clone();
                    dom::spawn(async move {
                        match api::post_empty("/admin/settings/api-key").await {
                            Ok(r) => {
                                if let Some(k) = dom::qs_in(&card, "#key") { set_secret_text(&k, &r.s("api_key")); }
                                toast("New API key generated", "ok");
                            }
                            Err(e) => dom::console_error(&e.message),
                        }
                    });
                });
            }
        });
        format!(r##"<div class="card stack mt" id="api-card">
      <h2>Encore API key</h2>
      <p class="muted small" style="margin:0">Send it in an <span class="kbd">X-Api-Key</span> header to automate Encore (scripts, Home Assistant…). It acts as the owner account – keep it private.</p>
      <div class="row wrap"><div class="code grow" id="key" data-secret data-value="{}"></div><button class="btn" id="cp" title="Copy">{}</button><button class="btn danger" id="regen">{}Regenerate</button></div>
    </div>"##, esc(&key), icon("copy"), icon("refresh"))
    }
}

/// The JS `show(id, flash)`: paints one settings page into the panel.
fn show(hub: &Rc<Hub>, id: String, flash: Option<Value>) -> Pin<Box<dyn Future<Output = ()>>> {
    let hub = hub.clone();
    Box::pin(async move {
        for b in dom::qsa_in(&hub.view, ".settings-nav button") { dom::toggle_class(&b, "active", dom::data(&b, "s") == id); }
        if let Some(sel) = dom::qs_in(&hub.view, ".settings-select") { dom::set_value(&sel, &id); }
        replace_state(&format!("#/settings?s={id}"));
        if let Some(page) = embedded(&id) {
            dom::set_html(&hub.panel, r##"<div class="skeleton" style="height:200px"></div>"##);
            if let Err(e) = page(hub.panel.clone(), hub.ctx.clone()).await { dom::console_error(&e); }
            return;
        }
        if id == "about" || id == "api" || id == "keys" { return show(&hub, "general".to_string(), None).await; }
        let group = GROUPS.iter().find(|g| g.id == id).unwrap_or(&GROUPS[0]);
        // only what this server has: Encore has Prowlarr instead of Lidarr
        let secs: Vec<(&'static Section, Vec<Field>)> = {
            let values = hub.values.borrow();
            group.sections()
                .filter_map(|sid| SECTIONS.iter().find(|x| x.id == sid))
                .map(|sec| (sec, sec.fields.iter().copied().filter(|f| values.has(f.key)).collect::<Vec<_>>()))
                .filter(|(_, fields)| !fields.is_empty())
                .collect()
        };
        if secs.iter().any(|(x, _)| x.id == "lidarr") && hub.values.borrow().truthy("lidarr_api_key") && hub.lidarr_opts.borrow().is_none() {
            let r = api::get("/system/lidarr/options").await;
            *hub.lidarr_opts.borrow_mut() = r.ok();
        }
        let values = hub.values.borrow().clone();
        let meta = values.o("_meta");
        let lidarr_opts = hub.lidarr_opts.borrow().clone();
        let n = secs.len();
        let focus = hub.focus.borrow_mut().take();
        // the first card starts open (unless a link asked for a specific one); every app is its own drop-down
        let mut first = true;
        let card = |sec: &'static Section, fields: &Vec<Field>, open: bool| -> String {
            let flash_here = flash.as_ref().map(|f| f.s("sec") == sec.id).unwrap_or(false);
            let desc = if sec.desc.is_empty() { String::new() } else { format!(r##"<div class="small muted" style="margin-top:4px;font-weight:400">{}</div>"##, esc(sec.desc)) };
            let test = match sec.test {
                Some(_) => format!(r##"<button class="btn sm" data-test>{}{}</button>"##, icon("refresh"), esc(sec.test_label.unwrap_or("Test connection"))),
                None => String::new(),
            };
            let chev = { let c = icon("chevron"); if c.is_empty() { "▾".to_string() } else { c } };
            let flash_html = match &flash {
                Some(f) if flash_here => {
                    let ok = f.truthy("ok");
                    format!(r##"<div class="test-result {}">{}<span>{}</span></div>"##, if ok { "ok" } else { "err" }, if ok { icon("check") } else { icon("x") }, esc(&f.s("message")))
                }
                _ => String::new(),
            };
            let fields_html = fields_html(fields, |f| field(f, values.g(f.key), lidarr_opts.as_ref(), &meta));
            format!(r##"<details class="card settings-card" data-sec="{sid}" {open}>
      <summary class="row between" style="gap:10px;flex-wrap:nowrap;align-items:flex-start"><div style="min-width:0"><h2>{title}</h2>{desc}</div>
        <span class="row" style="gap:8px">{test}<span class="chev">{chev}</span></span></summary>
      <div style="height:14px"></div>
      <div data-out>{flash_html}</div>
      {fields_html}{tidal}{cadenza}
      {slsk}
      <div class="row mt" style="justify-content:flex-end"><button class="btn primary" data-save>{check}Save</button></div>
    </details>"##,
                tidal = if sec.id == "streamrip" { tidal_block() } else { String::new() },
                cadenza = if sec.id == "cadenza" { cadenza_cache_block() + &engine_settings_block() } else { String::new() },
                slsk = if sec.id == "slskd" { soulseek_status_block() } else { String::new() },
                sid = sec.id, open = if open { "open" } else { "" }, title = esc(sec.title), check = icon("check"))
        };
        let mut html = if group.id == "tasks" { r##"<div id="tasks-host" style="margin-bottom:16px"></div>"##.to_string() } else { String::new() };
        for (label, ids) in group.parts {
            let list: Vec<&(&'static Section, Vec<Field>)> = secs.iter().filter(|(sec, _)| ids.contains(&sec.id)).collect();
            if list.is_empty() { continue; }
            if !label.is_empty() { html.push_str(&format!(r##"<div class="nav-label" style="padding:18px 4px 8px">{}</div>"##, esc(label))); }
            if *label == "Download sources" { html.push_str(r##"<div id="tools-host"></div>"##); }
            for (sec, fields) in list {
                let flash_here = flash.as_ref().map(|f| f.s("sec") == sec.id).unwrap_or(false);
                let open = (first && focus.is_none()) || n == 1 || flash_here || focus.as_deref() == Some(sec.id);
                first = false;
                html.push_str(&card(sec, fields, open));
            }
        }
        if group.id == "apps" { html.push_str(&pipeline_help()); }
        if group.id == "general" { html.push_str(&hub.api_card().await); html.push_str(&about_card().await); }
        dom::set_html(&hub.panel, &html);
        if group.id == "tasks" { if let Some(host) = dom::qs_in(&hub.panel, "#tasks-host") { dom::spawn(render_tasks(host)); } }
        if let Some(host) = dom::qs_in(&hub.panel, "#tools-host") { dom::spawn(render_tools(host, hub.clone())); }
        if let Some(f) = focus { if let Some(c) = dom::qs_in(&hub.panel, &format!(r#"[data-sec="{f}"]"#)) { c.scroll_into_view(); } }

        for card in dom::qsa_in(&hub.panel, "[data-sec]") {
            let sec_id = dom::data(&card, "sec");
            let Some(sec) = SECTIONS.iter().find(|x| x.id == sec_id) else { continue };
            let Some(out) = dom::qs_in(&card, "[data-out]") else { continue };
            if sec.id == "streamrip" { bind_tidal(&hub.view, &card); }
            if sec.id == "cadenza" { bind_cadenza_cache(&hub, &card); }
            if sec.id == "slskd" { bind_vpn(&hub, sec, &card); bind_soulseek(&hub, &card); }
            let paint: Rc<dyn Fn(&Value)> = { let out = out.clone(); Rc::new(move |r: &Value| dom::set_html(&out, &test_result(r))) };
            if let Some(b) = dom::qs_in(&card, "[data-save]") {
                let (hub2, card2) = (hub.clone(), card.clone());
                dom::on(&b, "click", move |_| {
                    let (hub, card) = (hub2.clone(), card2.clone());
                    dom::spawn(async move { let _ = hub.save(sec, false, &card).await; });
                });
            }
            {
                let (hub2, id2) = (hub.clone(), id.clone());
                dom::on_each(&card, "[data-clear]", "click", move |a, e| {
                    dom::prevent(e);
                    let key = dom::data(a, "clear");
                    let (hub, id) = (hub2.clone(), id2.clone());
                    dom::spawn(async move {
                        let mut body = Map::new();
                        body.insert(key, Value::String(String::new()));
                        match api::put("/admin/settings", Value::Object(body)).await {
                            Ok(v) => {
                                *hub.values.borrow_mut() = v;
                                toast("Cleared", "ok");
                                show(&hub, id, None).await;
                            }
                            Err(e) => dom::console_error(&e.message),
                        }
                    });
                });
            }
            {
                let card2 = card.clone();
                dom::on_each(&card, "[data-reveal]", "click", move |b, e| {
                    dom::prevent(e);
                    let key = dom::data(b, "reveal");
                    let Some(input) = dom::qs_in(&card2, &format!(r#"[data-k="{key}"]"#)) else { return };
                    let is_textarea = input.tag_name() == "TEXTAREA";
                    let hidden = if is_textarea { dom::data(&input, "shown").is_empty() } else { input_type(&input) == "password" };
                    let b = b.clone();
                    dom::spawn(async move {
                        if hidden {
                            if dom::value(&input).is_empty() {
                                match secret_of(&key).await {
                                    Ok(v) => { dom::set_value(&input, &v); dom::set_data(&input, "revealed", "1"); }
                                    Err(err) => { toast(&err.message, "err"); return; }
                                }
                            }
                            if !is_textarea { set_input_type(&input, "text"); }
                            dom::set_data(&input, "shown", "1");
                        } else {
                            if !dom::data(&input, "revealed").is_empty() { dom::set_value(&input, ""); let _ = input.remove_attribute("data-revealed"); }
                            if !is_textarea { set_input_type(&input, "password"); }
                            let _ = input.remove_attribute("data-shown");
                        }
                        if dom::has_class(&b, "eye-btn") {
                            dom::set_html(&b, if hidden { EYE_OFF } else { EYE });
                            let _ = b.set_attribute("title", if hidden { "Hide" } else { "Show" });
                        } else {
                            dom::set_html(&b, &if hidden { format!("{EYE_OFF}Hide") } else { format!("{EYE}Show saved") });
                        }
                    });
                });
            }
            {
                let card2 = card.clone();
                dom::on_each(&card, "[data-copy]", "click", move |b, e| {
                    dom::prevent(e);
                    let key = dom::data(b, "copy");
                    let Some(input) = dom::qs_in(&card2, &format!(r#"[data-k="{key}"]"#)) else { return };
                    dom::spawn(async move {
                        let v = dom::value(&input);
                        let text = if !v.is_empty() { v } else {
                            match secret_of(&key).await { Ok(s) => s, Err(err) => { toast(&err.message, "err"); return; } }
                        };
                        copy_text(&text);
                    });
                });
            }
            if let Some(t) = dom::qs_in(&card, "[data-test]") {
                let (hub2, card2, out2, t2, id2, paint2) = (hub.clone(), card.clone(), out.clone(), t.clone(), id.clone(), paint.clone());
                dom::on(&t, "click", move |ev| {
                    dom::prevent(ev);
                    let _ = card2.set_attribute("open", "");
                    dom::set_disabled(&t2, true);
                    dom::set_html(&out2, r##"<div class="test-result"><span class="spinner"></span><span>Testing…</span></div>"##);
                    let (hub, card, t, id, paint) = (hub2.clone(), card2.clone(), t2.clone(), id2.clone(), paint2.clone());
                    dom::spawn(async move {
                        let failed = |e: ApiError| json!({ "ok": false, "message": if e.message.is_empty() { "Test failed".to_string() } else { e.message } });
                        let r = match hub.save(sec, true, &card).await {
                            Ok(()) => match api::post_empty(&format!("/system/test/{}", sec.test.unwrap_or(""))).await { Ok(r) => r, Err(e) => failed(e) },
                            Err(e) => failed(e),
                        };
                        if r.truthy("options") {
                            *hub.lidarr_opts.borrow_mut() = Some(r.g("options").clone());
                            let mut flash = r.clone();
                            if let Some(o) = flash.as_object_mut() { o.insert("sec".to_string(), Value::String(sec.id.to_string())); }
                            show(&hub, id, Some(flash)).await;
                        } else { paint(&r); }
                        dom::set_disabled(&t, false);
                    });
                });
            }
        }
    })
}

pub fn render(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let admin = matches!(ctx.user.s("role").as_str(), "admin" | "owner");
        let values = if admin { api::get("/admin/settings").await.map_err(|e| e.message)? } else { json!({}) };
        REVEAL.with(|r| r.set(app::has_feature("reveal")));
        let s = ctx.q("s");
        let mut current = if s.is_empty() { "account".to_string() } else { group_of(&s) };
        let mut nav: Vec<(&str, Vec<(&str, &str)>)> = vec![
            ("You", vec![("account", "Account"), ("appearance", "Appearance"), ("playback", "Playback"), ("connections", "Connections")]),
        ];
        if admin {
            nav.push(("People", vec![("users", "Users & permissions"), ("invites", "Invites")]));
            nav.push(("Server", GROUPS.iter().map(|g| (g.id, g.title)).collect()));
            nav.push(("System", vec![("diagnostics", "Diagnostics")]));
        }
        let pages: Vec<&str> = nav.iter().flat_map(|(_, items)| items.iter().map(|(id, _)| *id)).collect();
        if !pages.iter().any(|p| *p == current) { current = "account".to_string(); }
        let select_html: String = nav.iter().map(|(label, items)| format!(r##"<optgroup label="{}">{}</optgroup>"##,
            esc(if label.is_empty() { "Encore" } else { label }),
            items.iter().map(|(id, t)| format!(r##"<option value="{id}">{}</option>"##, esc(t))).collect::<String>())).collect();
        let nav_html: String = nav.iter().map(|(label, items)| format!("{}{}",
            if label.is_empty() { r##"<div style="height:10px"></div>"##.to_string() } else { format!(r##"<div class="nav-label">{}</div>"##, esc(label)) },
            items.iter().map(|(id, t)| format!(r##"<button data-s="{id}">{}</button>"##, esc(t))).collect::<String>())).collect();
        dom::set_html(&view, &format!(r##"
    <h1 style="margin-bottom:18px">Settings</h1>
    <select class="input settings-select" aria-label="Settings page">{select_html}</select>
    <div class="settings-grid">
      <nav class="settings-nav">{nav_html}</nav>
      <div id="panel"></div>
    </div>"##));
        let panel = dom::qs_in(&view, "#panel").ok_or_else(|| "Settings: no panel".to_string())?;
        let focus = if !s.is_empty() && current != s { Some(s.clone()) } else { None };
        let hub = Rc::new(Hub { view: view.clone(), panel, ctx: ctx.clone(), values: RefCell::new(values), lidarr_opts: RefCell::new(None), focus: RefCell::new(focus) });

        if let Some(nav_el) = dom::qs_in(&view, ".settings-nav") {
            let hub2 = hub.clone();
            dom::on(&nav_el, "click", move |e| {
                if let Some(b) = dom::closest(e, "[data-s]") { dom::spawn(show(&hub2, dom::data(&b, "s"), None)); }
            });
        }
        if let Some(sel) = dom::qs_in(&view, ".settings-select") {
            let (hub2, sel2) = (hub.clone(), sel.clone());
            dom::on(&sel, "change", move |_| dom::spawn(show(&hub2, dom::value(&sel2), None)));
        }
        dom::spawn(show(&hub, current, None));
        Ok(())
    })
}

/// Version, build and app downloads – shown at the bottom of General (and of Account for members).
pub async fn about_card() -> String {
    let info = app::info();
    let server = "Encore";
    let version = { let v = info.g("version"); if truthy(v) { js_string(v) } else { "?".to_string() } };
    let build = { let v = info.g("build"); if truthy(v) { js_string(v) } else { "dev".to_string() } };
    format!(r##"<div class="card stack mt"><h2>About Encore</h2>
    <div class="row wrap" style="gap:24px"><div><div class="small muted">Version</div><div style="font-weight:700;font-size:20px">v{}</div></div>
      <div><div class="small muted">Server</div><div>{}</div></div>
      <div><div class="small muted">Build</div><div class="mono">{}</div></div></div>
    <div class="small muted">Apps for Android, Windows, macOS and Linux: <a href="https://github.com/Krifmaz/Encore/releases" target="_blank" rel="noopener">github.com/Krifmaz/Encore/releases</a></div>
    <div class="small muted">Interface: Rust (WebAssembly, preview) · <a href="/{}">Back to the standard interface</a></div></div>"##,
        esc(&version), esc(server), esc(&build), esc(&{ let h = dom::hash(); if h.is_empty() { "#/".to_string() } else { h } }))
}

/// Settings → Playback: stream quality, buffering, what happens when the queue ends, server-side Cadenza processing of streams
/// (where an admin switched it on and allowed the account; greyed out otherwise) and the switches of a linked Cadenza device.
fn render_playback(el: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let server_prefs = app::has_feature("theme");
        let p = crate::playback::current();
        // server-side Cadenza: on for this server, and allowed for this account? (fetched once per visit to the tab)
        let dsp = api::get("/playback/dsp").await.unwrap_or_else(|_| json!({ "enabled": false }));
        // the desktop app carries Cadenza: its { running, device_id, name, error } – asked again every couple of seconds
        let native = app::native_truthy("engine");
        let engine_st = if native { engine_status().await } else { Value::Null };
        let st = Rc::new(Look { el, p: RefCell::new(p), server_prefs, dsp, native, engine_st: RefCell::new(engine_st), painted: RefCell::new(String::new()) });
        // what the Cadenza engine section is painted from – a repaint only when it changes
        let st2 = st.clone();
        dom::interval(&st.el, 2000, move || {
            if !st2.el.is_connected() { return; }
            let st3 = st2.clone();
            dom::spawn(async move {
                if st3.native { *st3.engine_st.borrow_mut() = engine_status().await; }
                if st3.el.is_connected() && engine_key(&st3) != *st3.painted.borrow() { paint_playback(&st3); }
            });
        });
        paint_playback(&st);
        Ok(())
    })
}

const QUALITY_HELP: [(&str, &str); 3] = [
    ("auto", "The original file. Encore measures your connection while it plays and steps the stream down as far as it needs to keep the music playing – down to 32 kbps – then back up as soon as the connection allows."),
    ("original", "Always the original file, even if it stutters on a slow connection. Formats a browser can't play are converted to FLAC (lossless)."),
    ("saver", "Always a small 96 kbps stream (AAC, or Opus in browsers without AAC) – for mobile data and slow links."),
];

/// `seg()` of the JS renderPlayback: compares as strings, so the numeric kbps matches its button.
fn seg_str(p: &Value, key: &str, opts: &[(&str, &str)]) -> String {
    let cur = js_string(p.g(key));
    format!(r##"<div class="seg" data-key="{key}">{}</div>"##,
        opts.iter().map(|(v, l)| format!(r##"<button type="button" data-v="{v}" class="{}">{l}</button>"##, if cur == *v { "on" } else { "" })).collect::<String>())
}

/// `await window.EncoreNative.engineStatus()` – null when it fails.
async fn engine_status() -> Value { app::native_call_async("engineStatus", None).await.unwrap_or(Value::Null) }

/// `JSON.stringify([link?.id, link?.name, link?.state?.engine ?? null, engineSt])`.
fn engine_key(st: &Look) -> String {
    let link = crate::player::linked();
    let eng = link.as_ref().map(|l| l.state.g("engine").clone()).unwrap_or(Value::Null);
    json!([link.as_ref().map(|l| l.id.clone()), link.as_ref().map(|l| l.name.clone()), eng, *st.engine_st.borrow()]).to_string()
}

// Settings → Playback → Cadenza engine, when there is no engine to show (the same words in web/js/pages/settings.js)
const ENGINE_GREYED: &str = r#"<div class="stack" style="gap:8px;opacity:.45;pointer-events:none" aria-disabled="true"><label class="row" style="gap:8px;align-items:center"><input type="checkbox" disabled> Bit-perfect – send the file to the DAC untouched (no upsampling, dither or volume)</label><label class="stack" style="gap:4px"><span class="small muted">Upsampling filter</span><select class="input" disabled><option>–</option></select></label><label class="stack" style="gap:4px"><span class="small muted">Dither / noise shaping</span><select class="input" disabled><option>–</option></select></label><label class="stack" style="gap:4px"><span class="small muted">Output rate</span><select class="input" disabled><option>–</option></select></label></div><span class="small muted">Cadenza is Encore's own playback engine: long upsampling filters, DSD, dither and noise shaping, exclusive output straight into the DAC. It is built into the Encore desktop app – install the app on the computer connected to your DAC and this section comes alive on its own. Cadenza can also run on its own on a small box next to the DAC; link to it under Your devices. To have the server do the upsampling for a browser or phone instead, see Server processing above.</span>"#;
const ENGINE_OFF_COPY: &str = r#"<span class="small muted">Cadenza is off for this computer – music plays through the browser audio of this window. Link to This computer (Cadenza) under Your devices to turn it back on.</span>"#;

/// `${(Number(v) || 0).toFixed(1)} dB`.
fn db_text(n: f64) -> String { let n = if n.is_finite() && n != 0.0 { n } else { 0.0 }; format!("{n:.1} dB") }
/// `Number(v) || 0` printed like JS.
fn js_num_of(v: &Value) -> String { let n = v.as_f64().unwrap_or(0.0); js_string(&json!(n)) }
/// A numeric string as a JSON number (whole numbers as integers, like `JSON.stringify(Number(s))`), else None.
fn eng_num(t: &str) -> Option<Value> {
    let body = t.strip_prefix('-').unwrap_or(t);
    let (int, frac) = body.split_once('.').map(|(a, b)| (a, Some(b))).unwrap_or((body, None));
    let digits = |x: &str| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit());
    if !digits(int) || frac.is_some_and(|f| !digits(f)) { return None; }
    let n: f64 = t.parse().ok()?;
    Some(if n.fract() == 0.0 { json!(n as i64) } else { json!(n) })
}

/// A linked Cadenza describes every switch in its state (see Cadenza's encore/state.rs): outputs, exclusive, bit-perfect, PCM and DSD groups, headroom, soft clipper, GPU, matrix.
fn engine_html(e: &Value) -> String {
    let pair = |o: &Value| -> (Value, Value) { (o.get(0).cloned().unwrap_or(Value::Null), o.get(1).cloned().unwrap_or(Value::Null)) };
    let sel = |key: &str, label: &str, list: &Value, cur: &Value, extra: &str| -> String {
        let Some(items) = list.as_array() else { return String::new() };
        let cur = js_string(cur);
        let opts: String = items.iter().map(|o| {
            let (v, l) = pair(o);
            format!(r#"<option value="{}" {}>{}</option>"#, esc(&js_string(&v)), if js_string(&v) == cur { "selected" } else { "" }, esc(&js_string(&l)))
        }).collect();
        format!(r#"<label class="stack" style="gap:4px"><span class="small muted">{label}</span><select class="input" data-eng="{key}" {extra}>{opts}</select></label>"#)
    };
    let seg_e = |key: &str, list: &[Value], cur: &Value| -> String {
        let cur = js_string(cur);
        format!(r#"<div class="seg" data-eng-seg="{key}">{}</div>"#, list.iter().map(|o| {
            let (v, l) = pair(o);
            format!(r#"<button type="button" data-v="{}" class="{}">{}</button>"#, esc(&js_string(&v)), if js_string(&v) == cur { "on" } else { "" }, esc(&js_string(&l)))
        }).collect::<String>())
    };
    let range = |key: &str, label: &str, id: &str, v: &Value, min: &str| -> String {
        format!(r#"<label class="stack" style="gap:4px"><span class="small muted">{label}: <span id="{id}">{}</span></span><input type="range" min="{min}" max="0" step="0.5" value="{}" data-eng="{key}" data-v="{id}" style="accent-color:var(--accent)"></label>"#,
            db_text(v.as_f64().unwrap_or(0.0)), js_num_of(v))
    };
    let bp = if e.truthy("bitperfect") { "disabled" } else { "" };
    let dsd = e.o("dsd");
    let manual = !e.g("headroom_db").is_null();
    let gpu_name = if e.truthy("gpu_name") { e.s("gpu_name") } else { "NVIDIA GPU".to_string() };
    let gpu_line = if e.truthy("gpu_in_use") { format!("In use: {}", esc(&gpu_name)) } else if e.truthy("gpu_available") { format!("{} found, not in use", esc(&gpu_name)) } else { "No NVIDIA GPU found".to_string() };
    let exclusive_on = if !e.g("exclusive_wanted").is_null() { e.truthy("exclusive_wanted") } else { e.truthy("exclusive") };
    let exclusive = if e.has("exclusive") { format!(r#"<label class="row" style="gap:8px;align-items:center"><input type="checkbox" data-eng="exclusive" {}> Exclusive – Cadenza alone uses the DAC, nothing else is mixed in</label>"#, if exclusive_on { "checked" } else { "" }) } else { String::new() };
    let modes = if e.g("output_modes").is_array() { format!(r#"<div class="stack" style="gap:4px"><span class="small muted">Output mode</span>{}</div>"#, seg_e("output_mode", &e.a("output_modes"), e.g("output_mode"))) } else { String::new() };
    let group = if e.g("output_mode").as_str() == Some("dsd") {
        format!("{}\n      {}\n      {}\n      {}\n      {}",
            sel("dsd_rate", "DSD rate", dsd.g("rates"), dsd.g("rate"), bp),
            sel("dsd_modulator", "DSD modulator", dsd.g("modulators"), dsd.g("modulator"), bp),
            sel("dsd_packing", "DSD packing", dsd.g("packings"), dsd.g("packing"), bp),
            if dsd.g("oversamplings").is_array() { format!(r#"<div class="stack" style="gap:4px"><span class="small muted">DSD oversampling</span>{}</div>"#, seg_e("dsd_oversampling", &dsd.a("oversamplings"), dsd.g("oversampling"))) } else { String::new() },
            if dsd.has("gain_db") { range("dsd_gain_db", "DSD gain", "eng-dsd-gain-v", dsd.g("gain_db"), "-6") } else { String::new() })
    } else {
        let bits: Value = if e.g("bits_list").is_array() { Value::Array(e.a("bits_list").iter().map(|b| json!([b, format!("{}-bit", js_string(b))])).collect()) } else { Value::Null };
        format!("{}\n      {}\n      {}\n      {}",
            sel("filter", "Upsampling filter", e.g("filters"), e.g("filter"), bp),
            sel("dither", "Dither / noise shaping", e.g("dithers"), e.g("dither"), bp),
            sel("rate", "Output rate", e.g("rates"), e.g("rate"), bp),
            sel("bits", "Bit depth", &bits, e.g("bits"), bp))
    };
    let headroom = if e.has("headroom_db") {
        format!("<label class=\"row\" style=\"gap:8px;align-items:center\"><input type=\"checkbox\" id=\"eng-headroom-auto\" {}> Automatic headroom – room for the filters, so loud peaks do not clip</label>\n      {}",
            if manual { "" } else { "checked" },
            if manual { range("headroom_db", "Headroom", "eng-headroom-v", e.g("headroom_db"), "-12") } else { String::new() })
    } else { String::new() };
    let soft = if e.has("soft_clip") { format!(r#"<label class="row" style="gap:8px;align-items:center"><input type="checkbox" data-eng="soft_clip" {}> Soft clipper – round off peaks that would clip instead of cutting them</label>"#, if e.truthy("soft_clip") { "checked" } else { "" }) } else { String::new() };
    let gpu = if e.has("gpu") { format!(r#"<div class="stack" style="gap:4px"><span class="small muted">GPU</span>{}<span class="small muted">{gpu_line}</span></div>"#, seg_e("gpu", &[json!(["auto", "Auto"]), json!(["on", "On"]), json!(["off", "Off"])], e.g("gpu"))) } else { String::new() };
    let matrix_cur = if e.g("matrix").is_null() { json!("off") } else { e.g("matrix").clone() };
    format!(r#"<div class="stack" style="gap:8px">
      {output}
      {exclusive}
      <label class="row" style="gap:8px;align-items:center"><input type="checkbox" data-eng="bitperfect" {bp_on}> Bit-perfect – send the file to the DAC untouched (no upsampling, dither or volume)</label>
      {modes}
      {group}
      {headroom}
      {soft}
      {gpu}
      {matrix}
      <span class="small muted">Output: {out} · {excl}</span>
      {error}
    </div>"#,
        output = sel("output", "Output device", e.g("outputs"), e.g("output_id"), ""),
        bp_on = if e.truthy("bitperfect") { "checked" } else { "" },
        matrix = sel("matrix", "Matrix (channel mixing)", e.g("matrix_presets"), &matrix_cur, ""),
        out = esc(&if e.truthy("output") { e.s("output") } else { "–".to_string() }),
        excl = if e.truthy("exclusive") { "Exclusive" } else { "Shared" },
        error = if e.truthy("error") { format!(r#"<span class="small" style="color:var(--err)">{}</span>"#, esc(&e.s("error"))) } else { String::new() })
}

/// Desktop app: the engine is signing in but its device has not shown up yet (or says why not).
fn engine_starting_html(st: &Value) -> String {
    if st.truthy("error") {
        format!(r#"<span class="small" style="color:var(--err)">{}</span><span class="small muted">Encore keeps trying.</span>"#, esc(&st.s("error")))
    } else { r#"<span class="small muted">Starting the engine…</span>"#.to_string() }
}


const DSP_RATES: [i64; 8] = [88200, 96000, 176400, 192000, 352800, 384000, 705600, 768000];

/// `opt()` of the JS renderPlayback: compares as strings, so a numeric rate matches its option.
fn dsp_opt(v: &Value, l: &Value, cur: &Value) -> String {
    format!(r#"<option value="{}" {}>{}</option>"#, escv(v), if js_string(v) == js_string(cur) { "selected" } else { "" }, escv(l))
}

/// `dspHtml()` of the JS renderPlayback: the Server processing section, greyed out unless switched on and allowed.
fn dsp_html(p: &Value, dsp: &Value) -> String {
    let greyed = r#"<div class="stack" style="gap:8px;opacity:.45;pointer-events:none" aria-disabled="true"><label class="row" style="gap:8px;align-items:center"><input type="checkbox" disabled> Upsample my streams on the server</label><label class="stack" style="gap:4px"><span class="small muted">Upsampling filter</span><select class="input" disabled><option>–</option></select></label><label class="stack" style="gap:4px"><span class="small muted">Dither / noise shaping</span><select class="input" disabled><option>–</option></select></label><label class="stack" style="gap:4px"><span class="small muted">Output rate</span><select class="input" disabled><option>–</option></select></label></div>"#;
    if !dsp.truthy("enabled") { return format!(r#"{greyed}<span class="small muted">Turned off on this server. Cadenza upsampling for streams uses the server's processor, so an admin has to switch it on (Settings → Apps & sources → Server processing) and allow your account.</span>"#); }
    if !dsp.truthy("allowed") { return format!(r#"{greyed}<span class="small muted">Your account isn't allowed to use the server's processing. Ask an admin.</span>"#); }
    let dev = crate::player::device_rate();
    let off = if p.truthy("dsp") { "" } else { "disabled" };
    let max_rate = dsp.f("max_rate");
    let max_rate = if max_rate.is_nan() { 0.0 } else { max_rate };
    let mut rates: Vec<(Value, String)> = vec![
        (json!("device"), if dev != 0.0 && !dev.is_nan() { format!("Match this device ({} kHz)", crate::player::khz(dev)) } else { "Match this device".to_string() }),
        (json!("auto"), "Highest the server allows".to_string()),
    ];
    rates.extend(DSP_RATES.iter().filter(|r| (**r as f64) <= max_rate).map(|r| (json!(r), format!("{} kHz", crate::player::khz(*r as f64)))));
    let pairs = |key: &str, cur: &Value| -> String {
        dsp.a(key).iter().map(|o| {
            let it = o.items();
            dsp_opt(it.first().unwrap_or(&Value::Null), it.get(1).unwrap_or(&Value::Null), cur)
        }).collect()
    };
    [
        format!(r#"<label class="row" style="gap:8px;align-items:center"><input type="checkbox" id="pb-dsp" {}> Upsample my streams on the server</label>"#, if p.truthy("dsp") { "checked" } else { "" }),
        format!(r#"<label class="stack" style="gap:4px"><span class="small muted">Upsampling filter</span><select class="input" id="pb-dsp-filter" {off}>{}{}</select></label>"#,
            dsp_opt(&json!(""), &json!("Server default"), p.g("dsp_filter")), pairs("filters", p.g("dsp_filter"))),
        format!(r#"<label class="stack" style="gap:4px"><span class="small muted">Dither / noise shaping</span><select class="input" id="pb-dsp-dither" {off}>{}</select></label>"#, pairs("dithers", p.g("dsp_dither"))),
        format!(r#"<label class="stack" style="gap:4px"><span class="small muted">Output rate</span><select class="input" id="pb-dsp-rate" {off}>{}</select></label>"#,
            rates.iter().map(|(v, l)| dsp_opt(v, &json!(l), p.g("dsp_rate"))).collect::<String>()),
        r#"<span class="small muted">The server's Cadenza upsamples the original file and sends lossless 24-bit FLAC. It uses more data (about 4.6 Mbit/s at 96 kHz, 9.2 Mbit/s at 192 kHz) and the server's processor; Data saver and Auto still step down to a lighter stream when the connection can't keep up.</span>"#.to_string(),
    ].join("\n        ")
}

/// `${v > 0 ? '+' : ''}${v} dB`
fn sign_db(v: f64) -> String { format!("{}{} dB", if v > 0.0 { "+" } else { "" }, v) }
fn fade_text(v: f64) -> String { if v > 0.0 { format!("{v} s") } else { "Off".into() } }
/// `+input.value` as a JSON number (NaN → null, which clean() turns into the default).
fn num_json(s: &str) -> Value { let t = s.trim(); let n = if t.is_empty() { 0.0 } else { t.parse::<f64>().unwrap_or(f64::NAN) }; crate::playback::jnum(n) }
fn num_of(s: &str) -> f64 { let t = s.trim(); if t.is_empty() { 0.0 } else { t.parse::<f64>().unwrap_or(f64::NAN) } }
/// The bands on screen: the preset's own until you edit one (then the preset is Custom).
fn sound_bands(p: &Value) -> Vec<Value> { crate::playback::eq_bands(p) }
fn band_row(b: &Value, i: usize) -> String {
    let n = i + 1;
    format!(r#"<tr data-i="{i}">
      <td><select class="input" data-f="type" aria-label="Band {n} type">{types}</select></td>
      <td><input class="input" type="number" data-f="freq" min="20" max="20000" step="1" value="{freq}" aria-label="Band {n} frequency (Hz)"></td>
      <td><input class="input" type="number" data-f="gain" min="-24" max="24" step="0.5" value="{gain}" aria-label="Band {n} gain (dB)"></td>
      <td><input class="input" type="number" data-f="q" min="0.1" max="18" step="0.1" value="{q}" aria-label="Band {n} Q"></td>
      <td><button type="button" class="btn ghost icon sm" data-rm-band="{i}" aria-label="Remove band" title="Remove band">✕</button></td></tr>"#,
        types = crate::playback::EQ_TYPES.iter().map(|(v, l)| dsp_opt(&json!(v), &json!(l), b.g("type"))).collect::<String>(),
        freq = js_string(b.g("freq")), gain = js_string(b.g("gain")), q = js_string(b.g("q")))
}
fn sound_html(p: &Value) -> String {
    let list = sound_bands(p);
    let on = p.truthy("fx_on");
    let presets: String = crate::playback::EQ_PRESETS.iter().map(|x| dsp_opt(&json!(x.id), &json!(x.label), p.g("eq_preset"))).collect::<String>()
        + &if p.s("eq_preset") == "custom" { dsp_opt(&json!("custom"), &json!("Custom"), p.g("eq_preset")) } else { String::new() };
    let rows = if list.is_empty() { r#"<tr><td colspan="5" class="small muted">No bands.</td></tr>"#.to_string() } else { list.iter().enumerate().map(|(i, b)| band_row(b, i)).collect() };
    format!(r#"<label class="row" style="gap:8px;align-items:center"><input type="checkbox" id="pb-fx" {fx_checked}> Use sound effects</label>
      <fieldset class="stack" id="pb-fx-box" style="gap:8px;border:0;padding:0;margin:0;min-width:0;{fx_style}" {fx_disabled}>
        <label class="stack" style="gap:4px"><span class="small muted">Equalizer preset</span><select class="input" id="pb-eq-preset">{presets}</select></label>
        <label class="stack" style="gap:4px"><span class="small muted">Preamp <b id="pb-eq-preamp-v">{preamp_text}</b></span><input type="range" id="pb-eq-preamp" min="-24" max="12" step="0.5" value="{preamp}"></label>
        <div style="overflow-x:auto"><table class="eq-bands" style="width:100%;border-collapse:collapse">
          <thead><tr class="small muted" style="text-align:left"><th>Type</th><th>Frequency (Hz)</th><th>Gain (dB)</th><th>Q</th><th></th></tr></thead>
          <tbody>{rows}</tbody></table></div>
        <div class="row"><button type="button" class="btn sm" id="pb-eq-add" {add_disabled}>{plus}Add band</button></div>
        <label class="stack" style="gap:4px"><span class="small muted">Crossfeed</span><select class="input" id="pb-crossfeed">{crossfeed}</select></label>
        <span class="small muted">For headphones: mixes a little of each channel into the other, like speakers in a room.</span>
        <label class="stack" style="gap:4px"><span class="small muted">Balance <b id="pb-balance-v">{balance_text}</b></span><input type="range" id="pb-balance" min="-100" max="100" step="1" value="{balance}"></label>
        <label class="row" style="gap:8px;align-items:center"><input type="checkbox" id="pb-mono" {mono}> Mono</label>
      </fieldset>
      <span class="small muted">Runs in this browser or app – it costs your server nothing. Works with every quality setting. Turning it on means the sound is no longer the untouched file (the Signal path says so).</span>
      <label class="stack" style="gap:4px"><span class="small muted">Crossfade <b id="pb-crossfade-v">{fade_text}</b></span><input type="range" id="pb-crossfade" min="0" max="12" step="1" value="{crossfade}"></label>
      <label class="row" style="gap:8px;align-items:center"><input type="checkbox" id="pb-crossfade-gapless" {gapless}> Keep albums gapless (no fade between songs from the same album)</label>
      <span class="small muted">Blends the end of one song into the start of the next.</span>"#,
        fx_checked = if on { "checked" } else { "" },
        fx_style = if on { "" } else { "opacity:.45" },
        fx_disabled = if on { "" } else { "disabled" },
        preamp_text = sign_db(p.f("eq_preamp")), preamp = js_string(p.g("eq_preamp")),
        add_disabled = if list.len() >= 10 { "disabled" } else { "" }, plus = icon("plus"),
        crossfeed = crate::playback::CROSSFEED.iter().map(|(v, l)| dsp_opt(&json!(v), &json!(l), p.g("crossfeed"))).collect::<String>(),
        balance_text = crate::playback::balance_text(p.f("balance")), balance = js_string(p.g("balance")),
        mono = if p.truthy("mono") { "checked" } else { "" },
        fade_text = fade_text(p.f("crossfade")), crossfade = js_string(p.g("crossfade")),
        gapless = if p.truthy("crossfade_album_gapless") { "checked" } else { "" })
}
fn bind_sound(st: &Rc<Look>) {
    let el = st.el.clone();
    let check = |sel: &str, key: &'static str| {
        if let Some(c) = dom::qs_in(&el, sel) {
            let (st2, c2) = (st.clone(), c.clone());
            dom::on(&c, "change", move |_| set_playback(&st2, json!({ key: dom::checked(&c2) })));
        }
    };
    check("#pb-fx", "fx_on");
    check("#pb-mono", "mono");
    check("#pb-crossfade-gapless", "crossfade_album_gapless");
    if let Some(c) = dom::qs_in(&el, "#pb-eq-preset") {
        let (st2, c2) = (st.clone(), c.clone());
        dom::on(&c, "change", move |_| {
            let v = dom::value(&c2);
            if let Some(x) = crate::playback::EQ_PRESETS.iter().find(|y| y.id == v) {
                set_playback(&st2, json!({ "eq_preset": x.id, "eq_bands": crate::playback::preset_bands_json(x), "eq_preamp": crate::playback::jnum(x.preamp) }));
            }
        });
    }
    // a range slider: its value shows while dragging, and is saved when let go
    let slider = |sel: &str, label: &'static str, key: &'static str, text: fn(f64) -> String| {
        let Some(r) = dom::qs_in(&el, sel) else { return };
        let (el2, r2) = (el.clone(), r.clone());
        dom::on(&r, "input", move |_| { if let Some(v) = dom::qs_in(&el2, label) { dom::set_text(&v, &text(num_of(&dom::value(&r2)))); } });
        let (st2, r2) = (st.clone(), r.clone());
        dom::on(&r, "change", move |_| set_playback(&st2, json!({ key: num_json(&dom::value(&r2)) })));
    };
    slider("#pb-eq-preamp", "#pb-eq-preamp-v", "eq_preamp", sign_db);
    slider("#pb-balance", "#pb-balance-v", "balance", crate::playback::balance_text);
    slider("#pb-crossfade", "#pb-crossfade-v", "crossfade", fade_text);
    {
        let st2 = st.clone();
        dom::on_each(&el, ".eq-bands tr[data-i] [data-f]", "change", move |c, _| {
            let mut list = sound_bands(&st2.p.borrow());
            let Some(tr) = dom::closest_of(c, "tr") else { return };
            let i = num_of(&dom::data(&tr, "i")) as usize;
            let f = dom::data(c, "f");
            let v = if f == "type" { json!(dom::value(c)) } else { num_json(&dom::value(c)) };
            if let Some(b) = list.get_mut(i) { b[f.as_str()] = v; }
            set_playback(&st2, json!({ "eq_preset": "custom", "eq_bands": list }));
        });
    }
    {
        let st2 = st.clone();
        dom::on_each(&el, "[data-rm-band]", "click", move |b, _| {
            let mut list = sound_bands(&st2.p.borrow());
            let i = num_of(&dom::data(b, "rmBand")) as usize;
            if i < list.len() { list.remove(i); }
            set_playback(&st2, json!({ "eq_preset": "custom", "eq_bands": list }));
        });
    }
    if let Some(b) = dom::qs_in(&el, "#pb-eq-add") {
        let st2 = st.clone();
        dom::on(&b, "click", move |_| {
            let mut list = sound_bands(&st2.p.borrow());
            if list.len() < 10 {
                list.push(json!({ "type": "peaking", "freq": 1000, "gain": 0, "q": 1 }));
                set_playback(&st2, json!({ "eq_preset": "custom", "eq_bands": list }));
            }
        });
    }
    if let Some(c) = dom::qs_in(&el, "#pb-crossfeed") {
        let (st2, c2) = (st.clone(), c.clone());
        dom::on(&c, "change", move |_| set_playback(&st2, json!({ "crossfeed": dom::value(&c2) })));
    }
}

fn paint_playback(st: &Rc<Look>) {
    let p = st.p.borrow().clone();
    let link = crate::player::linked();
    let eng = link.as_ref().map(|l| l.state.g("engine").clone()).filter(|e| crate::json::truthy(e));
    let help = QUALITY_HELP.iter().find(|(k, _)| *k == p.s("quality")).map(|(_, h)| *h).unwrap_or("undefined");
    // the engine inside this very app, or a Cadenza somewhere else?
    let local = eng.as_ref().is_some_and(|e| e.truthy("embedded")) && link.as_ref().is_some_and(|l| crate::remote::is_engine(&l.id));
    *st.painted.borrow_mut() = engine_key(st);
    let eng_title = match (&eng, &link) {
        (Some(_), Some(l)) => if local { " · this computer".to_string() } else { format!(" · {}", esc(&l.name)) },
        _ => if st.native { " · this computer".to_string() } else { String::new() },
    };
    let engine_block = match (&eng, &link) {
        (Some(e), Some(_)) => engine_html(e),
        // the desktop app before its engine shows up (or after you unlinked it); a browser: greyed out
        _ => if st.native { if crate::remote::engine_off() { ENGINE_OFF_COPY.to_string() } else { engine_starting_html(&st.engine_st.borrow()) } } else { ENGINE_GREYED.to_string() },
    };
    dom::set_html(&st.el, &format!(r##"<div class="card stack">
      <h2>Playback</h2>
      <p class="small muted" style="margin:0">{saved}</p>
      <div class="stack" style="gap:6px"><h3>Quality</h3>{quality}
        <span class="small muted">{help}</span></div>
      <div class="stack" style="gap:6px"><h3>Buffering</h3>
        <label class="row" style="gap:8px;align-items:center"><input type="checkbox" id="pb-preload" {preload}> Pre-load the next song while this one plays</label>
        <span class="small muted">The next song is fetched once the current one is safely buffered, so it starts without a gap. Turn it off on a very slow connection to leave all the bandwidth to the song playing.</span></div>
      <div class="stack" style="gap:6px"><h3>When the queue ends</h3>
        <label class="row" style="gap:8px;align-items:center"><input type="checkbox" id="pb-autoplay" {autoplay}> Keep going with a shuffle of the whole library</label>
        <label class="row" style="gap:8px;align-items:center"><input type="checkbox" id="pb-resume" {resume}> Resume where I left off when the app opens</label></div>
      <div class="stack" style="gap:6px"><h3>Sound (on this device)</h3>
        {sound}</div>
      <div class="stack" style="gap:6px"><h3>Pure playback</h3>
        <span class="small muted">Encore streams the original file untouched – the quality badge on the player opens the <b>Signal path</b>, which shows what the file is, what was sent and what this device does with it. A browser hands the sound to the operating system, which mixes it with other sounds and may resample it; bit-perfect output needs an exclusive-mode engine.</span></div>
      <div class="stack" style="gap:6px"><h3>Server processing (Cadenza)</h3>
        {dsp_block}</div>
      <div class="stack" style="gap:6px"><h3>Cadenza engine{eng_name}</h3>
        {engine_block}</div>
    </div>"##,
        saved = if st.server_prefs { "Saved to your account, so it follows you to every device." } else { "Saved on this device." },
        quality = seg_str(&p, "quality", &[("auto", "Auto"), ("original", "Original"), ("saver", "Data saver")]),
        preload = if p.truthy("preload") { "checked" } else { "" },
        autoplay = if p.truthy("autoplay") { "checked" } else { "" },
        resume = if p.truthy("resume") { "checked" } else { "" },
        sound = sound_html(&p),
        dsp_block = dsp_html(&p, &st.dsp),
        eng_name = eng_title));
    {
        let st2 = st.clone();
        dom::on_each(&st.el, ".seg[data-key] button", "click", move |b, _| {
            let Some(sg) = dom::closest_of(b, ".seg") else { return };
            set_playback(&st2, json!({ dom::data(&sg, "key"): dom::data(b, "v") }));
        });
    }
    if let Some(c) = dom::qs_in(&st.el, "#pb-preload") {
        let (st2, c2) = (st.clone(), c.clone());
        dom::on(&c, "change", move |_| set_playback(&st2, json!({ "preload": dom::checked(&c2) })));
    }
    if let Some(c) = dom::qs_in(&st.el, "#pb-autoplay") {
        let (st2, c2) = (st.clone(), c.clone());
        dom::on(&c, "change", move |_| set_playback(&st2, json!({ "autoplay": dom::checked(&c2) })));
    }
    if let Some(c) = dom::qs_in(&st.el, "#pb-resume") {
        let (st2, c2) = (st.clone(), c.clone());
        dom::on(&c, "change", move |_| set_playback(&st2, json!({ "resume": dom::checked(&c2) })));
    }
    bind_sound(st);
    if let Some(c) = dom::qs_in(&st.el, "#pb-dsp") {
        let (st2, c2) = (st.clone(), c.clone());
        dom::on(&c, "change", move |_| set_playback(&st2, json!({ "dsp": dom::checked(&c2) })));
        for (sel, key) in [("#pb-dsp-filter", "dsp_filter"), ("#pb-dsp-dither", "dsp_dither"), ("#pb-dsp-rate", "dsp_rate")] {
            let Some(c) = dom::qs_in(&st.el, sel) else { continue };
            let (st2, c2) = (st.clone(), c.clone());
            dom::on(&c, "change", move |_| {
                let v = dom::value(&c2);
                // the rate is stored as a number when it is one
                let v = if key == "dsp_rate" && !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) { v.parse::<f64>().map(|n| json!(n as i64)).unwrap_or(json!(v)) } else { json!(v) };
                set_playback(&st2, json!({ key: v }));
            });
        }
    }
    if let Some(l) = link {
        // every engine switch goes to the linked device as { action: 'engine', set: { key: value } }
        let l = Rc::new(l);
        let send_eng: Rc<dyn Fn(Value)> = Rc::new(move |set| {
            (l.send)(json!({ "action": "engine", "set": set }));
            if !local { toast(&format!("Sent to {}", l.name), "ok"); }
        });
        for c in dom::qsa_in(&st.el, "[data-eng]") {
            if c.get_attribute("type").as_deref() != Some("range") { continue; }
            let (el, c2) = (st.el.clone(), c.clone());
            dom::on(&c, "input", move |_| {
                if let Some(v) = dom::qs_in(&el, &format!("#{}", dom::data(&c2, "v"))) { dom::set_text(&v, &db_text(dom::value(&c2).parse().unwrap_or(0.0))); }
            });
        }
        {
            let send_eng = send_eng.clone();
            dom::on_each(&st.el, "[data-eng]", "change", move |c, _| {
                let k = dom::data(c, "eng");
                let ty = c.get_attribute("type").unwrap_or_default();
                let raw = dom::value(c);
                let v = if ty == "checkbox" { json!(dom::checked(c)) }
                    else if ty == "range" { eng_num(&raw).unwrap_or(json!(0)) }
                    else if k == "rate" || k == "bits" { eng_num(&raw).unwrap_or_else(|| json!(raw)) }
                    else { json!(raw) };
                send_eng(json!({ k: v }));
            });
        }
        {
            let send_eng = send_eng.clone();
            dom::on_each(&st.el, "[data-eng-seg] button", "click", move |b, _| {
                let Some(sg) = dom::closest_of(b, ".seg") else { return };
                send_eng(json!({ dom::data(&sg, "engSeg"): dom::data(b, "v") }));
            });
        }
        if let Some(ha) = dom::qs_in(&st.el, "#eng-headroom-auto") {
            let ha2 = ha.clone();
            dom::on(&ha, "change", move |_| send_eng(json!({ "headroom_db": if dom::checked(&ha2) { Value::Null } else { json!(-3) } })));
        }
    }
}

fn set_playback(st: &Rc<Look>, patch: Value) {
    let p = {
        let mut cur = st.p.borrow_mut();
        if let (Some(dst), Some(src)) = (cur.as_object_mut(), patch.as_object()) {
            for (k, v) in src { dst.insert(k.clone(), v.clone()); }
        }
        cur.clone()
    };
    let server_prefs = st.server_prefs;
    let st2 = st.clone();
    dom::spawn(async move {
        let c = crate::playback::save(&p, server_prefs).await;
        *st2.p.borrow_mut() = c;
        paint_playback(&st2);
    });
    paint_playback(st);
}

/// The state of the JS `renderAppearance` closure: the panel, `p` and `serverPrefs`.
struct Look {
    el: Element, p: RefCell<Value>, server_prefs: bool, dsp: Value,
    /// the desktop app carries Cadenza (`window.EncoreNative.engine`)
    native: bool,
    /// its `{ running, device_id, name, error }` – asked again every couple of seconds
    engine_st: RefCell<Value>,
    /// what the Cadenza engine section was last painted from (`engine_key`)
    painted: RefCell<String>,
}

/// Settings → Appearance: theme, accent colour, faded/solid background.
fn render_appearance(el: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let server_prefs = app::has_feature("theme");
        let st = Rc::new(Look { el, p: RefCell::new(appearance::current()), server_prefs, dsp: Value::Null, native: false, engine_st: RefCell::new(Value::Null), painted: RefCell::new(String::new()) });
        paint_appearance(&st);
        Ok(())
    })
}

fn seg(p: &Value, key: &str, opts: &[(&str, &str)]) -> String {
    format!(r##"<div class="seg" data-key="{key}">{}</div>"##,
        opts.iter().map(|(v, l)| format!(r##"<button type="button" data-v="{v}" class="{}">{l}</button>"##, if p.g(key).as_str() == Some(*v) { "on" } else { "" })).collect::<String>())
}

fn paint_appearance(st: &Rc<Look>) {
    let p = st.p.borrow().clone();
    let presets: String = appearance::PRESETS.iter().map(|x| {
        let bg = match x.theme { "encore" => "#161617", "oled" => "#000", "light" => "#f6f6fb", "dark" => "#0b0b12", _ => "undefined" };
        let (a, b) = match appearance::ACCENTS.iter().find(|a| a.id == x.accent) {
            Some(ac) => (ac.a, ac.b),
            None => { let c = x.custom.unwrap_or("undefined"); (c, c) }
        };
        let on = p.s("theme") == x.theme && p.s("accent") == x.accent && p.s("background") == x.background && (x.accent != "custom" || p.g("custom").as_str() == x.custom);
        format!(r##"<button type="button" class="preset {}" data-preset="{}"><i style="background:linear-gradient(135deg, {a} 0 22%, {b} 22% 34%, {bg} 34%)"></i>{}</button>"##,
            if on { "on" } else { "" }, x.id, esc(x.name))
    }).collect();
    let swatches: String = appearance::ACCENTS.iter().map(|a| format!(r##"<button type="button" class="swatch {}" data-accent="{}" title="{}" aria-label="{}" style="background:linear-gradient(135deg, {}, {})"></button>"##,
        if p.s("accent") == a.id { "on" } else { "" }, a.id, esc(a.name), esc(a.name), a.a, a.b)).collect();
    let custom = esc(&p.s("custom"));
    dom::set_html(&st.el, &format!(r##"<div class="card stack">
      <h2>Appearance</h2>
      <p class="small muted" style="margin:0">{saved}</p>
      <div class="stack" style="gap:8px"><h3>Presets</h3>
        <div class="presets">{presets}</div></div>
      <div class="stack" style="gap:6px"><h3>Theme</h3>{theme}</div>
      <div class="stack" style="gap:8px"><h3>Accent colour</h3>
        <div class="swatches">{swatches}</div>
        <div class="custom-colour"><span class="small muted">Any colour:</span><input type="color" id="custom" value="{custom}" title="Pick any colour">
          <input class="input" id="custom-hex" value="{custom}" maxlength="7" spellcheck="false" aria-label="Hex colour">
          {in_use}</div></div>
      <div class="stack" style="gap:6px"><h3>Background</h3>{background}</div>
      <div class="row"><button class="btn primary" type="button">{check}Example button</button><span class="badge accent">Example badge</span></div>
    </div>"##,
        saved = if st.server_prefs { "Saved to your account, so it follows you to every device." } else { "Saved on this device." },
        theme = seg(&p, "theme", &[("encore", "Encore"), ("dark", "Dark"), ("oled", "OLED black"), ("light", "Light"), ("system", "Match device")]),
        in_use = if p.s("accent") == "custom" { r##"<span class="badge accent">in use</span>"## } else { "" },
        background = seg(&p, "background", &[("fade", "Faded colour"), ("solid", "Solid")]),
        check = icon("check")));
    {
        let st2 = st.clone();
        dom::on_each(&st.el, ".seg button", "click", move |b, _| {
            let Some(sg) = dom::closest_of(b, ".seg") else { return };
            let mut patch = Map::new();
            patch.insert(dom::data(&sg, "key"), Value::String(dom::data(b, "v")));
            set_appearance(&st2, Value::Object(patch));
        });
    }
    {
        let st2 = st.clone();
        dom::on_each(&st.el, "[data-accent]", "click", move |b, _| set_appearance(&st2, json!({ "accent": dom::data(b, "accent") })));
    }
    {
        let st2 = st.clone();
        dom::on_each(&st.el, "[data-preset]", "click", move |b, _| {
            let preset = dom::data(b, "preset");
            let Some(x) = appearance::PRESETS.iter().find(|x| x.id == preset) else { return };
            // const { id, name, ...look } = preset
            let mut look = json!({ "theme": x.theme, "accent": x.accent, "background": x.background });
            if let Some(c) = x.custom { look["custom"] = Value::String(c.to_string()); }
            set_appearance(&st2, look);
        });
    }
    if let Some(custom_el) = dom::qs_in(&st.el, "#custom") {
        {
            let (st2, c2) = (st.clone(), custom_el.clone());
            dom::on(&custom_el, "input", move |_| {
                let v = dom::value(&c2);
                let p = {
                    let mut p = st2.p.borrow_mut();
                    p["accent"] = Value::String("custom".to_string());
                    p["custom"] = Value::String(v.clone());
                    p.clone()
                };
                if let Some(h) = dom::qs_in(&st2.el, "#custom-hex") { dom::set_value(&h, &v); }
                appearance::apply(&p);
            });
        }
        {
            let (st2, c2) = (st.clone(), custom_el.clone());
            dom::on(&custom_el, "change", move |_| set_appearance(&st2, json!({ "accent": "custom", "custom": dom::value(&c2) })));
        }
    }
    if let Some(hex) = dom::qs_in(&st.el, "#custom-hex") {
        let (st2, h2) = (st.clone(), hex.clone());
        dom::on(&hex, "change", move |_| {
            let mut v = dom::value(&h2).trim().to_string();
            if !v.starts_with('#') { v = format!("#{v}"); }
            let digits = &v[1..];
            // /^#([0-9a-f]{3}|[0-9a-f]{6})$/i
            if (digits.len() == 3 || digits.len() == 6) && digits.chars().all(|c| c.is_ascii_hexdigit()) {
                let custom = if v.len() == 4 { format!("#{}", digits.chars().flat_map(|c| [c, c]).collect::<String>()) } else { v.to_lowercase() };
                set_appearance(&st2, json!({ "accent": "custom", "custom": custom }));
            } else {
                toast("Use a hex colour like #ff7a00", "err");
            }
        });
    }
}

/// `set(patch)`: `p = { ...p, ...patch }`, save (device + account), repaint.
fn set_appearance(st: &Rc<Look>, patch: Value) {
    let p = {
        let mut cur = st.p.borrow_mut();
        if let (Some(dst), Some(src)) = (cur.as_object_mut(), patch.as_object()) {
            for (k, v) in src { dst.insert(k.clone(), v.clone()); }
        }
        cur.clone()
    };
    let server_prefs = st.server_prefs;
    dom::spawn(async move { appearance::save(&p, server_prefs).await; });
    paint_appearance(st);
}

// The settings grid of a card. The vpn_ fields (Soulseek) get a sub-block of their own, in place,
// with the tunnel's state and Test VPN under them.
fn fields_html(fields: &[Field], one: impl Fn(&Field) -> String) -> String {
    let is_vpn = |f: &Field| f.key.starts_with("vpn_");
    let Some(at) = fields.iter().position(|f| is_vpn(f)) else {
        return format!(r##"<div class="form-grid">{}</div>"##, fields.iter().map(&one).collect::<String>());
    };
    let rest: Vec<&Field> = fields.iter().filter(|f| !is_vpn(f)).collect();
    let at = at.min(rest.len());
    let vpn: String = fields.iter().filter(|f| is_vpn(f)).map(&one).collect();
    let after: String = rest[at..].iter().map(|f| one(f)).collect();
    format!(r##"<div class="form-grid">{}</div>{}{}"##,
        rest[..at].iter().map(|f| one(f)).collect::<String>(), vpn_block(&vpn),
        if rest.len() > at { format!(r##"<div class="form-grid mt">{after}</div>"##) } else { String::new() })
}

// Settings → Soulseek → VPN for Soulseek: the WireGuard fields, the tunnel's state (polled every 5 s while the card is open) and Test VPN.
fn vpn_block(inner: &str) -> String {
    format!(r##"<div class="card flat mt" id="vpn-block" style="padding:12px 14px">
    <b>VPN for Soulseek</b>
    <div class="form-grid mt">{inner}</div>
    <div class="row between wrap mt" style="gap:8px">
      <div class="small muted" id="vpn-state" style="min-width:0">Checking…</div>
      <button class="btn sm" id="vpn-test" type="button">{refresh}Test VPN</button>
    </div>
  </div>"##, refresh = icon("refresh"))
}

thread_local! {
    /// Bumped on every paint of the Soulseek card, so the 5 s poll of an earlier paint stops.
    static VPN_POLL: Cell<u32> = const { Cell::new(0) };
}

fn bind_vpn(hub: &Rc<Hub>, sec: &'static Section, card: &Element) {
    let (Some(state), Some(btn)) = (dom::qs_in(card, "#vpn-state"), dom::qs_in(card, "#vpn-test")) else { return };
    let refresh: Rc<dyn Fn()> = {
        let (state, btn) = (state.clone(), btn.clone());
        Rc::new(move || {
            let (state, btn) = (state.clone(), btn.clone());
            dom::spawn(async move {
                match api::get("/system/vpn").await {
                    Ok(r) => { if !btn.has_attribute("disabled") { dom::set_html(&state, &diagnostics::vpn_state_html(&r)); } }
                    Err(_) => dom::set_text(&state, "Status unavailable"),
                }
            });
        })
    };
    refresh();
    let gen = VPN_POLL.with(|g| { g.set(g.get().wrapping_add(1)); g.get() });
    {
        let (card, btn, refresh) = (card.clone(), btn.clone(), refresh.clone());
        dom::interval(&hub.view, 5000, move || {
            if VPN_POLL.with(|g| g.get()) != gen || !dom::is_connected(&card) { return; }
            if card.has_attribute("open") && !btn.has_attribute("disabled") { refresh(); }
        });
    }
    let (hub2, card2, btn2) = (hub.clone(), card.clone(), btn.clone());
    dom::on(&btn, "click", move |e| {
        dom::prevent(e);
        let (hub, card, btn, state) = (hub2.clone(), card2.clone(), btn2.clone(), state.clone());
        dom::set_disabled(&btn, true);
        dom::set_html(&state, r##"<span class="spinner" style="width:10px;height:10px;border-width:2px"></span> Testing…"##);
        dom::spawn(async move {
            let r = match hub.save(sec, true, &card).await {
                Ok(()) => api::post_empty("/system/vpn/test").await,
                Err(e) => Err(e),
            };
            match r {
                Ok(r) => {
                    let n = r.g("natpmp");
                    let failed = if n.is_object() && n.g("ok") == &Value::Bool(false) && n.truthy("error") {
                        format!(r##" · <span style="color:var(--err)">Port forwarding failed: {}</span>"##, esc(&n.s("error")))
                    } else { String::new() };
                    dom::set_html(&state, &format!("{}{failed}", diagnostics::vpn_state_html(&r)));
                }
                Err(err) => {
                    let m = if err.message.is_empty() { "Test failed".to_string() } else { err.message.clone() };
                    dom::set_html(&state, &format!(r##"<span style="color:var(--err)">{}</span>"##, esc(&m)));
                }
            }
            dom::set_disabled(&btn, false);
        });
    });
}

// Server processing: how full the Cadenza pre-render cache is, how fast it renders, and a button to empty it.
fn cadenza_cache_block() -> String {
    format!(r##"<div class="card flat mt" id="cadenza-cache" style="padding:12px 14px">
    <div class="row between wrap" style="gap:8px">
      <div style="min-width:0"><b>Pre-render cache</b><div class="small muted" id="cadenza-cache-state">Checking…</div></div>
      <button class="btn sm" id="cadenza-cache-clear" type="button">{}Clear cache</button>
    </div>
    <div class="small muted mt" id="cadenza-engine">Engine: …</div>
  </div>"##, icon("trash"))
}

/// "Cache: 812 MB of 4096 MB · 23 songs · renders at 6.2× real time" (the factor only once a render has finished).
pub fn cadenza_cache_text(c: &Value) -> String {
    if !c.is_object() { return "Cache: not available on this server".into(); }
    let mb = |v: f64| { let r = crate::playback::js_round(if v.is_finite() { v } else { 0.0 }); format!("{}", r as i64) };
    let n = c.f("files");
    let n = if n.is_finite() { n } else { 0.0 };
    let f = c.f("last_factor");
    format!("Cache: {} MB of {} MB · {} {}{}", mb(c.f("used_mb")), mb(c.f("max_mb")), n, if n == 1.0 { "song" } else { "songs" },
        if f > 0.0 { format!(" · renders at {}× real time", crate::player::to_fixed1(f)) } else { String::new() })
}

/// "Engine: Cadenza 2.1.0 (updated)" / "Engine: Cadenza (built in)" – `engine` of GET /playback/dsp.
pub fn cadenza_engine_text(e: &Value) -> String {
    format!("Engine: {}", if e.is_object() && e.s("source") == "updated" && e.truthy("version") { format!("Cadenza {} (updated)", e.s("version")) } else { "Cadenza (built in)".to_string() })
}

// Server processing → Engine settings: drawn only from the list the active engine gives (engine.settings in
// GET /playback/dsp: `cadenza render --describe`, or the built-in engine's own), saved to cadenza_engine_settings.
struct EngineSettings {
    settings: Vec<Value>,
    values: Value,
}

thread_local! {
    /// { settings, values } of the engine the block was drawn for
    static ENGINE: RefCell<Option<EngineSettings>> = const { RefCell::new(None) };
}

fn engine_settings_block() -> String {
    r##"<div class="card flat mt" id="cadenza-engine-settings" style="padding:12px 14px">
    <b>Engine settings</b>
    <div class="small muted" id="cadenza-engine-hint">Checking…</div>
    <div class="form-grid mt" id="cadenza-engine-fields" hidden></div>
  </div>"##.to_string()
}

const ENGINE_TYPES: &[&str] = &["select", "bool", "number"];

/// JavaScript `Number(v)`.
fn to_num(v: &Value) -> f64 {
    match v {
        Value::Null => 0.0,
        Value::Bool(b) => if *b { 1.0 } else { 0.0 },
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(t) => { let t = t.trim(); if t.is_empty() { 0.0 } else { t.parse().unwrap_or(f64::NAN) } }
        _ => f64::NAN,
    }
}

/// `Array.isArray(s.options) ? s.options : []`, each as (value, label).
fn engine_options(s: &Value) -> Vec<(Value, Value)> {
    s.a("options").iter().map(|o| (o.get(0).cloned().unwrap_or(Value::Null), o.get(1).cloned().unwrap_or(Value::Null))).collect()
}

fn engine_default_text(s: &Value) -> String {
    match s.s("type").as_str() {
        "bool" => (if s.truthy("default") { "on" } else { "off" }).to_string(),
        "select" => engine_options(s).into_iter().find(|(v, _)| js_string(v) == s.s("default")).map(|(_, l)| js_string(&l)).unwrap_or_else(|| s.s("default")),
        _ => s.s("default"),
    }
}

/// One engine setting as a form field (data-ek = its key). A saved select value the engine no longer offers shows the default.
fn engine_field(s: &Value, v: &Value, locked: bool) -> String {
    let dis = if locked { "disabled" } else { "" };
    let k = esc(&s.s("key"));
    let label = esc(&if s.truthy("label") { s.s("label") } else { s.s("key") });
    let help = format!("<small>{}Default: {}</small>", if s.truthy("help") { format!("{} · ", esc(&s.s("help"))) } else { String::new() }, esc(&engine_default_text(s)));
    match s.s("type").as_str() {
        "bool" => format!(r##"<div class="field" style="justify-content:center"><label class="switch"><input type="checkbox" data-ek="{k}" {} {dis}><i></i>{label}</label>{help}</div>"##,
            if truthy(v) { "checked" } else { "" }),
        "select" => {
            let opts = engine_options(s);
            let pick = if opts.iter().any(|(o, _)| js_string(o) == js_string(v)) { v.clone() } else { s.g("default").clone() };
            let options: String = opts.iter().map(|(val, lab)| format!(r##"<option value="{}" {}>{}</option>"##,
                escv(val), if js_string(val) == js_string(&pick) { "selected" } else { "" }, escv(lab))).collect();
            format!(r##"<label class="field"><span>{label}</span><select class="input" data-ek="{k}" {dis}>{options}</select>{help}</label>"##)
        }
        _ => {
            let attr = |n: &str| -> String { let x = s.g(n); if !x.is_null() && to_num(x).is_finite() { format!(r##" {n}="{}""##, escv(x)) } else { String::new() } };
            format!(r##"<label class="field"><span>{label}</span><input class="input" type="number" data-ek="{k}"{}{}{} value="{}" autocomplete="off" {dis}>{help}</label>"##,
                attr("min"), attr("max"), attr("step"), escv(v))
        }
    }
}

fn paint_engine_settings(card: &Element, engine: &Value, locked: bool) {
    let (Some(hint), Some(bx)) = (dom::qs_in(card, "#cadenza-engine-hint"), dom::qs_in(card, "#cadenza-engine-fields")) else { return };
    if !dom::data(&bx, "loaded").is_empty() { return; }
    let settings: Vec<Value> = if engine.is_object() { engine.a("settings") } else { vec![] }
        .into_iter().filter(|s| s.is_object() && s.truthy("key") && ENGINE_TYPES.contains(&s.s("type").as_str())).collect();
    let values = if engine.is_object() && engine.g("values").is_object() { engine.g("values").clone() } else { json!({}) };
    dom::set_data(&bx, "loaded", "1");
    let html: String = settings.iter().map(|s| engine_field(s, values.get(s.s("key").as_str()).unwrap_or(s.g("default")), locked)).collect();
    let empty = settings.is_empty();
    ENGINE.with(|e| *e.borrow_mut() = Some(EngineSettings { settings, values }));
    if empty { dom::set_text(&hint, "This engine has no settings of its own."); return; }
    dom::set_text(&hint, &format!("The settings of the engine in use. People’s own filter and dither choices under Settings → Playback still win for their streams.{}", if locked { " Set by an environment variable." } else { "" }));
    dom::set_html(&bx, &html);
    dom::hide(&bx, false);
}

/// The engine settings as the JSON text for cadenza_engine_settings (only values that differ from the default;
/// keys this engine doesn't have stay as saved), or None when the block isn't drawn.
fn engine_settings_value(root: &Element) -> Option<String> {
    let bx = dom::qs_in(root, "#cadenza-engine-fields")?;
    if dom::data(&bx, "loaded").is_empty() { return None; }
    ENGINE.with(|e| {
        let e = e.borrow();
        let en = e.as_ref()?;
        let mut out = en.values.as_object().cloned().unwrap_or_default();
        for el in dom::qsa_in(&bx, "[data-ek]") {
            let key = dom::data(&el, "ek");
            let Some(s) = en.settings.iter().find(|x| x.s("key") == key) else { continue };
            let v = match s.s("type").as_str() {
                "bool" => Value::Bool(dom::checked(&el)),
                "number" => {
                    let raw = dom::value(&el);
                    let n = if raw.trim().is_empty() { f64::NAN } else { to_num(&Value::String(raw)) };
                    if !n.is_finite() { out.remove(&key); continue; }
                    if n == to_num(s.g("default")) { out.remove(&key); continue; }
                    if n.fract() == 0.0 && n.abs() < 1e15 { Value::from(n as i64) } else { Value::from(n) }
                }
                _ => {
                    let raw = dom::value(&el);
                    engine_options(s).into_iter().find(|(o, _)| js_string(o) == raw).map(|(o, _)| o).unwrap_or(Value::String(raw))
                }
            };
            if s.s("type") != "number" && &v == s.g("default") { out.remove(&key); } else { out.insert(key, v); }
        }
        Some(Value::Object(out).to_string())
    })
}

fn bind_cadenza_cache(hub: &Rc<Hub>, card: &Element) {
    let (Some(state), Some(btn)) = (dom::qs_in(card, "#cadenza-cache-state"), dom::qs_in(card, "#cadenza-cache-clear")) else { return };
    let line = dom::qs_in(card, "#cadenza-engine");
    let locked = hub.values.borrow().o("_meta").o("cadenza_engine_settings").s("source") == "env";
    let refresh: Rc<dyn Fn()> = {
        let (state, card) = (state.clone(), card.clone());
        Rc::new(move || {
            let (state, card, line) = (state.clone(), card.clone(), line.clone());
            dom::spawn(async move {
                match api::get("/playback/dsp").await {
                    Ok(d) => {
                        dom::set_text(&state, &cadenza_cache_text(d.g("cache")));
                        if let Some(l) = &line { dom::set_text(l, &cadenza_engine_text(d.g("engine"))); }
                        paint_engine_settings(&card, d.g("engine"), locked);
                    }
                    Err(_) => dom::set_text(&state, "Status unavailable"),
                }
            });
        })
    };
    refresh();
    let btn2 = btn.clone();
    dom::on(&btn, "click", move |e| {
        dom::prevent(e);
        let (btn, refresh) = (btn2.clone(), refresh.clone());
        dom::set_disabled(&btn, true);
        dom::spawn(async move {
            match api::post_empty("/playback/dsp/cache/clear").await { Ok(_) => toast("Cache cleared", "ok"), Err(err) => toast(&err.message, "err") }
            dom::set_disabled(&btn, false);
            refresh();
        });
    });
}

// Settings → Soulseek: how Encore's Soulseek engine is doing (GET /soulseek/status), asked again every 5 s while
// the card is open. The listen port is hidden while the VPN is on (the VPN's forwarded port is used).
fn soulseek_status_block() -> String {
    r##"<div class="card flat mt" id="slsk-engine" style="padding:12px 14px">
    <b>Status</b>
    <div class="small muted" id="slsk-engine-state">Checking…</div>
  </div>"##.to_string()
}

/// "Running v0.24.1 · signed in as alice" / "Not installed · Install or update it under Download tools" / "Waiting for the VPN" …
pub fn soulseek_state_html(r: &Value) -> String {
    if !r.is_object() { return "Status unavailable".into(); }
    let red = |t: &str| format!(r##"<span style="color:var(--err)">{}</span>"##, esc(t));
    let v = if r.truthy("version") { let ver = r.s("version"); format!(" v{}", esc(ver.strip_prefix('v').unwrap_or(&ver))) } else { String::new() };
    let error = r.s("error");
    let line = match r.s("state").as_str() {
        "not_installed" => "Not installed".to_string(),
        "starting" => format!("Starting…{}", if r.truthy("error") { format!(" · {}", red(&error)) } else { String::new() }),
        "running" => format!("Running{v}{}", if r.truthy("logged_in") { format!(" · signed in as {}", esc(&if r.truthy("username") { r.s("username") } else { "?".to_string() })) }
            else if r.truthy("error") { format!(" · {}", red(&error)) } else { " · signing in…".to_string() }),
        "crashed" => red(&format!("Crashed{}", if r.truthy("error") { format!(" – {error}") } else { String::new() })),
        "waiting_vpn" => "Waiting for the VPN".to_string(),
        _ => if r.g("enabled") == &Value::Bool(false) { "Off – switch on Enable Soulseek and save".to_string() } else { "Off".to_string() },
    };
    if r.truthy("hint") { format!("{line} · {}", esc(&r.s("hint"))) } else { line }
}

thread_local! {
    /// Bumped on every paint of the Soulseek card, so the 5 s status poll of an earlier paint stops.
    static SLSK_POLL: Cell<u32> = const { Cell::new(0) };
}

fn bind_soulseek(hub: &Rc<Hub>, card: &Element) {
    let vpn = dom::qs_in(card, r#"[data-k="vpn_enabled"]"#);
    let field = dom::qs_in(card, r#"[data-k="soulseek_port"]"#).and_then(|p| dom::closest_of(&p, ".field"));
    if let (Some(vpn), Some(field)) = (vpn, field) {
        dom::hide(&field, dom::checked(&vpn));
        let vpn2 = vpn.clone();
        dom::on(&vpn, "change", move |_| dom::hide(&field, dom::checked(&vpn2)));
    }
    let Some(state) = dom::qs_in(card, "#slsk-engine-state") else { return };
    let refresh = move || {
        let state = state.clone();
        dom::spawn(async move {
            match api::get("/soulseek/status").await {
                Ok(r) => dom::set_html(&state, &soulseek_state_html(&r)),
                Err(_) => dom::set_text(&state, "Status unavailable"),
            }
        });
    };
    refresh();
    let gen = SLSK_POLL.with(|g| { g.set(g.get().wrapping_add(1)); g.get() });
    let card = card.clone();
    dom::interval(&hub.view, 5000, move || {
        if SLSK_POLL.with(|g| g.get()) != gen || !dom::is_connected(&card) { return; }
        if card.has_attribute("open") { refresh(); }
    });
}

// Settings → Apps & sources → Download tools: yt-dlp, streamrip, Cadenza (server processing) and slskd
// (Soulseek engine) update in place – kept in the config folder, so no new Encore image is needed.
const TOOL_SPIN: &str = r##"<span class="spinner" style="width:10px;height:10px;border-width:2px"></span>"##;
fn tool_list(r: &Value) -> Vec<Value> { if r.is_array() { r.items() } else { r.a("tools") } }
fn tool_updatable(t: &Value) -> bool { t.truthy("updatable") }
/// `a || b`: the first truthy of the two, as text.
fn tool_or(t: &Value, a: &str, b: &str) -> String { if t.truthy(a) { t.s(a) } else if t.truthy(b) { t.s(b) } else { String::new() } }

/// The reset link of a row (link, confirm question, toast): Cadenza goes back to the engine built into Encore,
/// slskd is removed, the rest go back to the bundled copy.
fn tool_reset(t: &Value) -> (String, String, String) {
    match t.s("id").as_str() {
        "cadenza" => ("Use the built-in engine".into(), "Go back to the engine built into Encore? The downloaded Cadenza is removed.".into(), "Back to the built-in engine".into()),
        "slskd" => ("Remove slskd".into(), "Remove slskd? Soulseek stays off until you install it again.".into(), "slskd removed".into()),
        _ => {
            let name = tool_or(t, "name", "id");
            ("Use the bundled version".into(), format!("Go back to the {name} version that came with Encore? The updated copy is removed."), format!("Back to the bundled {name}"))
        }
    }
}
/// slskd is installed by Encore, not bundled: before that its button says Install.
fn tool_install(t: &Value) -> bool { t.s("id") == "slskd" && !t.truthy("installed") }

fn tool_row(t: &Value, busy: Option<&str>) -> String {
    let id = t.s("id");
    let latest = if t.truthy("latest") { t.s("latest") } else { String::new() };
    let action = if busy == Some(id.as_str()) {
        format!(r##"<button class="btn sm" type="button" disabled>{TOOL_SPIN} {}</button>"##, if tool_install(t) { "Installing…" } else { "Updating…" })
    } else if tool_install(t) && tool_updatable(t) {
        format!(r##"<button class="btn sm primary" type="button" data-tool-update="{}" {}>{}Install{}</button>"##,
            esc(&id), if busy.is_some() { "disabled" } else { "" }, icon("download"), if latest.is_empty() { String::new() } else { format!(" {}", esc(&latest)) })
    } else if t.truthy("update_available") && tool_updatable(t) {
        format!(r##"<button class="btn sm primary" type="button" data-tool-update="{}" {}>{}Update to {}</button>"##,
            esc(&id), if busy.is_some() { "disabled" } else { "" }, icon("download"), esc(&latest))
    } else if t.truthy("update_available") {
        format!(r##"<span class="badge warn">{} available</span>"##, esc(&latest))
    } else if t.truthy("latest") {
        format!(r##"<span class="badge ok">{} Up to date</span>"##, icon("check"))
    } else if tool_updatable(t) {
        r##"<span class="small faint">Not checked yet</span>"##.to_string()
    } else { String::new() };
    let updated = t.s("source") == "updated" && id != "slskd";
    format!(r##"<div class="list-item" style="align-items:flex-start;flex-wrap:wrap;gap:12px" data-tool="{eid}">
      <div class="grow" style="min-width:200px"><div style="font-weight:650">{name} <span class="small muted mono">{ver}</span>{upd}</div>
        {note}
        {err}
        {reset}</div>
      <div class="row" style="gap:8px;align-items:center">{action}</div></div>"##,
        eid = esc(&id),
        name = esc(&tool_or(t, "name", "id")),
        ver = if t.truthy("installed") { esc(&t.s("installed")) } else { "not installed".to_string() },
        upd = if updated { r##" <span class="small muted">(updated)</span>"## } else { "" },
        note = if t.truthy("note") { format!(r##"<div class="small muted">{}</div>"##, esc(&t.s("note"))) } else { String::new() },
        err = if t.truthy("error") { format!(r##"<div class="small" style="color:var(--err)">{}</div>"##, esc(&t.s("error"))) } else { String::new() },
        reset = if t.truthy("resettable") && tool_updatable(t) {
            format!(r##"<a href="#" class="small muted" data-tool-reset="{}" {}>{}</a>"##, esc(&id), if busy.is_some() { "hidden" } else { "" }, esc(&tool_reset(t).0))
        } else { String::new() })
}

/// What the Download tools card shows: the tool list, the tool being updated, and whether a check runs.
struct Tools {
    list: Element,
    check: Element,
    tools: RefCell<Vec<Value>>,
    busy: RefCell<Option<String>>,
    checking: Cell<bool>,
}

impl Tools {
    fn paint(&self) {
        let busy = self.busy.borrow().clone();
        let rows: String = self.tools.borrow().iter().map(|t| tool_row(t, busy.as_deref())).collect();
        dom::set_html(&self.list, &rows);
        dom::set_disabled(&self.check, self.checking.get() || busy.is_some());
        dom::set_html(&self.check, &if self.checking.get() { format!("{TOOL_SPIN} Checking…") } else { format!("{}Check for updates", icon("refresh")) });
    }
    async fn reload(&self) { if let Ok(r) = api::get("/system/tools").await { *self.tools.borrow_mut() = tool_list(&r); } }
    fn name_of(&self, id: &str) -> String {
        self.tools.borrow().iter().find(|t| t.s("id") == id).map(|t| t.s("name")).filter(|n| !n.is_empty()).unwrap_or_else(|| id.to_string())
    }
    fn tool(&self, id: &str) -> Value {
        self.tools.borrow().iter().find(|t| t.s("id") == id).cloned().unwrap_or_else(|| json!({ "id": id }))
    }
}

async fn render_tools(host: Element, hub: Rc<Hub>) {
    let tools = match api::get("/system/tools").await { Ok(r) => tool_list(&r), Err(_) => { dom::set_html(&host, ""); return; } };
    let values = hub.values.borrow().clone();
    let meta = values.o("_meta");
    let toggle = |key: &str, label: &str| -> String {
        if !values.has(key) { return String::new(); }
        let locked = meta.o(key).s("source") == "env";
        format!(r##"<label class="switch"><input type="checkbox" data-tool-auto="{key}" {} {}><i></i>{}</label>"##,
            if values.truthy(key) { "checked" } else { "" }, if locked { "disabled" } else { "" }, esc(label))
    };
    let toggles = toggle("tools_auto_ytdlp", "Update yt-dlp automatically (recommended – sites change often)")
        + &toggle("tools_auto_streamrip", "Update streamrip automatically when a new release is out")
        + &toggle("tools_auto_cadenza", "Update Cadenza automatically when a new release is out")
        + &toggle("tools_auto_slskd", "Update slskd automatically when a new release is out");
    dom::set_html(&host, &format!(r##"<div class="card" id="tools-card" style="margin-bottom:12px">
    <div class="row between" style="margin-bottom:8px;gap:10px"><h2>Download tools</h2><button class="btn sm" type="button" id="tools-check">{refresh}Check for updates</button></div>
    <p class="muted small" style="margin:0 0 14px">yt-dlp, streamrip, Cadenza and slskd update in place – no new Encore image needed. Updates are kept in the config folder.</p>
    <div class="list" id="tools-list"></div>
    <div class="stack mt" style="gap:8px">{toggles}<div class="small muted">Every tool is checked daily; updates install by themselves only where the switch is on. Change the schedule under <a href="#/settings?s=tasks">Scheduled tasks</a>.</div></div>
  </div>"##, refresh = icon("refresh")));
    let (Some(list), Some(check)) = (dom::qs_in(&host, "#tools-list"), dom::qs_in(&host, "#tools-check")) else { return };
    let st = Rc::new(Tools { list: list.clone(), check: check.clone(), tools: RefCell::new(tools), busy: RefCell::new(None), checking: Cell::new(false) });
    st.paint();
    {
        let st = st.clone();
        dom::on(&check, "click", move |_| {
            let st = st.clone();
            st.checking.set(true); st.paint();
            dom::spawn(async move {
                match api::post_empty("/system/tools/check").await {
                    Ok(r) => *st.tools.borrow_mut() = tool_list(&r),
                    Err(e) => toast(&e.message, "err"),
                }
                st.checking.set(false); st.paint();
            });
        });
    }
    {
        let st = st.clone();
        dom::on(&list, "click", move |e| {
            let idle = st.busy.borrow().is_none();
            if let Some(up) = dom::closest(e, "[data-tool-update]") {
                if !idle { return; }
                let id = dom::data(&up, "toolUpdate");
                let (name, install) = (st.name_of(&id), tool_install(&st.tool(&id)));
                *st.busy.borrow_mut() = Some(id.clone()); st.paint();
                let st = st.clone();
                dom::spawn(async move {
                    match api::post_empty(&format!("/system/tools/{id}/update")).await {
                        Ok(r) => toast(&format!("{name} {} {}", if install { "installed" } else { "updated to" }, tool_or(&r, "installed", "latest")), "ok"),
                        Err(err) => toast(&err.message, "err"),
                    }
                    *st.busy.borrow_mut() = None;
                    st.reload().await; st.paint();
                });
            } else if let Some(reset) = dom::closest(e, "[data-tool-reset]") {
                if !idle { return; }
                dom::prevent(e);
                let id = dom::data(&reset, "toolReset");
                let (_, ask, done) = tool_reset(&st.tool(&id));
                if !dom::confirm(&ask) { return; }
                let st = st.clone();
                dom::spawn(async move {
                    match api::post_empty(&format!("/system/tools/{id}/reset")).await {
                        Ok(_) => toast(&done, "ok"),
                        Err(err) => toast(&err.message, "err"),
                    }
                    st.reload().await; st.paint();
                });
            }
        });
    }
    for b in dom::qsa_in(&host, "[data-tool-auto]") {
        let (b2, hub) = (b.clone(), hub.clone());
        dom::on(&b, "change", move |_| {
            let (b, hub) = (b2.clone(), hub.clone());
            let (key, on) = (dom::data(&b, "toolAuto"), dom::checked(&b));
            dom::spawn(async move {
                let mut body = Map::new();
                body.insert(key, Value::Bool(on));
                match api::put("/admin/settings", Value::Object(body)).await {
                    Ok(v) => { *hub.values.borrow_mut() = v; toast("Saved", "ok"); }
                    Err(err) => { dom::set_checked(&b, !dom::checked(&b)); toast(&err.message, "err"); }
                }
            });
        });
    }
}

// Tidal downloads need your Tidal account, not an API key: Encore signs in once through a link
// (device code, one step per streamrip "lane"). This block shows the link and waits for the approval;
// when Tidal blocks the server's address the tokens can be pasted from a computer that has access.
fn tidal_block() -> String {
    format!(r##"<div class="card flat mt" id="tidal-login" style="padding:12px 14px">
    <div class="row between wrap" style="gap:8px">
      <div style="min-width:0"><b>Tidal account</b><div class="small muted" id="tidal-state">Checking…</div></div>
      <div class="row" style="gap:10px;align-items:center"><a href="#" class="small muted" id="tidal-signout" hidden>Sign out of Tidal</a><button class="btn sm" id="tidal-signin" type="button">{link}Sign in to Tidal</button></div>
    </div>
    <div class="small mt" id="tidal-out" hidden></div>
    <details class="mt" id="tidal-paste">
      <summary class="small" style="cursor:pointer">Paste tokens instead</summary>
      <div class="small muted mt">On a computer with Tidal access: install streamrip (<code>pip install streamrip</code>), run <code>rip url https://tidal.com/browse/track/1</code>, approve the link it prints, then paste the [tidal] section of its config file here (<code>rip config path</code> shows where it is).</div>
      <textarea id="tidal-toml" class="mt" rows="6" spellcheck="false" style="width:100%;font-family:monospace" placeholder="[tidal]&#10;user_id = &quot;…&quot;&#10;access_token = &quot;…&quot;"></textarea>
      <div class="row wrap mt" style="gap:8px"><button class="btn sm" id="tidal-save-tokens" type="button">{check}Save tokens</button></div>
    </details>
  </div>"##, link = icon("link"), check = icon("check"))
}

const TIDAL_SPINNER: &str = r##"<span class="spinner" style="width:10px;height:10px;border-width:2px"></span>"##;

fn bind_tidal(view: &Element, card: &Element) {
    let (Some(state), Some(out), Some(btn)) = (dom::qs_in(card, "#tidal-state"), dom::qs_in(card, "#tidal-out"), dom::qs_in(card, "#tidal-signin")) else { return };
    let (Some(paste), Some(toml), Some(save), Some(signout)) = (dom::qs_in(card, "#tidal-paste"), dom::qs_in(card, "#tidal-toml"), dom::qs_in(card, "#tidal-save-tokens"), dom::qs_in(card, "#tidal-signout")) else { return };
    let paint: Rc<dyn Fn(&Value)> = {
        let (state, out, btn, paste, signout) = (state.clone(), out.clone(), btn.clone(), paste.clone(), signout.clone());
        Rc::new(move |r: &Value| {
            // partial (default lane only) and signed_in never come together: the server reports one or the other
            let partial = r.truthy("partial");
            let signed = r.truthy("signed_in") || partial;
            dom::set_text(&state, if partial { "Signed in for 16/44.1 FLAC only – sign in again for hi-res (step 2 was not finished)." }
                else if r.truthy("signed_in") { "Signed in – Tidal downloads use this account." }
                else { "Not signed in – Tidal needs a one-time sign-in with your own Tidal account (no API key)." });
            dom::set_html(&btn, &format!("{}{}", icon("link"), if signed { "Sign in again" } else { "Sign in to Tidal" }));
            dom::hide(&signout, !signed);
            let status = r.s("status");
            if status == "waiting" && !r.s("url").is_empty() {
                let steps = r.num("steps").filter(|n| *n >= 1.0).unwrap_or(1.0) as i64;
                let step = r.num("step").filter(|n| *n >= 1.0).unwrap_or(1.0) as i64;
                let prefix = if steps > 1 { format!("Step {step} of {steps} · ") } else { String::new() };
                dom::hide(&out, false);
                dom::set_html(&out, &format!(r##"{prefix}Open <a href="{u}" target="_blank" rel="noopener" style="color:var(--accent)">{u}</a> on any device, approve in your Tidal account, then come back here. {TIDAL_SPINNER} Waiting…"##, u = esc(&r.s("url"))));
            } else if status == "done" {
                dom::hide(&out, false);
                dom::set_html(&out, &format!(r##"<span class="badge ok">{} {}</span>"##, icon("check"), esc(&r.s("message"))));
            } else if status == "failed" {
                dom::hide(&out, false);
                let m = r.s("message");
                dom::set_html(&out, &format!(r##"<span class="badge err">{}</span>"##, esc(if m.is_empty() { "Sign-in failed" } else { &m })));
                if m.contains("paste the tokens") { let _ = paste.set_attribute("open", ""); }
            }
        })
    };
    {
        let (paint, state) = (paint.clone(), state.clone());
        dom::spawn(async move {
            match api::get("/system/streamrip/tidal-login").await {
                Ok(r) => paint(&r),
                Err(_) => dom::set_text(&state, "Status unavailable"),
            }
        });
    }
    // Each step has its own link: open the new one when the server moves to step 2.
    let opened: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let open_link: Rc<dyn Fn(&Value)> = {
        let opened = opened.clone();
        Rc::new(move |r: &Value| {
            let url = r.s("url");
            if r.s("status") == "waiting" && !url.is_empty() && *opened.borrow() != url {
                *opened.borrow_mut() = url.clone();
                let _ = dom::window().open_with_url_and_target_and_features(&url, "_blank", "noopener");
            }
        })
    };
    // one poll at a time: a new click stops the previous poller
    let polling: Rc<Cell<u32>> = Rc::new(Cell::new(0));
    {
        let (view, card, out2, btn2, paint, open_link, polling) = (view.clone(), card.clone(), out.clone(), btn.clone(), paint.clone(), open_link.clone(), polling.clone());
        dom::on(&btn, "click", move |_| {
            dom::set_disabled(&btn2, true);
            dom::hide(&out2, false);
            dom::set_html(&out2, &format!("{TIDAL_SPINNER} Starting sign-in…"));
            let (view, card, out, btn, paint, open_link, polling) = (view.clone(), card.clone(), out2.clone(), btn2.clone(), paint.clone(), open_link.clone(), polling.clone());
            dom::spawn(async move {
                match api::post_empty("/system/streamrip/tidal-login").await {
                    Ok(r) => {
                        paint(&r);
                        if r.s("status") == "waiting" {
                            open_link(&r);
                            // poll every 3 s until the sign-in finishes or the card is gone
                            let gen = polling.get() + 1;
                            polling.set(gen);
                            let (card, btn, paint, open_link, polling) = (card.clone(), btn.clone(), paint.clone(), open_link.clone(), polling.clone());
                            dom::interval(&view, 3000, move || {
                                if polling.get() != gen || !dom::is_connected(&card) { return; }
                                let (btn, paint, open_link, polling) = (btn.clone(), paint.clone(), open_link.clone(), polling.clone());
                                dom::spawn(async move {
                                    if polling.get() != gen { return; }
                                    if let Ok(r) = api::get("/system/streamrip/tidal-login").await {
                                        if polling.get() != gen { return; }
                                        paint(&r);
                                        open_link(&r);
                                        if r.s("status") != "waiting" { polling.set(0); dom::set_disabled(&btn, false); }
                                    }
                                });
                            });
                        } else { dom::set_disabled(&btn, false); }
                    }
                    Err(e) => { dom::set_html(&out, &format!(r##"<span class="badge err">{}</span>"##, esc(&e.message))); dom::set_disabled(&btn, false); }
                }
            });
        });
    }
    {
        let (save2, toml, paint) = (save.clone(), toml.clone(), paint.clone());
        dom::on(&save, "click", move |_| {
            dom::set_disabled(&save2, true);
            let (save, toml, paint) = (save2.clone(), toml.clone(), paint.clone());
            dom::spawn(async move {
                match api::post("/system/streamrip/tidal-tokens", json!({ "toml": dom::value(&toml) })).await {
                    Ok(r) => { paint(&r); dom::set_value(&toml, ""); toast("Tidal tokens saved", "ok"); }
                    Err(e) => toast(&e.message, "err"),
                }
                dom::set_disabled(&save, false);
            });
        });
    }
    {
        let (out, paint) = (out.clone(), paint.clone());
        dom::on(&signout, "click", move |e| {
            e.prevent_default();
            let (out, paint) = (out.clone(), paint.clone());
            dom::spawn(async move {
                match api::del("/system/streamrip/tidal-tokens").await {
                    Ok(r) => { paint(&r); dom::hide(&out, true); toast("Signed out of Tidal", "ok"); }
                    Err(err) => toast(&err.message, "err"),
                }
            });
        });
    }
}

fn pipeline_help() -> String {
    r##"<div class="card mt"><h3>How requests are fulfilled</h3>
    <ol class="muted" style="line-height:1.9;padding-left:18px;margin:10px 0 0">
      <li><b>Library check</b> – if the album/track is already in your library (by MusicBrainz id or name) it's marked available instantly.</li>
      <li><b>streamrip</b> – lossless from Qobuz, Tidal or Deezer with your own accounts, matched by ISRC.</li>
      <li><b>Torrents & Usenet</b> – Prowlarr searches your indexers; the best album release goes to qBittorrent or SABnzbd and is imported when it finishes. If it stalls past the timeout, the next release or source is tried.</li>
      <li><b>Soulseek</b> – Encore searches the network, results are ranked by track count, format, bitrate, folder name and the uploader's free slots/speed. Stalled or failed users are skipped automatically.</li>
      <li><b>yt-dlp</b> – each track from the MusicBrainz tracklist is matched on YouTube (preferring official "Topic" uploads with matching length).</li>
      <li><b>Import</b> – files are matched to the MusicBrainz release, tagged (incl. MBIDs, ISRCs, cover art), renamed with your template, and your media servers are refreshed.</li>
    </ol></div>"##.to_string()
}
