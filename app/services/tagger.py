"""MusicBrainz matching, tag writing, cover art and renaming."""
import asyncio
import os
import re
import shutil
from difflib import SequenceMatcher

import mutagen

from .. import config, db
from . import library, musicbrainz as mb
from .http import client

TAG_KEYS = {
    "title": "title", "artist": "artist", "album": "album", "albumartist": "albumartist",
    "tracknumber": "tracknumber", "discnumber": "discnumber", "date": "date", "genre": "genre",
    "musicbrainz_trackid": "musicbrainz_trackid", "musicbrainz_albumid": "musicbrainz_albumid",
    "musicbrainz_releasegroupid": "musicbrainz_releasegroupid", "musicbrainz_artistid": "musicbrainz_artistid",
    "musicbrainz_albumartistid": "musicbrainz_albumartistid", "musicbrainz_releasetrackid": "musicbrainz_releasetrackid",
    "isrc": "isrc", "label": "organization", "barcode": "barcode", "catalognumber": "catalognumber",
    "releasecountry": "releasecountry", "musicbrainz_albumtype": "musicbrainz_albumtype",
    "musicbrainz_albumstatus": "musicbrainz_albumstatus",
}


def _norm(s):
    s = (s or "").lower()
    s = re.sub(r"\s*[\(\[](feat|ft|with)\.?[^\)\]]*[\)\]]", "", s)
    s = re.sub(r"[^a-z0-9]+", " ", s)
    return " ".join(s.split())


def sim(a, b) -> float:
    a, b = _norm(a), _norm(b)
    if not a or not b:
        return 0.0
    if a == b:
        return 1.0
    return SequenceMatcher(None, a, b).ratio()


# ---- writing ----------------------------------------------------------------

def write_tags(path: str, values: dict, cover: bytes | None = None) -> None:
    f = mutagen.File(path, easy=True)
    if f is None:
        raise ValueError(f"Unsupported file: {path}")
    if f.tags is None:
        f.add_tags()
    for key, val in values.items():
        tag = TAG_KEYS.get(key, key)
        if val in (None, ""):
            continue
        try:
            f.tags[tag] = [str(val)]
        except (KeyError, ValueError, TypeError):
            continue  # container doesn't support this field (e.g. EasyMP4 + isrc)
    f.save()
    if cover and config.get("embed_cover"):
        try:
            embed_cover(path, cover)
        except Exception as e:  # noqa: BLE001
            db.log("warning", f"Could not embed cover in {os.path.basename(path)}: {e}")


def embed_cover(path: str, data: bytes) -> None:
    ext = os.path.splitext(path)[1].lower()
    mime = "image/png" if data[:8] == b"\x89PNG\r\n\x1a\n" else "image/jpeg"
    if ext == ".flac":
        from mutagen.flac import FLAC, Picture
        f = FLAC(path)
        pic = Picture()
        pic.type, pic.mime, pic.desc, pic.data = 3, mime, "Cover", data
        f.clear_pictures()
        f.add_picture(pic)
        f.save()
    elif ext == ".mp3":
        from mutagen.id3 import APIC, ID3, ID3NoHeaderError
        try:
            tags = ID3(path)
        except ID3NoHeaderError:
            tags = ID3()
        tags.delall("APIC")
        tags.add(APIC(encoding=3, mime=mime, type=3, desc="Cover", data=data))
        tags.save(path)
    elif ext in (".m4a", ".mp4", ".aac", ".alac"):
        from mutagen.mp4 import MP4, MP4Cover
        f = MP4(path)
        f["covr"] = [MP4Cover(data, imageformat=MP4Cover.FORMAT_PNG if mime == "image/png" else MP4Cover.FORMAT_JPEG)]
        f.save()
    elif ext in (".ogg", ".opus"):
        import base64
        from mutagen.flac import Picture
        f = mutagen.File(path)
        pic = Picture()
        pic.type, pic.mime, pic.desc, pic.data = 3, mime, "Cover", data
        f["metadata_block_picture"] = [base64.b64encode(pic.write()).decode()]
        f.save()


async def fetch_cover(release_id: str | None, rgid: str | None = None) -> bytes | None:
    urls = []
    if release_id:
        urls.append(mb.cover_release(release_id, 1200))
    if rgid:
        urls.append(mb.cover_rg(rgid, 1200))
    for url in urls:
        try:
            r = await client().get(url)
            if r.status_code == 200 and r.content:
                return r.content
        except Exception:
            continue
    return None


# ---- naming -----------------------------------------------------------------

_BAD = re.compile(r'[<>:"/\\|?*\x00-\x1f]')


def clean(s: str) -> str:
    s = _BAD.sub("_", str(s or "")).strip().rstrip(".")
    return s[:120] or "_"


class _Fmt(dict):
    def __missing__(self, key):
        return ""


def render_path(values: dict, ext: str, single: bool = False) -> str:
    tpl = config.get("single_template") if single else config.get("naming_template")
    disc_count = int(values.get("disccount") or 1)
    data = _Fmt({
        "artist": clean(values.get("artist")),
        "albumartist": clean(values.get("albumartist") or values.get("artist")),
        "album": clean(values.get("album") or "Unknown Album"),
        "title": clean(values.get("title")),
        "year": clean(values.get("year") or "0000"),
        "genre": clean(values.get("genre") or ""),
        "track": int(values.get("track") or 0),
        "disc": int(values.get("disc") or 1),
        "discprefix": f"{int(values.get('disc') or 1)}-" if disc_count > 1 else "",
        "label": clean(values.get("label") or ""),
        "type": clean(values.get("type") or ""),
        "format": ext.lstrip(".").upper(),
    })
    try:
        rel = tpl.format_map(data)
    except (ValueError, KeyError, IndexError):
        rel = "{albumartist}/{album} ({year})/{track:02d} - {title}".format_map(data)
    rel = rel.replace(" ()", "").replace("(0000)", "").strip()
    parts = [clean(p) for p in rel.split("/") if p.strip()]
    return os.path.join(config.get("library_path"), *parts) + ext.lower()


def move_file(src: str, dest: str) -> str:
    if os.path.abspath(src) == os.path.abspath(dest):
        return src
    os.makedirs(os.path.dirname(dest), exist_ok=True)
    base, ext = os.path.splitext(dest)
    i = 2
    while os.path.exists(dest):
        dest = f"{base} ({i}){ext}"
        i += 1
    shutil.move(src, dest)
    return dest


def prune_empty(path: str, stop: str) -> None:
    """Remove empty dirs (and leftover junk) upward from path until stop."""
    stop = os.path.abspath(stop)
    path = os.path.abspath(path)
    while path.startswith(stop) and path != stop:
        try:
            names = os.listdir(path)
        except OSError:
            return
        junk = [n for n in names if n.lower().endswith((".jpg", ".jpeg", ".png", ".nfo", ".m3u", ".m3u8", ".sfv", ".txt", ".log", ".cue", ".db", ".ini")) or n == ".DS_Store"]
        if len(junk) != len(names):
            return
        for n in junk:
            try:
                os.remove(os.path.join(path, n))
            except OSError:
                return
        try:
            os.rmdir(path)
        except OSError:
            return
        path = os.path.dirname(path)


# ---- matching ---------------------------------------------------------------

def map_files(files: list[dict], tracks: list[dict]) -> tuple[list[tuple[dict, dict | None]], float]:
    """Assign local files to release tracks. Returns pairs and a confidence 0..1."""
    numbered = all(f.get("track") for f in files) and len({(f.get("disc") or 1, f["track"]) for f in files}) == len(files)
    multi_disc = len({t["disc"] for t in tracks}) > 1
    scores = []
    for i, f in enumerate(files):
        for j, t in enumerate(tracks):
            s = sim(f.get("title"), t["title"]) * 0.6
            if numbered:
                same_no = f["track"] == t["track"] and ((f.get("disc") or 1) == t["disc"] or not multi_disc)
                s += 0.3 if same_no else 0
            else:
                s += 0.15
            if f.get("duration") and t["length"]:
                d = abs(f["duration"] - t["length"])
                s += 0.1 if d < 3 else (0.05 if d < 10 else -0.15)
            else:
                s += 0.05
            scores.append((s, i, j))
    scores.sort(reverse=True)
    used_f, used_t, assign = set(), set(), {}
    for s, i, j in scores:
        if i in used_f or j in used_t:
            continue
        used_f.add(i)
        used_t.add(j)
        assign[i] = (j, s)
    pairs, total = [], 0.0
    for i, f in enumerate(files):
        if i in assign:
            j, s = assign[i]
            pairs.append((f, tracks[j]))
            total += max(min(s, 1.0), 0)
        else:
            pairs.append((f, None))
    conf = total / max(len(files), 1)
    if len(files) != len(tracks):
        conf *= max(0.0, 1 - abs(len(files) - len(tracks)) / max(len(tracks), 1))
    return pairs, round(conf, 3)


def release_values(rel: dict, t: dict) -> dict:
    return {
        "title": t["title"],
        "artist": t["artist"] or rel["artist"],
        "album": rel["title"],
        "albumartist": rel["artist"],
        "tracknumber": t["track"],
        "track": t["track"],
        "discnumber": t["disc"],
        "disc": t["disc"],
        "disccount": rel["disc_count"],
        "date": rel["date"],
        "year": rel["year"],
        "genre": rel["genre"],
        "label": rel["label"],
        "barcode": rel["barcode"],
        "catalognumber": rel["catalog"],
        "releasecountry": rel["country"],
        "type": rel["type"],
        "musicbrainz_albumtype": (rel["type"] or "").lower(),
        "musicbrainz_albumstatus": (rel["status"] or "").lower(),
        "isrc": t.get("isrc"),
        "musicbrainz_trackid": t["recording_id"],
        "musicbrainz_releasetrackid": t["track_id"],
        "musicbrainz_albumid": rel["id"],
        "musicbrainz_releasegroupid": rel["release_group_id"],
        "musicbrainz_artistid": t.get("artist_id") or rel["artist_id"],
        "musicbrainz_albumartistid": rel["artist_id"],
    }


WRITE_KEYS = [k for k in TAG_KEYS]


async def candidates_for_album(album_id: str, query_artist: str | None = None, query_album: str | None = None) -> dict:
    tracks = db.query("SELECT * FROM tracks WHERE album_id=? ORDER BY disc, track, path", (album_id,))
    if not tracks:
        raise ValueError("Album not found")
    artist = query_artist if query_artist is not None else tracks[0]["albumartist"]
    album = query_album if query_album is not None else (tracks[0]["album"] or os.path.basename(os.path.dirname(tracks[0]["path"])))
    results = await mb.search_releases(artist if artist != "Unknown Artist" else "", album, 20)
    n = len(tracks)
    for r in results:
        r["match"] = round(r["score"] / 100 * 0.5 + (0.5 if r["track_count"] == n else max(0, 0.5 - abs(r["track_count"] - n) * 0.1)), 3)
    results.sort(key=lambda r: -r["match"])
    return {"artist": artist, "album": album, "track_count": n, "candidates": results}


async def preview(paths: list[str], release_id: str, single: bool = False) -> dict:
    rel = await mb.release(release_id)
    files = []
    for p in paths:
        t = await asyncio.to_thread(library.read_tags, p)
        t["path"] = p
        files.append(t)
    pairs, conf = map_files(files, rel["tracks"])
    rows = []
    for f, t in pairs:
        row = {"path": f["path"], "old_title": f["title"], "old_track": f.get("track"), "old_artist": f["artist"], "old_album": f["album"]}
        if t:
            vals = release_values(rel, t)
            ext = os.path.splitext(f["path"])[1]
            row.update({
                "new_title": t["title"], "new_track": t["track"], "new_disc": t["disc"], "new_artist": vals["artist"],
                "new_path": render_path(vals, ext, single) if config.get("rename_on_import") else f["path"],
                "values": vals,
            })
        rows.append(row)
    return {"release": {k: v for k, v in rel.items() if k != "tracks"}, "tracks": rel["tracks"], "rows": rows, "confidence": conf}


async def apply_preview(pv: dict, rename: bool | None = None, source_root: str | None = None) -> list[str]:
    rename = config.get("rename_on_import") if rename is None else rename
    rel = pv["release"]
    cover = await fetch_cover(rel["id"], rel.get("release_group_id"))
    new_paths, old_dirs = [], set()

    def _go():
        for row in pv["rows"]:
            path = row["path"]
            if not row.get("values"):
                new_paths.append(path)
                continue
            vals = {k: v for k, v in row["values"].items() if k in TAG_KEYS}
            write_tags(path, vals, cover)
            if rename:
                old_dirs.add(os.path.dirname(path))
                path = move_file(path, row["new_path"])
            new_paths.append(path)
        if cover and config.get("save_cover") and new_paths:
            d = os.path.dirname(new_paths[0])
            if not any(os.path.exists(os.path.join(d, n)) for n in ("cover.jpg", "cover.png", "folder.jpg")):
                with open(os.path.join(d, "cover.jpg"), "wb") as fh:
                    fh.write(cover)
        for d in old_dirs:
            prune_empty(d, source_root or config.get("library_path"))

    await asyncio.to_thread(_go)
    return new_paths


async def retag_album(album_id: str, release_id: str, rename: bool | None = None) -> list[str]:
    old = db.query("SELECT path FROM tracks WHERE album_id=?", (album_id,))
    paths = [t["path"] for t in old]
    pv = await preview(paths, release_id)
    new_paths = await apply_preview(pv, rename)
    for p in paths:
        db.execute("DELETE FROM tracks WHERE path=?", (p,))
    await library.index_paths(new_paths)
    cached = os.path.join(db.DATA_DIR, "covers", album_id + ".jpg")
    if os.path.exists(cached):
        os.remove(cached)
    db.log("info", f"Tagged album '{pv['release']['title']}' by {pv['release']['artist']} from MusicBrainz")
    return new_paths


async def auto_match(paths: list[str], artist: str, album: str, rgid: str | None = None) -> dict | None:
    """Find the best release for a set of files. Returns preview or None if not confident."""
    threshold = float(config.get("autotag_threshold"))
    release_ids = []
    if rgid:
        rid = await mb.best_release_for_group(rgid, len(paths))
        if rid:
            release_ids.append(rid)
    if not release_ids and album:
        res = await mb.search_releases(artist, album, 8)
        res.sort(key=lambda r: (-(r["track_count"] == len(paths)), -r["score"]))
        release_ids = [r["id"] for r in res[:3]]
    best = None
    for rid in release_ids:
        pv = await preview(paths, rid)
        if not best or pv["confidence"] > best["confidence"]:
            best = pv
        if pv["confidence"] >= 0.92:
            break
    if best and (best["confidence"] >= threshold or (rgid and best["confidence"] >= threshold - 0.2)):
        return best
    return None


async def import_single(path: str, artist: str, title: str, recording_id: str | None = None, source_root: str | None = None) -> str:
    """Tag a single track (from MB recording if possible) and move it into the library."""
    vals = {"title": title, "artist": artist, "albumartist": artist}
    cover = None
    try:
        rec = None
        if recording_id:
            rec = await mb.recording(recording_id)
        else:
            res = await mb.search_recordings(title, 5, artist=artist)
            res = [r for r in res if sim(r["title"], title) > 0.8 and sim(r["artist"], artist) > 0.6]
            if res:
                rec = await mb.recording(res[0]["id"])
        if rec:
            vals.update({
                "title": rec["title"], "artist": rec["artist"], "albumartist": rec["artist"], "album": rec["album"],
                "date": rec["year"], "year": rec["year"], "isrc": rec["isrc"],
                "musicbrainz_trackid": rec["id"], "musicbrainz_artistid": rec["artist_id"],
                "musicbrainz_albumartistid": rec["artist_id"],
            })
            cover = await fetch_cover(rec["release_id"], rec["release_group_id"])
    except Exception as e:  # noqa: BLE001
        db.log("warning", f"MusicBrainz lookup failed for {artist} - {title}: {e}")
    ext = os.path.splitext(path)[1]

    def _go():
        write_tags(path, {k: v for k, v in vals.items() if k in TAG_KEYS}, cover)
        dest = render_path(vals, ext, single=True)
        new = move_file(path, dest)
        prune_empty(os.path.dirname(path), source_root or config.get("staging_path"))
        return new

    return await asyncio.to_thread(_go)


async def import_album_files(paths: list[str], artist: str, album: str, rgid: str | None, source_root: str) -> list[str]:
    """Tag + move a downloaded album. Falls back to existing tags when MB match is weak."""
    pv = None
    if config.get("autotag_on_import"):
        try:
            pv = await auto_match(paths, artist, album, rgid)
        except Exception as e:  # noqa: BLE001
            db.log("warning", f"Auto-tag failed for {artist} - {album}: {e}")
    if pv:
        return await apply_preview(pv, True, source_root)

    def _go():
        out = []
        for p in paths:
            t = library.read_tags(p)
            vals = {**t, "albumartist": t["albumartist"] if t["albumartist"] != "Unknown Artist" else artist,
                    "album": t["album"] or album}
            dest = render_path(vals, os.path.splitext(p)[1])
            out.append(move_file(p, dest))
            prune_empty(os.path.dirname(p), source_root)
        return out

    db.log("warning", f"No confident MusicBrainz match for {artist} - {album}; imported with existing tags")
    return await asyncio.to_thread(_go)


def edit_tags(track_ids: list[int], values: dict) -> list[str]:
    """Manual tag edit for library tracks."""
    allowed = {"title", "artist", "album", "albumartist", "tracknumber", "discnumber", "date", "genre"}
    vals = {k: v for k, v in values.items() if k in allowed and v is not None}
    paths = []
    for tid in track_ids:
        t = db.one("SELECT path FROM tracks WHERE id=?", (tid,))
        if t:
            write_tags(t["path"], vals)
            paths.append(t["path"])
    return paths
