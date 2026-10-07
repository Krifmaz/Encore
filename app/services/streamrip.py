"""Lossless downloads from Qobuz / Tidal / Deezer through the external `streamrip` tool.

Encore only *finds* the right release (by ISRC or version-aware search) and hands the link to streamrip,
which signs in with your own subscription (configured in streamrip's config.toml).
"""
import asyncio
import os
import re
import shlex
import shutil

from .. import config, db
from . import matcher, sources
from .http import client

DEFAULT_CMD = "{bin} --config-path {config} --folder {folder} --quality {quality} --no-db --no-progress url {url}"
AUDIO = (".flac", ".mp3", ".m4a", ".alac", ".ogg", ".opus", ".wav", ".aiff")


def binary() -> str | None:
    custom = config.get("streamrip_bin")
    if custom:
        return shutil.which(custom) or (custom if os.path.isfile(custom) else None)
    return shutil.which("streamrip") or shutil.which("rip")


def enabled() -> bool:
    return bool(config.get("streamrip_enabled") and binary())


def config_path() -> str:
    return os.path.join(db.DATA_DIR, "streamrip", "config.toml")


def _template() -> str | None:
    try:
        import streamrip as _sr  # installed alongside Encore in the Docker image
        p = os.path.join(os.path.dirname(_sr.__file__), "config.toml")
        return open(p).read() if os.path.exists(p) else None
    except Exception:  # noqa: BLE001
        return None


def _set_toml(text: str, section: str, key: str, value) -> str:
    """Minimal in-place TOML edit (keeps the user's comments and other settings)."""
    val = ("true" if value else "false") if isinstance(value, bool) else (str(value) if isinstance(value, int) else '"' + str(value).replace('"', '\\"') + '"')
    sec = re.search(rf"(?m)^\[{re.escape(section)}\]\s*$", text)
    if not sec:
        return text + f"\n[{section}]\n{key} = {val}\n"
    start = sec.end()
    nxt = re.search(r"(?m)^\[", text[start:])
    end = start + nxt.start() if nxt else len(text)
    body = text[start:end]
    line = re.compile(rf"(?m)^{re.escape(key)}\s*=.*$")
    body = line.sub(f"{key} = {val}", body, count=1) if line.search(body) else body.rstrip("\n") + f"\n{key} = {val}\n\n"
    return text[:start] + body + text[end:]


async def apply_credentials() -> str:
    """Write the credentials entered in Encore's settings into streamrip's config file."""
    path = config_path()
    os.makedirs(os.path.dirname(path), exist_ok=True)
    text = open(path).read() if os.path.exists(path) else (_template() or "")
    if config.get("streamrip_deezer_arl"):
        text = _set_toml(text, "deezer", "arl", config.get("streamrip_deezer_arl"))
    if config.get("streamrip_qobuz_user") and config.get("streamrip_qobuz_token"):
        text = _set_toml(text, "qobuz", "user_id", config.get("streamrip_qobuz_user"))
        text = _set_toml(text, "qobuz", "auth_token", config.get("streamrip_qobuz_token"))
    # files land in Encore's staging folder and are renamed on import, keep streamrip's own layout simple
    text = _set_toml(text, "database", "downloads_enabled", False)
    with open(path, "w") as fh:
        fh.write(text)
    return path


# ---- resolving a request to a service link ---------------------------------------------

async def _deezer_search(kind: str, q: str) -> list[dict]:
    r = await client().get(f"https://api.deezer.com/search/{kind}", params={"q": q, "limit": 15})
    return r.json().get("data", [])


async def resolve_album(artist: str, album: str, track_count: int | None, first_isrc: str | None) -> list[str]:
    urls = []
    services = [s.strip() for s in config.get("streamrip_sources").split(",") if s.strip()]
    want = matcher.describe(album, artist)
    for svc in services:
        try:
            if svc == "deezer":
                if first_isrc:
                    t = await sources.deezer_isrc(first_isrc)
                    if t and t.get("album"):
                        urls.append(f"https://www.deezer.com/album/{t['album']['id']}")
                        continue
                best, best_s = None, 0.0
                for a in await _deezer_search("album", f'artist:"{artist}" album:"{album}"') or await _deezer_search("album", f"{artist} {album}"):
                    v = matcher.compare(want, matcher.describe(a.get("title"), (a.get("artist") or {}).get("name")))
                    if not v.match:
                        continue
                    s = v.score + (0.2 if track_count and a.get("nb_tracks") == track_count else 0)
                    if s > best_s:
                        best, best_s = a, s
                if best:
                    urls.append(f"https://www.deezer.com/album/{best['id']}")
            elif svc == "tidal" and first_isrc and config.get("tidal_client_id"):
                tok = await sources._tidal_token()
                r = await client().get("https://openapi.tidal.com/v2/tracks", params={"countryCode": "US", "filter[isrc]": first_isrc, "include": "albums"},
                                       headers={"Authorization": f"Bearer {tok}", "Accept": "application/vnd.api+json"})
                inc = [x for x in r.json().get("included", []) if x.get("type") == "albums"]
                if inc:
                    urls.append(f"https://tidal.com/browse/album/{inc[0]['id']}")
        except Exception as e:  # noqa: BLE001
            db.log("warning", f"streamrip: {svc} lookup failed: {e}")
    return urls


async def resolve_track(artist: str, title: str, isrc: str | None, length: float | None) -> list[str]:
    urls = []
    services = [s.strip() for s in config.get("streamrip_sources").split(",") if s.strip()]
    want = matcher.describe(title, artist, length=length, isrc=isrc)
    for svc in services:
        try:
            if svc == "deezer":
                t = await sources.deezer_isrc(isrc) if isrc else None
                if not t:
                    for c in await _deezer_search("track", f'artist:"{artist}" track:"{title}"'):
                        ctitle = c.get("title") or ""
                        if c.get("title_version") and c["title_version"].lower() not in ctitle.lower():
                            ctitle += " " + c["title_version"]
                        if matcher.compare(want, matcher.describe(ctitle, (c.get("artist") or {}).get("name"), length=c.get("duration"))).match:
                            t = c
                            break
                if t:
                    urls.append(f"https://www.deezer.com/track/{t['id']}")
            elif svc == "tidal" and isrc and config.get("tidal_client_id"):
                tok = await sources._tidal_token()
                r = await client().get("https://openapi.tidal.com/v2/tracks", params={"countryCode": "US", "filter[isrc]": isrc},
                                       headers={"Authorization": f"Bearer {tok}", "Accept": "application/vnd.api+json"})
                data = r.json().get("data", [])
                if data:
                    urls.append(f"https://tidal.com/browse/track/{data[0]['id']}")
        except Exception as e:  # noqa: BLE001
            db.log("warning", f"streamrip: {svc} lookup failed: {e}")
    return urls


# ---- running streamrip -------------------------------------------------------------

async def download(url: str, folder: str, timeout: int = 3600) -> list[str]:
    os.makedirs(folder, exist_ok=True)
    if not os.path.exists(config_path()):
        await apply_credentials()
    tpl = config.get("streamrip_command") or DEFAULT_CMD
    cmd = tpl.format(bin=shlex.quote(binary() or "streamrip"), config=shlex.quote(config_path()), folder=shlex.quote(folder),
                     quality=int(config.get("streamrip_quality")), url=shlex.quote(url))
    proc = await asyncio.create_subprocess_shell(cmd, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.STDOUT,
                                                 env={**os.environ, "NO_COLOR": "1", "TERM": "dumb"})
    try:
        out, _ = await asyncio.wait_for(proc.communicate(), timeout)
    except asyncio.TimeoutError:
        proc.kill()
        raise RuntimeError("streamrip timed out")
    files = []
    for root, _, names in os.walk(folder):
        files += [os.path.join(root, n) for n in names if n.lower().endswith(AUDIO)]
    if not files:
        lines = [re.sub(r"[│╭╰╮╯─━┃]+", " ", ln).strip() for ln in (out or b"").decode(errors="replace").splitlines()]
        lines = [ln for ln in lines if ln and not re.match(r"^\d+\s", ln)]
        errs = [ln for ln in lines if re.search(r"error|invalid|fail|not (found|available)|login|arl|token|subscription", ln, re.I)]
        raise RuntimeError("streamrip produced no files: " + " / ".join((errs or lines)[-3:])[:300])
    return sorted(files)
