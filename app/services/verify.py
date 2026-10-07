"""Verify files are the song they claim to be, and find duplicates in the library."""
import os
import time

from .. import config, db
from . import acoustid, matcher

state = {"running": False, "done": 0, "total": 0, "verified": 0, "mismatch": 0, "unknown": 0, "current": ""}


async def check_file(path: str, title: str, artist: str, length: float | None = None,
                     recording_id: str | None = None) -> dict:
    """→ {status: verified|mismatch|unknown, acoustid, detail}"""
    ident = await acoustid.identify(path)
    if ident is None:
        return {"status": "unknown", "acoustid": None, "detail": "fingerprinting failed"}
    results = [r for r in ident["results"] if r["score"] >= 0.6]
    if not results:
        return {"status": "unknown", "acoustid": None, "detail": "audio not in AcoustID database"}
    best = results[0]
    recs = [rec for r in results for rec in r["recordings"]]
    if not recs:
        return {"status": "unknown", "acoustid": best["acoustid"], "detail": "fingerprint known but not linked to MusicBrainz"}
    if recording_id and any(r["id"] == recording_id for r in recs):
        return {"status": "verified", "acoustid": best["acoustid"], "detail": "MusicBrainz recording confirmed by audio"}
    want = matcher.describe(title, artist, length=length)
    for rec in recs:
        if matcher.compare(want, matcher.describe(rec["title"], rec["artist"], length=rec["length"]), strict_duration=False).match:
            return {"status": "verified", "acoustid": best["acoustid"], "detail": f"audio matches “{rec['artist']} – {rec['title']}”"}
    top = recs[0]
    v = matcher.compare(want, matcher.describe(top["title"], top["artist"], length=top["length"]), strict_duration=False)
    return {"status": "mismatch", "acoustid": best["acoustid"],
            "detail": f"audio is “{top['artist']} – {top['title']}” ({v.reason})", "actual": top}


async def verify_track(t: dict) -> dict:
    res = await check_file(t["path"], t["title"], t["artist"], t["duration"], t["mb_trackid"])
    db.execute("UPDATE tracks SET verify_status=?, verify_detail=?, acoustid_id=COALESCE(?, acoustid_id), verified_at=? WHERE id=?",
               (res["status"], res["detail"], res["acoustid"], time.time(), t["id"]))
    return res


async def verify_library(only_new: bool = True) -> None:
    if state["running"]:
        return
    where = "WHERE verify_status IS NULL" if only_new else ""
    rows = db.query(f"SELECT id, path, title, artist, duration, mb_trackid FROM tracks {where} ORDER BY id")
    state.update(running=True, done=0, total=len(rows), verified=0, mismatch=0, unknown=0, current="")
    try:
        for t in rows:
            state["current"] = f"{t['artist']} – {t['title']}"
            try:
                res = await verify_track(t)
                state[res["status"]] += 1
            except Exception as e:  # noqa: BLE001
                state["unknown"] += 1
                db.execute("UPDATE tracks SET verify_status='error', verify_detail=? WHERE id=?", (str(e)[:200], t["id"]))
                if "invalid API key" in str(e) or "client" in str(e).lower():
                    db.log("error", f"AcoustID: {e}")
                    break
            state["done"] += 1
        db.log("info", f"Verification finished: {state['verified']} verified, {state['mismatch']} mismatched, {state['unknown']} unknown")
    finally:
        state.update(running=False, current="")


# ---- duplicates --------------------------------------------------------------

LOSSLESS = {"flac", "wav", "alac", "aiff", "aif", "ape", "wv"}


def quality(t: dict) -> tuple:
    return (
        t.get("verify_status") == "verified",
        t.get("format") in LOSSLESS,
        t.get("bitrate") or 0,
        bool(t.get("mb_trackid")),
        -len(t.get("path") or ""),
    )


def find_duplicates() -> list[dict]:
    rows = db.query("""SELECT id, path, title, artist, album, albumartist, album_id, duration, format, bitrate, size,
                       isrc, mb_trackid, acoustid_id, verify_status FROM tracks""")
    parent = {r["id"]: r["id"] for r in rows}
    reason: dict[int, str] = {}

    def find(x):
        while parent[x] != x:
            parent[x] = parent[parent[x]]
            x = parent[x]
        return x

    def union(a, b, why):
        ra, rb = find(a), find(b)
        if ra != rb:
            parent[rb] = ra
            reason[ra] = reason.get(ra) or why

    def group_by(keyfn, why):
        seen: dict = {}
        for r in rows:
            k = keyfn(r)
            if not k:
                continue
            if k in seen:
                union(seen[k], r["id"], why)
            else:
                seen[k] = r["id"]

    group_by(lambda r: r["acoustid_id"], "identical audio (AcoustID)")
    group_by(lambda r: r["mb_trackid"], "same MusicBrainz recording")
    group_by(lambda r: r["isrc"], "same ISRC")
    # metadata: same artist + base title + version, durations within 3s
    buckets: dict[str, list] = {}
    for r in rows:
        d = matcher.describe(r["title"], r["artist"], length=r["duration"])
        if d.base:
            buckets.setdefault(matcher.key(d), []).append(r)
    for items in buckets.values():
        items.sort(key=lambda r: r["duration"] or 0)
        for a, b in zip(items, items[1:]):
            if a["duration"] and b["duration"] and abs(a["duration"] - b["duration"]) <= 3:
                union(a["id"], b["id"], "same song & version (tags + length)")

    groups: dict[int, list] = {}
    for r in rows:
        groups.setdefault(find(r["id"]), []).append(r)
    ignored = {r["key"] for r in db.query("SELECT key FROM dup_ignore")}
    out = []
    for root, items in groups.items():
        if len(items) < 2:
            continue
        gkey = ",".join(str(i["id"]) for i in sorted(items, key=lambda x: x["id"]))
        if gkey in ignored:
            continue
        items.sort(key=quality, reverse=True)
        lib = config.get("library_path").rstrip("/") + "/"
        for i in items:
            i["relpath"] = i["path"][len(lib):] if i["path"].startswith(lib) else i["path"]
        out.append({"key": gkey, "reason": reason.get(root, ""), "keep": items[0]["id"], "tracks": items})
    out.sort(key=lambda g: (g["tracks"][0]["artist"] or "").lower())
    return out


def delete_tracks(ids: list[int], replacement: int | None = None) -> int:
    """Delete files; playlist entries pointing at them move to `replacement`."""
    from . import tagger
    n = 0
    for tid in ids:
        t = db.one("SELECT path FROM tracks WHERE id=?", (tid,))
        if not t:
            continue
        try:
            os.remove(t["path"])
        except OSError:
            pass
        db.execute("DELETE FROM tracks WHERE id=?", (tid,))
        db.execute("UPDATE playlist_items SET track_id=? WHERE track_id=?", (replacement, tid))
        tagger.prune_empty(os.path.dirname(t["path"]), config.get("library_path"))
        n += 1
    return n

