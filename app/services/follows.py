"""Follow artists: check MusicBrainz for new releases and request them automatically (Lidarr-style monitoring)."""
import json
import time
from datetime import date

from .. import config, db
from . import library, musicbrainz as mb, notify

TYPES = ("Album", "EP", "Single")


def _wanted(rg: dict, types: list[str]) -> bool:
    # studio releases only: compilations, live albums, soundtracks, remixes… are skipped
    return rg.get("type") in types and not rg.get("secondary")


def _released(rg: dict) -> bool:
    d = rg.get("date") or ""
    if not d:
        return True
    try:
        parts = [int(x) for x in d.split("-")] + [1, 1]
        return date(parts[0], parts[1] or 1, parts[2] or 1) <= date.today()
    except ValueError:
        return True


def _request(user: dict, follow: dict, rg: dict) -> int | None:
    from fastapi import HTTPException

    from ..routers.requests_routes import NewRequest, _create
    try:
        rid = _create(user, NewRequest(kind="album", mbid=rg["id"], artist=follow["name"], title=rg["title"],
                                       album=rg["title"], year=rg.get("year"), cover_url=rg.get("cover")))
    except HTTPException as e:
        if e.status_code == 409:     # someone already requested it
            return None
        raise
    auto = config.get("auto_request_follows") and (user["role"] == "admin" or user.get("auto_download"))
    r = db.get_request(rid)
    r["detail"]["followed"] = True
    db.update_request(rid, detail=r["detail"])
    if auto and r["status"] == "pending":
        db.update_request(rid, status="queued", message="New release from a followed artist")
    return rid


async def follow(user: dict, mbid: str, name: str | None = None, types: list[str] | None = None, backfill: bool = False) -> dict:
    """Start following. Existing releases are marked as seen (or requested now with backfill=True)."""
    types = [t for t in (types or ["Album", "EP"]) if t in TYPES] or ["Album"]
    art = await mb.artist(mbid, fresh=True)
    existing = db.one("SELECT * FROM follows WHERE user_id=? AND mbid=?", (user["id"], mbid))
    if existing:
        db.execute("UPDATE follows SET types=? WHERE id=?", (json.dumps(types), existing["id"]))
        return {"id": existing["id"], "requested": 0}
    seen, requested = [], 0
    fid = db.execute("INSERT INTO follows (user_id, mbid, name, types, seen, created_at, last_checked) VALUES (?,?,?,?,?,?,?)",
                     (user["id"], mbid, name or art["name"], json.dumps(types), "[]", time.time(), time.time()))
    f = db.one("SELECT * FROM follows WHERE id=?", (fid,))
    for rg in art["release_groups"]:
        if not _released(rg):
            continue                       # announced but not out yet: picked up when it releases
        seen.append(rg["id"])
        if backfill and _wanted(rg, types) and not library.has_release_group(rg["id"]) and not library.has_album(art["name"], rg["title"]):
            if _request(user, f, rg):
                requested += 1
    db.execute("UPDATE follows SET seen=? WHERE id=?", (json.dumps(seen), fid))
    db.log("info", f"{user['username']} followed {art['name']}" + (f" ({requested} missing release(s) requested)" if requested else ""))
    return {"id": fid, "requested": requested}


async def check(f: dict) -> list[dict]:
    """Look for releases that appeared since the last check; request the wanted ones."""
    user = db.one("SELECT * FROM users WHERE id=? AND disabled=0", (f["user_id"],))
    if not user:
        return []
    try:
        art = await mb.artist(f["mbid"], fresh=True)
    except Exception as e:  # noqa: BLE001
        db.execute("UPDATE follows SET last_error=?, last_checked=? WHERE id=?", (str(e)[:200], time.time(), f["id"]))
        return []
    seen = set(json.loads(f["seen"] or "[]"))
    types = json.loads(f["types"] or '["Album","EP"]')
    new = []
    for rg in art["release_groups"]:
        if rg["id"] in seen or not _released(rg):
            continue
        seen.add(rg["id"])
        if not _wanted(rg, types) or library.has_release_group(rg["id"]):
            continue
        rid = _request(user, f, rg)
        if rid:
            new.append({**rg, "request_id": rid})
    db.execute("UPDATE follows SET seen=?, last_checked=?, last_error=NULL, found_count=found_count+?, last_found=COALESCE(?, last_found) WHERE id=?",
               (json.dumps(sorted(seen)), time.time(), len(new), time.time() if new else None, f["id"]))
    for rg in new:
        db.log("info", f"New release from {f['name']}: {rg['title']} ({rg['type']}) – requested")
        await notify.send(f"🆕 New {rg['type'].lower()} from {f['name']}", rg["title"], rg.get("cover"))
    return new


async def check_all(user_id: int | None = None) -> int:
    sql, params = "SELECT * FROM follows", ()
    if user_id:
        sql, params = sql + " WHERE user_id=?", (user_id,)
    n = 0
    for f in db.query(sql + " ORDER BY last_checked", params):
        n += len(await check(f))
    return n


async def follow_library_artists(user: dict) -> dict:
    """Follow every album artist in the library (MusicBrainz id from tags, or an exact-name match)."""
    rows = db.query("""SELECT albumartist name, MAX(mb_albumartistid) mbid FROM tracks
                       WHERE albumartist NOT IN ('', 'Unknown Artist', 'Various Artists') GROUP BY lower(albumartist)""")
    have = {r["mbid"] for r in db.query("SELECT mbid FROM follows WHERE user_id=?", (user["id"],))}
    added, unmatched = 0, []
    for r in rows:
        mbid = r["mbid"]
        if not mbid:
            try:
                hits = await mb.search_artists(r["name"], 3)
                exact = [h for h in hits if h["name"].lower() == r["name"].lower() and h["score"] >= 95]
                mbid = exact[0]["id"] if len(exact) == 1 else None
            except Exception:  # noqa: BLE001
                mbid = None
        if not mbid:
            unmatched.append(r["name"])
            continue
        if mbid in have:
            continue
        await follow(user, mbid, r["name"])
        have.add(mbid)
        added += 1
    return {"followed": added, "unmatched": unmatched[:50], "unmatched_count": len(unmatched)}
