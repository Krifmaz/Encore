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
        s("source_order", "Source order", "text").full().help("The one place to set what is searched, in order, comma-separated: qobuz, tidal, deezer (all through streamrip), slskd (Soulseek), torznab (torrents/Usenet, albums only), ytdlp (YouTube Music). Leave one out to never use it."),
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
    Section { id: "slskd", title: "Soulseek", desc: "Searches the Soulseek file-sharing network – great for FLAC and rare releases. Built in: no slskd container needed – enter any username and password and an account is created at the first sign-in. Forward the port for the best speeds.", test: Some("slskd"), test_label: None, fields: &[
        s("slskd_enabled", "Enable Soulseek", "bool"),
        s("soulseek_client", "Client", "select").options(&[("auto", "Built-in when a username is set, else slskd"), ("builtin", "Built-in client"), ("slskd", "slskd (external)")]),
        s("soulseek_username", "Soulseek username", "text").help("New name + password = new account. Don’t use the same login in slskd or another client at the same time."),
        s("soulseek_password", "Soulseek password", "password"),
        s("soulseek_port", "Listen port", "number").help("TCP port the built-in client listens on – forward it on your router (and publish it in Docker) so users behind firewalls can send to you"),
        s("soulseek_port_file", "Listen port from a file (VPN port forwarding)", "text").help("Behind gluetun: mount its folder and point this at forwarded_port – the port the VPN assigned is used and followed when it changes"),
        s("soulseek_share", "Share your library on Soulseek (recommended)", "bool").help("The network runs on sharing; many users refuse downloads to people who share nothing. Only the library folder is shared, read-only."),
        s("soulseek_upload_slots", "Upload slots", "number"),
        s("slskd_url", "slskd URL", "text").help("Only for the slskd client, e.g. http://slskd:5030"),
        s("slskd_api_key", "slskd API key", "password").help("Only for the slskd client: web.authentication.api_keys in slskd.yml"),
        s("slskd_download_path", "Downloads folder (as mounted in this container)", "text").full(),
        s("slskd_preferred_format", "Preferred format", "select").options(&[("flac", "FLAC / lossless"), ("mp3", "MP3"), ("any", "Best available")]),
        s("slskd_min_bitrate", "Minimum MP3 bitrate (kbps)", "number"),
        s("slskd_search_timeout", "Search time (seconds)", "number"),
        s("slskd_stall_minutes", "Give up on a stalled user after (minutes)", "number"),
        s("slskd_max_candidates", "Max users to try per request", "number"),
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
        ("Services", &["spotify", "media", "roon"]),
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
    let reveal = reveal() && truthy(v);   // also for values set by environment variables (read-only, but viewable)
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
        let app_name = body.get("app_name").cloned().unwrap_or(Value::Null);
        match api::put("/admin/settings", Value::Object(body)).await {
            Ok(v) => {
                *self.values.borrow_mut() = v;
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

    // Encore's own API key, at the bottom of General (only the owner may see it on Encore 2)
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
        // only what this server has: Encore 2 (Rust) has Prowlarr instead of Lidarr
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
            let fields_html: String = fields.iter().map(|f| field(f, values.g(f.key), lidarr_opts.as_ref(), &meta)).collect();
            format!(r##"<details class="card settings-card" data-sec="{sid}" {open}>
      <summary class="row between" style="gap:10px;flex-wrap:nowrap;align-items:flex-start"><div style="min-width:0"><h2>{title}</h2>{desc}</div>
        <span class="row" style="gap:8px">{test}<span class="chev">{chev}</span></span></summary>
      <div style="height:14px"></div>
      <div data-out>{flash_html}</div>
      <div class="form-grid">{fields_html}</div>{tidal}
      <div class="row mt" style="justify-content:flex-end"><button class="btn primary" data-save>{check}Save</button></div>
    </details>"##,
                tidal = if sec.id == "streamrip" { tidal_block() } else { String::new() },
                sid = sec.id, open = if open { "open" } else { "" }, title = esc(sec.title), check = icon("check"))
        };
        let mut html = if group.id == "tasks" { r##"<div id="tasks-host" style="margin-bottom:16px"></div>"##.to_string() } else { String::new() };
        for (label, ids) in group.parts {
            let list: Vec<&(&'static Section, Vec<Field>)> = secs.iter().filter(|(sec, _)| ids.contains(&sec.id)).collect();
            if list.is_empty() { continue; }
            if !label.is_empty() { html.push_str(&format!(r##"<div class="nav-label" style="padding:18px 4px 8px">{}</div>"##, esc(label))); }
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
        if let Some(f) = focus { if let Some(c) = dom::qs_in(&hub.panel, &format!(r#"[data-sec="{f}"]"#)) { c.scroll_into_view(); } }

        for card in dom::qsa_in(&hub.panel, "[data-sec]") {
            let sec_id = dom::data(&card, "sec");
            let Some(sec) = SECTIONS.iter().find(|x| x.id == sec_id) else { continue };
            let Some(out) = dom::qs_in(&card, "[data-out]") else { continue };
            if sec.id == "streamrip" { bind_tidal(&hub.view, &card); }
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
    let server = if info.s("server") == "rust" { "Encore 2 (Rust)" } else { "Encore 1 (Python)" };
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

/// Settings → Playback: stream quality, buffering, what happens when the queue ends, and the engine switches of a linked Cadenza device.
fn render_playback(el: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let server_prefs = app::has_feature("theme");
        let st = Rc::new(Look { el, p: RefCell::new(crate::playback::current()), server_prefs });
        paint_playback(&st);
        Ok(())
    })
}

const QUALITY_HELP: [(&str, &str); 3] = [
    ("auto", "The original file. When the connection can't keep up, the rest of the song and the next ten minutes come as the smaller stream, then the original is tried again."),
    ("original", "Always the original file, even if it stutters on a slow connection. Formats a browser can't play are converted to FLAC (lossless)."),
    ("saver", "Always the smaller stream – for mobile data and slow links."),
];

/// `seg()` of the JS renderPlayback: compares as strings, so the numeric kbps matches its button.
fn seg_str(p: &Value, key: &str, opts: &[(&str, &str)]) -> String {
    let cur = js_string(p.g(key));
    format!(r##"<div class="seg" data-key="{key}">{}</div>"##,
        opts.iter().map(|(v, l)| format!(r##"<button type="button" data-v="{v}" class="{}">{l}</button>"##, if cur == *v { "on" } else { "" })).collect::<String>())
}

/// A linked engine device describes its switches in its state: { name, bitperfect, filter, filters[], dither, dithers[], rate, rates[], output }
fn engine_html(e: &Value) -> String {
    let select = |key: &str, label: &str, list: &str, cur: &Value| -> String {
        if !e.g(list).is_array() { return String::new(); }
        let opts: String = e.a(list).iter().map(|o| {
            let (v, l) = (o.items().first().cloned().unwrap_or(Value::Null), o.items().get(1).cloned().unwrap_or(Value::Null));
            format!(r#"<option value="{}" {}>{}</option>"#, esc(&js_string(&v)), if js_string(&v) == js_string(cur) { "selected" } else { "" }, esc(&js_string(&l)))
        }).collect();
        format!(r#"<label class="stack" style="gap:4px"><span class="small muted">{label}</span><select class="input" data-eng="{key}" {}>{opts}</select></label>"#, if e.truthy("bitperfect") { "disabled" } else { "" })
    };
    format!(r#"<div class="stack" style="gap:8px">
      <label class="row" style="gap:8px;align-items:center"><input type="checkbox" data-eng="bitperfect" {}> Bit-perfect – send the file to the DAC untouched (no upsampling, dither or volume)</label>
      {}
      {}
      {}
      {}
    </div>"#,
        if e.truthy("bitperfect") { "checked" } else { "" },
        select("filter", "Upsampling filter", "filters", e.g("filter")),
        select("dither", "Dither / noise shaping", "dithers", e.g("dither")),
        select("rate", "Output rate", "rates", e.g("rate")),
        if e.truthy("output") { format!(r#"<span class="small muted">Output: {}</span>"#, esc(&e.s("output"))) } else { String::new() })
}

fn paint_playback(st: &Rc<Look>) {
    let p = st.p.borrow().clone();
    let link = crate::player::linked();
    let eng = link.as_ref().map(|l| l.state.g("engine").clone()).filter(|e| crate::json::truthy(e));
    let help = QUALITY_HELP.iter().find(|(k, _)| *k == p.s("quality")).map(|(_, h)| *h).unwrap_or("undefined");
    let engine_block = match (&eng, &link) {
        (Some(e), Some(_)) => engine_html(e),
        _ => format!(r#"<span class="small muted">{} A <b>Cadenza</b> device (Encore's open playback engine: exclusive-mode output, bit-perfect switch, upsampling filters, dither and noise shaping) shows its switches here once you link to it under <b>Your devices</b>.</span>"#,
            match &link { Some(l) => format!("{} has no engine switches.", esc(&l.name)), None => "No engine device is linked.".to_string() }),
    };
    dom::set_html(&st.el, &format!(r##"<div class="card stack">
      <h2>Playback</h2>
      <p class="small muted" style="margin:0">{saved}</p>
      <div class="stack" style="gap:6px"><h3>Quality</h3>{quality}
        <span class="small muted">{help}</span></div>
      <div class="stack" style="gap:6px"><h3>Smaller stream</h3>{kbps}
        <span class="small muted">What Auto falls back to and what Data saver always uses. The server makes it from the original on the fly.</span></div>
      <div class="stack" style="gap:6px"><h3>Buffering</h3>
        <label class="row" style="gap:8px;align-items:center"><input type="checkbox" id="pb-preload" {preload}> Pre-load the next song while this one plays</label>
        <span class="small muted">The next song is fetched once the current one is safely buffered, so it starts without a gap. Turn it off on a very slow connection to leave all the bandwidth to the song playing.</span></div>
      <div class="stack" style="gap:6px"><h3>When the queue ends</h3>
        <label class="row" style="gap:8px;align-items:center"><input type="checkbox" id="pb-autoplay" {autoplay}> Keep going with a shuffle of the whole library</label></div>
      <div class="stack" style="gap:6px"><h3>Pure playback</h3>
        <span class="small muted">Encore streams the original file untouched – the quality badge on the player opens the <b>Signal path</b>, which shows what the file is, what was sent and what this device does with it. A browser hands the sound to the operating system, which mixes it with other sounds and may resample it; bit-perfect output needs an exclusive-mode engine.</span></div>
      <div class="stack" style="gap:6px"><h3>Engine{eng_name}</h3>
        {engine_block}</div>
    </div>"##,
        saved = if st.server_prefs { "Saved to your account, so it follows you to every device." } else { "Saved on this device." },
        quality = seg_str(&p, "quality", &[("auto", "Auto"), ("original", "Original"), ("saver", "Data saver")]),
        kbps = seg_str(&p, "kbps", &[("96", "AAC 96 kbps"), ("160", "AAC 160 kbps"), ("256", "AAC 256 kbps")]),
        preload = if p.truthy("preload") { "checked" } else { "" },
        autoplay = if p.truthy("autoplay") { "checked" } else { "" },
        eng_name = match (&eng, &link) { (Some(e), Some(l)) => format!(" · {}", esc(&if e.truthy("name") { e.s("name") } else { l.name.clone() })), _ => String::new() }));
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
    if let Some(l) = link {
        let l = Rc::new(l);
        dom::on_each(&st.el, "[data-eng]", "change", move |c, _| {
            let is_check = c.get_attribute("type").as_deref() == Some("checkbox");
            let v = if is_check { json!(dom::checked(c)) } else { json!(dom::value(c)) };
            (l.send)(json!({ "action": "engine", "set": { dom::data(c, "eng"): v } }));
            toast(&format!("Sent to {}", l.name), "ok");
        });
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
struct Look { el: Element, p: RefCell<Value>, server_prefs: bool }

/// Settings → Appearance: theme, accent colour, faded/solid background.
fn render_appearance(el: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let server_prefs = app::has_feature("theme");
        let st = Rc::new(Look { el, p: RefCell::new(appearance::current()), server_prefs });
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
        let bg = match x.theme { "oled" => "#000", "light" => "#f6f6fb", "dark" => "#0b0b12", _ => "undefined" };
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
        theme = seg(&p, "theme", &[("dark", "Dark"), ("oled", "OLED black"), ("light", "Light"), ("system", "Match device")]),
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

// Tidal downloads need your Tidal account, not an API key: streamrip signs in once through a link
// (device code). The server runs that flow; this block shows the link and waits for the approval.
fn tidal_block() -> String {
    format!(r##"<div class="card flat mt" id="tidal-login" style="padding:12px 14px">
    <div class="row between wrap" style="gap:8px">
      <div style="min-width:0"><b>Tidal account</b><div class="small muted" id="tidal-state">Checking…</div></div>
      <button class="btn sm" id="tidal-signin" type="button">{}Sign in to Tidal</button>
    </div>
    <div class="small mt" id="tidal-out" hidden></div>
  </div>"##, icon("link"))
}

fn bind_tidal(view: &Element, card: &Element) {
    let (Some(state), Some(out), Some(btn)) = (dom::qs_in(card, "#tidal-state"), dom::qs_in(card, "#tidal-out"), dom::qs_in(card, "#tidal-signin")) else { return };
    let paint: Rc<dyn Fn(&Value)> = {
        let (state, out, btn) = (state.clone(), out.clone(), btn.clone());
        Rc::new(move |r: &Value| {
            let signed = r.truthy("signed_in");
            dom::set_text(&state, if signed { "Signed in – Tidal downloads use this account." } else { "Not signed in – Tidal needs a one-time sign-in with your own Tidal account (no API key)." });
            dom::set_html(&btn, &format!("{}{}", icon("link"), if signed { "Sign in again" } else { "Sign in to Tidal" }));
            let status = r.s("status");
            if status == "waiting" && !r.s("url").is_empty() {
                dom::hide(&out, false);
                dom::set_html(&out, &format!(r##"Open <a href="{u}" target="_blank" rel="noopener" style="color:var(--accent)">{u}</a> in a browser, approve in your Tidal account, then come back here. <span class="spinner" style="width:10px;height:10px;border-width:2px"></span> Waiting…"##, u = esc(&r.s("url"))));
            } else if status == "done" {
                dom::hide(&out, false);
                dom::set_html(&out, &format!(r##"<span class="badge ok">{} {}</span>"##, icon("check"), esc(&r.s("message"))));
            } else if status == "failed" {
                dom::hide(&out, false);
                let m = r.s("message");
                dom::set_html(&out, &format!(r##"<span class="badge err">{}</span>"##, esc(if m.is_empty() { "Sign-in failed" } else { &m })));
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
    let (view, card, out2, btn2) = (view.clone(), card.clone(), out.clone(), btn.clone());
    dom::on(&btn, "click", move |_| {
        dom::set_disabled(&btn2, true);
        dom::hide(&out2, false);
        dom::set_html(&out2, r##"<span class="spinner" style="width:10px;height:10px;border-width:2px"></span> Starting streamrip…"##);
        let (view, card, out, btn, paint) = (view.clone(), card.clone(), out2.clone(), btn2.clone(), paint.clone());
        dom::spawn(async move {
            match api::post_empty("/system/streamrip/tidal-login").await {
                Ok(r) => {
                    paint(&r);
                    if r.s("status") == "waiting" {
                        if !r.s("url").is_empty() { let _ = dom::window().open_with_url_and_target_and_features(&r.s("url"), "_blank", "noopener"); }
                        // poll every 3 s until the sign-in finishes or the card is gone
                        let polling = Rc::new(Cell::new(true));
                        let (card, btn, paint, polling2) = (card.clone(), btn.clone(), paint.clone(), polling.clone());
                        dom::interval(&view, 3000, move || {
                            if !polling2.get() || !dom::is_connected(&card) { polling2.set(false); return; }
                            let (btn, paint, polling) = (btn.clone(), paint.clone(), polling2.clone());
                            dom::spawn(async move {
                                if let Ok(r) = api::get("/system/streamrip/tidal-login").await {
                                    paint(&r);
                                    if r.s("status") != "waiting" { polling.set(false); dom::set_disabled(&btn, false); }
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

fn pipeline_help() -> String {
    r##"<div class="card mt"><h3>How requests are fulfilled</h3>
    <ol class="muted" style="line-height:1.9;padding-left:18px;margin:10px 0 0">
      <li><b>Library check</b> – if the album/track is already in your library (by MusicBrainz id or name) it's marked available instantly.</li>
      <li><b>streamrip</b> – lossless from Qobuz, Tidal or Deezer with your own accounts, matched by ISRC.</li>
      <li><b>Torrents & Usenet</b> – Prowlarr searches your indexers; the best album release goes to qBittorrent or SABnzbd and is imported when it finishes. If it stalls past the timeout, the next release or source is tried.</li>
      <li><b>Soulseek</b> – the built-in client (or slskd) searches the network, results are ranked by track count, format, bitrate, folder name and the uploader's free slots/speed. Stalled or failed users are skipped automatically.</li>
      <li><b>yt-dlp</b> – each track from the MusicBrainz tracklist is matched on YouTube (preferring official "Topic" uploads with matching length).</li>
      <li><b>Import</b> – files are matched to the MusicBrainz release, tagged (incl. MBIDs, ISRCs, cover art), renamed with your template, and your media servers are refreshed.</li>
    </ol></div>"##.to_string()
}
