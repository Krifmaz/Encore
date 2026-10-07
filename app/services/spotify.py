"""Spotify playlist/album import (client-credentials flow)."""
import re
import time

from .. import config
from .http import client

_token = {"value": None, "exp": 0}


def enabled() -> bool:
    return bool(config.get("spotify_client_id") and config.get("spotify_client_secret"))


async def _auth() -> str:
    if _token["value"] and _token["exp"] > time.time() + 30:
        return _token["value"]
    r = await client().post(
        "https://accounts.spotify.com/api/token",
        data={"grant_type": "client_credentials"},
        auth=(config.get("spotify_client_id"), config.get("spotify_client_secret")),
    )
    r.raise_for_status()
    d = r.json()
    _token.update(value=d["access_token"], exp=time.time() + d.get("expires_in", 3600))
    return _token["value"]


def parse(url: str) -> tuple[str, str] | None:
    m = re.search(r"open\.spotify\.com/(?:intl-[a-z]+/)?(playlist|album|track)/([A-Za-z0-9]+)", url) or re.match(r"spotify:(playlist|album|track):([A-Za-z0-9]+)", url)
    return (m.group(1), m.group(2)) if m else None


async def tracks(url: str) -> dict:
    kind, sid = parse(url) or (None, None)
    if not kind:
        raise ValueError("Not a Spotify playlist/album/track URL")
    h = {"Authorization": f"Bearer {await _auth()}"}
    c = client()
    if kind == "track":
        t = (await c.get(f"https://api.spotify.com/v1/tracks/{sid}", headers=h)).json()
        return {"name": t["name"], "tracks": [_t(t)]}
    meta = (await c.get(f"https://api.spotify.com/v1/{kind}s/{sid}", headers=h)).json()
    if "error" in meta:
        raise ValueError(meta["error"].get("message", "Spotify error"))
    items, nxt = [], f"https://api.spotify.com/v1/{kind}s/{sid}/tracks?limit=100"
    while nxt and len(items) < 1000:
        page = (await c.get(nxt, headers=h)).json()
        for it in page.get("items", []):
            t = it.get("track") if kind == "playlist" else it
            if t and t.get("name"):
                items.append(_t(t, meta if kind == "album" else None))
        nxt = page.get("next")
    return {"name": meta.get("name"), "kind": kind, "tracks": items}


def _t(t: dict, album: dict | None = None) -> dict:
    alb = album or t.get("album") or {}
    imgs = alb.get("images") or []
    return {
        "title": t["name"],
        "artist": ", ".join(a["name"] for a in t.get("artists", [])[:1]),
        "album": alb.get("name"),
        "length": (t.get("duration_ms") or 0) / 1000,
        "isrc": (t.get("external_ids") or {}).get("isrc"),
        "cover": imgs[0]["url"] if imgs else None,
    }
