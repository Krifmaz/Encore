//! The admin settings form: every field of web-rs/src/pages/settings.rs `SECTIONS` with the same
//! keys, labels, help, options and `when` rules, the cards they sit in and the nav groups
//! (`GROUPS`) that show them. Copied from the web so both apps render the same form.

/// One `S(key, label, type, extra)` of the web: a setting the server knows.
#[derive(Clone, Copy, Debug)]
pub struct Field {
    pub key: &'static str,
    pub label: &'static str,
    /// `text`, `number`, `password`, `bool`, `select`, `checks`, `textarea`.
    pub kind: &'static str,
    pub help: Option<&'static str>,
    /// Takes the whole row.
    pub full: bool,
    /// A textarea whose saved value is never sent back (WireGuard config, cookies).
    pub secret: bool,
    pub step: Option<&'static str>,
    pub options: &'static [(&'static str, &'static str)],
    /// `when: (key, values)`: shown only while the select `key` has one of the values.
    pub when: Option<(&'static str, &'static [&'static str])>,
}

const fn s(key: &'static str, label: &'static str, kind: &'static str) -> Field {
    Field { key, label, kind, help: None, full: false, secret: false, step: None, options: &[], when: None }
}

impl Field {
    const fn help(mut self, h: &'static str) -> Self {
        self.help = Some(h);
        self
    }
    const fn full(mut self) -> Self {
        self.full = true;
        self
    }
    const fn secret(mut self) -> Self {
        self.secret = true;
        self
    }
    const fn step(mut self, st: &'static str) -> Self {
        self.step = Some(st);
        self
    }
    const fn options(mut self, o: &'static [(&'static str, &'static str)]) -> Self {
        self.options = o;
        self
    }
    const fn when(mut self, key: &'static str, values: &'static [&'static str]) -> Self {
        self.when = Some((key, values));
        self
    }

    /// A password or a secret textarea: the saved value is masked and never echoed back.
    pub fn is_secret(&self) -> bool {
        self.kind == "password" || self.secret
    }

    /// The field takes the whole row of the form grid.
    pub fn spans(&self) -> bool {
        self.full || self.kind == "textarea" || self.kind == "checks"
    }
}

// Settings → Release types: MusicBrainz primary types, then the extras to skip (server: release_types.rs)
pub const RELEASE_KINDS: &[(&str, &str)] = &[("Album", "Album"), ("EP", "EP"), ("Single", "Single"), ("Broadcast", "Broadcast"), ("Other", "Other")];
pub const RELEASE_EXTRAS: &[(&str, &str)] = &[("Compilation", "Compilation"), ("Soundtrack", "Soundtrack"), ("Spokenword", "Spoken word"), ("Interview", "Interview"), ("Audiobook", "Audiobook"),
    ("Audio drama", "Audio drama"), ("Live", "Live"), ("Remix", "Remix"), ("DJ-mix", "DJ-mix"), ("Mixtape/Street", "Mixtape / Street"), ("Demo", "Demo"), ("Field recording", "Field recording"),
    ("Instrumental", "Instrumental"), ("Acoustic", "Acoustic"), ("Sped up", "Sped up / Slowed"), ("Karaoke", "Karaoke"), ("Covers", "Covers / Tributes")];

/// The server's key for a release type: lower case letters and digits only.
pub fn type_key(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit()).collect()
}

/// One card of the form (a `<details>` on the web): a titled group of fields with an optional
/// Test button (`POST /api/system/test/<test>`).
pub struct Card {
    pub id: &'static str,
    pub title: &'static str,
    pub desc: &'static str,
    pub test: Option<&'static str>,
    pub test_label: Option<&'static str>,
    pub fields: &'static [Field],
}

pub const SECTIONS: &[Card] = &[
    Card { id: "general", title: "General", desc: "Your server's name and address, who can sign up, and how requests are approved.", test: None, test_label: None, fields: &[
        s("app_name", "App name", "text"),
        s("base_url", "Public URL", "text").help("e.g. https://music.example.com – used in links & notifications"),
        s("local_url", "Local network address", "text").help("e.g. http://192.168.1.10:8080 – the desktop, phone and TV apps use it instead of the public URL while they're on your home network"),
        s("share_preview", "Shared link preview", "select").options(&[("small", "Small card (small cover)"), ("large", "Big picture"), ("playable", "Playable card (play button and seek bar)")]).help("How shared links look in Discord, X and other chat apps"),
        s("signup_mode", "Sign-up mode", "select").options(&[("invite", "Invite only"), ("open", "Open to anyone"), ("closed", "Closed")]),
        s("auto_approve", "Auto-approve all requests", "bool").help("Otherwise only admins and users with auto-approve skip the approval queue"),
        s("manual_search", "Manual download picker", "select").options(&[("everyone", "Everyone"), ("admins", "Admins only")]).help("Who can search the sources and pick an exact download"),
        s("default_request_limit", "Default weekly request limit for new users", "number").help("0 = unlimited"),
        s("session_days", "Stay signed in for (days)", "number"),
        s("app_session_days", "Keep the Encore app signed in for (days)", "number").help("Counted from the last time the app was used"),
        s("contact_email", "Contact email for MusicBrainz", "text").help("MusicBrainz asks API clients to identify themselves"),
        s("musicbrainz_url", "MusicBrainz server", "text").help("Use your own mirror for faster lookups"),
        s("musicbrainz_rate", "MusicBrainz requests / second", "number").step("0.1").help("Keep at 1 for the public server"),
        s("github_token", "GitHub token (optional)", "password").full().help("For the update checks of the plugins and of Encore itself: GitHub allows 60 checks an hour per address without a token and 5,000 with one. A fine-grained token with no permissions is enough (github.com → Settings → Developer settings); if the Encore repository is private, give the token read access to it."),
    ] },
    // 0.31: the server's settings.toml card `grouplisten`, on Server → General after the General card
    Card { id: "grouplisten", title: "Group listens", desc: "Several people on this server hear the same songs at the same time, each on their own device. Anyone signed in can start one and invite others with a code or a link.", test: None, test_label: None, fields: &[
        s("group_listen", "Allow group listens", "bool").help("Off hides every group listen entry in the apps and ends the ones that are running"),
        s("group_listen_guests", "Let people without an account join with the invite link", "bool").help("The host also switches “Guests can join with the link” on for their group; both must be on. Guests listen on a web page: they can't add songs or control playback, and nothing they hear is logged or scrobbled. Their access ends with the group, when they leave, or after 12 hours."),
        s("group_listen_max", "Most people in one group listen", "number").help("Guests count too. A host can set a lower limit for their group."),
        s("group_listen_keep", "Keep group listens through a server restart", "bool").help("They come back paused where they were, the apps join again by themselves and the host presses play. Off: a restart ends them. Group listens nobody used for 12 hours end either way."),
    ] },
    Card { id: "library", title: "Library & tagging", desc: "Where your music lives, and how downloaded files are tagged, named and organised.", test: None, test_label: None, fields: &[
        s("library_path", "Music library path", "text").help("Inside the container (mount your music folder here)"),
        s("library_extra_roots", "Extra library folders", "textarea").full().help("One folder per line, inside the container. Their songs are in the library where and as they are: Encore never moves, renames or retags files there. Files → Import with “Keep the files where they are” adds a folder here."),
        s("staging_path", "Staging folder for downloads", "text").help("Where downloads (streamrip, yt-dlp, links) are saved before they are tagged and moved into the library"),
        s("scan_interval_minutes", "Rescan library every (minutes)", "number").help("0 disables periodic scans"),
        s("autotag_on_import", "Tag downloads from MusicBrainz on import", "bool"),
        s("autotag_threshold", "Auto-tag confidence threshold", "number").step("0.05").help("0–1. Matches below this are imported with their existing tags"),
        s("rename_on_import", "Rename & organise files", "bool"),
        s("naming_template", "Album naming template", "text").full().help("Fields: {albumartist} {artist} {album} {year} {disc} {discprefix} {track:02d} {title} {genre} {label} {type} {format}"),
        s("single_template", "Single-track naming template", "text").full(),
        s("file_names", "Emoji and symbols in file names", "select").options(&[("keep", "Keep them"), ("words", "Write them as words (♾️ → Infinity)"), ("ascii", "Plain ASCII names only")]).help("Only the file and folder names change; tags and the library keep the real titles. For disks that refuse emoji (exFAT, FAT, some NAS shares)"),
        s("save_cover", "Save cover.jpg in album folders", "bool"),
        s("embed_cover", "Embed cover art in files", "bool"),
    ] },
    Card { id: "pipeline", title: "Download pipeline", desc: "Which sources are searched for a request, and in what order. The first one that has it wins.", test: None, test_label: None, fields: &[
        s("source_order", "Source order", "text").full().help("The one place to set what is searched, in order, comma-separated: qobuz, tidal, deezer (all through streamrip), slskd (Soulseek), torznab (torrents/Usenet, albums only), ytdlp (YouTube Music). Leave one out to never use it; sources you haven’t set up are skipped. Recommended (the default): qobuz, tidal, deezer, slskd, torznab, ytdlp – exact hi-res/lossless releases first, then Soulseek, then torrents/Usenet for whole albums, YouTube Music last. With Highest quality first on (the default), requests try Qobuz/Tidal/Deezer, then FLAC on Soulseek, then torrents/Usenet, then Soulseek in any format, then YouTube Music."),
    ] },
    Card { id: "releases", title: "Release types", desc: "Which releases Request discography, the artist pages and New releases count, like the metadata profiles in Lidarr. Song requests skip other versions (live, remix, sped up and so on) unless the song asked for is one.", test: None, test_label: None, fields: &[
        s("release_types", "Kinds to request", "checks").full().options(RELEASE_KINDS).help("Request discography gets these. Each request can change them in its own panel."),
        s("release_skip", "Skip releases that are", "checks").full().options(RELEASE_EXTRAS).help("A release with any ticked extra is left out. Instrumental, acoustic, sped up, karaoke and covers are read from the titles."),
    ] },
    Card { id: "verify", title: "Verification & duplicates", desc: "Listens to each download and checks it's really the song you asked for, not a remix, live or wrong track.", test: Some("acoustid"), test_label: None, fields: &[
        s("acoustid_api_key", "AcoustID API key", "password").full().help("Free – register an application at acoustid.org/new-application. Used to fingerprint audio and confirm each file is the right song/version."),
        s("verify_on_import", "Fingerprint every download before import", "bool"),
        s("reject_mismatch", "Reject downloads whose audio is a different song/version (and try the next source)", "bool"),
        s("dedupe_on_import", "Never import a second copy of a song (keep the better-quality file)", "bool"),
        s("upgrade_check_audio", "Before replacing a song with a better copy, compare the audio – a different recording is kept as its own song", "bool"),
    ] },
    // 0.33: the server's settings.toml card `translations` (the sources of translated titles), on Server → Library
    Card { id: "translations", title: "Translated titles", desc: "Where translated and romanized titles come from, strongest first: corrections by hand, official names, romanization built into the server, DeepL with your API key, and the local model when the Translator plugin is installed. Each listener turns translations on under Settings → Language, which also shows admins the state of each source.", test: None, test_label: None, fields: &[
        s("translate_official", "Official names from MusicBrainz and Apple", "bool").help("MusicBrainz pseudo-releases and aliases, and Apple's English names for Japanese releases. Asks MusicBrainz and Apple about the songs in your library."),
        s("translate_romanize", "Romanization built into the server", "bool").help("Japanese kana, Korean, Cyrillic, Greek, Hebrew, Arabic, Thai and Devanagari, made on this server. Titles with kanji and Chinese titles are left to the other sources."),
        s("translate_deepl", "DeepL, when an API key is set", "bool").help("Translates song and album titles, never lyrics or artist names."),
        s("deepl_api_key", "DeepL API key", "password").full().help("From deepl.com → Account → API keys. A key ending in :fx is a free-plan key (500,000 characters a month at DeepL)."),
        s("deepl_char_budget", "DeepL characters per month", "number").help("Encore stops asking DeepL for the rest of the month at this many characters. 0 = no limit."),
        s("translate_local_model", "Local model, when it is installed", "bool").help("Translates and romanizes what the other sources leave, on this server, with the Translator plugin (llama.cpp and a model). It is not installed by default: install the Translator in Plugins."),
    ] },
    Card { id: "streamrip", title: "Qobuz / Tidal / Deezer (streamrip)", desc: "Searches Qobuz, Tidal and Deezer with your own accounts and downloads lossless (FLAC). Usually the best quality. streamrip is only used for these three: its SoundCloud support would give the same quality as yt-dlp, so SoundCloud links go through yt-dlp.", test: Some("streamrip"), test_label: Some("Test & write config"), fields: &[
        s("streamrip_enabled", "Enable streamrip source", "bool").help("Downloads lossless from your own paid accounts. Respect each service’s terms – use at your own risk."),
        s("streamrip_quality", "Max quality", "select").options(&[("1", "MP3 320 / AAC"), ("2", "CD – 16-bit FLAC"), ("3", "Hi-res ≤ 24/96"), ("4", "Hi-res max")]),
        s("streamrip_deezer_arl", "Deezer ARL cookie", "password").full().help("From deezer.com → browser dev tools → Cookies → arl"),
        s("streamrip_qobuz_user", "Qobuz user id", "text"),
        s("streamrip_qobuz_token", "Qobuz auth token", "password").help("play.qobuz.com → dev tools → Network → user/login response"),
        s("streamrip_bin", "streamrip executable (optional)", "text").help("Leave empty to auto-detect"),
        s("streamrip_command", "Command template (advanced)", "text").full(),
    ] },
    Card { id: "sync", title: "Playlists & sync", desc: "Keeps liked songs and playlists in step with Spotify, YouTube Music, Jellyfin, Plex and the rest. Runs as the “Sync likes & playlists” task above.", test: None, test_label: None, fields: &[
        s("sync_interval_minutes", "Sync likes & playlists every (minutes)", "number").help("0 disables automatic sync – same as the task’s “every” above"),
        s("auto_request_followed", "Automatically download new tracks in followed playlists (more rules under Admin → Auto-download)", "bool"),
    ] },
    Card { id: "torznab", title: "Torrents & Usenet (Prowlarr)", desc: "Searches your torrent and Usenet indexers (through Prowlarr) for whole albums and sends them to your torrent client (qBittorrent, Transmission, Deluge or rTorrent) or Usenet client (SABnzbd or NZBGet), or drops them into a watch folder.", test: Some("torznab"), test_label: None, fields: &[
        s("torznab_enabled", "Search torrent and Usenet indexers through Prowlarr", "bool").help("Albums and songs: a song is searched on its own and on the album it is on, and from an album only the song's file is downloaded (torrents) or imported (Usenet). Prowlarr searches every indexer you added there; torrents go to the torrent client below, Usenet to the Usenet client."),
        s("prowlarr_url", "Prowlarr URL", "text").help("Internal address, e.g. http://prowlarr:9696"),
        s("prowlarr_api_key", "Prowlarr API key", "password").help("Prowlarr → Settings → General → API Key"),
        s("torznab_min_seeders", "Minimum seeders", "number"),
        s("torznab_timeout_minutes", "Try the next release after a stall of (minutes)", "number"),
        s("import_client_downloads", "Import downloads Encore didn't start", "bool").help("Finished downloads in Encore's category of qBittorrent or SABnzbd that you (or another app) added are imported too, from a copy: the download stays in the client as it is and keeps seeding."),
        s("torrent_client", "Torrent client", "select").options(&[("qbittorrent", "qBittorrent"), ("transmission", "Transmission"), ("deluge", "Deluge"), ("rtorrent", "rTorrent / ruTorrent"), ("blackhole", "Blackhole folder (any client with a watch folder)")]),
        s("qbit_url", "qBittorrent URL", "text").help("e.g. http://qbittorrent:8080 (leave empty to skip torrents)").when("torrent_client", &["qbittorrent"]),
        s("qbit_user", "qBittorrent user", "text").when("torrent_client", &["qbittorrent"]),
        s("qbit_password", "qBittorrent password", "password").when("torrent_client", &["qbittorrent"]),
        s("qbit_category", "qBittorrent category", "text").when("torrent_client", &["qbittorrent"]),
        s("qbit_download_path", "Torrent download folder (same path in both containers)", "text").full().when("torrent_client", &["qbittorrent"]),
        s("transmission_url", "Transmission URL", "text").help("e.g. http://transmission:9091 (leave empty to skip torrents)").when("torrent_client", &["transmission"]),
        s("transmission_user", "Transmission user", "text").help("Leave empty when Transmission asks for no login").when("torrent_client", &["transmission"]),
        s("transmission_password", "Transmission password", "password").when("torrent_client", &["transmission"]),
        s("transmission_label", "Transmission label", "text").help("Transmission 4.0 or newer").when("torrent_client", &["transmission"]),
        s("transmission_download_path", "Torrent download folder (same path in both containers)", "text").full().when("torrent_client", &["transmission"]),
        s("deluge_url", "Deluge Web UI URL", "text").help("e.g. http://deluge:8112 (leave empty to skip torrents)").when("torrent_client", &["deluge"]),
        s("deluge_password", "Deluge Web UI password", "password").when("torrent_client", &["deluge"]),
        s("deluge_label", "Deluge label", "text").help("Needs the Label plugin; skipped when it is off").when("torrent_client", &["deluge"]),
        s("deluge_download_path", "Torrent download folder (same path in both containers)", "text").full().when("torrent_client", &["deluge"]),
        s("rtorrent_url", "rTorrent XML-RPC URL", "text").full().help("e.g. http://rutorrent:8080/RPC2 or http://rutorrent/plugins/rpc/rpc.php (leave empty to skip torrents)").when("torrent_client", &["rtorrent"]),
        s("rtorrent_user", "rTorrent user", "text").help("Leave empty when the address asks for no login").when("torrent_client", &["rtorrent"]),
        s("rtorrent_password", "rTorrent password", "password").when("torrent_client", &["rtorrent"]),
        s("rtorrent_label", "rTorrent label (shown in ruTorrent)", "text").when("torrent_client", &["rtorrent"]),
        s("rtorrent_download_path", "Torrent download folder (same path in both containers)", "text").full().when("torrent_client", &["rtorrent"]),
        s("torrent_blackhole_watch", "Torrent watch folder", "text").full().help("Encore drops .torrent and .magnet files here for your client to pick up").when("torrent_client", &["blackhole"]),
        s("torrent_blackhole_completed", "Finished torrents folder", "text").full().help("Where your client puts finished downloads, as mounted in this container").when("torrent_client", &["blackhole"]),
        s("nzb_client", "Usenet client", "select").options(&[("sabnzbd", "SABnzbd"), ("nzbget", "NZBGet"), ("blackhole", "Blackhole folder (any client with a watch folder)")]),
        s("sab_url", "SABnzbd URL", "text").help("e.g. http://sabnzbd:8080 (leave empty to skip Usenet)").when("nzb_client", &["sabnzbd"]),
        s("sab_api_key", "SABnzbd API key", "password").when("nzb_client", &["sabnzbd"]),
        s("sab_category", "SABnzbd category", "text").when("nzb_client", &["sabnzbd"]),
        s("sab_download_path", "Usenet download folder (as mounted in this container)", "text").full().when("nzb_client", &["sabnzbd"]),
        s("nzbget_url", "NZBGet URL", "text").help("e.g. http://nzbget:6789 (leave empty to skip Usenet)").when("nzb_client", &["nzbget"]),
        s("nzbget_user", "NZBGet user", "text").when("nzb_client", &["nzbget"]),
        s("nzbget_password", "NZBGet password", "password").when("nzb_client", &["nzbget"]),
        s("nzbget_category", "NZBGet category", "text").when("nzb_client", &["nzbget"]),
        s("nzbget_download_path", "Usenet download folder (as mounted in this container)", "text").full().when("nzb_client", &["nzbget"]),
        s("nzb_blackhole_watch", "Usenet watch folder", "text").full().help("Encore drops .nzb files here for your client to pick up").when("nzb_client", &["blackhole"]),
        s("nzb_blackhole_completed", "Finished Usenet downloads folder", "text").full().help("Where your client puts finished downloads, as mounted in this container").when("nzb_client", &["blackhole"]),
    ] },
    Card { id: "slskd", title: "Soulseek", desc: "Searches the Soulseek file-sharing network – great for FLAC and rare releases. Encore runs Soulseek for you: enter any username and password and an account is created at the first sign-in. Forward the port for the best speeds.", test: Some("slskd"), test_label: None, fields: &[
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
    Card { id: "ytdlp", title: "YouTube Music & links (yt-dlp)", desc: "Last-resort source: finds the song on YouTube Music. yt-dlp also downloads SoundCloud links you paste in the Downloader, along with YouTube, Bandcamp, Mixcloud and about 1,800 other sites.", test: None, test_label: None, fields: &[
        s("ytdlp_enabled", "Enable yt-dlp as a source", "bool").help("Searches YouTube Music for requests, and downloads links you paste in the Downloader from YouTube, SoundCloud, Bandcamp, Mixcloud and ~1,800 other sites."),
        s("ytdlp_last_resort", "Always try YouTube Music last, so downloads (almost) never fail", "bool"),
        s("ytdlp_format", "Audio format", "select").options(&[("best", "Original stream – best quality, no re-encode (recommended)"), ("m4a", "M4A (AAC, re-encoded)"), ("mp3", "MP3 (re-encoded)"), ("opus", "Opus")]),
        s("ytdlp_quality", "MP3/M4A quality", "text").help("Bitrate like 320, or 0 for best VBR"),
    ] },
    Card { id: "youtube", title: "YouTube sign-in", desc: "Signs yt-dlp in to YouTube: YouTube Music Premium unlocks 256 kbps audio, and signed-in downloads see fewer bot checks. Best: connect the Encore browser extension and hand over a sign-in from a private window – Encore then owns that session and keeps it alive by itself. Pasted cookies are the backup.", test: Some("youtube"), test_label: Some("Test cookies"), fields: &[
        s("ytdlp_cookies", "Pasted cookies.txt (backup)", "textarea").full().secret().help("Netscape cookie file exported from music.youtube.com. Used only when there is no signed-in hand-off or extension sync. Export from a private window and close it right after, so your own browser does not rotate them."),
        s("youtube_light_refresh", "Refresh pasted cookies every 4 hours", "bool").help("Only for pasted cookies while they are in use: Encore renews them the way an open YouTube tab does and saves the new ones. A handed-over sign-in is always refreshed every 4 hours; an everyday browser’s cookies never are (that would sign that browser out)."),
    ] },
    Card { id: "spotify", title: "Integrations", desc: "API keys that let users connect their Spotify, Tidal, Last.fm, Discogs and YouTube Music accounts, and power recommendations.", test: Some("spotify"), test_label: Some("Test all"), fields: &[
        s("spotify_client_id", "Spotify Client ID", "text").help("developer.spotify.com – add <Public URL>/api/connections/spotify/callback as redirect URI. Development-mode apps allow 5 users and need a Premium owner (2026 rules); without it, public playlists still import via the embed fallback."),
        s("spotify_client_secret", "Spotify Client secret", "password"),
        s("tidal_client_id", "Tidal Client ID", "text").help("Only for looking up Tidal albums/songs by ISRC and for the Tidal connection. Get one at developer.tidal.com → Dashboard → Create new app (any name), redirect URI <Public URL>/api/connections/tidal/callback, then copy the Client ID and Client secret here. Downloads from Tidal use your own account instead: Plugins → streamrip → Sign in to Tidal."),
        s("tidal_client_secret", "Tidal Client secret", "password"),
        s("lastfm_api_key", "Last.fm API key", "password").help("last.fm/api/account/create (free) – loved tracks, similar artists/songs, genre pages, For you"),
        s("lastfm_shared_secret", "Last.fm shared secret", "password").help("Shown next to the API key. Needed for scrobbling: users sign in with Last.fm on the Connections page."),
        s("discogs_token", "Discogs personal access token", "password").help("discogs.com/settings/developers → “Generate new token” and paste that personal access token here (not an application’s Consumer Key/Secret – no app is needed). Finds vinyl, bootlegs and old releases MusicBrainz is missing."),
        s("ytmusic_client_id", "YouTube Music OAuth Client ID", "text").help("Google Cloud → YouTube Data API v3 → OAuth client of type “TVs and Limited Input devices”. Lets users sign in to YouTube Music with a code (they can also paste browser headers instead)."),
        s("ytmusic_client_secret", "YouTube Music OAuth Client secret", "password"),
    ] },
    Card { id: "media", title: "Media servers", desc: "Tells Navidrome, Jellyfin or Plex to rescan after new music arrives, so it shows up there right away.", test: Some("mediaservers"), test_label: Some("Trigger refresh"), fields: &[
        s("navidrome_url", "Navidrome URL", "text").help("URLs here let users link their own accounts on the Connections page; the admin credentials below are only used to trigger library rescans"), s("navidrome_user", "Navidrome user", "text"), s("navidrome_password", "Navidrome password", "password"),
        s("jellyfin_url", "Jellyfin URL", "text"), s("jellyfin_api_key", "Jellyfin API key", "password"),
        s("plex_url", "Plex URL", "text"), s("plex_token", "Plex token", "password"),
    ] },
    Card { id: "roon", title: "Roon", desc: "Encore as a Roon extension: your Roon zones show up under Your devices with what they play, transport and volume, and “Play in Roon” starts a search result in a zone. Enable it here, then approve Encore once in Roon → Settings → Extensions.", test: Some("roon"), test_label: Some("Test connection"), fields: &[
        s("roon_enabled", "Enable Roon", "bool").help("Roon Core (or Nucleus / ROCK) on the same network. Roon 1.8 or newer."),
        s("roon_host", "Roon Core address", "text").help("Leave empty to find the Core on the network. Running Encore in Docker? Multicast does not cross the bridge network – enter the Core’s IP here (add :port if it is not 9330)."),
    ] },
    Card { id: "cadenza", title: "Server processing (Cadenza)", desc: "Lets permitted users have their streams upsampled on this server by Cadenza (long-filter upsampling, dither and noise shaping, lossless 24-bit FLAC out). Off by default: it costs real processor time per stream. Allow users under People → Users & permissions.", test: Some("cadenza"), test_label: Some("Test"), fields: &[
        s("cadenza_server_enabled", "Enable server processing", "bool"),
        s("cadenza_max_streams", "Streams at once", "number").help("Processed streams the whole server runs at the same time; more are played as the original."),
        s("cadenza_max_rate", "Highest output rate", "select").options(&[("96000", "96 kHz"), ("192000", "192 kHz"), ("384000", "384 kHz"), ("768000", "768 kHz")]),
        s("cadenza_max_taps", "Longest filter", "select").options(&[("16384", "16k taps (light)"), ("65536", "64k taps"), ("262144", "256k taps"), ("1048576", "1M taps (heavy)")]),
        s("cadenza_cache_mb", "Cache size (MB)", "number").help("Each song is rendered once and kept on disk, so seeking and skipping need no new processing and the next song is ready before it plays. The least recently played renders make room. 0 = no cache (every stream is processed live)."),
    ] },
    Card { id: "email", title: "Email", desc: "Lets Encore send password-reset and sign-in emails.", test: Some("email"), test_label: Some("Send test email"), fields: &[
        s("smtp_host", "SMTP server", "text").help("e.g. smtp.fastmail.com – used for password-reset emails"),
        s("smtp_port", "Port", "number").help("587 (STARTTLS) or 465 (SSL)"),
        s("smtp_user", "Username", "text"),
        s("smtp_password", "Password", "password"),
        s("smtp_from", "From address", "text").help("e.g. Encore <music@example.com>"),
        s("smtp_security", "Security", "select").options(&[("starttls", "STARTTLS"), ("ssl", "SSL/TLS"), ("none", "None")]),
    ] },
    Card { id: "notify", title: "Notifications", desc: "Get a message on Discord, ntfy or any webhook when downloads finish or fail.", test: Some("notify"), test_label: Some("Send test"), fields: &[
        s("discord_webhook", "Discord webhook URL", "password").full(),
        s("ntfy_url", "ntfy topic URL", "text").help("e.g. https://ntfy.sh/my-music"),
        s("ntfy_token", "ntfy access token", "password"),
        s("webhook_url", "Generic webhook (JSON POST)", "password").full(),
    ] },
];

/// One nav entry of the Server group; `parts` splits a long group into labelled runs of cards.
pub struct Group {
    pub id: &'static str,
    pub parts: &'static [(&'static str, &'static [&'static str])],
}

impl Group {
    /// The card ids of every part, in order.
    pub fn sections(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.parts.iter().flat_map(|(_, ids)| ids.iter().copied())
    }
}

pub const GROUPS: &[Group] = &[
    Group { id: "general", parts: &[("", &["general", "grouplisten"])] },
    // Apps & sources until R2, now four pages (the server's settings.toml sections)
    Group { id: "library", parts: &[("", &["library", "verify", "translations"])] },
    // 0.32: the source cards are plugin cards on Settings → Plugins (plugins.rs); see LEGACY_SOURCES
    Group { id: "downloads", parts: &[("", &["pipeline", "releases"])] },
    Group { id: "integrations", parts: &[("", &["spotify", "media", "roon"])] },
    Group { id: "processing", parts: &[("", &["cadenza"])] },
    Group { id: "accounts", parts: &[("", &["email", "notify"])] },
    Group { id: "tasks", parts: &[("", &["sync"])] },
];

/// The source cards Downloads had before 0.32, still drawn there for a server without plugins.
pub const LEGACY_SOURCES: &[&str] = &["streamrip", "torznab", "slskd", "ytdlp", "youtube"];

/// The card with this id.
pub fn card(id: &str) -> Option<&'static Card> {
    SECTIONS.iter().find(|c| c.id == id)
}

/// The group with this id.
pub fn group(id: &str) -> Option<&'static Group> {
    GROUPS.iter().find(|g| g.id == id)
}

/// The group a card belongs to (`releases` → `downloads`).
pub fn group_of(card_id: &str) -> Option<&'static Group> {
    GROUPS.iter().find(|g| g.sections().any(|s| s == card_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_matches_the_web() {
        assert_eq!(SECTIONS.len(), 19);
        assert_eq!(GROUPS.iter().map(|g| g.id).collect::<Vec<_>>(), ["general", "library", "downloads", "integrations", "processing", "accounts", "tasks"]);
        let general = card("general").unwrap();
        assert_eq!(general.fields[0].key, "app_name");
        assert_eq!(general.fields.last().unwrap().kind, "password");
        assert!(general.fields.last().unwrap().full);
        let t = card("torznab").unwrap();
        let qbit = t.fields.iter().find(|f| f.key == "qbit_url").unwrap();
        assert_eq!(qbit.when, Some(("torrent_client", &["qbittorrent"][..])));
        let slskd = card("slskd").unwrap();
        let wg = slskd.fields.iter().find(|f| f.key == "vpn_wireguard_config").unwrap();
        assert!(wg.is_secret() && wg.spans() && wg.kind == "textarea");
        assert_eq!(group_of("cadenza").unwrap().id, "processing");
        assert_eq!(group_of("verify").unwrap().id, "library");
        assert_eq!(group_of("releases").unwrap().id, "downloads");
        assert_eq!(group_of("translations").unwrap().id, "library");
        assert!(group_of("torznab").is_none(), "a plugin card since 0.32");
        for id in LEGACY_SOURCES {
            assert!(card(id).is_some(), "{id}");
        }
        assert_eq!(group_of("roon").unwrap().id, "integrations");
        assert_eq!(group_of("sync").unwrap().id, "tasks");
        assert_eq!(group_of("grouplisten").unwrap().id, "general");
        assert_eq!(card("grouplisten").unwrap().fields.iter().map(|f| f.key).collect::<Vec<_>>(), ["group_listen", "group_listen_guests", "group_listen_max", "group_listen_keep"]);
        assert!(group_of("nowhere").is_none());
        // every card of every group exists
        for g in GROUPS {
            for id in g.sections() {
                assert!(card(id).is_some(), "{id}");
            }
        }
        assert_eq!(type_key("Mixtape/Street"), "mixtapestreet");
        assert_eq!(type_key("DJ-mix"), "djmix");
    }
}
