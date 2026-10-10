"""Organize the library into folders from tags (your naming template, e.g. Artist/Album (Year)/01 - Title).

Files MusicBrainz couldn't tag still get sensible names: missing title/artist come from the file name or an
AcoustID fingerprint, missing album/track number/year from Deezer. The fixed tags are written into the file."""
import asyncio
import os
import re

from .. import config, db
from . import acoustid, library, tagger
from .http import client

state = {"running": False, "done": 0, "total": 0, "moved": 0, "fixed": 0, "current": "", "message": ""}
UNKNOWN = {"", "unknown artist", "unknown album", "_"}


def _from_filename(path: str) -> dict:
    """'Artist - Title', '01 - Title', '01. Artist - Title' → whatever can be read from the name."""
    name = re.sub(r"\s*\(\d+\)$", "", os.path.splitext(os.path.basename(path))[0])
    out = {}
    m = re.match(r"^(\d{1,3})[\s.\-_]+(.*)$", name)
    if m:
        out["track"], name = int(m.group(1)), m.group(2)
    if " - " in name:
        a, t = name.split(" - ", 1)
        out["artist"], out["title"] = a.strip(), t.strip()
    elif name:
        out["title"] = name.strip()
    return out


async def _deezer(artist: str, title: str) -> dict:
    try:
        r = await client().get("https://api.deezer.com/search/track", params={"q": f'artist:"{artist}" track:"{title}"', "limit": 3})
        hit = (r.json().get("data") or [None])[0]
        if not hit:
            return {}
        alb = (await client().get(f"https://api.deezer.com/album/{hit['album']['id']}")).json()
        pos = next((i for i, t in enumerate((alb.get("tracks") or {}).get("data", []), 1) if t.get("id") == hit.get("id")), None)
        return {"album": hit["album"].get("title"), "albumartist": (alb.get("artist") or {}).get("name") or artist,
                "year": (alb.get("release_date") or "")[:4], "track": pos, "genre": ((alb.get("genres") or {}).get("data") or [{}])[0].get("name")}
    except Exception:  # noqa: BLE001
        return {}


def _real_tags(path: str) -> dict:
    """What's actually written in the file (the library index guesses missing ones from folder names)."""
    import mutagen
    try:
        f = mutagen.File(path, easy=True)
        g = lambda k: ((f.get(k) or [""])[0] if f else "").strip()  # noqa: E731
        return {"title": g("title"), "artist": g("artist"), "album": g("album"), "albumartist": g("albumartist"),
                "track": g("tracknumber").split("/")[0], "year": g("date")[:4], "genre": g("genre")}
    except Exception:  # noqa: BLE001
        return {}


async def _fill(t: dict) -> dict:
    """Missing tags for one library row, best source first. Returns only what changed."""
    fix = {}
    real = await asyncio.to_thread(_real_tags, t["path"])
    t = {**t, **{k: real.get(k) or (None if k in real else t.get(k)) for k in ("title", "artist", "album", "albumartist", "track", "year", "genre")}}
    if real.get("track") in ("", "0"):
        t["track"] = None
    have = lambda k: str(t.get(k) or "").strip().lower() not in UNKNOWN  # noqa: E731
    if not (have("title") and have("artist")):
        guess = _from_filename(t["path"])
        if (not have("title") or not have("artist")) and acoustid.available():
            try:
                res = await acoustid.identify(t["path"])
                rec = next((r for x in (res or {}).get("results", []) if x["score"] > 0.8 for r in x["recordings"] if r.get("title")), None)
                if rec:
                    guess.update(title=rec["title"], artist=rec["artist"])
                    if rec["release_groups"]:
                        guess.setdefault("album", rec["release_groups"][0])
            except Exception:  # noqa: BLE001
                pass
        for k in ("title", "artist", "album", "track"):
            if not have(k) and guess.get(k):
                fix[k] = guess[k]
    cur = {**t, **fix}
    if (not have("album") and not fix.get("album")) or not t.get("track") or not t.get("year"):
        dz = await _deezer(cur.get("artist") or "", cur.get("title") or "")
        for k in ("album", "albumartist", "genre"):
            if dz.get(k) and not have(k) and not fix.get(k):
                fix[k] = dz[k]
        for k in ("year", "track"):
            if dz.get(k) and not cur.get(k):
                fix[k] = dz[k]
    return fix, t


async def organize_library(fill_missing: bool = True) -> str:
    if state["running"]:
        return "already running"
    lib = config.get("library_path")
    rows = db.query("SELECT id, path, title, artist, album, albumartist, track, disc, year, genre, album_id FROM tracks ORDER BY album_id, disc, track")
    counts = {r["album_id"]: 0 for r in rows}
    for r in rows:
        counts[r["album_id"]] += 1
    state.update(running=True, done=0, total=len(rows), moved=0, fixed=0, current="", message="")
    try:
        for t in rows:
            state["current"] = f"{t['artist']} – {t['title']}"
            state["done"] += 1
            if not os.path.isfile(t["path"]):
                continue
            fix, t = await _fill(t) if fill_missing else ({}, t)
            if fix:
                tag = {"tracknumber" if k == "track" else "date" if k == "year" else k: v for k, v in fix.items()}
                try:
                    await asyncio.to_thread(tagger.write_tags, t["path"], tag)
                    state["fixed"] += 1
                except Exception as e:  # noqa: BLE001
                    db.log("warning", f"Organize: couldn't tag {os.path.basename(t['path'])}: {e}")
            vals = {**t, **fix}
            single = counts[t["album_id"]] == 1 and not str(vals.get("album") or "").strip()
            dest = tagger.render_path(vals, os.path.splitext(t["path"])[1], single=single)
            if os.path.abspath(dest) != os.path.abspath(t["path"]):
                old_dir = os.path.dirname(t["path"])
                new = await asyncio.to_thread(tagger.move_file, t["path"], dest, True)
                tagger.relink([t["path"]], [new])
                tagger.prune_empty(old_dir, lib)
                state["moved"] += 1
                await library.index_paths([new])
            elif fix:
                await library.index_paths([t["path"]])
        state["message"] = f"{state['moved']} moved, {state['fixed']} re-tagged"
        db.log("info", f"Organized the library: {state['message']}")
        return state["message"]
    finally:
        state["running"] = False
        state["current"] = ""
