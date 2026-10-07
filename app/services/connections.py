"""Per-user account connections (streaming services + media servers) and their credentials."""
import base64
import hashlib
import json
import secrets
import time
import uuid

from .. import config, db
from .http import client

SERVICES = {
    # service: (label, kind) kind: media = can push likes/playlists, stream = import source, scrobble = ListenBrainz/Last.fm
    "jellyfin": ("Jellyfin / Finamp", "media"),
    "plex": ("Plex / Plexamp", "media"),
    "navidrome": ("Navidrome / Subsonic", "media"),
    "ytmusic": ("YouTube Music", "stream"),
    "spotify": ("Spotify", "stream"),
    "tidal": ("Tidal", "stream"),
    "deezer": ("Deezer", "stream"),
    "listenbrainz": ("ListenBrainz", "scrobble"),
    "lastfm": ("Last.fm", "scrobble"),
}


class ConnectionError_(Exception):
    pass


def get(user_id: int, service: str) -> dict | None:
    c = db.one("SELECT * FROM connections WHERE user_id=? AND service=?", (user_id, service))
    if c:
        c["data"] = json.loads(c["data"] or "{}")
    return c


def list_for(user_id: int) -> list[dict]:
    rows = db.query("SELECT * FROM connections WHERE user_id=? ORDER BY service", (user_id,))
    for r in rows:
        r["data"] = json.loads(r["data"] or "{}")
    return rows


def save(user_id: int, service: str, account: str, data: dict) -> int:
    existing = get(user_id, service)
    if existing:
        db.execute("UPDATE connections SET account=?, data=?, last_error=NULL WHERE id=?", (account, json.dumps(data), existing["id"]))
        return existing["id"]
    return db.execute("INSERT INTO connections (user_id, service, account, data, created_at) VALUES (?,?,?,?,?)",
                      (user_id, service, account, json.dumps(data), time.time()))


def update_data(conn_id: int, data: dict) -> None:
    db.execute("UPDATE connections SET data=? WHERE id=?", (json.dumps(data), conn_id))


def public(c: dict) -> dict:
    return {k: c[k] for k in ("id", "service", "account", "sync_likes", "sync_playlists", "last_sync", "last_error", "created_at")} | {
        "label": SERVICES.get(c["service"], (c["service"],))[0], "kind": SERVICES.get(c["service"], ("", ""))[1]}


def redirect_uri(service: str) -> str:
    base = (config.get("base_url") or "").rstrip("/")
    if not base:
        raise ConnectionError_("Set the Public URL in Settings → General first (OAuth needs a callback address)")
    return f"{base}/api/connections/{service}/callback"


# ---- OAuth (Spotify / Tidal, PKCE) ---------------------------------------------

_pending: dict[str, dict] = {}


def _pkce() -> tuple[str, str]:
    verifier = secrets.token_urlsafe(64)[:96]
    challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).decode().rstrip("=")
    return verifier, challenge


def oauth_start(user_id: int, service: str) -> str:
    verifier, challenge = _pkce()
    state = secrets.token_urlsafe(24)
    for k in [k for k, v in _pending.items() if v["ts"] < time.time() - 900]:
        _pending.pop(k, None)
    _pending[state] = {"user_id": user_id, "service": service, "verifier": verifier, "ts": time.time()}
    from urllib.parse import urlencode
    if service == "spotify":
        if not config.get("spotify_client_id"):
            raise ConnectionError_("The admin needs to add a Spotify Client ID in Settings → Integrations")
        q = {"client_id": config.get("spotify_client_id"), "response_type": "code", "redirect_uri": redirect_uri("spotify"),
             "code_challenge_method": "S256", "code_challenge": challenge, "state": state,
             "scope": "playlist-read-private playlist-read-collaborative user-library-read"}
        return "https://accounts.spotify.com/authorize?" + urlencode(q)
    if service == "tidal":
        if not config.get("tidal_client_id"):
            raise ConnectionError_("The admin needs to add a Tidal Client ID in Settings → Integrations")
        q = {"client_id": config.get("tidal_client_id"), "response_type": "code", "redirect_uri": redirect_uri("tidal"),
             "code_challenge_method": "S256", "code_challenge": challenge, "state": state,
             "scope": "user.read collection.read playlists.read"}
        return "https://login.tidal.com/authorize?" + urlencode(q)
    raise ConnectionError_("Unsupported OAuth service")


async def oauth_finish(service: str, code: str, state: str, user_id: int) -> None:
    p = _pending.pop(state, None)
    if not p or p["service"] != service or p["user_id"] != user_id:
        raise ConnectionError_("Login expired or was started in another session – try again")
    if service == "spotify":
        r = await client().post("https://accounts.spotify.com/api/token", data={
            "grant_type": "authorization_code", "code": code, "redirect_uri": redirect_uri("spotify"),
            "client_id": config.get("spotify_client_id"), "code_verifier": p["verifier"]})
        tok = r.json()
        if "access_token" not in tok:
            raise ConnectionError_(f"Spotify: {tok.get('error_description') or tok}")
        me = (await client().get("https://api.spotify.com/v1/me", headers={"Authorization": f"Bearer {tok['access_token']}"})).json()
        save(user_id, "spotify", me.get("display_name") or me.get("id") or "Spotify", {
            "access_token": tok["access_token"], "refresh_token": tok.get("refresh_token"),
            "expires": time.time() + tok.get("expires_in", 3600), "user_id": me.get("id")})
    elif service == "tidal":
        r = await client().post("https://auth.tidal.com/v1/oauth2/token", data={
            "grant_type": "authorization_code", "code": code, "redirect_uri": redirect_uri("tidal"),
            "client_id": config.get("tidal_client_id"), "code_verifier": p["verifier"]})
        tok = r.json()
        if "access_token" not in tok:
            raise ConnectionError_(f"Tidal: {tok.get('error_description') or tok}")
        uid = str(tok.get("user_id") or (tok.get("user") or {}).get("userId") or "")
        save(user_id, "tidal", (tok.get("user") or {}).get("username") or uid or "Tidal", {
            "access_token": tok["access_token"], "refresh_token": tok.get("refresh_token"),
            "expires": time.time() + tok.get("expires_in", 3600), "user_id": uid,
            "country": (tok.get("user") or {}).get("countryCode") or "US"})


async def _refresh(conn: dict, url: str, client_id: str) -> str | None:
    d = conn["data"]
    if d.get("expires", 0) > time.time() + 60:
        return d["access_token"]
    if not d.get("refresh_token"):
        return None
    r = await client().post(url, data={"grant_type": "refresh_token", "refresh_token": d["refresh_token"], "client_id": client_id})
    tok = r.json()
    if "access_token" not in tok:
        db.execute("UPDATE connections SET last_error=? WHERE id=?", ("Login expired – reconnect", conn["id"]))
        return None
    d.update(access_token=tok["access_token"], expires=time.time() + tok.get("expires_in", 3600))
    if tok.get("refresh_token"):
        d["refresh_token"] = tok["refresh_token"]
    update_data(conn["id"], d)
    return d["access_token"]


async def spotify_token(user_id: int) -> str | None:
    c = get(user_id, "spotify")
    return await _refresh(c, "https://accounts.spotify.com/api/token", config.get("spotify_client_id")) if c else None


async def tidal_token(user_id: int) -> str | None:
    c = get(user_id, "tidal")
    return await _refresh(c, "https://auth.tidal.com/v1/oauth2/token", config.get("tidal_client_id")) if c else None


# ---- Jellyfin -------------------------------------------------------------------

def jellyfin_auth_header(token: str | None = None, device: str = "encore") -> dict:
    v = f'MediaBrowser Client="Encore", Device="Encore", DeviceId="{device}", Version="1.0"'
    if token:
        v += f', Token="{token}"'
    h = {"Authorization": v, "X-Emby-Authorization": v}
    if token:
        h["X-Emby-Token"] = token
    return h


async def connect_jellyfin(user_id: int, username: str, password: str) -> dict:
    url = (config.get("jellyfin_url") or "").rstrip("/")
    if not url:
        raise ConnectionError_("The admin hasn't set a Jellyfin URL in Settings → Media servers")
    r = await client().post(f"{url}/Users/AuthenticateByName", json={"Username": username, "Pw": password},
                            headers=jellyfin_auth_header(device=f"encore-{user_id}"))
    if r.status_code != 200:
        raise ConnectionError_("Jellyfin rejected that username/password")
    d = r.json()
    save(user_id, "jellyfin", d["User"]["Name"], {"token": d["AccessToken"], "user_id": d["User"]["Id"]})
    return {"account": d["User"]["Name"]}


# ---- Plex (PIN sign-in) ------------------------------------------------------------

def plex_client_id() -> str:
    cid = config.get("plex_client_id")
    if not cid:
        cid = str(uuid.uuid4())
        config.set_many({"plex_client_id": cid})
    return cid


def plex_headers(token: str | None = None) -> dict:
    h = {"X-Plex-Product": "Encore", "X-Plex-Client-Identifier": plex_client_id(), "X-Plex-Version": "1.0", "Accept": "application/json"}
    if token:
        h["X-Plex-Token"] = token
    return h


async def plex_pin_start() -> dict:
    r = await client().post("https://plex.tv/api/v2/pins", params={"strong": "true"}, headers=plex_headers())
    d = r.json()
    from urllib.parse import urlencode
    q = {"clientID": plex_client_id(), "code": d["code"], "context[device][product]": "Encore"}
    if config.get("base_url"):
        q["forwardUrl"] = config.get("base_url").rstrip("/") + "/#/connections"
    return {"pin_id": d["id"], "url": "https://app.plex.tv/auth#?" + urlencode(q)}


async def plex_pin_check(user_id: int, pin_id: int) -> dict:
    r = await client().get(f"https://plex.tv/api/v2/pins/{pin_id}", headers=plex_headers())
    tok = r.json().get("authToken")
    if not tok:
        return {"done": False}
    me = (await client().get("https://plex.tv/api/v2/user", headers=plex_headers(tok))).json()
    server_token = tok
    plex_url = (config.get("plex_url") or "").rstrip("/")
    if plex_url and config.get("plex_token"):
        try:
            ident = (await client().get(f"{plex_url}/identity", headers=plex_headers(config.get("plex_token")))).json()
            mid = ident["MediaContainer"]["machineIdentifier"]
            res = (await client().get("https://plex.tv/api/v2/resources", params={"includeHttps": 1}, headers=plex_headers(tok))).json()
            for dev in res:
                if dev.get("clientIdentifier") == mid and dev.get("accessToken"):
                    server_token = dev["accessToken"]
                    break
            else:
                raise ConnectionError_("Your Plex account doesn't have access to this server – ask the admin to share the library with you")
        except ConnectionError_:
            raise
        except Exception:  # noqa: BLE001
            pass
    save(user_id, "plex", me.get("username") or me.get("title") or "Plex", {"token": server_token, "account_token": tok})
    return {"done": True, "account": me.get("username")}


# ---- Navidrome / Subsonic -------------------------------------------------------

def subsonic_params(user: str, password: str) -> dict:
    salt = secrets.token_hex(6)
    return {"u": user, "t": hashlib.md5((password + salt).encode()).hexdigest(), "s": salt, "v": "1.16.1", "c": "encore", "f": "json"}


async def connect_navidrome(user_id: int, username: str, password: str) -> dict:
    url = (config.get("navidrome_url") or "").rstrip("/")
    if not url:
        raise ConnectionError_("The admin hasn't set a Navidrome URL in Settings → Media servers")
    r = await client().get(f"{url}/rest/ping", params=subsonic_params(username, password))
    if r.json().get("subsonic-response", {}).get("status") != "ok":
        raise ConnectionError_("Navidrome rejected that username/password")
    save(user_id, "navidrome", username, {"user": username, "password": password})
    return {"account": username}


# ---- ListenBrainz / Last.fm / Deezer -------------------------------------------------

async def connect_listenbrainz(user_id: int, token: str) -> dict:
    r = await client().get("https://api.listenbrainz.org/1/validate-token", headers={"Authorization": f"Token {token}"})
    d = r.json()
    if not d.get("valid"):
        raise ConnectionError_("ListenBrainz token is not valid")
    save(user_id, "listenbrainz", d["user_name"], {"token": token, "user": d["user_name"]})
    return {"account": d["user_name"]}


async def connect_lastfm(user_id: int, username: str) -> dict:
    if not config.get("lastfm_api_key"):
        raise ConnectionError_("The admin needs to add a Last.fm API key in Settings → Integrations")
    r = await client().get("https://ws.audioscrobbler.com/2.0/", params={
        "method": "user.getinfo", "user": username, "api_key": config.get("lastfm_api_key"), "format": "json"})
    d = r.json()
    if "user" not in d:
        raise ConnectionError_("Last.fm user not found")
    save(user_id, "lastfm", d["user"]["name"], {"user": d["user"]["name"]})
    return {"account": d["user"]["name"]}


async def connect_deezer(user_id: int, profile: str) -> dict:
    import re
    m = re.search(r"(\d{3,})", profile)
    if not m:
        raise ConnectionError_("Paste your Deezer profile link or user id (your favourites must be public)")
    r = await client().get(f"https://api.deezer.com/user/{m.group(1)}")
    d = r.json()
    if d.get("error"):
        raise ConnectionError_("Deezer profile not found")
    save(user_id, "deezer", d.get("name") or m.group(1), {"user_id": m.group(1)})
    return {"account": d.get("name")}
