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
        if filter == "albums":
            return [{"browseId": "MPREb_fake", "title": "Fake Album", "artists": [{"name": "Fake Band"}]}] if "fake album" in ql else []
        out = [self._song(t, s) for t, _, s, _ in ALBUM_TRACKS + [fakenet.SINGLE] if t.lower() in ql]
        if "alpha" in ql:
            out.insert(0, self._song("Alpha (Fan Remix)", 5, "vid-fanremix"))  # must never be picked for "Alpha"
        return out

    def get_album(self, bid):
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
        if "music.youtube.com" not in url:
            return await real_download(url, outdir, progress, playlist)
        files = []
        if "OLAK5uy_fake" in url:
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
