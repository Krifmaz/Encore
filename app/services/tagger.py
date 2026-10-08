"""MusicBrainz matching, tag writing, cover art and renaming."""
import asyncio
import contextvars
import os
import re
import shutil
from difflib import SequenceMatcher

import mutagen

from .. import config, db
from . import library, musicbrainz as mb
from .http import client


class VerificationFailed(Exception):
    """Downloaded audio turned out to be a different song/version."""


async def _verify(path: str, title: str, artist: str, length: float | None, recording_id: str | None) -> dict | None:
    from . import acoustid, verify
    if not (config.get("verify_on_import") and acoustid.available()):
        return None
    try:
        return await verify.check_file(path, title, artist, length, recording_id)
    except Exception as e:  # noqa: BLE001
        db.log("warning", f"Verification of {os.path.basename(path)} failed: {e}")
        return None


def _record_verification(results: dict[str, dict]) -> None:
    import time as _t
    for path, res in results.items():
        if res:
            db.execute("UPDATE tracks SET verify_status=?, verify_detail=?, acoustid_id=?, verified_at=? WHERE path=?",
                       (res["status"], res["detail"], res.get("acoustid"), _t.time(), path))


def _existing_copy(vals: dict, exclude: set[str]) -> dict | None:
    from .matcher import LibraryIndex, describe
    idx = LibraryIndex(db.query("SELECT id, path, title, artist, album, album_id, duration, isrc, mb_trackid, format, bitrate, verify_status FROM tracks"))
    want = describe(vals.get("title"), vals.get("artist"), length=vals.get("length"), isrc=vals.get("isrc"), mbid=vals.get("musicbrainz_trackid"))
    hit, _ = idx.find(want)
    return hit if hit and hit["path"] not in exclude else None

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


# ---- artwork for releases MusicBrainz doesn't know ---------------------------------


# set by the worker around an import: the cover URL that came with the request (Spotify/Deezer/YT Music…)
COVER_HINT: contextvars.ContextVar = contextvars.ContextVar("cover_hint", default=None)

_IMG_MAGIC = (b"\xff\xd8\xff", b"\x89PNG", b"RIFF")


async def _get_image(url: str) -> bytes | None:
    try:
        r = await client().get(url, timeout=15)
    except Exception:  # noqa: BLE001
        return None
    if r.status_code == 200 and r.content[:4].startswith(_IMG_MAGIC) and len(r.content) > 1500:
        return r.content
    return None


def _big(url: str) -> str:
    """Ask image CDNs for a large version."""
    url = re.sub(r"=w\d+-h\d+[^&?/]*$", "=w1200-h1200-l90-rj", url)                  # YouTube Music / Google
    url = re.sub(r"/\d+x\d+(bb)?\.(jpg|png|webp)$", r"/1200x1200bb.jpg", url)         # Apple/iTunes
    url = re.sub(r"/(\d+)x\1-\d+-\d+-\d+-\d+\.jpg$", "/1000x1000-000000-80-0-0.jpg", url)  # Deezer
    return url


async def _square(data: bytes) -> bytes:
    """YouTube video thumbnails are 16:9 – crop the middle square."""
    proc = await asyncio.create_subprocess_exec(
        "ffmpeg", "-v", "error", "-i", "pipe:0", "-vf", "crop='min(iw,ih)':'min(iw,ih)'", "-f", "image2", "-c:v", "mjpeg", "pipe:1",
        stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
    out, _ = await proc.communicate(data)
    return out if proc.returncode == 0 and out else data


async def find_art(artist: str, album: str | None = None, title: str | None = None, hint: str | None = None) -> tuple[bytes, str] | None:
    """Cover art when MusicBrainz has none: the request's own cover, then Deezer, then iTunes, then YouTube.

    Search hits must match the artist and album/title so we never embed the wrong artwork. Returns (bytes, where)."""
    hint = hint or COVER_HINT.get()
    if hint and "ytimg.com" not in hint:
        data = await _get_image(_big(hint)) or await _get_image(hint)
        if data:
            return data, "request"
    want_artist = artist or ""
    for name, q in (("album", album), ("track", title)):
        if not q:
            continue
        try:
            r = await client().get(f"https://api.deezer.com/search/{name}", params={"q": f'artist:"{want_artist}" {name}:"{q}"', "limit": 5})
            for hit in r.json().get("data", []):
                h_artist = (hit.get("artist") or {}).get("name", "")
                h_title = hit.get("title", "")
                alb = hit if name == "album" else (hit.get("album") or {})
                if sim(h_artist, want_artist) >= 0.8 and sim(h_title, q) >= 0.75 and alb.get("cover_xl"):
                    data = await _get_image(alb["cover_xl"])
                    if data:
                        return data, "Deezer"
        except Exception:  # noqa: BLE001
            pass
        try:
            r = await client().get("https://itunes.apple.com/search", params={"term": f"{want_artist} {q}", "entity": "album" if name == "album" else "song", "limit": 5})
            for hit in r.json().get("results", []):
                h_title = hit.get("collectionName" if name == "album" else "trackName", "")
                if sim(hit.get("artistName", ""), want_artist) >= 0.8 and sim(h_title, q) >= 0.75 and hit.get("artworkUrl100"):
                    data = await _get_image(_big(hit["artworkUrl100"]))
                    if data:
                        return data, "iTunes"
        except Exception:  # noqa: BLE001
            pass
    if hint:
        data = await _get_image(_big(hint)) or await _get_image(hint)
        if data:
            return await _square(data), "YouTube"
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
    if rel.get("provider") and rel["provider"] != "MusicBrainz":
        mbkeys = ("musicbrainz_trackid", "musicbrainz_releasetrackid", "musicbrainz_albumid", "musicbrainz_releasegroupid",
                  "musicbrainz_artistid", "musicbrainz_albumartistid", "musicbrainz_albumtype", "musicbrainz_albumstatus")
        return {k: v for k, v in _release_values(rel, t).items() if k not in mbkeys}
    return _release_values(rel, t)


def _release_values(rel: dict, t: dict) -> dict:
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


async def candidates_for_album(album_id: str, query_artist: str | None = None, query_album: str | None = None, provider: str = "musicbrainz") -> dict:
    tracks = db.query("SELECT * FROM tracks WHERE album_id=? ORDER BY disc, track, path", (album_id,))
    if not tracks:
        raise ValueError("Album not found")
    artist = query_artist if query_artist is not None else tracks[0]["albumartist"]
    album = query_album if query_album is not None else (tracks[0]["album"] or os.path.basename(os.path.dirname(tracks[0]["path"])))
    from . import meta_providers
    n = len(tracks)
    results = await meta_providers.search(provider, artist if artist != "Unknown Artist" else "", album, n, [t["path"] for t in tracks])
    for r in results:
        tc = r.get("track_count") or n
        r["match"] = round(r["score"] / 100 * 0.5 + (0.5 if tc == n else max(0, 0.5 - abs(tc - n) * 0.1)), 3)
    results.sort(key=lambda r: -r["match"])
    return {"artist": artist, "album": album, "track_count": n, "candidates": results, "provider": provider}


async def preview(paths: list[str], release_id: str, single: bool = False) -> dict:
    from . import meta_providers
    rel = await meta_providers.release(release_id)
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
    cover = await _get_image(rel["cover_url"]) if rel.get("cover_url") else None
    cover = cover or (await fetch_cover(rel["id"], rel.get("release_group_id")) if rel.get("provider") in (None, "MusicBrainz") else None)
    if not cover:
        art = await find_art(rel["artist"], rel["title"])
        cover = art[0] if art else None
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
    relink([r["path"] for r in pv["rows"]], new_paths)
    await library.index_paths(new_paths)
    cached = os.path.join(db.DATA_DIR, "covers", album_id + ".jpg")
    if os.path.exists(cached):
        os.remove(cached)
    db.log("info", f"Tagged album '{pv['release']['title']}' by {pv['release']['artist']} from {pv['release'].get('provider') or 'MusicBrainz'}")
    return new_paths


def relink(old_paths: list[str], new_paths: list[str]) -> None:
    """Files were renamed/moved: keep their library rows (and so likes/playlist entries) by updating the path."""
    for old, new in zip(old_paths, new_paths):
        if old == new:
            continue
        clash = db.one("SELECT id FROM tracks WHERE path=?", (new,))
        if clash:
            db.execute("UPDATE playlist_items SET track_id=(SELECT id FROM tracks WHERE path=?) WHERE track_id=?", (old, clash["id"]))
            db.execute("DELETE FROM tracks WHERE id=?", (clash["id"],))
        db.execute("UPDATE tracks SET path=? WHERE path=?", (new, old))


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


async def import_single(path: str, artist: str, title: str, recording_id: str | None = None,
                        source_root: str | None = None, length: float | None = None, reject: bool = True) -> tuple[str, str]:
    """Verify, tag (from MB recording if possible), de-duplicate and move a single track into the library.

    Returns (final path, note)."""
    check = await _verify(path, title, artist, length, recording_id)
    if check and check["status"] == "mismatch" and reject and config.get("reject_mismatch"):
        try:
            os.remove(path)
        except OSError:
            pass
        raise VerificationFailed(check["detail"])
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
    if not cover:
        art = await find_art(vals["artist"], vals.get("album"), vals["title"])
        cover = art[0] if art else None
    ext = os.path.splitext(path)[1]
    from .verify import delete_tracks, quality
    existing = _existing_copy(vals, {path}) if config.get("dedupe_on_import") else None
    if existing:
        new_q = library.read_tags(path)
        # only replace the existing file if the new one is strictly better (verified / lossless / bitrate / tagged)
        if quality({**new_q, "verify_status": (check or {}).get("status")})[:4] <= quality(existing)[:4]:
            os.remove(path)
            prune_empty(os.path.dirname(path), source_root or config.get("staging_path"))
            return existing["path"], "already in library – kept existing copy"

    def _go():
        write_tags(path, {k: v for k, v in vals.items() if k in TAG_KEYS}, cover)
        dest = render_path(vals, ext, single=True)
        new = move_file(path, dest)
        prune_empty(os.path.dirname(path), source_root or config.get("staging_path"))
        return new

    new = await asyncio.to_thread(_go)
    await library.index_paths([new])
    _record_verification({new: check})
    note = ""
    if existing:
        nt = db.one("SELECT id FROM tracks WHERE path=?", (new,))
        await asyncio.to_thread(delete_tracks, [existing["id"]], nt["id"] if nt else None)
        note = "replaced a lower-quality copy"
    return new, note


async def import_album_files(paths: list[str], artist: str, album: str, rgid: str | None, source_root: str,
                             reject: bool = True, source_meta: dict | None = None) -> list[str]:
    """Tag + move a downloaded album. Falls back to existing tags when MB match is weak."""
    pv = None
    if config.get("autotag_on_import"):
        try:
            pv = await auto_match(paths, artist, album, rgid)
        except Exception as e:  # noqa: BLE001
            db.log("warning", f"Auto-tag failed for {artist} - {album}: {e}")
    checks: dict[str, dict] = {}
    if pv:
        mism = []
        for row in pv["rows"]:
            v = row.get("values")
            if not v:
                continue
            res = await _verify(row["path"], v["title"], v["artist"], None, v.get("musicbrainz_trackid"))
            checks[row["path"]] = res
            if res and res["status"] == "mismatch":
                mism.append(f"{v['title']}: {res['detail']}")
        if mism and reject and config.get("reject_mismatch") and len(mism) > max(1, len(paths) // 4):
            raise VerificationFailed(f"{len(mism)} of {len(paths)} files are not what they claim – e.g. {mism[0]}")
        old_to_new = {}
        new = await apply_preview(pv, True, source_root)
        for row, p in zip(pv["rows"], new):
            old_to_new[p] = checks.get(row["path"])
        await library.index_paths(new)
        _record_verification(old_to_new)
        await _drop_superseded(new)
        return new

    art = await find_art(artist, album)
    cover = art[0] if art else None

    src_tracks = list((source_meta or {}).get("tracks") or [])

    def _source_track(t: dict, i: int) -> dict | None:
        """The Deezer/Apple track a file is: closest title, else same disc+number, else same position."""
        best = max(src_tracks, key=lambda x: sim(x["title"], t["title"]), default=None)
        if best and sim(best["title"], t["title"]) >= 0.8:
            return best
        same = [x for x in src_tracks if t["track"] and x.get("track") == t["track"] and (x.get("disc") or 1) == (t["disc"] or 1)]
        if same and len({x.get("track") for x in src_tracks}) == len(src_tracks):
            return same[0]
        return src_tracks[i] if len(src_tracks) == len(paths) else None

    def _go():
        out = []
        for i, p in enumerate(sorted(paths)):
            t = library.read_tags(p)
            vals = {**t, "albumartist": t["albumartist"] if t["albumartist"] != "Unknown Artist" else artist,
                    "album": t["album"] or album}
            st = _source_track(t, i) if src_tracks else None
            if source_meta:
                # not in MusicBrainz: tag from the store's metadata (left untagged in MB terms → shows under "Needs tagging")
                vals.update(album=album, albumartist=artist, year=source_meta.get("year") or t["year"])
                if st:
                    vals.update(title=st["title"], artist=st.get("artist") or artist, track=st.get("track"), disc=st.get("disc") or 1)
                try:
                    write_tags(p, {"title": vals["title"], "artist": vals["artist"], "album": album, "albumartist": artist,
                                   "tracknumber": vals.get("track"), "discnumber": vals.get("disc"), "date": vals.get("year"),
                                   "isrc": (st or {}).get("isrc"), "barcode": source_meta.get("upc")})
                except Exception as e:  # noqa: BLE001
                    db.log("warning", f"Could not tag {os.path.basename(p)}: {e}")
            if cover and config.get("embed_cover"):
                try:
                    embed_cover(p, cover)
                except Exception:  # noqa: BLE001
                    pass
            dest = render_path(vals, os.path.splitext(p)[1])
            out.append(move_file(p, dest))
            prune_empty(os.path.dirname(p), source_root)
        if cover and config.get("save_cover") and out:
            d = os.path.dirname(out[0])
            if all(os.path.dirname(x) == d for x in out) and not os.path.exists(os.path.join(d, "cover.jpg")):
                with open(os.path.join(d, "cover.jpg"), "wb") as fh:
                    fh.write(cover)
        return out

    db.log("warning", f"No confident MusicBrainz match for {artist} - {album}; imported with existing tags")
    new = await asyncio.to_thread(_go)
    await library.index_paths(new)
    await _drop_superseded(new)
    return new


async def _drop_superseded(new_paths: list[str]) -> None:
    """After an album import, remove copies it makes redundant: loose singles of the same recordings,
    and any lower-quality copy (e.g. a YouTube/MP3 version replaced by FLAC). Playlists follow the new file."""
    if not config.get("dedupe_on_import"):
        return
    from .matcher import LibraryIndex, describe
    from .verify import delete_tracks, quality
    new_set = set(new_paths)
    sizes = {r["album_id"]: r["n"] for r in db.query("SELECT album_id, COUNT(*) n FROM tracks GROUP BY album_id")}
    others = [t for t in db.query("""SELECT id, path, title, artist, album, album_id, duration, isrc, mb_trackid, format, bitrate, verify_status
                                     FROM tracks""") if t["path"] not in new_set]
    if not others:
        return
    idx = LibraryIndex(others)
    for p in new_paths:
        t = db.one("SELECT id, title, artist, duration, isrc, mb_trackid, format, bitrate, verify_status, path FROM tracks WHERE path=?", (p,))
        if not t:
            continue
        hit, _ = idx.find(describe(t["title"], t["artist"], length=t["duration"], isrc=t["isrc"], mbid=t["mb_trackid"]))
        if not hit:
            continue
        loose_single = sizes.get(hit["album_id"], 0) == 1
        if loose_single or quality(t)[:4] > quality(hit)[:4]:
            await asyncio.to_thread(delete_tracks, [hit["id"]], t["id"])
            why = "loose single" if loose_single else "lower-quality copy"
            db.log("info", f"Removed {why} of “{hit['artist']} – {hit['title']}” – replaced by the new album import")


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
