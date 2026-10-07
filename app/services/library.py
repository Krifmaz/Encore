"""Scan the music library into the database."""
import asyncio
import hashlib
import os
import time

import mutagen

from .. import config, db

AUDIO_EXT = {".flac", ".mp3", ".m4a", ".mp4", ".aac", ".ogg", ".opus", ".wav", ".aiff", ".aif", ".wma", ".ape", ".wv", ".alac"}
COVER_NAMES = ("cover", "folder", "front", "album", "artwork")
state = {"scanning": False, "progress": 0, "total": 0, "last_scan": None, "message": ""}


def _first(tags, *keys):
    if not tags:
        return None
    for k in keys:
        try:
            v = tags.get(k)
        except Exception:
            v = None
        if v:
            v = v[0] if isinstance(v, list) else v
            return str(v).strip() or None
    return None


def _num(v):
    if not v:
        return None
    try:
        return int(str(v).split("/")[0])
    except ValueError:
        return None


def album_key(albumartist: str, album: str, path: str) -> str:
    if album:
        key = f"{(albumartist or '').lower().strip()}|{album.lower().strip()}"
    else:
        key = "dir|" + os.path.dirname(path)
    return hashlib.sha1(key.encode()).hexdigest()[:16]


def read_tags(path: str) -> dict:
    try:
        f = mutagen.File(path, easy=True)
    except Exception:
        f = None
    tags = f.tags if f is not None else None
    info = getattr(f, "info", None)
    title = _first(tags, "title") or os.path.splitext(os.path.basename(path))[0]
    artist = _first(tags, "artist")
    album = _first(tags, "album")
    albumartist = _first(tags, "albumartist", "album artist") or artist
    if not artist:
        parts = os.path.normpath(path).split(os.sep)
        if len(parts) >= 3:
            artist = albumartist = parts[-3]
            album = album or parts[-2]
    return {
        "title": title,
        "artist": artist or "Unknown Artist",
        "album": album,
        "albumartist": albumartist or artist or "Unknown Artist",
        "track": _num(_first(tags, "tracknumber")),
        "disc": _num(_first(tags, "discnumber")) or 1,
        "year": (_first(tags, "date", "originaldate", "year") or "")[:4],
        "genre": _first(tags, "genre"),
        "duration": round(getattr(info, "length", 0) or 0, 2),
        "bitrate": int((getattr(info, "bitrate", 0) or 0) / 1000),
        "format": os.path.splitext(path)[1].lstrip(".").lower(),
        "mb_trackid": _first(tags, "musicbrainz_trackid"),
        "mb_albumid": _first(tags, "musicbrainz_albumid"),
        "mb_releasegroupid": _first(tags, "musicbrainz_releasegroupid"),
        "mb_artistid": _first(tags, "musicbrainz_artistid"),
        "mb_albumartistid": _first(tags, "musicbrainz_albumartistid"),
        "isrc": ((_first(tags, "isrc", "tsrc") or "").upper().replace("-", "") or None),
    }


COLS = ["title", "artist", "album", "albumartist", "track", "disc", "year", "genre", "duration", "bitrate", "format",
        "mb_trackid", "mb_albumid", "mb_releasegroupid", "mb_artistid", "mb_albumartistid", "isrc"]


def index_file(path: str, st=None) -> None:
    st = st or os.stat(path)
    t = read_tags(path)
    aid = album_key(t["albumartist"], t["album"], path)
    existing = db.one("SELECT id, added_at, mtime, size FROM tracks WHERE path=?", (path,))
    vals = [t[c] for c in COLS]
    if existing:
        db.execute(
            f"UPDATE tracks SET album_id=?, mtime=?, size=?, {', '.join(c + '=?' for c in COLS)} WHERE id=?",
            (aid, st.st_mtime, st.st_size, *vals, existing["id"]),
        )
        if (existing["mtime"], existing["size"]) != (st.st_mtime, st.st_size) and not _verified_recently(existing["id"], st.st_mtime):
            db.execute("UPDATE tracks SET verify_status=NULL WHERE id=?", (existing["id"],))
    else:
        db.execute(
            f"INSERT INTO tracks (path, album_id, mtime, size, {', '.join(COLS)}, added_at) VALUES (?,?,?,?,{','.join('?' * len(COLS))},?)",
            (path, aid, st.st_mtime, st.st_size, *vals, time.time()),
        )


def _verified_recently(track_id: int, mtime: float) -> bool:
    """Tag writes change mtime but not the audio; keep a verification made after the last change."""
    r = db.one("SELECT verified_at FROM tracks WHERE id=?", (track_id,))
    return bool(r and r["verified_at"] and r["verified_at"] >= mtime - 1)


def _scan_sync(root: str, full: bool) -> tuple[int, int]:
    known = {r["path"]: r["mtime"] for r in db.query("SELECT path, mtime FROM tracks WHERE path LIKE ?", (root.rstrip("/") + "/%",))}
    seen = set()
    files = []
    for dirpath, dirnames, filenames in os.walk(root, followlinks=True):
        dirnames[:] = [d for d in dirnames if not d.startswith(".")]
        for fn in filenames:
            if os.path.splitext(fn)[1].lower() in AUDIO_EXT and not fn.startswith("._"):
                files.append(os.path.join(dirpath, fn))
    state["total"] = len(files)
    changed = 0
    for i, path in enumerate(files):
        state["progress"] = i + 1
        seen.add(path)
        try:
            st = os.stat(path)
        except OSError:
            continue
        if not full and known.get(path) == st.st_mtime:
            continue
        try:
            index_file(path, st)
            changed += 1
        except Exception as e:  # noqa: BLE001
            db.log("warning", f"Could not read {path}: {e}")
    removed = [p for p in known if p not in seen]
    for p in removed:
        # playlist entries / likes of vanished files get re-linked by the next rematch
        db.execute("UPDATE playlist_items SET track_id=NULL WHERE track_id=(SELECT id FROM tracks WHERE path=?)", (p,))
        db.execute("DELETE FROM tracks WHERE path=?", (p,))
    if removed or changed:
        try:
            from . import playlists
            playlists.rematch()
        except Exception:  # noqa: BLE001
            pass
    return changed, len(removed)


async def scan(full: bool = False, root: str | None = None) -> dict:
    root = root or config.get("library_path")
    if not os.path.isdir(root):
        state["message"] = f"Library path {root} does not exist"
        return state
    from .http import loop_local
    async with loop_local("scan", asyncio.Lock):
        state.update(scanning=True, progress=0, total=0, message="Scanning…")
        try:
            changed, removed = await asyncio.to_thread(_scan_sync, root, full)
            state["message"] = f"Scan complete: {changed} updated, {removed} removed"
            if changed or removed:
                db.log("info", state["message"])
        finally:
            state.update(scanning=False, last_scan=time.time())
    return state


async def index_paths(paths: list[str]) -> None:
    def _go():
        for p in paths:
            if os.path.isfile(p):
                index_file(p)
    await asyncio.to_thread(_go)


def find_cover(album_id: str) -> str | None:
    tracks = db.query("SELECT path FROM tracks WHERE album_id=? ORDER BY disc, track LIMIT 50", (album_id,))
    dirs = []
    for t in tracks:
        d = os.path.dirname(t["path"])
        if d not in dirs:
            dirs.append(d)
            parent = os.path.dirname(d)
            if parent not in dirs:
                dirs.append(parent)  # multi-disc layouts: Album/CD1/…
    for d in dirs:
        try:
            names = os.listdir(d)
        except OSError:
            continue
        images = [n for n in names if n.lower().endswith((".jpg", ".jpeg", ".png", ".webp"))]
        for pref in COVER_NAMES:
            for n in images:
                if n.lower().startswith(pref):
                    return os.path.join(d, n)
        if images and d == os.path.dirname(tracks[0]["path"]):
            return os.path.join(d, images[0])
    # embedded art → cache
    cache_dir = os.path.join(db.DATA_DIR, "covers")
    cached = os.path.join(cache_dir, album_id + ".jpg")
    if os.path.isfile(cached):
        return cached
    if tracks:
        data = extract_embedded(tracks[0]["path"])
        if data:
            os.makedirs(cache_dir, exist_ok=True)
            with open(cached, "wb") as fh:
                fh.write(data)
            return cached
    return None


def extract_embedded(path: str) -> bytes | None:
    try:
        f = mutagen.File(path)
    except Exception:
        return None
    if f is None:
        return None
    pics = getattr(f, "pictures", None)
    if pics:
        return pics[0].data
    tags = getattr(f, "tags", None)
    if tags is None:
        return None
    try:
        for k in tags.keys():
            if k.startswith("APIC"):
                return tags[k].data
    except Exception:
        pass
    covr = tags.get("covr") if hasattr(tags, "get") else None
    if covr:
        return bytes(covr[0])
    pic = tags.get("metadata_block_picture") if hasattr(tags, "get") else None
    if pic:
        import base64
        from mutagen.flac import Picture
        try:
            return Picture(base64.b64decode(pic[0])).data
        except Exception:
            return None
    return None


def stats() -> dict:
    s = db.one("SELECT COUNT(*) tracks, COUNT(DISTINCT album_id) albums, COUNT(DISTINCT albumartist) artists, COALESCE(SUM(size),0) size, COALESCE(SUM(duration),0) duration FROM tracks")
    s["untagged_albums"] = db.one("SELECT COUNT(DISTINCT album_id) n FROM tracks WHERE mb_albumid IS NULL OR mb_albumid=''")["n"]
    s["verify_counts"] = {r["verify_status"] or "unchecked": r["n"] for r in db.query("SELECT verify_status, COUNT(*) n FROM tracks GROUP BY verify_status")}
    s["formats"] = db.query("SELECT format, COUNT(*) n FROM tracks GROUP BY format ORDER BY n DESC")
    return s


def has_release_group(rgid: str) -> bool:
    return bool(rgid and db.one("SELECT 1 FROM tracks WHERE mb_releasegroupid=? LIMIT 1", (rgid,)))


def has_album(artist: str, album: str) -> bool:
    if not album:
        return False
    return bool(db.one(
        "SELECT 1 FROM tracks WHERE lower(album)=lower(?) AND (lower(albumartist)=lower(?) OR lower(artist)=lower(?)) LIMIT 1",
        (album, artist, artist),
    ))


def has_track(mbid: str | None, artist: str, title: str) -> bool:
    if mbid and db.one("SELECT 1 FROM tracks WHERE mb_trackid=? LIMIT 1", (mbid,)):
        return True
    return bool(db.one(
        "SELECT 1 FROM tracks WHERE lower(title)=lower(?) AND (lower(artist)=lower(?) OR lower(albumartist)=lower(?)) LIMIT 1",
        (title, artist, artist),
    ))
