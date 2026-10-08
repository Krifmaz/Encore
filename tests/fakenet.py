"""A fake internet for end-to-end tests.

Every outbound HTTP request Encore makes goes through `app.services.http.client()`; tests point that client
at this ASGI app, which routes on the request's Host header and imitates the real APIs (MusicBrainz, Cover
Art Archive, AcoustID, Lidarr, slskd, Deezer, Spotify, Tidal, Apple Music, Qobuz, ListenBrainz, Last.fm,
Jellyfin, Plex + plex.tv, Navidrome and notification webhooks). State lives on `NET` so tests can assert on
what Encore did (likes pushed, playlists created, notifications sent…).
"""
import json
import os
import subprocess
from collections import defaultdict

from fastapi import FastAPI, Request
from fastapi.responses import HTMLResponse, JSONResponse, Response

# ---------------------------------------------------------------- catalogue ----------
ARTIST = {"id": "a-fake", "name": "Fake Band"}
# title, recording id, seconds, isrc
ALBUM_TRACKS = [("Alpha", "rec-alpha", 5, "QZAAA0000001"), ("Beta", "rec-beta", 6, "QZAAA0000002"), ("Gamma", "rec-gamma", 7, "QZAAA0000003")]
SINGLE = ("Delta", "rec-delta", 8, "QZAAA0000004")
WRONG_SECONDS = 11  # audio of this length fingerprints as a different song
DURATION_TO_REC = {5: "rec-alpha", 6: "rec-beta", 7: "rec-gamma", 8: "rec-delta"}
REC_TITLES = {"rec-alpha": "Alpha", "rec-beta": "Beta", "rec-gamma": "Gamma", "rec-delta": "Delta"}
JPEG = bytes.fromhex("ffd8ffe000104a46494600010100000100010000ffdb004300") + b"\x08" * 4000 + bytes.fromhex("ffd9")


def make_audio(path: str, title: str, artist: str = "Fake Band", album: str = "", track: int = 1, seconds: int = 5, isrc: str = ""):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    meta = ["-metadata", f"title={title}", "-metadata", f"artist={artist}", "-metadata", f"track={track}"]
    if album:
        meta += ["-metadata", f"album={album}"]
    if isrc:
        meta += ["-metadata", f"isrc={isrc}"]
    subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-f", "lavfi", "-i", f"sine=frequency={300 + seconds * 40}:duration={seconds}",
                    *meta, path], check=True)
    return path


def credit(name=ARTIST["name"], aid=ARTIST["id"]):
    return [{"name": name, "joinphrase": "", "artist": {"id": aid, "name": name}}]


def rg(rid, title, ptype="Album", secondary=None, date="2020-01-01"):
    return {"id": rid, "title": title, "primary-type": ptype, "secondary-types": secondary or [], "first-release-date": date,
            "artist-credit": credit()}


RGS = {"rg-album": rg("rg-album", "Fake Album"), "rg-single": rg("rg-single", "Delta", "Single", date="2021-05-05"),
       "rg-comp": rg("rg-comp", "Hits Compilation", "Album", ["Compilation"], "2019-01-01")}
RELEASES = {
    "r-album": {"rg": "rg-album", "title": "Fake Album", "date": "2020-01-01", "tracks": ALBUM_TRACKS},
    "r-single": {"rg": "rg-single", "title": "Delta", "date": "2021-05-05", "tracks": [SINGLE]},
    "r-comp": {"rg": "rg-comp", "title": "Hits Compilation", "date": "2019-01-01", "tracks": [ALBUM_TRACKS[0]]},
}


def release_json(rid, full=True):
    r = RELEASES[rid]
    base = {"id": rid, "title": r["title"], "date": r["date"], "status": "Official", "country": "XW",
            "release-group": RGS[r["rg"]], "artist-credit": credit(), "track-count": len(r["tracks"]),
            "media": [{"position": 1, "format": "Digital Media", "track-count": len(r["tracks"])}],
            "label-info": [{"label": {"name": "Fake Records"}, "catalog-number": "FR001"}], "barcode": "0000"}
    if full:
        base["media"][0]["tracks"] = [{"id": f"t-{rec}", "position": i + 1, "number": str(i + 1), "title": t, "length": s * 1000,
                                       "recording": {"id": rec, "title": t, "length": s * 1000, "isrcs": [isrc], "artist-credit": credit()},
                                       "artist-credit": credit()} for i, (t, rec, s, isrc) in enumerate(r["tracks"])]
        base["genres"] = [{"name": "electronic", "count": 3}]
    return base


def recording_json(rec):
    rels = [rid for rid, r in RELEASES.items() if any(x[1] == rec for x in r["tracks"])]
    rels.sort(key=lambda rid: RELEASES[rid]["date"])
    t = next(x for r in RELEASES.values() for x in r["tracks"] if x[1] == rec)
    return {"id": rec, "title": t[0], "length": t[2] * 1000, "isrcs": [t[3]], "artist-credit": credit(),
            "first-release-date": RELEASES[rels[0]]["date"],
            "releases": [{**{k: v for k, v in release_json(rid, False).items() if k not in ("media",)}, "release-group": RGS[RELEASES[rid]["rg"]]} for rid in rels]}


class State:
    def __init__(self):
        self.reset()

    def reset(self):
        self.calls = []
        self.notifications = []
        self.library_path = None
        self.slskd_path = None
        self.lidarr = {"artists": {}, "albums": {}, "commands": []}
        self.slskd = {"transfers": defaultdict(list)}
        self.jf = {"favs": set(), "playlists": {}, "refreshed": 0}
        self.plex = {"ratings": {}, "playlists": {}, "pins": {}, "refreshed": 0}
        self.nd = {"stars": set(), "playlists": {}, "scans": 0}
        self.lb = {"loves": {}, "playlists": {}}
        self.spotify_codes = {}
        self.tidal_codes = {}
        self.server_paths = {}  # item id → path relative to library


NET = State()
app = FastAPI()


@app.middleware("http")
async def by_host(request: Request, call_next):
    host = request.headers.get("host", "").split(":")[0]
    NET.calls.append((request.method, host, request.url.path))
    # MusicBrainz release browse by release-group (dashed query param can't be a function argument)
    if host == "mb.test" and request.url.path == "/ws/2/release" and "release-group" in request.query_params:
        return JSONResponse(_release_browse("", request.query_params["release-group"]))
    request.scope["path"] = f"/{host}{request.scope['path']}"
    return await call_next(request)


def lib_items():
    """Library files as the media servers would list them (ids derived from paths)."""
    import mutagen
    out = []
    root = NET.library_path
    for dp, _, fs in os.walk(root):
        for f in sorted(fs):
            if not f.endswith((".mp3", ".flac", ".m4a", ".opus")):
                continue
            p = os.path.join(dp, f)
            rel = os.path.relpath(p, root)
            tags = mutagen.File(p, easy=True) or {}
            g = lambda k: (tags.get(k) or [""])[0]  # noqa: E731
            iid = "i" + str(abs(hash(rel)) % 10**8)
            NET.server_paths[iid] = rel
            out.append({"id": iid, "rel": rel, "title": g("title"), "artist": g("artist"), "album": g("album"),
                        "duration": round(getattr(getattr(tags, "info", None), "length", 0) or 0)})
    return out


# ================================================================ MusicBrainz =========
@app.get("/mb.test/ws/2/artist")
def mb_artist_search(query: str = ""):
    return {"artists": [{"id": ARTIST["id"], "name": ARTIST["name"], "type": "Group", "country": "GB", "score": 100,
                         "life-span": {"begin": "2010"}, "tags": [{"name": "electronic", "count": 2}]}] if "fake" in query.lower() else []}


@app.get("/mb.test/ws/2/artist/{aid}")
def mb_artist(aid: str):
    return {"id": aid, "name": ARTIST["name"], "type": "Group", "country": "GB", "life-span": {"begin": "2010"},
            "tags": [{"name": "electronic", "count": 2}], "relations": [{"type": "bandcamp", "url": {"resource": "https://fake.bandcamp.com"}},
                                                     {"type": "wikidata", "url": {"resource": "https://www.wikidata.org/wiki/Q42"}},
                                                     {"type": "discogs", "url": {"resource": "https://www.discogs.com/artist/999-Fake-Band"}}]}


@app.get("/mb.test/ws/2/release-group")
def mb_rg(query: str = "", artist: str = "", offset: int = 0):
    if artist:
        return {"release-groups": list(RGS.values()), "release-group-count": len(RGS)}
    q = query.lower()
    hits = [{**g, "score": 100} for g in RGS.values() if g["title"].lower() in q or "fake" in q]
    return {"release-groups": hits}


@app.get("/mb.test/ws/2/release-group/{rid}")
def mb_rg_one(rid: str):
    return {**RGS[rid], "releases": [{"id": k} for k, v in RELEASES.items() if v["rg"] == rid], "tags": []}


@app.get("/mb.test/ws/2/release")
def mb_release_q(query: str = "", artist: str = "", offset: int = 0):
    if artist:   # browse by artist (barcodes for catalog matching)
        rels = [{**release_json(rid, False), "barcode": "5099000001" if rid == "r-album" else ""} for rid in RELEASES]
        return {"releases": rels, "release-count": len(rels)}
    return _release_browse(query, None)


def _release_browse(query, rgid):
    q = query.lower()
    rels = [rid for rid, r in RELEASES.items() if (rgid and r["rg"] == rgid) or (not rgid and (r["title"].lower() in q))]
    return {"releases": [{**release_json(rid, False), "score": 100} for rid in rels]}


@app.get("/mb.test/ws/2/release/{rid}")
def mb_release(rid: str):
    return release_json(rid)


@app.get("/mb.test/ws/2/recording")
def mb_rec_search(query: str = ""):
    q = query.lower()
    out = []
    for rec, title in REC_TITLES.items():
        if title.lower() in q:
            j = recording_json(rec)
            out.append({**j, "score": 100, "releases": [{"id": r["id"], "title": r["title"], "date": r["date"], "release-group": r["release-group"]} for r in j["releases"]]})
    return {"recordings": out}


@app.get("/mb.test/ws/2/recording/{rec}")
def mb_rec(rec: str):
    return recording_json(rec)


@app.get("/mb.test/ws/2/isrc/{isrc}")
def mb_isrc(isrc: str):
    for r in RELEASES.values():
        for t in r["tracks"]:
            if t[3] == isrc:
                return {"isrc": isrc, "recordings": [{"id": t[1], "title": t[0]}]}
    return JSONResponse({"error": "Not Found"}, 404)


# ========================================================== Cover Art / AcoustID ==========
@app.get("/coverartarchive.org/{path:path}")
def caa(path: str):
    return Response(JPEG, media_type="image/jpeg")


@app.post("/api.acoustid.org/v2/lookup")
async def acoustid_lookup(request: Request):
    form = await request.form()
    if form.get("client") != "good-key":
        return {"status": "error", "error": {"code": 4, "message": "invalid API key"}}
    dur = int(float(form.get("duration") or 0))
    rec = DURATION_TO_REC.get(dur)
    if dur == WRONG_SECONDS:
        recs = [{"id": "rec-other", "title": "Totally Different Song", "artists": [{"name": "Someone Else"}], "duration": 11}]
    elif rec:
        recs = [{"id": rec, "title": REC_TITLES[rec], "artists": [{"name": ARTIST["name"]}], "duration": dur}]
    else:
        return {"status": "ok", "results": []}
    return {"status": "ok", "results": [{"id": f"ac-{dur}", "score": 0.97, "recordings": recs}]}


@app.get("/api.acoustid.org/v2/lookup")
def acoustid_check(client: str = ""):
    return {"status": "ok"} if client == "good-key" else {"status": "error", "error": {"message": "invalid API key"}}


# ================================================================ Lidarr =============
L = "/lidarr.test/api/v1"


@app.get(L + "/system/status")
def lidarr_status():
    return {"version": "3.0.0-fake"}


@app.get(L + "/qualityprofile")
def lidarr_qp():
    return [{"id": 1, "name": "Lossless"}]


@app.get(L + "/metadataprofile")
def lidarr_mp():
    return [{"id": 1, "name": "Standard"}]


@app.get(L + "/rootfolder")
def lidarr_root():
    return [{"path": NET.library_path, "freeSpace": 10**12}]


@app.get(L + "/artist")
def lidarr_artists():
    return list(NET.lidarr["artists"].values())


@app.get(L + "/artist/lookup")
def lidarr_lookup(term: str):
    return [{"foreignArtistId": ARTIST["id"], "artistName": ARTIST["name"]}]


@app.post(L + "/artist")
async def lidarr_add(request: Request):
    body = await request.json()
    a = {**body, "id": 1}
    NET.lidarr["artists"][1] = a
    return a


@app.put(L + "/artist/{aid}")
async def lidarr_put(aid: int, request: Request):
    NET.lidarr["artists"][aid] = await request.json()
    return NET.lidarr["artists"][aid]


@app.get(L + "/album")
def lidarr_album_q(foreignAlbumId: str = "", includeAllArtistAlbums: str = ""):
    if foreignAlbumId:
        a = NET.lidarr["albums"].get(foreignAlbumId)
        return [a] if a else []
    return list(NET.lidarr["albums"].values())


@app.put(L + "/album/monitor")
def lidarr_monitor():
    return {}


@app.get(L + "/album/{aid}")
def lidarr_album(aid: int):
    return next(a for a in NET.lidarr["albums"].values() if a["id"] == aid)


@app.post(L + "/command")
async def lidarr_command(request: Request):
    body = await request.json()
    NET.lidarr["commands"].append(body["name"])
    if body["name"] == "RefreshArtist":
        NET.lidarr["albums"]["rg-album"] = {"id": 10, "title": "Fake Album", "foreignAlbumId": "rg-album",
                                            "statistics": {"trackFileCount": 0, "percentOfTracks": 0}}
    if body["name"] == "AlbumSearch":
        # "grab + import" instantly into the shared library folder
        for i, (t, rec, s, isrc) in enumerate(ALBUM_TRACKS, 1):
            make_audio(os.path.join(NET.library_path, "Fake Band", "Fake Album (2020)", f"{i:02d} - {t}.flac"), t, album="Fake Album", track=i, seconds=s)
        NET.lidarr["albums"]["rg-album"]["statistics"] = {"trackFileCount": 3, "percentOfTracks": 100}
    return {"id": len(NET.lidarr["commands"])}


@app.get(L + "/release")
def lidarr_releases(albumId: int):
    return [{"guid": "g-flac", "indexerId": 1, "title": "Fake Band - Fake Album (2020) [FLAC]", "indexer": "FakeIndexer", "protocol": "torrent",
             "size": 300_000_000, "seeders": 12, "approved": True, "quality": {"quality": {"name": "FLAC"}}, "rejections": []},
            {"guid": "g-mp3", "indexerId": 1, "title": "Fake Band - Fake Album MP3", "indexer": "FakeIndexer", "protocol": "usenet",
             "size": 90_000_000, "approved": False, "quality": {"quality": {"name": "MP3-320"}}, "rejections": ["Not wanted in profile"]}]


@app.post(L + "/release")
async def lidarr_grab(request: Request):
    body = await request.json()
    NET.lidarr["commands"].append(f"grab:{body['guid']}")
    for i, (t, rec, s, isrc) in enumerate(ALBUM_TRACKS, 1):
        make_audio(os.path.join(NET.library_path, "Fake Band", "Fake Album (2020)", f"{i:02d} - {t}.flac"), t, album="Fake Album", track=i, seconds=s)
    NET.lidarr["albums"]["rg-album"]["statistics"] = {"trackFileCount": 3, "percentOfTracks": 100}
    return {}


@app.get(L + "/queue")
def lidarr_queue():
    return {"records": []}


# ================================================================= slskd ==============
S = "/slskd.test/api/v0"
SLSK_ALBUM = [f"Music\\Fake Band\\Fake Album (2020) [FLAC]\\{i:02d} - {t}.flac" for i, (t, *_ ) in enumerate(ALBUM_TRACKS, 1)]
_searches: dict = {}


@app.get(S + "/application")
def slskd_app():
    return {"version": {"full": "0.21.0-fake"}, "server": {"state": "Connected, LoggedIn"}}


@app.post(S + "/searches")
async def slskd_search(request: Request):
    b = await request.json()
    _searches[b["id"]] = b["searchText"]
    return {"id": b["id"]}


@app.get(S + "/searches/{sid}")
def slskd_search_state(sid: str):
    return {"id": sid, "isComplete": True}


@app.get(S + "/searches/{sid}/responses")
def slskd_responses(sid: str):
    text = _searches.get(sid, "").lower()
    out = []
    if "album" in text:
        out.append({"username": "peer", "hasFreeUploadSlot": True, "queueLength": 0, "uploadSpeed": 2_000_000,
                    "files": [{"filename": f, "size": 1000, "bitDepth": 16} for f in SLSK_ALBUM]})
        out.append({"username": "mp3peer", "hasFreeUploadSlot": False, "queueLength": 3, "uploadSpeed": 500_000,
                    "files": [{"filename": f.replace("[FLAC]", "[MP3]").replace(".flac", ".mp3"), "size": 800, "bitRate": 320} for f in SLSK_ALBUM]})
    if "delta" in text:
        out.append({"username": "peer2", "hasFreeUploadSlot": True, "queueLength": 0, "uploadSpeed": 1_000_000,
                    "files": [{"filename": "Music\\Fake Band\\Fake Band - Delta.flac", "size": 1000, "length": 8}]})
    if "liar" in text:  # a user whose files are the wrong song
        out.append({"username": "liar", "hasFreeUploadSlot": True, "queueLength": 0, "uploadSpeed": 1,
                    "files": [{"filename": "Music\\Fake Band\\Fake Band - Liar Song.flac", "size": 1000}]})
    return out


@app.delete(S + "/searches/{sid}")
def slskd_del(sid: str):
    return {}


@app.post(S + "/transfers/downloads/{user}")
async def slskd_enqueue(user: str, request: Request):
    for i, f in enumerate(await request.json()):
        parts = f["filename"].split("\\")
        name, parent = parts[-1], parts[-2]
        dest = os.path.join(NET.slskd_path, parent, name)
        title = name.split(" - ")[-1].rsplit(".", 1)[0]
        secs = {"Alpha": 5, "Beta": 6, "Gamma": 7, "Delta": 8}.get(title, WRONG_SECONDS)
        make_audio(dest, title, album="Fake Album" if "Album" in parent else "", track=i + 1, seconds=secs)
        NET.slskd["transfers"][user].append({"id": f"{user}-{i}", "filename": f["filename"], "state": "Completed, Succeeded",
                                             "size": f["size"], "bytesTransferred": f["size"], "percentComplete": 100})
    return {}


@app.get(S + "/transfers/downloads/{user}")
def slskd_user(user: str):
    return {"username": user, "directories": [{"directory": "x", "files": NET.slskd["transfers"][user]}]}


@app.get(S + "/transfers/downloads")
def slskd_all():
    return [{"username": u, "directories": [{"directory": "Music\\x", "files": fs}]} for u, fs in NET.slskd["transfers"].items()]


@app.delete(S + "/transfers/downloads/{rest:path}")
def slskd_cancel(rest: str):
    return {}


# ================================================================= Deezer =============
DZ = "/api.deezer.com"


def dz_track(i, t, s, isrc, album_id=123):
    return {"id": 1000 + i, "title": t, "duration": s, "isrc": isrc, "artist": {"name": ARTIST["name"]}, "preview": f"https://cdns-preview.dzcdn.net/{1000 + i}.mp3",
            "album": {"id": album_id, "title": "Fake Album", "cover_xl": "https://coverartarchive.org/x.jpg"}}


@app.get(DZ + "/track/{tid}")
def dz_track_get(tid: str):
    if tid.startswith("isrc:"):
        isrc = tid[5:]
        for i, (t, rec, s, code) in enumerate(ALBUM_TRACKS + [SINGLE]):
            if code == isrc:
                return dz_track(i, t, s, code, 123 if i < 3 else 789)
        return {"error": {"message": "no data"}}
    return dz_track(0, *[ALBUM_TRACKS[0][0], ALBUM_TRACKS[0][2], ALBUM_TRACKS[0][3]])


@app.get(DZ + "/search/album")
def dz_search_album(q: str, limit: int = 15):
    return {"data": [{"id": 123, "title": "Fake Album", "nb_tracks": 3, "artist": {"name": ARTIST["name"]},
                      "cover_xl": "https://e-cdns-images.dzcdn.net/images/cover/123/1000x1000.jpg", "cover_medium": "https://e-cdns-images.dzcdn.net/images/cover/123/250x250.jpg"}]
            if "fake album" in q.lower() or "fake band fake album" in q.lower() else []}


@app.get(DZ + "/search/track")
def dz_search_track(q: str):
    return {"data": [dz_track(i, t, sec, code, 789 if i == 3 else 123) for i, (t, _, sec, code) in enumerate(ALBUM_TRACKS + [SINGLE])
                     if f'track:"{t.lower()}"' in q.lower() or ("track:" not in q and t.lower() in q.lower())]}


@app.get(DZ + "/playlist/{pid}")
def dz_playlist(pid: str):
    return {"id": pid, "title": "Deezer Mix", "description": "", "picture_xl": None}


@app.get(DZ + "/playlist/{pid}/tracks")
def dz_playlist_tracks(pid: str):
    return {"data": [dz_track(0, "Alpha", 5, ALBUM_TRACKS[0][3]), {**dz_track(9, "Alpha", 5, None), "title_version": "(Club Remix)"},
                     dz_track(3, "Delta", 8, SINGLE[3], 789)]}


@app.get(DZ + "/search/artist")
def dz_search_artist(q: str):
    return {"data": [{"id": 77, "name": ARTIST["name"], "nb_fan": 1234, "picture_xl": "https://e-cdns-images.dzcdn.net/images/artist/abc/1000x1000.jpg"},
                     {"id": 78, "name": "Fake Band Tribute", "nb_fan": 5, "picture_xl": ""}] if "fake" in q.lower() else []}


DZ_ALBUMS = {
    123: {"title": "Fake Album (Deluxe Edition)", "record_type": "album", "release_date": "2020-01-01", "upc": "", "isrcs": [t[3] for t in ALBUM_TRACKS]},
    789: {"title": "Delta", "record_type": "single", "release_date": "2021-05-05", "upc": "", "isrcs": [SINGLE[3]]},
    900: {"title": "Lost Tapes", "record_type": "ep", "release_date": "2023-03-03", "upc": "0000111222", "isrcs": ["QZLOST000001", "QZLOST000002"],
          "titles": ["Lost One", "Lost Two"]},
    901: {"title": "Renamed In Store", "record_type": "album", "release_date": "2019-09-09", "upc": "05099000001", "isrcs": [ALBUM_TRACKS[0][3]]},
    902: {"title": "Hits Compilation Vol 1", "record_type": "compile", "release_date": "2018-01-01", "upc": "0000999", "isrcs": [ALBUM_TRACKS[0][3]]},
}


def dz_album_tracks(aid):
    a = DZ_ALBUMS[aid]
    titles = a.get("titles") or [REC_TITLES.get(next((rec for t, rec, s, i in ALBUM_TRACKS + [SINGLE] if i == isrc), ""), "x") for isrc in a["isrcs"]]
    return [{"id": aid * 10 + n, "title": t, "isrc": isrc, "duration": 5 + n, "track_position": n + 1, "disk_number": 1, "preview": f"https://cdns-preview.dzcdn.net/a{aid}-{n}.mp3",
             "artist": {"name": ARTIST["name"]}} for n, (t, isrc) in enumerate(zip(titles, a["isrcs"]))]


@app.get(DZ + "/artist/{aid}/albums")
def dz_artist_albums(aid: int, limit: int = 100):
    return {"data": [{"id": k, "title": v["title"], "record_type": v["record_type"], "release_date": v["release_date"],
                      "cover_medium": f"https://e-cdns-images.dzcdn.net/images/cover/{k}/250x250.jpg", "nb_tracks": len(v["isrcs"])}
                     for k, v in DZ_ALBUMS.items()] if aid == 77 else []}


@app.get(DZ + "/album/{aid}")
def dz_album(aid: int):
    a = DZ_ALBUMS[aid]
    return {"id": aid, "title": a["title"], "upc": a["upc"], "cover_medium": f"https://e-cdns-images.dzcdn.net/images/cover/{aid}/250x250.jpg", "tracks": {"data": [{k: v for k, v in t.items() if k != "isrc"} for t in dz_album_tracks(aid)]}}


@app.get(DZ + "/album/{aid}/tracks")
def dz_album_tracks_get(aid: int, limit: int = 500):
    return {"data": dz_album_tracks(aid)}


@app.get(DZ + "/artist/{aid}/related")
def dz_related(aid: int, limit: int = 20):
    return {"data": [{"id": 88, "name": "Deezer Neighbour", "picture_medium": "https://e-cdns-images.dzcdn.net/images/artist/n/250x250.jpg", "nb_fan": 10}]}


@app.get(DZ + "/artist/{aid}/radio")
def dz_radio(aid: int, limit: int = 25):
    return {"data": [{**dz_track(1, "Beta", 6, ALBUM_TRACKS[1][3]), "title_short": "Beta"},
                     {**dz_track(9, "Radio Song", 200, "QZRAD0000001"), "title_short": "Radio Song"}]}


@app.get(DZ + "/artist/{aid}/top")
def dz_artist_top(aid: str, limit: int = 10):
    return {"data": [{**dz_track(i, t, sec, code), "title_short": t, "rank": 900000 - i} for i, (t, _, sec, code) in enumerate(ALBUM_TRACKS)]
            + [{**dz_track(9, "Unreleased Gem", 200, None), "title_short": "Unreleased Gem"}]}


@app.get("/www.wikidata.org/wiki/Special:EntityData/{qid}.json")
def wikidata(qid: str):
    return {"entities": {qid: {"sitelinks": {"enwiki": {"title": "Fake Band"}}}}}


@app.get("/en.wikipedia.org/api/rest_v1/page/summary/{title}")
def wiki_summary(title: str):
    return {"extract": "Fake Band is an English electronic group formed in 2010.",
            "content_urls": {"desktop": {"page": f"https://en.wikipedia.org/wiki/{title}"}}}


@app.get(DZ + "/user/{uid}")
def dz_user(uid: str):
    return {"id": uid, "name": "deezeruser"}


@app.get(DZ + "/user/{uid}/tracks")
def dz_user_tracks(uid: str):
    return {"data": [dz_track(1, "Beta", 6, ALBUM_TRACKS[1][3])]}


@app.get(DZ + "/user/{uid}/playlists")
def dz_user_pls(uid: str):
    return {"data": [{"id": 55, "title": "Deezer Mix", "link": "https://www.deezer.com/playlist/55"}]}


# ================================================================= LRCLIB =============
@app.get("/lrclib.net/api/get")
def lrclib_get(track_name: str, artist_name: str, album_name: str = "", duration: int = 0):
    if track_name == "Alpha":
        return {"id": 1, "trackName": "Alpha", "artistName": artist_name, "duration": 5, "instrumental": False,
                "plainLyrics": "First line\nSecond line", "syncedLyrics": "[00:00.50] First line\n[00:02.00] Second line"}
    return JSONResponse({"code": 404, "message": "Failed to find specified track"}, 404)


@app.get("/lrclib.net/api/search")
def lrclib_search(track_name: str = "", artist_name: str = "", q: str = ""):
    if track_name == "Beta":
        return [{"id": 2, "trackName": "Beta", "artistName": artist_name, "duration": 6, "instrumental": False, "plainLyrics": "Only plain words", "syncedLyrics": None}]
    return []


# ================================================================= Discogs ============
@app.get("/api.discogs.com/artists/{aid}/releases")
def discogs_releases(aid: int, page: int = 1, per_page: int = 100, sort: str = ""):
    return {"pagination": {"pages": 1}, "releases": [
        {"id": 1, "type": "master", "role": "Main", "title": "Fake Album", "year": 2020, "thumb": ""},
        {"id": 2, "type": "release", "role": "Main", "title": "Live At The Fake Club", "year": 1999, "format": "Vinyl, LP", "thumb": "https://i.discogs.com/2.jpg"},
        {"id": 3, "type": "release", "role": "Main", "title": "Lost Tapes", "year": 2023, "format": "Cass, EP"},
        {"id": 4, "type": "release", "role": "Appearance", "title": "Someone Else's Comp", "year": 2010, "format": "CD, Comp"}]}


@app.get("/api.discogs.com/releases/{rid}")
def discogs_release(rid: int):
    return {"id": rid, "title": "Live At The Fake Club", "tracklist": [
        {"position": "A", "type_": "heading", "title": "Side A"}, {"position": "A1", "type_": "track", "title": "Alpha (Live)", "duration": "4:05"},
        {"position": "B1", "type_": "track", "title": "Beta (Live)", "duration": "1:02:03"}]}


# ================================================================= Bandcamp ===========
BC = "/fake.bandcamp.com"


@app.get(BC + "/music")
def bc_music():
    return HTMLResponse("""<ol id="music-grid">
      <li data-item-id="album-1" class="music-grid-item"><a href="/album/fake-album"><div class="art"><img src="https://f4.bcbits.com/img/a1_2.jpg"></div>
        <p class="title">Fake Album</p></a></li>
      <li data-item-id="album-2" class="music-grid-item"><a href="/album/basement-demos"><div class="art"><img class="lazy" data-original="https://f4.bcbits.com/img/a2_2.jpg"></div>
        <p class="title">
            Basement Demos
        <br><span class="artist-override">Fake Band</span></p></a></li></ol>""")


@app.get(BC + "/album/{slug}")
def bc_album(slug: str):
    t = {"fake-album": ("Fake Album", "01 Jan 2020 00:00:00 GMT", ["Alpha", "Beta", "Gamma"]),
         "basement-demos": ("Basement Demos", "12 Jun 2022 00:00:00 GMT", ["Demo One", "Demo Two"])}[slug]
    ld = {"@type": "MusicAlbum", "name": t[0], "datePublished": t[1], "numTracks": len(t[2]), "image": f"https://f4.bcbits.com/img/{slug}_10.jpg",
          "track": {"itemListElement": [{"position": i + 1, "item": {"@type": "MusicRecording", "name": n, "duration": f"P00H00M{10 + i:02d}S"}}
                                        for i, n in enumerate(t[2])]}}
    return HTMLResponse(f'<html><script type="application/ld+json">{json.dumps(ld)}</script></html>')


# ================================================================= iTunes =============
APPLE_ALBUMS = [(601, "Fake Album", "2020-01-01T08:00:00Z", 3), (602, "Lost Tapes - EP", "2023-03-03T08:00:00Z", 2),
                (603, "Apple Exclusive - Single", "2024-04-04T08:00:00Z", 1)]


@app.get("/itunes.apple.com/lookup")
def itunes_lookup(id: int, entity: str = "album", limit: int = 200):
    if id == 555 and entity == "album":
        return {"results": [{"wrapperType": "artist", "artistId": 555, "artistName": ARTIST["name"]}] + [
            {"wrapperType": "collection", "collectionId": cid, "collectionName": t, "releaseDate": d, "trackCount": n,
             "artworkUrl100": f"https://is1-ssl.mzstatic.com/image/thumb/{cid}/100x100bb.jpg", "collectionViewUrl": f"https://music.apple.com/us/album/{cid}"}
            for cid, t, d, n in APPLE_ALBUMS]}
    if id == 603 and entity == "song":
        return {"results": [{"wrapperType": "collection"}, {"wrapperType": "track", "trackName": "Exclusive", "artistName": ARTIST["name"],
                                                             "trackNumber": 1, "discNumber": 1, "trackTimeMillis": 7000}]}
    return {"results": []}


@app.get("/itunes.apple.com/search")
def itunes(term: str, entity: str = "album"):
    if entity == "musicArtist":
        return {"results": [{"artistId": 555, "artistName": ARTIST["name"]}] if "fake band" in term.lower() else []}
    if "obscure" not in term.lower():
        return {"results": []}
    key = "collectionName" if entity == "album" else "trackName"
    return {"results": [{"artistName": "Obscure Artist", key: "Hidden Gem", "artworkUrl100": "https://is1-ssl.mzstatic.com/image/thumb/x/100x100bb.jpg"}]}


@app.get("/is1-ssl.mzstatic.com/{path:path}")
def itunes_art(path: str):
    NET.calls.append(("ART", "itunes", path))
    return Response(JPEG, media_type="image/jpeg")


# ================================================================= Spotify ============
def sp_track(t, s, isrc, tid):
    return {"id": tid, "type": "track", "name": t, "duration_ms": s * 1000, "external_ids": {"isrc": isrc},
            "artists": [{"name": ARTIST["name"]}], "album": {"name": "Fake Album", "images": [{"url": "https://coverartarchive.org/s.jpg"}]}}


@app.post("/accounts.spotify.com/api/token")
async def sp_token(request: Request):
    f = await request.form()
    if f.get("grant_type") == "client_credentials":
        return {"access_token": "app-token", "expires_in": 3600}
    if f.get("grant_type") == "authorization_code":
        return {"access_token": "user-token", "refresh_token": "r1", "expires_in": 3600}
    return {"access_token": "user-token-2", "expires_in": 3600}


@app.get("/api.spotify.com/v1/tracks/{tid}")
def sp_track_get(tid: str):
    t, _, sec, isrc = {"sp1": ALBUM_TRACKS[0], "sp4": SINGLE}.get(tid, ALBUM_TRACKS[1])
    return sp_track(t, sec, isrc, tid)


@app.get("/api.spotify.com/v1/me")
def sp_me():
    return {"id": "spuser", "display_name": "Spotify User"}


@app.get("/api.spotify.com/v1/me/tracks")
def sp_saved():
    return {"items": [{"track": sp_track("Gamma", 7, ALBUM_TRACKS[2][3], "sp3")}], "next": None}


@app.get("/api.spotify.com/v1/me/playlists")
def sp_my_pls():
    return {"items": [{"id": "pl1", "name": "Spotify Mix"}], "next": None}


@app.get("/api.spotify.com/v1/playlists/{pid}")
def sp_pl(pid: str):
    return {"name": "Spotify Mix", "description": "desc", "images": []}


@app.get("/api.spotify.com/v1/playlists/{pid}/items")
def sp_items(pid: str, request: Request):
    if request.headers.get("authorization") == "Bearer app-token" and pid == "private":
        return JSONResponse({"error": "forbidden"}, 403)
    return {"items": [{"item": sp_track("Alpha", 5, ALBUM_TRACKS[0][3], "sp1")}, {"item": sp_track("Delta", 8, SINGLE[3], "sp4")},
                      {"item": sp_track("Alpha - Remastered 2021", 5, None, "sp1b")}], "next": None}


@app.get("/open.spotify.com/embed/{kind}/{sid}")
def sp_embed(kind: str, sid: str):
    data = {"props": {"pageProps": {"state": {"data": {"entity": {"name": "Embed List", "trackList": [
        {"title": "Beta", "subtitle": "Fake Band", "duration": 6000, "uri": "spotify:track:sp2"}]}}}}}}
    return HTMLResponse(f'<script id="__NEXT_DATA__" type="application/json">{json.dumps(data)}</script>')


# ================================================================== Tidal =============
@app.post("/auth.tidal.com/v1/oauth2/token")
async def td_token(request: Request):
    f = await request.form()
    if f.get("grant_type") == "authorization_code":
        return {"access_token": "td-user", "refresh_token": "tr", "expires_in": 3600, "user_id": 77, "user": {"username": "tidaluser", "countryCode": "US"}}
    return {"access_token": "td-app", "expires_in": 3600}


TD = "/openapi.tidal.com/v2"


@app.get(TD + "/playlists/{pid}")
def td_pl(pid: str):
    return {"data": {"id": pid, "type": "playlists", "attributes": {"name": "Tidal Mix", "description": ""}}}


@app.get(TD + "/playlists/{pid}/relationships/items")
def td_items(pid: str):
    return {"data": [{"id": "t1", "type": "tracks"}, {"id": "t3", "type": "tracks"}], "links": {}}


@app.get(TD + "/userCollections/{uid}/relationships/tracks")
def td_coll(uid: str):
    return {"data": [{"id": "t1", "type": "tracks"}], "links": {}}


@app.get(TD + "/playlists")
def td_user_pls():
    return {"data": [{"id": "tp1", "attributes": {"name": "Tidal Mix"}}]}


@app.get(TD + "/tracks")
def td_tracks(request: Request):
    qp = request.query_params
    if qp.get("filter[isrc]"):
        isrc = qp["filter[isrc]"]
        hit = [x for x in ALBUM_TRACKS + [SINGLE] if x[3] == isrc]
        return {"data": [{"id": "t-isrc", "type": "tracks"}] if hit else [],
                "included": [{"id": "alb-1", "type": "albums", "attributes": {"title": "Fake Album"}}] if hit else []}
    ids = qp.get("filter[id]", "").split(",")
    m = {"t1": ("Alpha", 5, ALBUM_TRACKS[0][3], None), "t3": ("Gamma", 7, None, "Live")}
    data = [{"id": i, "type": "tracks", "attributes": {"title": m[i][0], "isrc": m[i][2], "duration": f"PT{m[i][1]}S", "version": m[i][3]},
             "relationships": {"artists": {"data": [{"id": "ar1", "type": "artists"}]}}} for i in ids if i in m]
    return {"data": data, "included": [{"id": "ar1", "type": "artists", "attributes": {"name": ARTIST["name"]}}]}


# ============================================================ Apple Music / Qobuz =====
@app.get("/music.apple.com/{path:path}")
def apple(path: str):
    data = {"sections": [{"items": [{"title": "Beta", "artistName": "Fake Band", "duration": 6000, "contentDescriptor": {}}]}]}
    return HTMLResponse(f'<script type="application/json" id="serialized-server-data">{json.dumps(data)}</script>')


@app.get("/www.qobuz.com/{path:path}")
def qobuz(path: str):
    ld = {"@context": "https://schema.org", "@type": "MusicPlaylist", "name": "Qobuz Picks",
          "track": [{"@type": "MusicRecording", "name": "Gamma", "byArtist": {"name": "Fake Band"}, "duration": "PT7S"}]}
    return HTMLResponse(f'<script type="application/ld+json">{json.dumps(ld)}</script>')


# ========================================================== ListenBrainz / Last.fm =====
LB = "/api.listenbrainz.org/1"


@app.get(LB + "/validate-token")
def lb_validate(request: Request):
    ok = request.headers.get("authorization") == "Token lb-token"
    return {"valid": ok, "user_name": "lbuser"} if ok else {"valid": False}


@app.get(LB + "/playlist/{pid}")
def lb_playlist(pid: str):
    if pid in LB_FOR_YOU:
        return {"playlist": {"title": LB_FOR_YOU[pid][0], "track": lb_jspf_tracks(pid)}}
    if pid in NET.lb["playlists"]:
        return {"playlist": {"title": "x", "track": NET.lb["playlists"][pid]}}
    return {"playlist": {"title": "LB Playlist", "track": [{"title": "Beta", "creator": "Fake Band",
                                                            "identifier": ["https://musicbrainz.org/recording/rec-beta"]}]}}


LB_FOR_YOU = {
    "lbx-1": ("Weekly Exploration for lbuser, week of 2026-10-05 Mon", "weekly-exploration", [
        ("Beta", "Fake Band", "rec-beta", "a-fake"), ("Strange Song", "Other Artist", "rec-strange", "a-other"),
        ("Odd Song", "Other Artist", "rec-odd", "a-other"), ("Third Song", "Third Artist", "rec-third", "a-third")]),
    "lbd-1": ("Daily Jams for lbuser, 2026-10-08 Wed", "daily-jams", [("Alpha", "Fake Band", "rec-alpha", "a-fake")]),
}


@app.get(LB + "/user/{user}/playlists/createdfor")
def lb_createdfor(user: str, count: int = 25):
    if user != "lbuser":
        return JSONResponse({"error": "Cannot find user"}, 404)
    return {"playlists": [{"playlist": {"identifier": f"https://listenbrainz.org/playlist/{pid}", "title": t, "date": "2026-10-05T00:00:00Z",
                                        "extension": {"https://musicbrainz.org/doc/jspf#playlist": {"additional_metadata": {"algorithm_metadata": {"source_patch": k}}}}}}
                          for pid, (t, k, _) in LB_FOR_YOU.items()], "playlist_count": len(LB_FOR_YOU)}


def lb_jspf_tracks(pid):
    return [{"title": t, "creator": a, "album": "", "identifier": [f"https://musicbrainz.org/recording/{rec}"], "duration": 200000,
             "extension": {"https://musicbrainz.org/doc/jspf#track": {"artist_identifiers": [f"https://musicbrainz.org/artist/{aid}"],
                                                                      "additional_metadata": {"caa_release_mbid": "rel-x", "caa_id": 42}}}}
            for t, a, rec, aid in LB_FOR_YOU[pid][2]]


@app.get("/labs.api.listenbrainz.org/similar-artists/json")
def lb_labs_similar(algorithm: str, artist_mbids: str):
    return [[{"type": "markup", "data": "x"}], [{"artist_mbid": artist_mbids, "name": "Fake Band", "score": 999},
                                               {"artist_mbid": "a-other", "name": "Other Artist", "score": 50}]]


@app.get(LB + "/feedback/user/{user}/get-feedback")
def lb_feedback(user: str):
    return {"feedback": [{"recording_mbid": m, "track_metadata": {"track_name": REC_TITLES.get(m, "?"), "artist_name": "Fake Band"}}
                         for m, s in NET.lb["loves"].items() if s == 1]}


@app.post(LB + "/feedback/recording-feedback")
async def lb_love(request: Request):
    b = await request.json()
    NET.lb["loves"][b["recording_mbid"]] = b["score"]
    return {"status": "ok"}


@app.post(LB + "/playlist/create")
async def lb_create(request: Request):
    b = await request.json()
    pid = f"lbpl-{len(NET.lb['playlists']) + 1}"
    NET.lb["playlists"][pid] = b["playlist"]["track"]
    return {"playlist_mbid": pid}


@app.post(LB + "/playlist/{pid}/item/delete")
def lb_del(pid: str):
    NET.lb["playlists"][pid] = []
    return {}


@app.post(LB + "/playlist/{pid}/item/add")
async def lb_add(pid: str, request: Request):
    NET.lb["playlists"][pid] += (await request.json())["playlist"]["track"]
    return {}


@app.get("/ws.audioscrobbler.com/2.0/")
def lastfm(method: str, user: str = "", tag: str = "", artist: str = "", mbid: str = "", track: str = ""):
    if method == "user.getinfo":
        return {"user": {"name": user}}
    if method == "artist.getsimilar":
        if mbid and mbid != "a-fake":
            return {"error": 6, "message": "The artist you supplied could not be found"}
        return {"similarartists": {"artist": [{"name": "Other Artist", "mbid": "a-other", "match": "0.9"},
                                              {"name": "No Id Artist", "mbid": "", "match": "0.5"}]}}
    if method == "track.getsimilar":
        return {"similartracks": {"track": [{"name": "Gamma", "artist": {"name": "Fake Band"}, "mbid": "rec-gamma", "duration": 7},
                                            {"name": "Strange Song", "artist": {"name": "Other Artist"}, "mbid": "", "duration": 200}]}}
    if method == "tag.gettopartists":
        return {"topartists": {"artist": [{"name": "Fake Band", "mbid": "a-fake"}, {"name": "Other Artist", "mbid": ""}]}}
    if method == "tag.getinfo":
        return {"tag": {"name": tag, "wiki": {"summary": f"{tag} is a genre. <a href=\"https://last.fm\">Read more on Last.fm</a>."}}}
    if method.lower() == "tag.gettoptags":
        return {"toptags": {"tag": [{"name": "electronic"}, {"name": "shoegaze"}]}}
    return {"lovedtracks": {"track": [{"name": "Delta", "artist": {"name": "Fake Band"}, "mbid": ""}], "@attr": {"totalPages": "1"}}}


# ================================================================== Jellyfin ==========
J = "/jellyfin.test"


@app.post(J + "/Users/AuthenticateByName")
async def jf_auth(request: Request):
    b = await request.json()
    if b["Pw"] != "jfpass":
        return JSONResponse({}, 401)
    return {"AccessToken": "jf-token", "User": {"Id": "jfu", "Name": b["Username"]}}


@app.get(J + "/Items")
def jf_items(request: Request):
    qp = request.query_params
    if qp.get("IncludeItemTypes") == "Playlist":
        return {"Items": [{"Id": k, "Name": v["name"]} for k, v in NET.jf["playlists"].items()], "TotalRecordCount": len(NET.jf["playlists"])}
    items = lib_items()
    if qp.get("Filters") == "IsFavorite":
        items = [i for i in items if i["id"] in NET.jf["favs"]]
    return {"Items": [{"Id": i["id"], "Name": i["title"], "Path": "/data/music/" + i["rel"], "Artists": [i["artist"]], "Album": i["album"],
                       "RunTimeTicks": i["duration"] * 10**7} for i in items], "TotalRecordCount": len(items)}


@app.post(J + "/UserFavoriteItems/{iid}")
def jf_fav(iid: str):
    NET.jf["favs"].add(iid)
    return {}


@app.delete(J + "/UserFavoriteItems/{iid}")
def jf_unfav(iid: str):
    NET.jf["favs"].discard(iid)
    return {}


@app.post(J + "/Playlists")
async def jf_pl_create(request: Request):
    b = await request.json()
    pid = f"jfpl{len(NET.jf['playlists']) + 1}"
    NET.jf["playlists"][pid] = {"name": b["Name"], "items": list(b["Ids"])}
    return {"Id": pid}


@app.get(J + "/Playlists/{pid}/Items")
def jf_pl_items(pid: str):
    return {"Items": [{"Id": i, "PlaylistItemId": f"e-{n}-{i}", "Name": i} for n, i in enumerate(NET.jf["playlists"][pid]["items"])]}


@app.post(J + "/Playlists/{pid}/Items")
def jf_pl_add(pid: str, ids: str):
    NET.jf["playlists"][pid]["items"] += ids.split(",")
    return {}


@app.delete(J + "/Playlists/{pid}/Items")
def jf_pl_del(pid: str, entryIds: str):
    drop = {e.split("-", 2)[2] for e in entryIds.split(",")}
    NET.jf["playlists"][pid]["items"] = [i for i in NET.jf["playlists"][pid]["items"] if i not in drop]
    return {}


@app.post(J + "/Library/Refresh")
def jf_refresh():
    NET.jf["refreshed"] += 1
    return {}


# ===================================================================== Plex ===========
P = "/plex.test"


def mc(**kw):
    return {"MediaContainer": kw}


@app.get(P + "/identity")
def plex_identity():
    return mc(machineIdentifier="plex-machine")


@app.get(P + "/library/sections")
def plex_sections():
    return mc(Directory=[{"key": "1", "type": "artist", "title": "Music"}])


@app.get(P + "/library/sections/1/all")
def plex_all():
    items = lib_items()
    return mc(size=len(items), totalSize=len(items), Metadata=[{
        "ratingKey": i["id"], "title": i["title"], "grandparentTitle": i["artist"], "parentTitle": i["album"],
        "duration": i["duration"] * 1000, **({"userRating": NET.plex["ratings"][i["id"]]} if i["id"] in NET.plex["ratings"] else {}),
        "Media": [{"Part": [{"file": "/plexmedia/" + i["rel"]}]}]} for i in items])


@app.put(P + "/:/rate")
def plex_rate(key: str, rating: float):
    if rating < 0:
        NET.plex["ratings"].pop(key, None)
    else:
        NET.plex["ratings"][key] = rating
    return {}


@app.get(P + "/playlists")
def plex_playlists():
    return mc(Metadata=[{"ratingKey": k, "title": v["name"]} for k, v in NET.plex["playlists"].items()])


@app.post(P + "/playlists")
def plex_pl_create(title: str, uri: str):
    pid = f"ppl{len(NET.plex['playlists']) + 1}"
    NET.plex["playlists"][pid] = {"name": title, "items": uri.rsplit("/", 1)[-1].split(",")}
    return mc(Metadata=[{"ratingKey": pid}])


@app.get(P + "/playlists/{pid}/items")
def plex_pl_items(pid: str):
    return mc(Metadata=[{"ratingKey": k, "title": k} for k in NET.plex["playlists"][pid]["items"]])


@app.delete(P + "/playlists/{pid}/items")
def plex_pl_clear(pid: str):
    NET.plex["playlists"][pid]["items"] = []
    return {}


@app.put(P + "/playlists/{pid}/items")
def plex_pl_add(pid: str, uri: str):
    NET.plex["playlists"][pid]["items"] += uri.rsplit("/", 1)[-1].split(",")
    return {}


@app.get(P + "/library/sections/all/refresh")
def plex_refresh():
    NET.plex["refreshed"] += 1
    return {}


@app.post("/plex.tv/api/v2/pins")
def plextv_pin():
    pid = len(NET.plex["pins"]) + 1
    NET.plex["pins"][pid] = 0
    return {"id": pid, "code": f"CODE{pid}"}


@app.get("/plex.tv/api/v2/pins/{pid}")
def plextv_pin_check(pid: int):
    NET.plex["pins"][pid] += 1   # "user approves" after the first poll
    return {"id": pid, "authToken": "plex-user-token" if NET.plex["pins"][pid] > 1 else None}


@app.get("/plex.tv/api/v2/user")
def plextv_user():
    return {"username": "plexuser"}


@app.get("/plex.tv/api/v2/resources")
def plextv_resources():
    return [{"clientIdentifier": "plex-machine", "provides": "server", "accessToken": "plex-server-token"}]


# ================================================================== Navidrome =========
N = "/navidrome.test/rest"


def sub(**kw):
    return {"subsonic-response": {"status": "ok", "version": "1.16.1", **kw}}


@app.get(N + "/ping")
def nd_ping(p: str = "", t: str = "", s: str = ""):
    import hashlib
    ok = t == hashlib.md5(("ndpass" + s).encode()).hexdigest()
    return sub() if ok else {"subsonic-response": {"status": "failed", "error": {"message": "Wrong username or password"}}}


@app.get(N + "/search3")
def nd_search(songOffset: int = 0):
    items = lib_items() if songOffset == 0 else []
    return sub(searchResult3={"song": [{"id": i["id"], "title": i["title"], "artist": i["artist"], "album": i["album"],
                                         "duration": i["duration"], "path": i["rel"]} for i in items]})


@app.get(N + "/getStarred2")
def nd_starred():
    return sub(starred2={"song": [{"id": i["id"], "title": i["title"], "artist": i["artist"], "path": i["rel"]}
                                  for i in lib_items() if i["id"] in NET.nd["stars"]]})


@app.get(N + "/star")
def nd_star(id: str):
    NET.nd["stars"].add(id)
    return sub()


@app.get(N + "/unstar")
def nd_unstar(id: str):
    NET.nd["stars"].discard(id)
    return sub()


@app.get(N + "/getPlaylists")
def nd_pls():
    return sub(playlists={"playlist": [{"id": k, "name": v["name"]} for k, v in NET.nd["playlists"].items()]})


@app.get(N + "/getPlaylist")
def nd_pl(id: str):
    return sub(playlist={"entry": [{"id": i} for i in NET.nd["playlists"][id]["items"]]})


@app.get(N + "/createPlaylist")
def nd_create(request: Request):
    qp = request.query_params
    songs = qp.getlist("songId")
    if qp.get("playlistId"):
        NET.nd["playlists"][qp["playlistId"]]["items"] = songs
        return sub()
    pid = f"ndpl{len(NET.nd['playlists']) + 1}"
    NET.nd["playlists"][pid] = {"name": qp["name"], "items": songs}
    return sub(playlist={"id": pid})


@app.get(N + "/startScan")
def nd_scan():
    NET.nd["scans"] += 1
    return sub()


# ============================================================== notifications =========
@app.post("/discord.test/{path:path}")
async def discord(path: str, request: Request):
    NET.notifications.append(("discord", (await request.json())["embeds"][0]["title"]))
    return {}


@app.post("/ntfy.test/{path:path}")
async def ntfy(path: str, request: Request):
    NET.notifications.append(("ntfy", request.query_params.get("title")))
    return {}


@app.post("/hook.test/{path:path}")
async def hook(path: str, request: Request):
    NET.notifications.append(("webhook", (await request.json())["title"]))
    return {}


class Transport:
    """httpx transport into the fake internet; unknown hosts fail like real DNS does."""

    def __init__(self):
        import httpx
        self._inner = httpx.ASGITransport(app=app)

    async def handle_async_request(self, request):
        import httpx
        if request.url.host.startswith("nonexistent"):
            raise httpx.ConnectError("[Errno -2] Name or service not known", request=request)
        return await self._inner.handle_async_request(request)

    async def aclose(self):
        await self._inner.aclose()
