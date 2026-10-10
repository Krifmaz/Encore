"""Playlists & liked songs: import, verify against the library, de-duplicate, merge, request missing."""
import json
import time

from .. import auth, db
from . import matcher


def get_liked(user_id: int) -> dict:
    pl = db.one("SELECT * FROM playlists WHERE user_id=? AND kind='liked'", (user_id,))
    if pl:
        return pl
    now = time.time()
    pid = db.execute("INSERT INTO playlists (user_id, name, kind, source, sync_targets, created_at, updated_at) VALUES (?,?,?,?,?,?,?)",
                     (user_id, "Liked Songs", "liked", "manual", "[]", now, now))
    return db.one("SELECT * FROM playlists WHERE id=?", (pid,))


def items(pid: int) -> list[dict]:
    return db.query("SELECT * FROM playlist_items WHERE playlist_id=? ORDER BY position, id", (pid,))


def item_desc(it: dict) -> matcher.Desc:
    return matcher.describe(it["title"], it["artist"], it.get("album") or "", it.get("duration"), it.get("isrc"), it.get("mbid"))


def track_desc(t: dict) -> matcher.Desc:
    return matcher.describe(t["title"], t["artist"], t.get("album") or "", t.get("duration"), t.get("isrc"), t.get("mb_trackid"))


def identity(it: dict, track_ids_key: bool = True) -> str:
    """Key used to spot duplicates inside a playlist."""
    if track_ids_key and it.get("track_id"):
        return f"t:{it['track_id']}"
    if it.get("mbid"):
        return f"m:{it['mbid']}"
    if it.get("isrc"):
        return f"i:{it['isrc']}"
    return "k:" + matcher.key(item_desc(it))


def _dedupe(rows: list[dict]) -> tuple[list[dict], int]:
    """Drop rows that are the same song+version as an earlier row (version-aware)."""
    out, seen_keys, kept_desc = [], set(), []
    dropped = 0
    for r in rows:
        k = identity(r)
        if k in seen_keys:
            dropped += 1
            continue
        d = item_desc(r)
        if not r.get("track_id") and any(matcher.compare(d, kd).match for kd in kept_desc if kd.base[:4] == d.base[:4]):
            dropped += 1
            continue
        seen_keys.add(k)
        # an item with a library match also blocks its other identities (isrc/mbid/key)
        seen_keys.add(identity(r, track_ids_key=False))
        kept_desc.append(d)
        out.append(r)
    return out, dropped


def match_rows(rows: list[dict], index: matcher.LibraryIndex | None = None) -> int:
    index = index or matcher.LibraryIndex.load()
    n = 0
    for r in rows:
        hit, v = index.find(item_desc(r))
        r["track_id"] = hit["id"] if hit else None
        r["match_reason"] = v.reason if hit else None
        n += bool(hit)
    return n


def create(user_id: int, name: str, tracks: list[dict], source: str = "manual", source_url: str | None = None,
           description: str = "", cover: str | None = None, follow: bool = False, kind: str = "playlist") -> dict:
    now = time.time()
    pid = db.execute(
        "INSERT INTO playlists (user_id, name, description, kind, source, source_url, follow, sync_targets, cover_url, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
        (user_id, name, description, kind, source, source_url, int(follow), "[]", cover, now, now))
    added, dupes = add_tracks(pid, tracks, source)
    return {"id": pid, "added": added, "duplicates_skipped": dupes}


def add_tracks(pid: int, tracks: list[dict], source: str | None = None, index: matcher.LibraryIndex | None = None) -> tuple[int, int]:
    """Append tracks, skipping anything already in the playlist (same song + version)."""
    existing = items(pid)
    filled = []
    for t in tracks:
        if t.get("track_id") and not t.get("title"):
            lt = db.one("SELECT title, artist, album, duration, isrc, mb_trackid FROM tracks WHERE id=?", (t["track_id"],))
            if not lt:
                continue
            t = {**t, "title": lt["title"], "artist": lt["artist"], "album": lt["album"], "duration": lt["duration"],
                 "isrc": lt["isrc"], "mbid": lt["mb_trackid"]}
        filled.append(t)
    tracks = filled
    rows = [{"title": t["title"], "artist": t["artist"], "album": t.get("album"), "duration": t.get("duration"),
             "isrc": t.get("isrc"), "mbid": t.get("mbid"), "cover_url": t.get("cover"), "source": source,
             "source_id": t.get("source_id"), "track_id": t.get("track_id")} for t in tracks]
    need = [r for r in rows if not r["track_id"]]
    match_rows(need, index)
    merged, dropped = _dedupe(existing + rows)
    new_rows = [r for r in merged if "id" not in r]
    pos = max((e["position"] for e in existing), default=-1) + 1
    now = time.time()
    db.executemany(
        "INSERT INTO playlist_items (playlist_id, position, title, artist, album, duration, isrc, mbid, cover_url, source, source_id, track_id, match_reason, added_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        [(pid, pos + i, r["title"], r["artist"], r["album"], r["duration"], r["isrc"], r["mbid"], r["cover_url"], r["source"],
          r["source_id"], r["track_id"], r.get("match_reason"), now) for i, r in enumerate(new_rows)])
    db.execute("UPDATE playlists SET updated_at=? WHERE id=?", (now, pid))
    return len(new_rows), dropped


def add_library_tracks(pid: int, track_ids: list[int]) -> tuple[int, int]:
    tracks = []
    for tid in track_ids:
        t = db.one("SELECT id, title, artist, album, duration, isrc, mb_trackid FROM tracks WHERE id=?", (tid,))
        if t:
            tracks.append({"title": t["title"], "artist": t["artist"], "album": t["album"], "duration": t["duration"],
                           "isrc": t["isrc"], "mbid": t["mb_trackid"], "track_id": t["id"], "match_reason": "added from library"})
    return add_tracks(pid, tracks, "library")


def dedupe(pid: int) -> int:
    rows = items(pid)
    kept, dropped = _dedupe(rows)
    keep_ids = {r["id"] for r in kept}
    for r in rows:
        if r["id"] not in keep_ids:
            db.execute("DELETE FROM playlist_items WHERE id=?", (r["id"],))
    renumber(pid)
    return dropped


def renumber(pid: int) -> None:
    for i, r in enumerate(items(pid)):
        if r["position"] != i:
            db.execute("UPDATE playlist_items SET position=? WHERE id=?", (i, r["id"]))


def rematch(pid: int | None = None, only_unmatched: bool = True, index: matcher.LibraryIndex | None = None) -> int:
    """Link items to library tracks (after downloads finish, rescans, retags…)."""
    sql = "SELECT * FROM playlist_items WHERE 1=1"
    params: list = []
    if pid:
        sql += " AND playlist_id=?"
        params.append(pid)
    if only_unmatched:
        sql += " AND track_id IS NULL"
    rows = db.query(sql, params)
    if not rows:
        return 0
    index = index or matcher.LibraryIndex.load()
    n = 0
    for r in rows:
        hit, v = index.find(item_desc(r))
        new = hit["id"] if hit else None
        if new != r["track_id"]:
            db.execute("UPDATE playlist_items SET track_id=?, match_reason=? WHERE id=?", (new, v.reason if hit else None, r["id"]))
            n += bool(new)
    # matching may reveal new duplicates (two different listings of the same file)
    for p in {r["playlist_id"] for r in rows}:
        dedupe(p)
    return n


def merge(user_id: int, ids: list[int], name: str) -> dict:
    tracks = []
    for pid in ids:
        pl = db.one("SELECT * FROM playlists WHERE id=? AND user_id=?", (pid, user_id))
        if not pl:
            continue
        for r in items(pid):
            tracks.append({**r, "cover": r["cover_url"]})
    total = len(tracks)
    res = create(user_id, name, tracks, "merged", description=f"Merged from {len(ids)} playlists")
    res["total"] = total
    return res


def request_missing(user: dict, pid: int, auto: bool = False, album_mode: str | None = None) -> int:
    """Create track requests for every item that's not in the library yet.

    auto=True is used by admin-enabled auto-download rules and skips the approval queue."""
    from ..routers.requests_routes import NewRequest, _create
    from fastapi import HTTPException
    n = 0
    for r in items(pid):
        if r["track_id"] or r["request_id"]:
            continue
        body = NewRequest(kind="track", mbid=r["mbid"], artist=r["artist"], title=r["title"], album=r["album"],
                          cover_url=r["cover_url"], length=r["duration"], isrc=r["isrc"], album_mode=album_mode)
        try:
            rid = _create(user, body, notify_admin=False)
        except HTTPException as e:
            if e.status_code == 409:
                continue
            raise
        req = db.get_request(rid)
        if r["isrc"]:
            req["detail"]["isrc"] = r["isrc"]
        if auto:
            req["detail"]["auto"] = True
        db.update_request(rid, detail=req["detail"])
        if req["status"] == "pending" and (auto or auth.can(user, "auto_approve_playlists")):
            db.update_request(rid, status="queued", message="Auto-download" if auto else "Auto-approved (playlist)")
        db.execute("UPDATE playlist_items SET request_id=? WHERE id=?", (rid, r["id"]))
        n += 1
    return n


def summary(pl: dict) -> dict:
    s = db.one("SELECT COUNT(*) total, SUM(CASE WHEN track_id IS NOT NULL THEN 1 ELSE 0 END) matched FROM playlist_items WHERE playlist_id=?", (pl["id"],))
    return {**pl, "sync_targets": json.loads(pl.get("sync_targets") or "[]"), "total": s["total"] or 0, "matched": s["matched"] or 0}


async def refresh_followed(pl: dict) -> tuple[int, int]:
    """Pull new tracks from the playlist's source (Spotify, Deezer, …)."""
    from . import sources
    if pl["source"] in ("jellyfin", "plex", "navidrome"):
        from . import sync
        data = await sync.fetch_remote_playlist(pl)
    else:
        data = await sources.fetch(pl["source_url"], pl["user_id"])
    added, dup = add_tracks(pl["id"], data["tracks"], pl["source"])
    return added, dup
