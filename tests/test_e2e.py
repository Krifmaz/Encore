"""End-to-end tests: the real app + worker + sync engine against fake versions of every external service."""
import http.server
import json
import os
import socketserver
import sys
import threading
import time

import pytest

import fakenet
from fakenet import ALBUM_TRACKS, NET, make_audio

H = {"X-Requested-With": "encore"}
TERMINAL = ("completed", "failed", "denied", "cancelled")


# ---------------------------------------------------------------- fixtures -----------
class FakeYTM:
    """Stands in for ytmusicapi.YTMusic (which uses `requests`, not our httpx client)."""
    rated: dict = {}
    playlists: dict = {}

    def __init__(self, *a, **k):
        pass

    @staticmethod
    def _song(title, secs, vid=None):
        return {"videoId": vid or f"vid-{title.lower()}", "title": title, "artists": [{"name": "Fake Band"}],
                "album": {"name": "Fake Album", "id": "MPREb_fake"}, "duration_seconds": secs, "thumbnails": []}

    def search(self, q, filter=None, scope=None, limit=20, *a):
        ql = q.lower()
        if filter == "artists":
            return [{"artist": "Fake Band", "browseId": "UCfake"}] if "fake band" in ql else []
        if filter == "albums":
            return [{"browseId": "MPREb_fake", "title": "Fake Album", "artists": [{"name": "Fake Band"}]}] if "fake album" in ql else []
        out = [self._song(t, s) for t, _, s, _ in ALBUM_TRACKS + [fakenet.SINGLE] if t.lower() in ql]
        if "alpha" in ql:
            out.insert(0, self._song("Alpha (Fan Remix)", 5, "vid-fanremix"))  # must never be picked for "Alpha"
        return out

    def get_artist(self, channel):
        return {"albums": {"results": [{"title": "Fake Album", "year": "2020", "browseId": "MPREb_fake"},
                                       {"title": "YT Mixtape", "year": "2022", "browseId": "MPREb_mix", "type": "Album"}]},
                "singles": {"results": [{"title": "Delta", "year": "2021", "browseId": "MPREb_delta"}]}}

    def get_album(self, bid):
        if bid == "MPREb_mix":
            return {"title": "YT Mixtape", "audioPlaylistId": "OLAK5uy_mix", "thumbnails": [],
                    "tracks": [{**self._song("Mix One", 15, "vid-mix1"), "trackNumber": 1}]}
        return {"title": "Fake Album", "audioPlaylistId": "OLAK5uy_fake", "thumbnails": [],
                "tracks": [self._song(t, s) for t, _, s, _ in ALBUM_TRACKS]}

    def get_album_browse_id(self, pid):
        return "MPREb_fake"

    def get_playlist(self, pid, limit=100, *a):
        if pid in FakeYTM.playlists:
            return {"title": "x", "tracks": [{"videoId": v, "setVideoId": "s" + v} for v in FakeYTM.playlists[pid]]}
        return {"title": "YTM Mix", "description": "", "thumbnails": [],
                "tracks": [self._song("Beta", 6), self._song("Gamma", 7), self._song("Beta", 6)]}

    def get_liked_songs(self, limit=100):
        secs = {f"vid-{t.lower()}": (t, s) for t, _, s, _ in ALBUM_TRACKS + [fakenet.SINGLE]}
        return {"tracks": [self._song(*secs[v], v) for v, r in FakeYTM.rated.items() if r == "LIKE" and v in secs]}

    def rate_song(self, vid, rating):
        FakeYTM.rated[vid] = rating

    def get_library_playlists(self, limit=25):
        return [{"playlistId": "PLytm1", "title": "YTM Mix"}, {"playlistId": "LM", "title": "Liked Music"}]

    def create_playlist(self, title, desc, privacy="PRIVATE", video_ids=None, *a):
        pid = f"PLnew{len(FakeYTM.playlists) + 1}"
        FakeYTM.playlists[pid] = list(video_ids or [])
        return pid

    def remove_playlist_items(self, pid, items):
        FakeYTM.playlists[pid] = []

    def add_playlist_items(self, pid, vids, source=None, duplicates=False):
        FakeYTM.playlists[pid] += vids

    def get_account_info(self):
        return {"accountName": "YT User"}

    def get_song(self, vid):
        return {"videoDetails": {"title": "Beta", "author": "Fake Band - Topic", "lengthSeconds": "6"}}


@pytest.fixture()
def world(env, monkeypatch, tmp_path):
    from fastapi.testclient import TestClient

    from app import config
    from app.services import http as http_mod, musicbrainz, ytdlp, ytmusic

    monkeypatch.setenv("ENCORE_WORKER", "0")
    NET.reset()
    lib, slsk, stage = tmp_path / "music", tmp_path / "slskd", tmp_path / "staging"
    for p in (lib, slsk, stage):
        p.mkdir(exist_ok=True)
    NET.library_path, NET.slskd_path = str(lib), str(slsk)
    monkeypatch.setattr(http_mod, "transport", fakenet.Transport())
    http_mod._clients.clear()
    musicbrainz._cache.clear()

    # fake streamrip executable: writes FLAC files for the Deezer/Tidal link it is given
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    rip = bin_dir / "streamrip"
    rip.write_text(f"""#!{sys.executable}
import sys; sys.path.insert(0, {os.path.dirname(__file__)!r})
from fakenet import make_audio, ALBUM_TRACKS, SINGLE
a = sys.argv
if '--version' in a: print('streamrip, version 2.4.9-fake'); sys.exit(0)
folder = a[a.index('--folder') + 1]; url = a[-1]
if 'album/123' in url:
    for i, (t, r, s, isrc) in enumerate(ALBUM_TRACKS, 1):
        make_audio(f'{{folder}}/Fake Band - Fake Album (2020) [FLAC]/{{i:02d}}. {{t}}.flac', t, album='Fake Album', track=i, seconds=s, isrc=isrc)
elif 'album/900' in url:   # a release MusicBrainz doesn't have, files without tags
    for i, t in enumerate(['Lost One', 'Lost Two'], 1):
        make_audio(f'{{folder}}/Lost Tapes/{{i:02d}}. {{t}}.flac', t, artist='', album='', seconds=12 + i)
elif 'track/' in url:
    t, r, s, isrc = (ALBUM_TRACKS + [SINGLE])[int(url.rsplit('/', 1)[1]) - 1000]
    make_audio(f'{{folder}}/Fake Band - {{t}}/01. {{t}}.flac', t, seconds=s, isrc=isrc)
else:
    print('Error: nothing found'); sys.exit(1)
""")
    rip.chmod(0o755)

    # local web server for real yt-dlp downloads
    www = tmp_path / "www"
    make_audio(str(www / "gamma.mp3"), "Gamma", album="", seconds=7)

    class Quiet(http.server.SimpleHTTPRequestHandler):
        def __init__(self, *a, **k):
            super().__init__(*a, directory=str(www), **k)

        def log_message(self, *a):
            pass
    srv = socketserver.TCPServer(("127.0.0.1", 0), Quiet)
    threading.Thread(target=srv.serve_forever, daemon=True).start()

    # YouTube Music (ytmusicapi uses requests) + yt-dlp for music.youtube.com links
    FakeYTM.rated, FakeYTM.playlists = {}, {}
    monkeypatch.setattr(ytmusic, "_anon", FakeYTM())
    monkeypatch.setattr(ytmusic, "YTMusic", FakeYTM)
    monkeypatch.setattr(ytmusic, "for_connection", lambda conn: FakeYTM())
    real_download = ytdlp.download
    secs = {t.lower(): s for t, _, s, _ in ALBUM_TRACKS + [fakenet.SINGLE]}

    async def download(url, outdir, progress=None, playlist=False):
        if "bandcamp.com" in url:
            return {"files": [make_audio(f"{outdir}/Fake Band - Basement Demos - 0{i} {t}.mp3", t, album="Basement Demos", track=i, seconds=15 + i)
                              for i, t in enumerate(["Demo One", "Demo Two"], 1)], "entries": []}
        if "music.youtube.com" not in url:
            return await real_download(url, outdir, progress, playlist)
        files = []
        if "OLAK5uy_mix" in url:
            files.append(make_audio(f"{outdir}/1 - Fake Band - Mix One.opus", "Mix One", album="", track=1, seconds=15))
        elif "OLAK5uy_fake" in url:
            for i, (t, _, s, _) in enumerate(ALBUM_TRACKS, 1):
                files.append(make_audio(f"{outdir}/{i} - Fake Band - {t}.opus", t, album="Fake Album", track=i, seconds=s))
        else:
            vid = url.split("v=")[-1]
            title = vid.replace("vid-", "").title()
            files.append(make_audio(f"{outdir}/Fake Band - {title}.opus", title, seconds=secs.get(title.lower(), fakenet.WRONG_SECONDS)))
        return {"files": files, "entries": [{"title": None, "artist": "Fake Band", "album": None}]}
    monkeypatch.setattr(ytdlp, "download", download)

    with TestClient(__import__("app.main", fromlist=["app"]).app) as c:
        config.set_many({
            "musicbrainz_url": "http://mb.test", "musicbrainz_rate": 1000, "base_url": "https://encore.test",
            "library_path": str(lib), "staging_path": str(stage), "scan_interval_minutes": 0, "sync_interval_minutes": 0,
            "lidarr_url": "http://lidarr.test", "lidarr_api_key": "lk", "slskd_url": "http://slskd.test", "slskd_api_key": "sk",
            "slskd_download_path": str(slsk), "slskd_search_timeout": 1, "acoustid_api_key": "good-key",
            "jellyfin_url": "http://jellyfin.test", "jellyfin_api_key": "admin-jf", "plex_url": "http://plex.test", "plex_token": "admin-plex",
            "navidrome_url": "http://navidrome.test", "navidrome_user": "admin", "navidrome_password": "ndpass",
            "discord_webhook": "http://discord.test/hook", "ntfy_url": "http://ntfy.test/music", "webhook_url": "http://hook.test/x",
            "spotify_client_id": "spid", "spotify_client_secret": "spsec", "tidal_client_id": "tdid", "tidal_client_secret": "tdsec",
            "lastfm_api_key": "lfm", "ytmusic_client_id": "ytid", "ytmusic_client_secret": "ytsec",
            "streamrip_bin": str(rip), "streamrip_sources": "deezer,tidal",
            "lidarr_enabled": False, "slskd_enabled": False, "streamrip_enabled": False, "ytdlp_enabled": False,
            "album_upgrade": "off",
        })
        assert c.post("/api/auth/login", json={"username": "admin", "password": "adminpass123"}).status_code == 200

        class W:
            client = c
            library = lib
            web = f"http://127.0.0.1:{srv.server_address[1]}"

            @staticmethod
            def run(fn, *a):
                return c.portal.call(fn, *a)

            @staticmethod
            def drive(rid, steps=25):
                from app import db, worker
                for _ in range(steps):
                    r = db.get_request(rid)
                    if r["status"] in TERMINAL:
                        return r
                    if r["detail"].get("lidarr"):
                        r["detail"]["lidarr"]["next"] = 0
                        db.update_request(rid, detail=r["detail"])
                    c.portal.call(worker.step, rid)
                return db.get_request(rid)

            def req(self, **body):
                r = c.post("/api/requests", json=body, headers=H)
                assert r.status_code == 200, r.text
                return r.json()["id"]
        yield W()
    srv.shutdown()


def tracks(db):
    return db.query("SELECT title, format, mb_releasegroupid, mb_trackid, verify_status, path FROM tracks ORDER BY title")


# ---------------------------------------------------------------- discover -----------
def test_discover_search_artist_album_probe(world):
    c = world.client
    d = c.get("/api/discover/search?q=fake").json()
    assert d["artists"][0]["name"] == "Fake Band"
    assert {a["title"] for a in d["albums"]} >= {"Fake Album", "Delta"}
    assert c.get("/api/discover/search?q=alpha&type=track").json()["tracks"][0]["title"] == "Alpha"
    a = c.get("/api/discover/artist/a-fake").json()
    assert a["name"] == "Fake Band" and len(a["release_groups"]) == 3 and a["links"]["bandcamp"]
    alb = c.get("/api/discover/album/rg-album").json()
    assert [t["title"] for t in alb["release"]["tracks"]] == ["Alpha", "Beta", "Gamma"] and alb["in_library"] is False
    p = c.get(f"/api/discover/probe?url={world.web}/gamma.mp3").json()
    assert p["is_playlist"] is False and p["extractor"] == "Generic"


# ---------------------------------------------------------------- request pipeline ---
def test_album_via_lidarr_notifies_and_refreshes(world):
    from app import config, db
    config.set_many({"lidarr_enabled": True})
    rid = world.req(kind="album", mbid="rg-album", artist="Fake Band", title="Fake Album")
    r = world.drive(rid)
    assert r["status"] == "completed" and "Lidarr" in r["message"], r
    assert NET.lidarr["commands"][:2] == ["RefreshArtist", "AlbumSearch"]
    assert len(tracks(db)) == 3
    assert {n[0] for n in NET.notifications} == {"discord", "ntfy", "webhook"}
    assert ("discord", "✅ Now available") in NET.notifications
    # the same album can't be requested twice
    assert world.client.post("/api/requests", json={"kind": "album", "mbid": "rg-album"}, headers=H).status_code == 409


def test_album_via_soulseek_lossless_tagged_and_verified(world):
    from app import config, db
    config.set_many({"slskd_enabled": True, "quality_first": True})
    rid = world.req(kind="album", mbid="rg-album", artist="Fake Band", title="Fake Album")
    r = world.drive(rid)
    assert r["status"] == "completed" and "Soulseek" in r["message"], r
    ts = tracks(db)
    assert [t["title"] for t in ts] == ["Alpha", "Beta", "Gamma"]
    assert all(t["format"] == "flac" and t["mb_releasegroupid"] == "rg-album" and t["verify_status"] == "verified" for t in ts), [(t["title"], t["verify_status"]) for t in ts] + [a["message"] for a in db.query("SELECT message FROM activity WHERE level!='info'")]
    album_dir = os.path.dirname(ts[0]["path"])
    assert album_dir.endswith("Fake Band/Fake Album (2020)") and os.path.exists(os.path.join(album_dir, "cover.jpg"))
    assert not any(os.scandir(NET.slskd_path)), "downloaded folder should be cleaned up"
    assert NET.jf["refreshed"] and NET.plex["refreshed"] and NET.nd["scans"]


def test_streamrip_album_and_isrc_track(world):
    from app import config, db
    config.set_many({"streamrip_enabled": True})
    r = world.drive(world.req(kind="album", mbid="rg-album", artist="Fake Band", title="Fake Album"))
    assert r["status"] == "completed" and "Deezer via streamrip" in r["message"], r
    r = world.drive(world.req(kind="track", mbid="rec-delta", artist="Fake Band", title="Delta"))
    assert r["status"] == "completed", r
    assert {t["title"] for t in tracks(db)} == {"Alpha", "Beta", "Gamma", "Delta"}
    assert db.one("SELECT verify_status FROM tracks WHERE title='Delta'")["verify_status"] == "verified"


def test_song_request_downloads_whole_album_not_compilation(world):
    from app import config, db
    config.set_many({"slskd_enabled": True, "album_upgrade": "album_ep"})
    tid = world.req(kind="track", mbid="rec-alpha", artist="Fake Band", title="Alpha")
    world.drive(tid, 2)
    t = db.get_request(tid)
    assert t["status"] == "monitoring" and t["source"] == "album", t
    album = db.one("SELECT * FROM requests WHERE kind='album'")
    assert album["mbid"] == "rg-album", "must pick the studio album, never the compilation"
    assert world.drive(album["id"])["status"] == "completed"
    assert world.drive(tid)["status"] == "completed"
    assert len(tracks(db)) == 3
    # the single (EP/Single type) is not an album → song is fetched on its own
    config.set_many({"album_upgrade": "album"})
    did = world.req(kind="track", mbid="rec-delta", artist="Fake Band", title="Delta")
    r = world.drive(did)
    assert r["status"] == "completed" and db.one("SELECT COUNT(*) n FROM requests WHERE kind='album'")["n"] == 1


def test_youtube_music_track_and_album(world):
    from app import config, db
    config.set_many({"ytdlp_enabled": True})
    r = world.drive(world.req(kind="track", artist="Fake Band", title="Alpha", length=5))
    assert r["status"] == "completed" and "YouTube Music" in r["message"], r
    t = db.one("SELECT * FROM tracks WHERE title='Alpha'")
    assert t["format"] == "opus" and t["verify_status"] == "verified"   # official song, not the fan remix
    r = world.drive(world.req(kind="album", mbid="rg-album", artist="Fake Band", title="Fake Album"))
    assert r["status"] == "completed", r
    assert {x["title"] for x in tracks(db)} == {"Alpha", "Beta", "Gamma"}
    assert db.one("SELECT COUNT(*) n FROM tracks WHERE title='Alpha'")["n"] == 1, "no duplicate Alpha after the album import"


def test_wrong_audio_is_rejected_then_next_source(world):
    from app import config
    config.set_many({"slskd_enabled": True, "ytdlp_enabled": True})
    r = world.drive(world.req(kind="track", artist="Fake Band", title="Liar Song"))
    assert r["status"] == "failed", (r["message"], r["source"], r["detail"].get("log"))
    assert any("wrong audio" in line for line in r["detail"]["log"]), r["detail"]["log"]
    assert not os.listdir(NET.library_path)


def test_direct_url_download_with_real_ytdlp(world):
    from app import config, db
    config.set_many({"ytdlp_enabled": True})
    r = world.drive(world.req(kind="url", url=f"{world.web}/gamma.mp3"))
    assert r["status"] == "completed", r
    t = db.one("SELECT * FROM tracks")
    assert t and t["title"] == "Gamma" and t["mb_trackid"] == "rec-gamma"


def test_request_approval_quota_cancel_retry(world):
    from app import db
    c = world.client
    c.post("/api/admin/users", json={"username": "bob", "password": "bobpassword"}, headers=H)
    from fastapi.testclient import TestClient
    bob = TestClient(c.app)
    bob.post("/api/auth/login", json={"username": "bob", "password": "bobpassword"})
    rid = bob.post("/api/requests", json={"kind": "album", "mbid": "rg-album", "artist": "Fake Band", "title": "Fake Album"}, headers=H).json()["id"]
    assert db.get_request(rid)["status"] == "pending"
    assert bob.post(f"/api/requests/{rid}/approve", headers=H).status_code == 403
    assert len(c.get("/api/requests?status=pending").json()) == 1
    c.post(f"/api/requests/{rid}/approve", headers=H)
    assert db.get_request(rid)["status"] == "queued"
    c.post(f"/api/requests/{rid}/cancel", headers=H)
    assert db.get_request(rid)["status"] == "cancelled"
    c.post(f"/api/requests/{rid}/retry", headers=H)
    assert db.get_request(rid)["status"] == "queued"
    c.post(f"/api/requests/{rid}/deny", headers=H)  # only pending can be denied → unchanged
    assert db.get_request(rid)["status"] == "queued"
    assert c.delete(f"/api/requests/{rid}", headers=H).status_code == 200


# ---------------------------------------------------------------- library tools -----
def _seed_messy_album(world):
    for i, (t, _, s, _) in enumerate(ALBUM_TRACKS, 1):
        make_audio(str(world.library / "unsorted" / f"{i} {t.lower()}.mp3"), t.lower(), album="fake album", track=i, seconds=s)
    world.client.post("/api/library/scan", headers=H)
    for _ in range(50):
        if not world.client.get("/api/library/stats").json()["scan"]["scanning"] and world.client.get("/api/library/stats").json()["tracks"]:
            break
        time.sleep(0.1)


def test_library_browse_match_retag_edit_zip_delete(world):
    from app import db
    c = world.client
    _seed_messy_album(world)
    albums = c.get("/api/library/albums").json()
    assert albums["total"] == 1 and albums["items"][0]["untagged"] == 3
    aid = albums["items"][0]["id"]
    assert c.get("/api/library/artists").json()[0]["name"] == "Fake Band"
    assert len(c.get("/api/library/tracks?q=beta").json()) == 1
    assert c.get("/api/library/recent").json()[0]["id"] == aid
    cands = c.get(f"/api/library/albums/{aid}/match").json()
    assert cands["candidates"][0]["id"] == "r-album" and cands["candidates"][0]["match"] > 0.9
    pv = c.get(f"/api/library/albums/{aid}/preview?release_id=r-album").json()
    assert pv["confidence"] > 0.9 and pv["rows"][0]["new_path"] == "Fake Band/Fake Album (2020)/01 - Alpha.mp3"
    alpha_id = db.one("SELECT id FROM tracks WHERE title='alpha'")["id"]
    c.post(f"/api/likes/{alpha_id}", headers=H)
    out = c.post(f"/api/library/albums/{aid}/retag", json={"release_id": "r-album"}, headers=H).json()
    new_aid = out["album_id"]
    assert db.one("SELECT title FROM tracks WHERE id=?", (alpha_id,))["title"] == "Alpha", "retag keeps the same library row"
    assert alpha_id in c.get("/api/likes").json(), "likes survive retag + rename"
    a = c.get(f"/api/library/albums/{new_aid}").json()
    assert [t["title"] for t in a["tracks"]] == ["Alpha", "Beta", "Gamma"] and a["mb_releasegroupid"] == "rg-album"
    assert c.get(f"/api/library/albums/{new_aid}/cover").status_code == 200
    ids = [t["id"] for t in a["tracks"]]
    out = c.post("/api/library/tags", json={"track_ids": ids, "values": {"genre": "Synthwave"}}, headers=H).json()
    assert c.get(f"/api/library/albums/{out['album_id']}").json()["genre"] == "Synthwave"
    z = c.get(f"/api/library/albums/{out['album_id']}/download")
    assert z.status_code == 200 and z.content[:2] == b"PK"
    assert c.delete(f"/api/library/albums/{out['album_id']}", headers=H).status_code == 200
    assert db.one("SELECT COUNT(*) n FROM tracks")["n"] == 0 and not (world.library / "Fake Band").exists()


def test_bulk_autotag_verify_duplicates(world):
    from app import db
    c = world.client
    _seed_messy_album(world)
    make_audio(str(world.library / "Singles" / "alpha copy.mp3"), "Alpha", seconds=5)          # duplicate recording
    make_audio(str(world.library / "Singles" / "Fake Band - Beta.mp3"), "Gamma", seconds=6)    # tagged Gamma, audio is Beta
    c.post("/api/library/scan", headers=H)
    time.sleep(1)
    c.post("/api/library/autotag", headers=H)
    for _ in range(100):
        s = c.get("/api/library/stats").json()
        if not s["bulk"]["running"] and s["bulk"]["done"]:
            break
        time.sleep(0.1)
    assert db.one("SELECT COUNT(*) n FROM tracks WHERE mb_releasegroupid='rg-album'")["n"] == 3
    assert c.post("/api/library/verify", headers=H).status_code == 200
    for _ in range(100):
        if not c.get("/api/library/stats").json()["verify"]["running"]:
            break
        time.sleep(0.1)
    mm = c.get("/api/library/mismatches").json()
    assert len(mm) == 1 and mm[0]["title"] == "Gamma" and "Beta" in mm[0]["verify_detail"]
    groups = c.get("/api/library/duplicates").json()
    alpha = [g for g in groups if g["tracks"][0]["title"] == "Alpha"]
    assert alpha and "AcoustID" in alpha[0]["reason"] or "MusicBrainz" in alpha[0]["reason"]
    r = c.post("/api/library/duplicates/resolve-all", headers=H).json()
    assert r["deleted"] >= 1
    assert db.one("SELECT COUNT(*) n FROM tracks WHERE title='Alpha'")["n"] == 1


# ---------------------------------------------------------------- playlists ---------
def test_playlist_imports_from_every_source(world):
    c = world.client
    cases = {
        "https://open.spotify.com/playlist/pl1": ("Spotify Mix", 3),
        "https://www.deezer.com/en/playlist/55": ("Deezer Mix", 3),
        "https://tidal.com/browse/playlist/abc-def": ("Tidal Mix", 2),
        "https://music.apple.com/us/playlist/x/pl.1": (None, 1),
        "https://www.qobuz.com/us-en/playlists/picks/1": ("Qobuz Picks", 1),
        "https://listenbrainz.org/playlist/11111111-2222-3333-4444-555555555555": ("LB Playlist", 1),
        "https://music.youtube.com/playlist?list=PLytm1": ("YTM Mix", 3),
    }
    for url, (name, n) in cases.items():
        p = c.post("/api/playlists/preview", json={"url": url}, headers=H)
        assert p.status_code == 200, (url, p.text)
        p = p.json()
        assert p["count"] == n, (url, p)
        if name:
            assert p["name"] == name
    # dedupe + version awareness on real imports
    sp = c.post("/api/playlists/preview", json={"url": "https://open.spotify.com/playlist/pl1"}, headers=H).json()
    assert sp["duplicates"] == 1                     # "Alpha - Remastered 2021" == "Alpha"
    dz = c.post("/api/playlists/preview", json={"url": "https://www.deezer.com/en/playlist/55"}, headers=H).json()
    assert dz["duplicates"] == 0                     # "Alpha (Club Remix)" ≠ "Alpha"
    yt = c.post("/api/playlists/preview", json={"url": "https://music.youtube.com/playlist?list=PLytm1"}, headers=H).json()
    assert yt["duplicates"] == 1
    td = c.post("/api/playlists/preview", json={"url": "https://tidal.com/browse/playlist/abc-def"}, headers=H).json()
    assert [t["title"] for t in td["tracks"]] == ["Alpha", "Gamma (Live)"]
    txt = c.post("/api/playlists/preview", json={"text": "Fake Band - Alpha\nFake Band - Beta"}, headers=H).json()
    assert txt["count"] == 2
    # Spotify without app credentials falls back to the embed page
    from app import config
    config.set_many({"spotify_client_id": "", "spotify_client_secret": ""})
    emb = c.post("/api/playlists/preview", json={"url": "https://open.spotify.com/playlist/xyz"}, headers=H).json()
    assert emb["name"] == "Embed List" and emb["tracks"][0]["title"] == "Beta"


def test_playlist_lifecycle_merge_request_missing_and_likes(world):
    from app import config, db
    c = world.client
    config.set_many({"slskd_enabled": True})
    world.drive(world.req(kind="album", mbid="rg-album", artist="Fake Band", title="Fake Album"))
    a = c.post("/api/playlists/import", json={"url": "https://open.spotify.com/playlist/pl1", "follow": True}, headers=H).json()
    assert a["added"] == 2 and a["duplicates_skipped"] == 1
    pl = c.get(f"/api/playlists/{a['id']}").json()
    assert [i["title"] for i in pl["items"]] == ["Alpha", "Delta"] and pl["matched"] == 1 and pl["follow"] == 1
    b = c.post("/api/playlists/import", json={"text": "Fake Band - Beta\nFake Band - Alpha", "name": "Mine"}, headers=H).json()
    m = c.post("/api/playlists/merge", json={"ids": [a["id"], b["id"]], "name": "All"}, headers=H).json()
    assert m["total"] == 4 and m["added"] == 3
    r = c.post(f"/api/playlists/{m['id']}/request-missing", headers=H).json()
    assert r["requested"] == 1
    req = db.one("SELECT * FROM requests WHERE kind='track'")
    assert req["title"] == "Delta" and json.loads(req["detail"])["isrc"] == "QZAAA0000004"
    assert world.drive(req["id"])["status"] == "completed"
    assert c.get(f"/api/playlists/{m['id']}").json()["matched"] == 3      # playlist re-matched after download
    items = c.get(f"/api/playlists/{m['id']}").json()["items"]
    c.post(f"/api/playlists/{m['id']}/move", json={"item_id": items[2]["id"], "to": 0}, headers=H)
    assert c.get(f"/api/playlists/{m['id']}").json()["items"][0]["id"] == items[2]["id"]
    c.delete(f"/api/playlists/{m['id']}/items/{items[0]['id']}", headers=H)
    assert c.get(f"/api/playlists/{m['id']}").json()["total"] == 2
    tid = db.one("SELECT id FROM tracks WHERE title='Gamma'")["id"]
    assert c.post(f"/api/playlists/{m['id']}/tracks", json={"track_ids": [tid, tid]}, headers=H).json()["added"] == 1
    assert c.post(f"/api/likes/{tid}", headers=H).json()["liked"] and tid in c.get("/api/likes").json()
    c.delete(f"/api/likes/{tid}", headers=H)
    assert tid not in c.get("/api/likes").json()
    assert c.post(f"/api/playlists/{a['id']}/refresh", headers=H).json()["added"] == 0
    c.patch(f"/api/playlists/{b['id']}", json={"name": "Renamed"}, headers=H)
    assert c.get(f"/api/playlists/{b['id']}").json()["name"] == "Renamed"
    assert c.delete(f"/api/playlists/{b['id']}", headers=H).status_code == 200
    liked = next(p for p in c.get("/api/playlists").json() if p["kind"] == "liked")
    assert c.delete(f"/api/playlists/{liked['id']}", headers=H).status_code == 400


# ---------------------------------------------------------------- connections & sync -
def test_connect_every_service(world):
    c = world.client
    assert c.post("/api/connections/jellyfin", json={"username": "me", "password": "nope"}, headers=H).status_code == 400
    assert c.post("/api/connections/jellyfin", json={"username": "me", "password": "jfpass"}, headers=H).json()["account"] == "me"
    pin = c.post("/api/connections/plex", json={}, headers=H).json()
    assert "app.plex.tv/auth" in pin["url"]
    assert c.post("/api/connections/plex/check", json={"pin_id": pin["pin_id"]}, headers=H).json()["done"] is False
    assert c.post("/api/connections/plex/check", json={"pin_id": pin["pin_id"]}, headers=H).json()["account"] == "plexuser"
    from app.services import connections
    assert connections.get(1, "plex")["data"]["token"] == "plex-server-token"
    assert c.post("/api/connections/navidrome", json={"username": "me", "password": "ndpass"}, headers=H).status_code == 200
    assert c.post("/api/connections/listenbrainz", json={"token": "lb-token"}, headers=H).json()["account"] == "lbuser"
    assert c.post("/api/connections/lastfm", json={"username": "lfmuser"}, headers=H).status_code == 200
    assert c.post("/api/connections/deezer", json={"profile": "https://www.deezer.com/profile/4242"}, headers=H).status_code == 200
    for svc in ("spotify", "tidal"):
        url = c.post(f"/api/connections/{svc}", json={}, headers=H).json()["url"]
        state = url.split("state=")[1].split("&")[0]
        r = c.get(f"/api/connections/{svc}/callback?code=abc&state={state}", follow_redirects=False)
        assert r.status_code in (302, 307) and f"connected={svc}" in r.headers["location"], r.headers.get("location")
    raw = "accept: */*\ncookie: SAPISID=abc; __Secure-3PAPISID=abc\nx-goog-authuser: 0\nuser-agent: Mozilla/5.0\n"
    assert c.post("/api/connections/ytmusic", json={"headers": raw}, headers=H).json()["account"] == "YT User"
    d = c.get("/api/connections").json()
    assert set(d["connections"]) == {"jellyfin", "plex", "navidrome", "listenbrainz", "lastfm", "deezer", "spotify", "tidal", "ytmusic"}
    for svc in ("jellyfin", "plex", "navidrome", "spotify", "tidal", "deezer", "ytmusic"):
        r = c.get(f"/api/connections/{svc}/playlists")
        assert r.status_code == 200, (svc, r.text)
    imp = c.post("/api/connections/ytmusic/playlists/PLytm1/import",
                 json={"name": "YTM Mix", "url": "https://music.youtube.com/playlist?list=PLytm1", "follow": True}, headers=H).json()
    assert imp["added"] == 2
    assert c.delete("/api/connections/lastfm", headers=H).status_code == 200


def test_two_way_likes_and_playlist_push_across_all_servers(world):
    from app import config, db
    c = world.client
    config.set_many({"slskd_enabled": True})
    world.drive(world.req(kind="album", mbid="rg-album", artist="Fake Band", title="Fake Album"))
    c.post("/api/connections/jellyfin", json={"username": "me", "password": "jfpass"}, headers=H)
    pin = c.post("/api/connections/plex", json={}, headers=H).json()
    c.post("/api/connections/plex/check", json={"pin_id": pin["pin_id"]}, headers=H)
    c.post("/api/connections/plex/check", json={"pin_id": pin["pin_id"]}, headers=H)
    c.post("/api/connections/navidrome", json={"username": "me", "password": "ndpass"}, headers=H)
    c.post("/api/connections/listenbrainz", json={"token": "lb-token"}, headers=H)
    c.post("/api/connections/ytmusic", json={"headers": "cookie: SAPISID=a\nx-goog-authuser: 0\n"}, headers=H)
    items = {i["title"]: i["id"] for i in fakenet.lib_items()}
    ids = {t["title"]: t["id"] for t in db.query("SELECT id, title FROM tracks")}

    # like Alpha in Finamp → appears everywhere
    NET.jf["favs"].add(items["Alpha"])
    s = c.post("/api/sync", headers=H).json()
    assert s["errors"] == 0, s
    assert NET.plex["ratings"].get(items["Alpha"]) == 10
    assert items["Alpha"] in NET.nd["stars"]
    assert NET.lb["loves"].get("rec-alpha") == 1
    assert FakeYTM.rated.get("vid-alpha") == "LIKE"
    assert ids["Alpha"] in c.get("/api/likes").json()

    # love Beta in Plexamp → reaches Jellyfin
    NET.plex["ratings"][items["Beta"]] = 10
    c.post("/api/sync", headers=H)
    assert items["Beta"] in NET.jf["favs"]

    # like Gamma on YouTube Music → reaches Navidrome
    FakeYTM.rated["vid-gamma"] = "LIKE"
    c.post("/api/sync", headers=H)
    assert items["Gamma"] in NET.nd["stars"] and items["Gamma"] in NET.jf["favs"]

    # un-favourite Alpha in Finamp → removed everywhere
    NET.jf["favs"].discard(items["Alpha"])
    c.post("/api/sync", headers=H)
    assert items["Alpha"] not in NET.nd["stars"] and items["Alpha"] not in NET.plex["ratings"]
    assert NET.lb["loves"].get("rec-alpha") == 0 and FakeYTM.rated.get("vid-alpha") == "INDIFFERENT"
    assert ids["Alpha"] not in c.get("/api/likes").json()

    # playlist mirrored to every server, then updated in place
    pl = c.post("/api/playlists/import", json={"text": "Fake Band - Gamma\nFake Band - Alpha", "name": "Road trip"}, headers=H).json()
    c.patch(f"/api/playlists/{pl['id']}", json={"sync_targets": ["jellyfin", "plex", "navidrome", "listenbrainz", "ytmusic"]}, headers=H)
    s = c.post(f"/api/playlists/{pl['id']}/sync", headers=H).json()
    assert s["errors"] == 0, s
    jf = next(v for v in NET.jf["playlists"].values() if v["name"] == "Road trip")
    assert jf["items"] == [items["Gamma"], items["Alpha"]]
    assert next(v for v in NET.plex["playlists"].values() if v["name"] == "Road trip")["items"] == [items["Gamma"], items["Alpha"]]
    assert next(v for v in NET.nd["playlists"].values() if v["name"] == "Road trip")["items"] == [items["Gamma"], items["Alpha"]]
    assert [t["identifier"] for t in list(NET.lb["playlists"].values())[0]] == [
        "https://musicbrainz.org/recording/rec-gamma", "https://musicbrainz.org/recording/rec-alpha"]
    assert list(FakeYTM.playlists.values())[0] == ["vid-gamma", "vid-alpha"]
    c.post(f"/api/playlists/{pl['id']}/tracks", json={"track_ids": [ids["Beta"]]}, headers=H)
    c.post(f"/api/playlists/{pl['id']}/sync", headers=H)
    assert len(NET.jf["playlists"]) == 1 and jf["items"] == [items["Gamma"], items["Alpha"], items["Beta"]]
    assert list(FakeYTM.playlists.values())[0] == ["vid-gamma", "vid-alpha", "vid-beta"]


def test_streaming_likes_pull_and_auto_download(world):
    from app import config, db
    c = world.client
    config.set_many({"streamrip_enabled": True, "auto_request_likes": True})
    for svc, body in (("deezer", {"profile": "4242"}), ("lastfm", {"username": "u"})):
        c.post(f"/api/connections/{svc}", json=body, headers=H)
    url = c.post("/api/connections/spotify", json={}, headers=H).json()["url"]
    c.get(f"/api/connections/spotify/callback?code=x&state={url.split('state=')[1].split('&')[0]}", follow_redirects=False)
    s = c.post("/api/sync", headers=H).json()
    assert s["liked_added"] == 3 and s.get("requested") == 3, s   # Beta (Deezer), Delta (Last.fm), Gamma (Spotify)
    reqs = db.query("SELECT id, title, status FROM requests ORDER BY title")
    assert [r["title"] for r in reqs] == ["Beta", "Delta", "Gamma"] and all(r["status"] == "queued" for r in reqs)
    for r in reqs:
        world.drive(r["id"])
    assert {t["title"] for t in tracks(db)} >= {"Beta", "Delta", "Gamma"}


# ---------------------------------------------------------------- system & admin -----
def test_system_status_and_service_tests(world):
    from app import config
    c = world.client
    config.set_many({"lidarr_enabled": True, "slskd_enabled": True, "streamrip_enabled": True})
    st = c.get("/api/system/status").json()["services"]
    assert st["musicbrainz"]["ok"] and st["lidarr"]["ok"] and st["slskd"]["ok"] and st["acoustid"]["ok"] and st["streamrip"]["ok"], st
    for svc in ("lidarr", "slskd", "musicbrainz", "notify", "mediaservers", "spotify", "tidal", "acoustid", "streamrip"):
        r = c.post(f"/api/system/test/{svc}", headers=H).json()
        assert r["ok"], (svc, r)
    assert c.post("/api/system/test/lidarr", headers=H).json()["options"]["root_folders"][0]["path"] == NET.library_path
    config.set_many({"acoustid_api_key": "bad"})
    assert c.post("/api/system/test/acoustid", headers=H).json()["ok"] is False
    assert c.get("/api/system/downloads").json()["errors"] == {}
    assert c.get("/api/system/lidarr/options").status_code == 200
    assert c.get("/api/system/activity").status_code == 200


def test_admin_autodownload_lossy_upgrade(world):
    from app import config, db
    c = world.client
    config.set_many({"ytdlp_enabled": True})
    world.drive(world.req(kind="album", mbid="rg-album", artist="Fake Band", title="Fake Album"))
    assert {t["format"] for t in tracks(db)} == {"opus"}
    config.set_many({"ytdlp_enabled": False, "slskd_enabled": True})
    assert c.post("/api/admin/autodownload/upgrade-now", headers=H).json()["queued"] == 1
    up = db.one("SELECT * FROM requests WHERE detail LIKE '%lossless_only%'")
    r = world.drive(up["id"])
    assert r["status"] == "completed", r
    ts = tracks(db)
    assert len(ts) == 3 and {t["format"] for t in ts} == {"flac"}, "lossless copies replace the YouTube ones"
    d = c.get("/api/admin/autodownload").json()
    assert d["stats"]["lossy_albums"] == 0 and d["stats"]["upgrades_done"] == 1
    assert c.post("/api/admin/autodownload/run-now", headers=H).json()["ok"]


def test_artist_requests_lidarr_and_discography(world):
    from app import config, db
    config.set_many({"lidarr_enabled": True})
    r = world.drive(world.req(kind="artist", mbid="a-fake", artist="Fake Band"))
    assert r["status"] == "completed" and "monitoring all albums" in r["message"]
    assert NET.lidarr["artists"][1]["addOptions"]["monitor"] == "all"
    config.set_many({"lidarr_enabled": False})
    db.execute("DELETE FROM requests")
    r = world.drive(world.req(kind="artist", mbid="a-fake", artist="Fake Band"))
    kids = db.query("SELECT title FROM requests WHERE parent_id=?", (r["id"],))
    assert [k["title"] for k in kids] == ["Fake Album"], "only studio albums/EPs, no singles or compilations"


def test_failed_transfer_moves_to_next_user_and_failure_notifies(world, monkeypatch):
    from app import config, db
    from app.services import slskd
    config.set_many({"slskd_enabled": True, "quality_first": False})
    real = slskd.user_downloads

    async def flaky(user):
        files = await real(user)
        if user == "peer":  # first user's transfers error out
            return [{**f, "state": "Completed, Errored"} for f in files]
        return files
    monkeypatch.setattr(slskd, "user_downloads", flaky)
    orig_rank = slskd.rank_album

    def two_users(*a, **k):
        cands = orig_rank(*a, **k)
        return cands + [{**cands[0], "username": "peer-b", "score": 1}] if cands else cands
    monkeypatch.setattr(slskd, "rank_album", two_users)
    r = world.drive(world.req(kind="album", mbid="rg-album", artist="Fake Band", title="Fake Album"))
    assert r["status"] == "completed", r
    assert "peer-b" in r["message"] or db.one("SELECT COUNT(*) n FROM tracks")["n"] == 3
    # nothing anywhere → failure notification
    NET.notifications.clear()
    config.set_many({"slskd_enabled": False})
    r = world.drive(world.req(kind="track", artist="Nobody", title="Nothing"))
    assert r["status"] == "failed" and ("discord", "❌ Request failed") in NET.notifications


def test_account_sessions_password_and_api_key(world):
    c = world.client
    s = c.get("/api/auth/sessions").json()
    assert len(s) == 1 and s[0]["current"]
    assert c.post("/api/auth/password", json={"current": "wrong", "new": "newpassword1"}, headers=H).status_code == 400
    assert c.post("/api/auth/password", json={"current": "adminpass123", "new": "newpassword1"}, headers=H).status_code == 200
    old = c.get("/api/system/api-key").json()["api_key"]
    new = c.post("/api/admin/settings/api-key", headers=H).json()["api_key"]
    assert new != old and c.get("/api/auth/me", headers={"X-Api-Key": new}).status_code == 200
    c.post("/api/auth/logout", headers=H)
    assert c.get("/api/auth/me").status_code == 401
    assert c.post("/api/auth/login", json={"username": "admin", "password": "newpassword1"}).status_code == 200


def test_import_media_server_playlist_and_follow(world):
    from app import config
    c = world.client
    config.set_many({"slskd_enabled": True})
    world.drive(world.req(kind="album", mbid="rg-album", artist="Fake Band", title="Fake Album"))
    c.post("/api/connections/jellyfin", json={"username": "me", "password": "jfpass"}, headers=H)
    items = {i["title"]: i["id"] for i in fakenet.lib_items()}
    NET.jf["playlists"]["jfx"] = {"name": "Made in Finamp", "items": [items["Beta"], items["Alpha"]]}
    lst = c.get("/api/connections/jellyfin/playlists").json()
    assert {"id": "jfx", "name": "Made in Finamp"} in lst
    r = c.post("/api/connections/jellyfin/playlists/jfx/import", json={"name": "Made in Finamp", "follow": True}, headers=H).json()
    pl = c.get(f"/api/playlists/{r['id']}").json()
    assert [i["title"] for i in pl["items"]] == ["Beta", "Alpha"] and pl["matched"] == 2
    NET.jf["playlists"]["jfx"]["items"].append(items["Gamma"])
    assert c.post(f"/api/playlists/{r['id']}/refresh", headers=H).json()["added"] == 1
    # can't push a playlist back to the server it follows
    out = c.patch(f"/api/playlists/{r['id']}", json={"sync_targets": ["jellyfin", "plex"]}, headers=H).json()
    assert out["sync_targets"] == ["plex"]


def test_lidarr_test_returns_message_and_options_and_diagnostics(world):
    from app import config
    c = world.client
    r = c.post("/api/system/test/lidarr", headers=H).json()
    assert r["ok"] and r["message"].startswith("Lidarr") and r["options"]["quality_profiles"]
    config.set_many({"slskd_api_key": "wrong"})
    r = c.post("/api/system/test/slskd", headers=H).json()
    assert r["ok"] is True  # fake slskd doesn't check keys – friendly 401 text covered in test_auth
    d = c.get("/api/system/diagnostics").json()
    names = {x["name"]: x for x in d["checks"]}
    assert names["Lidarr"]["ok"] and names["Lidarr"]["status"] == 200 and "lk" not in names["Lidarr"]["request"]
    assert names["Music library"]["ok"] and names["ffmpeg"]["ok"]
    config.set_many({"lidarr_url": "http://nonexistent-host-xyz:8686"})
    d = c.get("/api/system/diagnostics").json()
    lid = next(x for x in d["checks"] if x["name"] == "Lidarr")
    assert not lid["ok"] and "Can't find host" in lid["detail"]


def test_one_song_but_whole_album_from_a_link(world):
    from app import config, db
    c = world.client
    config.set_many({"slskd_enabled": True, "album_upgrade": "off"})   # admin default: songs only…
    r = c.post("/api/requests/from-link", json={"url": "https://open.spotify.com/track/sp1", "album_mode": "album"}, headers=H).json()
    assert r["requested"] == 1
    tid = r["ids"][0]
    world.drive(tid, 2)
    t = db.get_request(tid)
    assert t["detail"]["album_mode"] == "album" and t["source"] == "album", t   # …but this request pulls the album
    album = db.one("SELECT * FROM requests WHERE kind='album'")
    assert album["mbid"] == "rg-album"
    world.drive(album["id"])
    assert world.drive(tid)["status"] == "completed" and len(tracks(db)) == 3
    # and the reverse: admin default = albums, this request = just the song
    config.set_many({"album_upgrade": "album_ep", "slskd_enabled": False, "streamrip_enabled": True})
    r = c.post("/api/requests/from-link", json={"url": "https://open.spotify.com/track/sp4", "album_mode": "song"}, headers=H).json()
    assert world.drive(r["ids"][0])["status"] == "completed"
    assert db.one("SELECT COUNT(*) n FROM requests WHERE kind='album'")["n"] == 1


def test_origin_is_recorded_and_shown(world):
    from app import config, db
    c = world.client
    config.set_many({"slskd_enabled": True})
    world.drive(world.req(kind="album", mbid="rg-album", artist="Fake Band", title="Fake Album"))
    aid = db.one("SELECT album_id FROM tracks LIMIT 1")["album_id"]
    t = c.get(f"/api/library/albums/{aid}").json()["tracks"][0]
    assert t["source"] == "Soulseek" and "peer" in t["source_detail"] and t["downloaded_at"] and t["request_id"]
    config.set_many({"slskd_enabled": False, "ytdlp_enabled": True})
    world.drive(world.req(kind="track", mbid="rec-delta", artist="Fake Band", title="Delta"))
    d = db.one("SELECT * FROM tracks WHERE title='Delta'")
    assert d["source"] == "YouTube Music" and d["source_url"] == "https://music.youtube.com/watch?v=vid-delta"


def test_artwork_fallback_for_releases_musicbrainz_lacks(world):
    from app import db
    from app.services import library, tagger
    c = world.client
    stage = world.library.parent / "staging"
    f = make_audio(str(stage / "gem.opus"), "Hidden Gem", artist="Obscure Artist", seconds=9)
    p, _ = world.run(tagger.import_single, f, "Obscure Artist", "Hidden Gem", None, str(stage), None, False)
    assert library.extract_embedded(p), "cover from iTunes embedded into the file"
    aid = db.one("SELECT album_id FROM tracks WHERE path=?", (p,))["album_id"]
    assert c.get(f"/api/library/albums/{aid}/cover").status_code == 200
    # a second single in the same Singles folder never borrows the first one's cover.jpg
    assert not os.path.exists(os.path.join(os.path.dirname(p), "cover.jpg"))
    # bulk "find missing artwork" endpoint runs
    assert c.post("/api/library/artwork", headers=H).status_code == 200


def test_stream_original_and_flac_transcode(world):
    from app import db
    c = world.client
    make_audio(str(world.library / "A" / "B" / "01.mp3"), "Song", album="B")
    c.post("/api/library/scan", headers=H)
    for _ in range(50):
        if db.one("SELECT id FROM tracks"):
            break
        time.sleep(0.1)
    tid = db.one("SELECT id FROM tracks")["id"]
    r = c.get(f"/api/library/tracks/{tid}/stream")
    assert r.status_code == 200 and r.headers["content-type"] == "audio/mpeg"
    r = c.get(f"/api/library/tracks/{tid}/stream", headers={"Range": "bytes=0-99"})
    assert r.status_code == 206 and len(r.content) == 100              # seekable
    r = c.get(f"/api/library/tracks/{tid}/stream?transcode=1")
    assert r.status_code == 200 and r.content[:4] == b"fLaC" and r.headers["x-transcoded"] == "flac"


def test_follow_artist_new_releases(world, monkeypatch):
    from app import db
    c = world.client
    monkeypatch.setitem(fakenet.RGS, "rg-future", fakenet.rg("rg-future", "Next Year", date="2999-01-01"))
    # new-only: existing releases are marked seen, nothing requested
    f = c.post("/api/follows", json={"mbid": "a-fake", "types": ["Album", "EP"]}, headers=H).json()
    assert f["requested"] == 0 and not db.query("SELECT id FROM requests")
    row = c.get("/api/follows").json()[0]
    assert row["name"] == "Fake Band" and row["known_releases"] == 3, "future release not counted as seen"
    assert c.get("/api/discover/artist/a-fake").json()["follow"]["id"] == f["id"]
    assert c.post("/api/follows/check", headers=H).json() == {"new_releases": 0}
    # a brand-new album appears → requested and queued (admin gets auto-download)
    monkeypatch.setitem(fakenet.RGS, "rg-new", fakenet.rg("rg-new", "Brand New", date="2026-01-01"))
    monkeypatch.setitem(fakenet.RGS, "rg-new-single", fakenet.rg("rg-new-single", "New Single", "Single", date="2026-01-01"))
    assert c.post("/api/follows/check", headers=H).json() == {"new_releases": 1}
    reqs = db.query("SELECT title, status, kind, mbid FROM requests")
    assert [(r["title"], r["status"], r["kind"], r["mbid"]) for r in reqs] == [("Brand New", "queued", "album", "rg-new")]
    assert c.post("/api/follows/check", headers=H).json() == {"new_releases": 0}, "never requested twice"
    # announced release is picked up once it is out
    fakenet.RGS["rg-future"]["first-release-date"] = "2020-02-02"
    assert c.post("/api/follows/check", headers=H).json() == {"new_releases": 1}
    # types can be changed; unfollow removes it
    assert c.patch(f"/api/follows/{f['id']}", json={"types": ["Single", "Bogus"]}, headers=H).json()["types"] == ["Single"]
    assert c.delete(f"/api/follows/{f['id']}", headers=H).json()["ok"]
    assert c.get("/api/follows").json() == [] and c.get("/api/discover/artist/a-fake").json()["follow"] is None


def test_follow_backfill_and_library_artists(world):
    import time

    from app import db
    c = world.client
    # backfill: missing studio album requested, singles and compilations are not
    r = c.post("/api/follows", json={"mbid": "a-fake", "backfill": True}, headers=H).json()
    assert r["requested"] == 1
    assert [x["title"] for x in db.query("SELECT title FROM requests")] == ["Fake Album"]
    c.delete(f"/api/follows/{r['id']}", headers=H)
    # follow everyone in the library (artist matched by exact name on MusicBrainz)
    fakenet.make_audio(str(world.library / "Fake Band" / "x.mp3"), "Alpha", album="Fake Album")
    assert c.post("/api/library/scan", headers=H).status_code == 200
    for _ in range(50):
        if db.one("SELECT COUNT(*) n FROM tracks")["n"]:
            break
        time.sleep(0.1)
    assert c.post("/api/follows/library", headers=H).status_code == 200
    for _ in range(50):
        st = c.get("/api/follows/library").json()
        if not st.get("running"):
            break
        time.sleep(0.1)
    assert st["followed"] == 1, st
    assert [f["mbid"] for f in c.get("/api/follows").json()] == ["a-fake"]


def test_artist_page_full_discography_extras_and_library(world):
    from app.services import artistinfo
    artistinfo._cache.clear()
    c = world.client
    fakenet.make_audio(str(world.library / "Fake Band" / "Fake Album" / "01.flac"), "Alpha", album="Fake Album")
    fakenet.make_audio(str(world.library / "Fake Band" / "Basement Tapes" / "01.mp3"), "Demo One", album="Basement Tapes")
    world.run(__import__("app.services.library", fromlist=["x"]).scan)
    a = c.get("/api/discover/artist/a-fake").json()
    assert {r["id"] for r in a["release_groups"]} == {"rg-album", "rg-single", "rg-comp"}, "every release type is listed"
    assert {r["album"] for r in a["library_albums"]} == {"Fake Album", "Basement Tapes"} and a["library_tracks"] == 2
    assert [r["in_library"] for r in a["release_groups"] if r["id"] == "rg-album"] == [True]
    x = c.get("/api/discover/artist/a-fake/extras").json()
    assert x["image"].endswith("1000x1000.jpg") and x["bio"].startswith("Fake Band is") and x["bio_url"].endswith("/Fake_Band")
    assert [t["title"] for t in x["top"]][:2] == ["Alpha", "Beta"] and x["top"][0]["library"]["title"] == "Alpha"
    assert x["top"][1]["library"] is None and x["top"][-1]["title"] == "Unreleased Gem"
    assert len(c.get("/api/discover/artist/a-fake/library-tracks?name=Fake%20Band").json()) == 2
    assert c.get("/api/discover/artist-lookup?name=fake band").json() == {"mbid": "a-fake"}
    assert c.get("/api/discover/artist-lookup?name=Nobody Here").status_code == 404


def test_manual_download_picker(world):
    from app import config, db
    c = world.client
    config.set_many({"slskd_enabled": True, "streamrip_enabled": True, "ytdlp_enabled": True, "lidarr_enabled": True})
    assert [s["id"] for s in c.get("/api/picker/sources").json()["sources"]] == ["slskd", "deezer", "ytmusic", "lidarr"]
    album = {"kind": "album", "artist": "Fake Band", "title": "Fake Album", "mbid": "rg-album", "artist_mbid": "a-fake", "track_count": 3}

    # Soulseek: every folder is listed (best first) – pick the MP3 one on purpose
    res = c.post("/api/picker/search", json={"source": "slskd", **album}, headers=H).json()
    assert [(x["payload"]["username"], x["quality"], x["tracks"]) for x in res] == [("peer", "FLAC 16-bit", 3), ("mp3peer", "MP3 320k", 3)]
    req = {"kind": "album", "mbid": "rg-album", "artist": "Fake Band", "title": "Fake Album", "album": "Fake Album"}
    p = c.post("/api/picker/pick", json={"source": "slskd", "payload": res[1]["payload"], "title": res[1]["title"], "request": req}, headers=H).json()
    r = world.drive(p["id"])
    assert r["status"] == "completed" and "Soulseek (MP3)" in r["message"], r
    assert {t["format"] for t in tracks(db)} == {"mp3"}
    assert "mp3peer" in db.one("SELECT source_detail FROM tracks LIMIT 1")["source_detail"]

    # Deezer song (via streamrip) for an existing queued request → old one is replaced
    old = world.req(kind="track", mbid="rec-delta", artist="Fake Band", title="Delta")
    res = c.post("/api/picker/search", json={"source": "deezer", "kind": "track", "artist": "Fake Band", "title": "Delta", "length": 8}, headers=H).json()
    assert res[0]["title"] == "Delta" and res[0]["lossless"]
    p = c.post("/api/picker/pick", json={"source": "deezer", "payload": res[0]["payload"], "request_id": old}, headers=H).json()
    assert db.get_request(old)["status"] == "cancelled"
    r = world.drive(p["id"])
    assert r["status"] == "completed" and "Deezer" in r["message"], r

    # YouTube Music album → resolved to its playlist
    res = c.post("/api/picker/search", json={"source": "ytmusic", **album}, headers=H).json()
    assert res[0]["payload"] == {"browseId": "MPREb_fake"}
    db.execute("DELETE FROM tracks")
    p = c.post("/api/picker/pick", json={"source": "ytmusic", "payload": res[0]["payload"], "request": req}, headers=H).json()
    r = world.drive(p["id"])
    assert r["status"] == "completed" and "YouTube Music" in r["message"], r

    # Lidarr interactive search → grab that exact release
    first = c.post("/api/picker/search", json={"source": "lidarr", **album}, headers=H)
    assert first.status_code == 400 and "try again" in first.json()["detail"]   # Lidarr adds + loads the artist first
    res = c.post("/api/picker/search", json={"source": "lidarr", **album}, headers=H).json()
    assert [x["quality"] for x in res] == ["FLAC", "MP3-320"] and res[1]["warning"] == "Not wanted in profile"
    p = c.post("/api/picker/pick", json={"source": "lidarr", "payload": res[0]["payload"], "title": res[0]["title"], "request": req}, headers=H).json()
    r = world.drive(p["id"])
    assert r["status"] == "completed" and "grab:g-flac" in NET.lidarr["commands"], r

    # any link
    p = c.post("/api/picker/pick", json={"source": "link", "payload": {"url": f"{world.web}/gamma.mp3"},
                                         "request": {"kind": "track", "artist": "Fake Band", "title": "Gamma", "length": 7}}, headers=H).json()
    r = world.drive(p["id"])
    assert r["status"] == "completed", r
    assert c.post("/api/picker/pick", json={"source": "link", "payload": {"url": "nope"}, "request": req}, headers=H).status_code == 400

    # can be limited to admins
    config.set_many({"manual_search": "admins"})
    assert c.get("/api/picker/sources").json()["allowed"] is True


def test_catalog_merge_fills_musicbrainz_gaps(world):
    from app import config, db
    c = world.client
    rel = c.get("/api/discover/artist/a-fake/catalog").json()["releases"]
    got = {r["title"]: r for r in rel}
    # Deluxe edition (title), "Delta" single (title), Renamed In Store (UPC = MB barcode), Hits Vol 1 (ISRC → MB compilation) are matched;
    # Fake Album on YouTube Music / Bandcamp / Discogs too; a compilation the artist only appears on is ignored
    assert set(got) == {"Lost Tapes", "Apple Exclusive", "YT Mixtape", "Basement Demos", "Live At The Fake Club"}, list(got)
    lost = got["Lost Tapes"]
    assert [s["name"] for s in lost["sources"]] == ["Deezer", "Apple Music", "Discogs"] and lost["type"] == "EP" and lost["upc"] == "0000111222"
    assert lost["ext"] == {"deezer": 900, "apple": 602, "discogs": "r3", "upc": "0000111222"}
    assert got["YT Mixtape"]["ext"] == {"ytm": "MPREb_mix"} and got["Basement Demos"]["ext"] == {"bandcamp": "https://fake.bandcamp.com/album/basement-demos"}
    assert got["Basement Demos"]["date"] == "2022-06-12" and got["Basement Demos"]["tracks"] == 2 and got["Basement Demos"]["cover"].endswith("basement-demos_10.jpg")
    live = got["Live At The Fake Club"]
    assert live["ext"] == {"discogs": "r2"} and live["date"] == "1999" and live["sources"][0]["url"] == "https://www.discogs.com/release/2"
    from app.services import catalog
    assert [(t["title"], t["length"]) for t in world.run(catalog.tracklist, {"discogs": "r2"})] == [("Alpha (Live)", 245), ("Beta (Live)", 3723)]
    assert [(t["title"], t["length"]) for t in world.run(catalog.tracklist, {"bandcamp": "https://fake.bandcamp.com/album/basement-demos"})] == [("Demo One", 10), ("Demo Two", 11)]
    assert lost["harmony"].startswith("https://harmony.pulsewidth.org.uk/release?url=https%3A%2F%2Fwww.deezer.com%2Falbum%2F900") and "gtin=0000111222" in lost["harmony"]
    assert got["Apple Exclusive"]["type"] == "Single" and got["Apple Exclusive"]["sources"][0]["name"] == "Apple Music"

    # request it: no Lidarr, straight from Deezer via streamrip, tagged from Deezer's metadata
    config.set_many({"streamrip_enabled": True, "lidarr_enabled": True})
    body = {"kind": "album", "artist": "Fake Band", "title": "Lost Tapes", "album": "Lost Tapes", "year": "2023",
            "ext": {"deezer": 900, "apple": 602, "upc": "0000111222"}}
    r = world.drive(world.req(**body))
    assert r["status"] == "completed" and "Deezer" in r["message"], r
    assert r["source"] != "lidarr"
    ts = db.query("SELECT title, artist, album, albumartist, track, year, mb_albumid, path FROM tracks ORDER BY track")
    assert [(t["title"], t["album"], t["albumartist"], t["track"], t["year"]) for t in ts] == [
        ("Lost One", "Lost Tapes", "Fake Band", 1, "2023"), ("Lost Two", "Lost Tapes", "Fake Band", 2, "2023")]
    assert not ts[0]["mb_albumid"], "stays flagged as not in MusicBrainz"
    import mutagen
    assert mutagen.File(ts[0]["path"], easy=True)["isrc"] == ["QZLOST000001"]
    assert c.post("/api/requests", json=body, headers=H).status_code == 409
    from app import worker
    assert world.run(worker.retag_external) == 0, "still not in MusicBrainz → left as is"
    assert any(r["title"] == "Lost Tapes" and r["in_library"] for r in c.get("/api/discover/artist/a-fake/catalog").json()["releases"])
    assert c.post("/api/requests", json={"kind": "album", "artist": "x", "title": "y"}, headers=H).status_code == 400

    # YouTube Music- and Bandcamp-only releases download from exactly that release
    config.set_many({"ytdlp_enabled": True, "streamrip_enabled": False})
    r = world.drive(world.req(kind="album", artist="Fake Band", title="YT Mixtape", album="YT Mixtape", year="2022", ext=got["YT Mixtape"]["ext"]))
    assert r["status"] == "completed" and "YouTube Music" in r["message"], r
    r = world.drive(world.req(kind="album", artist="Fake Band", title="Basement Demos", album="Basement Demos", year="2022", ext=got["Basement Demos"]["ext"]))
    assert r["status"] == "completed" and "Bandcamp" in r["message"], r
    albums = {t["title"]: t["album"] for t in db.query("SELECT title, album FROM tracks")}
    assert {k: albums.get(k) for k in ("Mix One", "Demo One", "Demo Two")} == {"Mix One": "YT Mixtape", "Demo One": "Basement Demos", "Demo Two": "Basement Demos"}
    assert db.one("SELECT source FROM tracks WHERE title='Demo One'")["source"] == "Bandcamp"

    config.set_many({"discover_catalog_merge": False})
    assert c.get("/api/discover/artist/a-fake/catalog").json() == {"enabled": False, "releases": []}


def test_discovery_for_you_similar_and_genres(world, monkeypatch):
    from app import config, db, worker
    from app.services import discovery
    c = world.client
    discovery._cache.clear()
    assert c.get("/api/discover/for-you").json() == {"connected": False, "playlists": []}
    assert c.post("/api/connections/listenbrainz", json={"token": "lb-token"}, headers=H).status_code == 200
    fakenet.make_audio(str(world.library / "Fake Band" / "Fake Album" / "02 - Beta.flac"), "Beta", album="Fake Album", track=2, seconds=6)
    world.run(__import__("app.services.library", fromlist=["x"]).scan)

    d = c.get("/api/discover/for-you").json()
    assert d["user"] == "lbuser" and [p["kind"] for p in d["playlists"]] == ["weekly-exploration", "daily-jams"]
    wx = d["playlists"][0]["tracks"]
    assert [(t["title"], bool(t["library"])) for t in wx] == [("Beta", True), ("Strange Song", False), ("Odd Song", False), ("Third Song", False)]
    assert wx[1]["mbid"] == "rec-strange" and wx[1]["artist_mbid"] == "a-other" and wx[1]["cover"].endswith("/rel-x/42-250.jpg")

    # similar artists: Last.fm first, then ListenBrainz, then Deezer
    s = c.get("/api/discover/artist/a-fake/similar").json()
    assert s["source"] == "Last.fm" and [(a["name"], a["mbid"]) for a in s["artists"]] == [("Other Artist", "a-other"), ("No Id Artist", None)]
    discovery._cache.clear()
    config.set_many({"lastfm_api_key": ""})
    s = c.get("/api/discover/artist/a-fake/similar").json()
    assert s["source"] == "ListenBrainz" and [a["name"] for a in s["artists"]] == ["Other Artist"]
    discovery._cache.clear()
    monkeypatch.setattr(discovery, "LB_LABS", "http://nonexistent-labs.test")
    s = c.get("/api/discover/artist/a-fake/similar").json()
    assert s["source"] == "Deezer" and s["artists"][0]["name"] == "Deezer Neighbour"

    # "more like this": Deezer radio without a Last.fm key, Last.fm with one
    m = c.get("/api/discover/similar-tracks?artist=Fake%20Band&title=Alpha").json()
    assert m["source"] == "Deezer radio" and [(t["title"], bool(t["library"])) for t in m["tracks"]] == [("Beta", True), ("Radio Song", False)]
    discovery._cache.clear()
    config.set_many({"lastfm_api_key": "lfm"})
    m = c.get("/api/discover/similar-tracks?artist=Fake%20Band&title=Alpha&mbid=rec-alpha").json()
    assert m["source"] == "Last.fm" and m["tracks"][0]["title"] == "Gamma"

    # genres
    g = c.get("/api/discover/tag/shoegaze").json()
    assert g["source"] == "Last.fm" and g["summary"] == "shoegaze is a genre." and g["artists"][0] == {**g["artists"][0], "name": "Fake Band", "in_library": True}
    assert "shoegaze" in c.get("/api/discover/tags").json()["top"]

    # auto-discover: top N new songs from Weekly Exploration, once per playlist
    config.set_many({"discover_auto_count": 2})
    assert world.run(discovery.auto_discover) == 2
    assert [r["title"] for r in db.query("SELECT title FROM requests WHERE kind='track' ORDER BY id")] == ["Strange Song", "Odd Song"]
    assert world.run(discovery.auto_discover) == 0, "same playlist is never processed twice"
    del worker


def test_library_sorting_files_lyrics_plays_stats_and_cleanup(world, monkeypatch):
    import datetime
    import time as _t

    from app import config, db
    from app.services import discovery
    c = world.client
    lib = world.library
    fakenet.make_audio(str(lib / "Fake Band" / "Fake Album (2020)" / "01 - Alpha.mp3"), "Alpha", album="Fake Album", track=1, seconds=5)
    fakenet.make_audio(str(lib / "Fake Band" / "Fake Album (2020)" / "02 - Beta.mp3"), "Beta", album="Fake Album", track=2, seconds=6)
    fakenet.make_audio(str(lib / "Fake Band" / "Fake Album (2020)" / "03 - Gamma.mp3"), "Gamma", album="Fake Album", track=3, seconds=7)
    _t.sleep(1.1)
    fakenet.make_audio(str(lib / "Fake Band" / "Delta" / "01 - Delta.mp3"), "Delta", album="Delta", track=1, seconds=8)
    world.run(__import__("app.services.library", fromlist=["x"]).scan)

    # albums vs singles, sort + direction
    assert [a["album"] for a in c.get("/api/library/albums?kind=albums").json()["items"]] == ["Fake Album"]
    assert [a["album"] for a in c.get("/api/library/albums?kind=singles").json()["items"]] == ["Delta"]
    assert [a["album"] for a in c.get("/api/library/albums?sort=album&dir=asc").json()["items"]] == ["Delta", "Fake Album"]
    assert [a["album"] for a in c.get("/api/library/albums?sort=album&dir=desc").json()["items"]] == ["Fake Album", "Delta"]
    assert [t["title"] for t in c.get("/api/library/tracks?sort=title&dir=desc").json()] == ["Gamma", "Delta", "Beta", "Alpha"]
    assert [t["title"] for t in c.get("/api/library/tracks?sort=duration").json()][0] == "Delta"
    assert c.get("/api/library/artists?sort=tracks&dir=desc").json()[0]["tracks"] == 4

    alpha = db.one("SELECT id, path FROM tracks WHERE title='Alpha'")
    r = c.get(f"/api/library/tracks/{alpha['id']}/file")
    assert r.status_code == 200 and "Fake%20Band%20-%20Alpha.mp3" in r.headers["content-disposition"]

    # lyrics: synced → .lrc, plain → .txt, nothing → None
    ly = c.get(f"/api/library/tracks/{alpha['id']}/lyrics").json()
    assert ly["synced"] and "[00:02.00] Second line" in ly["lyrics"] and os.path.exists(alpha["path"][:-4] + ".lrc")
    assert c.get(f"/api/library/tracks/{alpha['id']}/lyrics?fetch=false").json()["source"] == "file"
    beta = db.one("SELECT id, path FROM tracks WHERE title='Beta'")
    assert c.get(f"/api/library/tracks/{beta['id']}/lyrics").json() == {"lyrics": "Only plain words", "synced": False, "source": "LRCLIB"}
    assert os.path.exists(beta["path"][:-4] + ".txt")
    album_id = db.one("SELECT album_id FROM tracks WHERE title='Gamma'")["album_id"]
    assert c.post(f"/api/library/albums/{album_id}/lyrics", headers=H).json() == {"checked": 1, "found": 0}

    # plays, shuffle, stats, leaderboard
    for _ in range(3):
        assert c.post("/api/library/plays", json={"track_id": alpha["id"], "seconds": 31}, headers=H).status_code == 200
    me = c.get("/api/stats/me").json()
    assert me["me"]["plays"] == 3 and me["me"]["top_artist"] == "Fake Band" and me["library"]["tracks"] == 4 and me["fun"]
    lb = c.get("/api/stats/leaderboard").json()
    assert lb["boards"]["plays"]["rows"][0] == {"username": "admin", "value": 3, "me": True}
    assert lb["top_songs"][0]["title"] == "Alpha" and any("lossless" in f["label"] for f in lb["facts"])
    sh = c.get(f"/api/library/shuffle?limit=10&exclude={alpha['id']}").json()
    assert len(sh) == 3 and alpha["id"] not in [t["id"] for t in sh]

    # storage
    st = c.get("/api/library/storage").json()
    assert st["library"]["tracks"] == 4 and st["formats"][0]["format"] == "MP3" and st["disk"]["total"] > 0

    # previews: Deezer by ISRC, by search; album previews; new releases
    p = c.get("/api/discover/preview?artist=Fake%20Band&title=Beta").json()
    assert p["preview_url"].endswith(".mp3") and p["source"] == "Deezer"
    assert c.get("/api/discover/preview?artist=Nobody&title=Nothing").status_code == 404
    pa = c.get("/api/discover/preview-album?artist=Fake%20Band&title=Fake%20Album").json()
    assert pa["source"] == "Deezer" and len(pa["tracks"]) == 3
    recent = (datetime.date.today() - datetime.timedelta(days=10)).isoformat()
    monkeypatch.setitem(fakenet.DZ_ALBUMS, 950, {"title": "Brand New Thing", "record_type": "album", "release_date": recent, "upc": "", "isrcs": ["QZNEW0000001"], "titles": ["New One"]})
    discovery._cache.clear()
    nr = c.get("/api/discover/new-releases").json()["releases"]
    assert [(r["title"], r["preview_album"]) for r in nr] == [("Brand New Thing", "dz:950")]
    assert len(c.get("/api/discover/preview-album?key=dz:950").json()["tracks"]) == 1

    # any link: streaming links are matched, everything else downloads as-is
    r = c.post("/api/requests/link", json={"url": "https://www.deezer.com/playlist/55"}, headers=H).json()
    assert r["mode"] == "matched" and r["requested"] >= 1
    r = c.post("/api/requests/link", json={"url": "https://soundcloud.com/someone/song"}, headers=H).json()
    assert r["mode"] == "direct" and db.get_request(r["ids"][0])["kind"] == "url"
    assert c.post("/api/requests/link", json={"url": "not a link"}, headers=H).status_code == 400

    # clean-up: delete a song, clear finished downloads and their files
    assert c.delete(f"/api/library/tracks/{alpha['id']}", headers=H).json()["ok"]
    assert not os.path.exists(alpha["path"]) and not os.path.exists(alpha["path"][:-4] + ".lrc")
    db.execute("UPDATE requests SET status='failed'")
    rid = db.one("SELECT id FROM requests ORDER BY id DESC LIMIT 1")["id"]
    stage = os.path.join(config.get("staging_path"), f"request-{rid}")
    os.makedirs(stage, exist_ok=True)
    open(os.path.join(stage, "partial.part"), "w").close()
    r = c.post("/api/requests/clear", json={"statuses": ["failed"], "files": True}, headers=H).json()
    assert r["removed"] >= 2 and r["deleted_files"] >= 1 and not os.path.exists(stage)
    assert not db.query("SELECT id FROM requests WHERE status='failed' AND parent_id IS NULL")


def test_manual_artwork_pick_and_upload(world):
    import base64

    import mutagen
    from app import db
    c = world.client
    fakenet.make_audio(str(world.library / "Fake Band" / "Fake Album (2020)" / "01 - Alpha.flac"), "Alpha", album="Fake Album", track=1, seconds=5)
    world.run(__import__("app.services.library", fromlist=["x"]).scan)
    t = db.one("SELECT album_id, path FROM tracks")
    cands = c.get(f"/api/library/albums/{t['album_id']}/art-candidates").json()
    assert "Deezer" in {x["source"] for x in cands} and cands[0]["url"]
    r = c.post(f"/api/library/albums/{t['album_id']}/art", json={"data": "data:image/jpeg;base64," + base64.b64encode(fakenet.JPEG).decode()}, headers=H)
    assert r.json() == {"embedded": 1}
    assert mutagen.File(t["path"]).pictures[0].data == fakenet.JPEG
    assert os.path.exists(os.path.join(os.path.dirname(t["path"]), "cover.jpg"))
    assert c.post(f"/api/library/albums/{t['album_id']}/art", json={"data": base64.b64encode(b"not an image").decode()}, headers=H).status_code == 400


def test_tag_album_from_deezer_apple_and_fingerprint(world):
    import mutagen
    from app import db
    c = world.client
    d = world.library / "Unknown" / "rip"
    for i, (t, _, s, _) in enumerate(fakenet.ALBUM_TRACKS, 1):
        fakenet.make_audio(str(d / f"track{i}.flac"), f"Track {i:02d}", artist="Unknown", album="", track=i, seconds=s)
    world.run(__import__("app.services.library", fromlist=["x"]).scan)
    aid = db.one("SELECT album_id FROM tracks LIMIT 1")["album_id"]

    # by audio: names are useless, fingerprints find the release
    fp = c.get(f"/api/library/albums/{aid}/match?provider=fingerprint&artist=&album=").json()
    assert fp["candidates"][0]["id"] == "r-album" and fp["candidates"][0]["provider"].startswith("Fingerprint · 3/3")

    # Deezer: tag + rename from Deezer's metadata (no MusicBrainz ids)
    dz = c.get(f"/api/library/albums/{aid}/match?provider=deezer&artist=Fake%20Band&album=Fake%20Album").json()
    assert dz["candidates"][0]["id"] == "deezer:123"
    pv = c.get(f"/api/library/albums/{aid}/preview?release_id=deezer:123").json()
    assert pv["release"]["provider"] == "Deezer" and [r["new_title"] for r in pv["rows"]] == ["Alpha", "Beta", "Gamma"]
    out = c.post(f"/api/library/albums/{aid}/retag", json={"release_id": "deezer:123", "rename": True}, headers=H).json()
    rows = db.query("SELECT title, album, albumartist, track, path, mb_albumid FROM tracks ORDER BY track")
    assert [(r["title"], r["album"], r["albumartist"], r["track"]) for r in rows] == [
        ("Alpha", "Fake Album (Deluxe Edition)", "Fake Band", 1), ("Beta", "Fake Album (Deluxe Edition)", "Fake Band", 2), ("Gamma", "Fake Album (Deluxe Edition)", "Fake Band", 3)]
    assert not rows[0]["mb_albumid"] and "Fake Band/Fake Album (Deluxe Edition)" in rows[0]["path"]
    assert mutagen.File(rows[0]["path"], easy=True)["isrc"] == ["QZAAA0000001"]

    # Apple Music
    aid = out["album_id"]
    ap = c.get(f"/api/library/albums/{aid}/preview?release_id=apple:601").json()
    assert ap["release"]["provider"] == "Apple Music" and ap["release"]["genre"] == "Electronic" and ap["confidence"] > 0.8
    assert c.get(f"/api/library/albums/{aid}/match?provider=discogs&artist=x&album=y").status_code == 400   # no token
    assert c.get(f"/api/library/albums/{aid}/match?provider=nope").status_code == 400


def test_scheduled_tasks(world):
    import time as _t

    from app import config, db
    from app.services import scheduler
    c = world.client
    tasks = {t["key"]: t for t in c.get("/api/admin/tasks").json()["tasks"]}
    assert {"scan", "sync", "follows", "lossy", "discover", "lyrics", "artwork", "retag", "verify", "cleanup"} <= set(tasks)
    assert tasks["scan"]["enabled"] is False and tasks["scan"]["locked"] is True       # ENCORE_SCAN_INTERVAL=0 in the environment wins
    assert tasks["sync"]["enabled"] is False and tasks["lyrics"]["enabled"] is True
    # switching on a task whose old setting used 0 = off restores a sensible interval
    assert c.patch("/api/admin/tasks/sync", json={"enabled": True}, headers=H).json()["minutes"] == 15
    assert c.patch("/api/admin/tasks/sync", json={"minutes": 180}, headers=H).json()["minutes"] == 180 and config.get("sync_interval_minutes") == 180
    assert c.patch("/api/admin/tasks/follows", json={"minutes": 360}, headers=H).json()["minutes"] == 360 and config.get("follow_check_hours") == 6
    assert c.patch("/api/admin/tasks/lyrics", json={"enabled": False}, headers=H).json()["enabled"] is False
    # run now: cleanup removes stale staging folders and old finished requests
    stale = os.path.join(config.get("staging_path"), "request-999")
    os.makedirs(stale)
    os.utime(stale, (_t.time() - 7200, _t.time() - 7200))
    db.execute("INSERT INTO requests (user_id, kind, artist, title, status, detail, created_at, updated_at) VALUES (1,'track','a','old','completed','{}',?,?)",
               (_t.time() - 40 * 86400, _t.time() - 40 * 86400))
    assert c.post("/api/admin/tasks/cleanup/run", headers=H).json()["ok"]
    for _ in range(50):
        st = {t["key"]: t for t in c.get("/api/admin/tasks").json()["tasks"]}["cleanup"]
        if not st["running"] and st["result"] != "running…":
            break
        _t.sleep(0.1)
    assert st["result"].startswith("1 leftover") and "1 old request" in st["result"] and not os.path.exists(stale)
    assert c.post("/api/admin/tasks/nope/run", headers=H).status_code == 404
    assert scheduler._first_delay("scan") == 30


def test_localized_names_per_user(world):
    from app import db
    from app.services import tagger
    c = world.client
    d = world.library / "ヨアソビ" / "夜に駆ける"
    for i, t in enumerate(["夜に駆ける", "群青"], 1):
        p = fakenet.make_audio(str(d / f"0{i}.flac"), t, artist="ヨアソビ", album="夜に駆ける", track=i, seconds=5 + i)
        tagger.write_tags(p, {"albumartist": "ヨアソビ", "musicbrainz_albumid": "r-jp", "musicbrainz_albumartistid": "a-jp", "musicbrainz_artistid": "a-jp"})
    fakenet.make_audio(str(world.library / "Москва" / "x" / "01.flac"), "Привет", artist="Москва", album="Город", track=1, seconds=9)
    world.run(__import__("app.services.library", fromlist=["x"]).scan)

    assert {t["title"] for t in c.get("/api/library/tracks").json()} == {"夜に駆ける", "群青", "Привет"}, "off by default"
    p = c.put("/api/auth/prefs", json={"localize": True, "lang": "en", "latin": True}, headers=H).json()
    assert p == {"localize": True, "lang": "en", "latin": True}
    from app.services import localize

    def settle():
        for _ in range(100):                # the prefs change started a lookup in the background
            if not localize.state["running"]:
                return
            __import__("time").sleep(0.05)
    settle()
    world.run(localize.localize_library, "en", True)
    tr = {t["title_orig"]: t for t in c.get("/api/library/tracks").json()}
    assert tr["夜に駆ける"]["title"] == "Racing into the Night" and tr["群青"]["title"] == "Gunjo"     # official English tracklist
    assert tr["夜に駆ける"]["artist"] == "YOASOBI"                                                    # MusicBrainz alias
    assert tr["Привет"]["title"] == "Privet" and tr["Привет"]["artist"] == "Moskva"                  # romanized
    albums = {a["album"] for a in c.get("/api/library/albums").json()["items"]}
    assert albums == {"Racing into the Night", "Gorod"}
    aid = db.one("SELECT album_id FROM tracks WHERE album='夜に駆ける'")["album_id"]
    assert c.get(f"/api/library/albums/{aid}").json()["album"] == "Racing into the Night"
    assert c.get(f"/api/library/albums/{aid}?raw=true").json()["album"] == "夜に駆ける", "the tag editor sees the real tags"

    # Japanese in Latin letters → the romaji tracklist
    c.put("/api/auth/prefs", json={"lang": "ja"}, headers=H)
    settle()
    world.run(localize.localize_library, "ja", True)
    assert {t["title"] for t in c.get("/api/library/tracks").json()} >= {"Yoru ni Kakeru", "Gunjou"}
    assert c.put("/api/auth/prefs", json={"lang": "xx"}, headers=H).status_code == 400
    c.put("/api/auth/prefs", json={"localize": False}, headers=H)
    assert "夜に駆ける" in {t["title"] for t in c.get("/api/library/tracks").json()}
    import mutagen
    assert mutagen.File(str(d / "01.flac"), easy=True)["title"] == ["夜に駆ける"], "files are never changed"


def test_hifi_api_source_and_picker(world):
    from app import config, db
    c = world.client
    config.set_many({"hifi_enabled": True, "hifi_api_url": "http://nonexistent-hifi.test, http://hifi.test", "source_order": "lidarr,streamrip,slskd,ytdlp"})
    assert c.post("/api/system/test/hifi", headers=H).json()["ok"], "second endpoint answers after the first fails"
    r = world.drive(world.req(kind="track", mbid="rec-delta", artist="Fake Band", title="Delta"))
    assert r["status"] == "completed" and "TIDAL" in r["message"] and r["source"] == "hifi", r
    r = world.drive(world.req(kind="album", mbid="rg-album", artist="Fake Band", title="Fake Album"))
    assert r["status"] == "completed" and "TIDAL" in r["message"], r
    assert {t["title"]: t["format"] for t in db.query("SELECT title, format FROM tracks")} == {"Alpha": "flac", "Beta": "flac", "Gamma": "flac", "Delta": "flac"}
    assert db.one("SELECT source FROM tracks WHERE title='Alpha'")["source"] == "TIDAL"
    res = c.post("/api/picker/search", json={"source": "hifi", "kind": "album", "artist": "Fake Band", "title": "Fake Album"}, headers=H).json()
    assert res[0]["payload"] == {"kind": "album", "id": 777, "title": "Fake Album"} and res[0]["lossless"]
    assert "hifi" in [s["id"] for s in c.get("/api/picker/sources").json()["sources"]]


def test_builtin_services_config_and_proxy(world, monkeypatch, tmp_path):
    import xml.etree.ElementTree as ET

    from app import config
    from app.services import embedded
    c = world.client
    monkeypatch.setenv("ENCORE_DOWNLOADS_PATH", str(tmp_path / "dl"))
    assert c.get("/api/admin/services").json()["enabled"] is False
    monkeypatch.setenv("ENCORE_EMBEDDED", "all")
    monkeypatch.setenv("ENCORE_EMBEDDED_ROOT", str(tmp_path / "none"))
    assert embedded.wanted() == ["qbittorrent", "sabnzbd", "slskd", "lidarr", "prowlarr"]

    # configs: localhost only, behind /svc/<name>, keys stable across restarts, Encore pointed at them
    config.set_many({"embedded_slsk_username": "me", "embedded_slsk_password": 'pa"ss'})
    k1 = embedded.prepare("slskd")["slskd"]
    assert embedded.prepare("slskd")["slskd"] == k1 and config.get("slskd_api_key") == k1
    assert config.get("slskd_url") == "http://127.0.0.1:5030/svc/slskd" and config.get("slskd_enabled")
    yml = open(os.path.join(embedded.data_dir("slskd"), "slskd.yml")).read()
    assert "ip_address: 127.0.0.1" in yml and 'password: "pa\\"ss"' in yml and "disabled: true" in yml
    lk = embedded.prepare("lidarr")["lidarr"]
    assert embedded.prepare("lidarr")["lidarr"] == lk and config.get("lidarr_url") == "http://127.0.0.1:8686/svc/lidarr"
    x = ET.parse(os.path.join(embedded.data_dir("lidarr"), "config.xml")).getroot()
    assert x.find("BindAddress").text == "127.0.0.1" and x.find("AuthenticationMethod").text == "External" and x.find("UrlBase").text == "/svc/lidarr"
    sk = embedded.prepare("sabnzbd")["sabnzbd"]
    assert embedded.prepare("sabnzbd")["sabnzbd"] == sk
    embedded.prepare("qbittorrent")
    conf = open(os.path.join(embedded.data_dir("qbittorrent"), "qBittorrent", "config", "qBittorrent.conf")).read()
    assert "WebUI\\Address=127.0.0.1" in conf and "WebUI\\LocalHostAuth=false" in conf
    world.run(embedded.start)
    st = {s["key"]: s for s in c.get("/api/admin/services").json()["services"]}
    assert st["lidarr"]["status"] == "missing" and "aio" in st["lidarr"]["error"]

    # proxy: admins only, same-origin writes only
    www = __import__("urllib.parse").parse.urlsplit(world.web)
    monkeypatch.setitem(embedded.SERVICES["lidarr"], "port", www.port)
    web_root = world.library.parent / "www" / "svc" / "lidarr"
    web_root.mkdir(parents=True, exist_ok=True)
    (web_root / "index.html").write_text("<title>Lidarr</title>")
    r = c.get("/svc/lidarr/index.html")
    assert r.status_code == 200 and "<title>Lidarr</title>" in r.text
    anon = __import__("fastapi.testclient", fromlist=["TestClient"]).TestClient(c.app)
    assert anon.get("/svc/lidarr/index.html", follow_redirects=False).status_code == 307
    assert c.post("/svc/lidarr/api/x", headers={"origin": "https://evil.example"}).status_code == 403
    assert c.get("/svc/nope/").status_code == 404
