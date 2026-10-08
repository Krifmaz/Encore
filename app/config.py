"""Settings stored in the database, seeded from environment variables."""
import json
import os
import secrets

from . import db

# key: (env var, default, secret?)
DEFAULTS: dict[str, tuple[str | None, object, bool]] = {
    # general
    "app_name": ("ENCORE_APP_NAME", "Encore", False),
    "base_url": ("ENCORE_BASE_URL", "", False),
    "signup_mode": ("ENCORE_SIGNUP_MODE", "invite", False),  # invite | open | closed
    "auto_approve": ("ENCORE_AUTO_APPROVE", False, False),
    "manual_search": ("ENCORE_MANUAL_SEARCH", "everyone", False),   # who may use the manual download picker: everyone | admins
    "default_request_limit": ("ENCORE_REQUEST_LIMIT", 0, False),  # per 7 days, 0 = unlimited
    "session_days": ("ENCORE_SESSION_DAYS", 30, False),
    # library
    "library_path": ("ENCORE_LIBRARY_PATH", "/music", False),
    "staging_path": ("ENCORE_STAGING_PATH", "/downloads/encore", False),
    "scan_interval_minutes": ("ENCORE_SCAN_INTERVAL", 60, False),
    "rename_on_import": ("ENCORE_RENAME_ON_IMPORT", True, False),
    "naming_template": (
        "ENCORE_NAMING_TEMPLATE",
        "{albumartist}/{album} ({year})/{discprefix}{track:02d} - {title}",
        False,
    ),
    "single_template": ("ENCORE_SINGLE_TEMPLATE", "{artist}/Singles/{artist} - {title}", False),
    "autotag_on_import": ("ENCORE_AUTOTAG", True, False),
    "autotag_threshold": ("ENCORE_AUTOTAG_THRESHOLD", 0.8, False),
    "save_cover": ("ENCORE_SAVE_COVER", True, False),
    "embed_cover": ("ENCORE_EMBED_COVER", True, False),
    # auto-download (admin → Auto-download)
    "album_upgrade": ("ENCORE_ALBUM_UPGRADE", "album_ep", False),     # off | album | album_ep
    "quality_first": ("ENCORE_QUALITY_FIRST", True, False),           # lossless sources before lossy ones
    "auto_request_likes": ("ENCORE_AUTO_REQUEST_LIKES", False, False),
    "auto_request_follows": ("ENCORE_AUTO_REQUEST_FOLLOWS", True, False),  # new releases of followed artists
    "follow_check_hours": ("ENCORE_FOLLOW_CHECK_HOURS", 12, False),
    "discover_auto_count": ("ENCORE_DISCOVER_AUTO_COUNT", 0, False),   # top N new songs from each Weekly Exploration (0 = off)
    "discover_catalog_merge": ("ENCORE_CATALOG_MERGE", True, False),   # show Deezer/Apple releases MusicBrainz lacks
    "upgrade_lossy": ("ENCORE_UPGRADE_LOSSY", False, False),           # periodically look for lossless copies
    "upgrade_lossy_days": ("ENCORE_UPGRADE_LOSSY_DAYS", 7, False),
    # verification
    "acoustid_api_key": ("ACOUSTID_API_KEY", "", True),
    "verify_on_import": ("ENCORE_VERIFY_ON_IMPORT", True, False),
    "reject_mismatch": ("ENCORE_REJECT_MISMATCH", True, False),
    "dedupe_on_import": ("ENCORE_DEDUPE_ON_IMPORT", True, False),
    # pipeline
    "source_order": ("ENCORE_SOURCE_ORDER", "lidarr,streamrip,slskd,ytdlp", False),
    # streamrip (Qobuz / Tidal / Deezer with your own subscription)
    "streamrip_enabled": ("STREAMRIP_ENABLED", False, False),
    "streamrip_bin": ("STREAMRIP_BIN", "", False),
    "streamrip_sources": ("STREAMRIP_SOURCES", "deezer,tidal", False),
    "streamrip_quality": ("STREAMRIP_QUALITY", 2, False),
    "streamrip_command": ("STREAMRIP_COMMAND", "{bin} --config-path {config} --folder {folder} --quality {quality} --no-db --no-progress url {url}", False),
    "streamrip_deezer_arl": ("STREAMRIP_DEEZER_ARL", "", True),
    "streamrip_qobuz_user": ("STREAMRIP_QOBUZ_USER", "", False),
    "streamrip_qobuz_token": ("STREAMRIP_QOBUZ_TOKEN", "", True),
    # musicbrainz
    "musicbrainz_url": ("MUSICBRAINZ_URL", "https://musicbrainz.org", False),
    "musicbrainz_rate": ("MUSICBRAINZ_RATE", 1.0, False),
    "contact_email": ("ENCORE_CONTACT_EMAIL", "", False),
    # lidarr
    "lidarr_enabled": ("LIDARR_ENABLED", False, False),
    "lidarr_url": ("LIDARR_URL", "http://lidarr:8686", False),
    "lidarr_api_key": ("LIDARR_API_KEY", "", True),
    "lidarr_root_folder": ("LIDARR_ROOT_FOLDER", "", False),
    "lidarr_quality_profile": ("LIDARR_QUALITY_PROFILE", 0, False),
    "lidarr_metadata_profile": ("LIDARR_METADATA_PROFILE", 0, False),
    "lidarr_timeout_minutes": ("LIDARR_TIMEOUT", 60, False),
    # slskd
    "slskd_enabled": ("SLSKD_ENABLED", False, False),
    "slskd_url": ("SLSKD_URL", "http://slskd:5030", False),
    "slskd_api_key": ("SLSKD_API_KEY", "", True),
    "slskd_download_path": ("SLSKD_DOWNLOAD_PATH", "/downloads/slskd", False),
    "slskd_preferred_format": ("SLSKD_PREFERRED_FORMAT", "flac", False),  # flac | mp3 | any
    "slskd_min_bitrate": ("SLSKD_MIN_BITRATE", 192, False),
    "slskd_search_timeout": ("SLSKD_SEARCH_TIMEOUT", 15, False),
    "slskd_stall_minutes": ("SLSKD_STALL_MINUTES", 20, False),
    "slskd_max_candidates": ("SLSKD_MAX_CANDIDATES", 4, False),
    # yt-dlp
    "ytdlp_enabled": ("YTDLP_ENABLED", True, False),
    "ytdlp_format": ("YTDLP_FORMAT", "best", False),  # best (original stream) | mp3 | m4a | opus
    "ytdlp_quality": ("YTDLP_QUALITY", "320", False),
    "ytdlp_cookies": ("YTDLP_COOKIES", "", True),
    # streaming services (playlist import / account connections)
    "spotify_client_id": ("SPOTIFY_CLIENT_ID", "", False),
    "spotify_client_secret": ("SPOTIFY_CLIENT_SECRET", "", True),
    "tidal_client_id": ("TIDAL_CLIENT_ID", "", False),
    "tidal_client_secret": ("TIDAL_CLIENT_SECRET", "", True),
    "lastfm_api_key": ("LASTFM_API_KEY", "", False),
    "discogs_token": ("DISCOGS_TOKEN", "", True),
    "ytmusic_client_id": ("YTMUSIC_CLIENT_ID", "", False),
    "ytmusic_client_secret": ("YTMUSIC_CLIENT_SECRET", "", True),
    # sync
    "sync_interval_minutes": ("ENCORE_SYNC_INTERVAL", 15, False),
    "auto_request_followed": ("ENCORE_AUTO_REQUEST_FOLLOWED", False, False),
    "plex_client_id": (None, "", False),
    # media servers
    "navidrome_url": ("NAVIDROME_URL", "", False),
    "navidrome_user": ("NAVIDROME_USER", "", False),
    "navidrome_password": ("NAVIDROME_PASSWORD", "", True),
    "jellyfin_url": ("JELLYFIN_URL", "", False),
    "jellyfin_api_key": ("JELLYFIN_API_KEY", "", True),
    "plex_url": ("PLEX_URL", "", False),
    "plex_token": ("PLEX_TOKEN", "", True),
    # notifications
    "discord_webhook": ("DISCORD_WEBHOOK", "", True),
    "ntfy_url": ("NTFY_URL", "", False),
    "ntfy_token": ("NTFY_TOKEN", "", True),
    "webhook_url": ("WEBHOOK_URL", "", True),
    # api
    "api_key": (None, "", True),
}

_cache: dict[str, object] = {}


def _coerce(default, raw):
    if isinstance(default, bool):
        if isinstance(raw, bool):
            return raw
        return str(raw).strip().lower() in ("1", "true", "yes", "on")
    if isinstance(default, int):
        try:
            return int(float(raw))
        except (TypeError, ValueError):
            return default
    if isinstance(default, float):
        try:
            return float(raw)
        except (TypeError, ValueError):
            return default
    return "" if raw is None else str(raw)


def _clean_env(raw: str) -> str:
    """Compose list syntax keeps quotes literally (- KEY="abc"), and copy/paste adds whitespace."""
    v = raw.strip()
    if len(v) >= 2 and v[0] == v[-1] and v[0] in "\"'":
        v = v[1:-1].strip()
    return v


_env: dict[str, object] = {}


def load() -> None:
    """Environment variables always win (shown as locked in the UI); otherwise the value saved in the UI."""
    stored = {r["key"]: json.loads(r["value"]) for r in db.query("SELECT key, value FROM settings")}
    _env.clear()
    for key, (env, default, _) in DEFAULTS.items():
        raw = os.environ.get(env) if env else None
        if raw is not None and _clean_env(raw) != "":
            _env[key] = _coerce(default, _clean_env(raw))
        _cache[key] = _coerce(default, stored[key]) if key in stored else default
    if not _cache.get("api_key"):
        set_many({"api_key": secrets.token_hex(24)})


def get(key: str):
    if key in _env:
        return _env[key]
    return _cache.get(key, DEFAULTS.get(key, (None, None, False))[1])


def source(key: str) -> str:
    if key in _env:
        return "env"
    return "ui" if db.one("SELECT 1 FROM settings WHERE key=?", (key,)) else "default"


def env_locked(key: str) -> bool:
    return key in _env


def set_many(values: dict) -> None:
    for key, val in values.items():
        if key not in DEFAULTS:
            continue
        val = _coerce(DEFAULTS[key][1], val)
        _cache[key] = val
        db.execute("INSERT OR REPLACE INTO settings VALUES (?,?)", (key, json.dumps(val)))


MASK = "••••"


def mask(val) -> str:
    """Secrets are never sent to the browser; show length + last 4 so you can tell which key is loaded."""
    s = str(val or "")
    if not s:
        return ""
    tail = s[-4:] if len(s) >= 12 else ""
    return f"{MASK}{tail} ({len(s)} chars)"


def is_masked(v) -> bool:
    return isinstance(v, str) and v.startswith(MASK)


def public_dump() -> dict:
    """All settings (secrets masked) plus where each value comes from."""
    out = {}
    meta = {}
    saved = {r["key"] for r in db.query("SELECT key FROM settings")}
    for key, (env, _, secret) in DEFAULTS.items():
        val = get(key)
        out[key] = mask(val) if secret and key != "api_key" else val
        meta[key] = {"source": "env" if key in _env else "ui" if key in saved else "default", "env": env}
    out["_meta"] = meta
    return out
