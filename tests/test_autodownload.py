import asyncio
import time

from conftest import make_audio


def base(env, **cfg):
    from app import config, db
    db.connect()
    config.load()
    config.set_many(cfg)
    return db, config


def new_req(db, **kw):
    import json
    now = time.time()
    d = {"user_id": 1, "kind": "track", "mbid": None, "artist": "Band", "title": "Hit", "status": "queued", "detail": "{}"} | kw
    if not isinstance(d["detail"], str):
        d["detail"] = json.dumps(d["detail"])
    return db.execute("INSERT INTO requests (user_id, kind, mbid, artist, title, status, detail, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?)",
                      (d["user_id"], d["kind"], d["mbid"], d["artist"], d["title"], d["status"], d["detail"], now, now))


def test_song_upgrades_to_album(env, monkeypatch):
    from app import worker
    from app.services import musicbrainz as mb, notify
    db, config = base(env, album_upgrade="album_ep", slskd_enabled=False, ytdlp_enabled=False)

    async def resolve(*a, **k):
        return "rec-1"

    async def album(rid, allow_ep=True):
        return {"id": "rg-9", "title": "The Album", "type": "Album", "artist": "Band", "year": "2020", "cover": None}

    async def nothing(*a, **k):
        return None
    monkeypatch.setattr(mb, "resolve_recording", resolve)
    monkeypatch.setattr(mb, "album_for_recording", album)
    monkeypatch.setattr(notify, "send", nothing)
    monkeypatch.setattr(notify, "refresh_media_servers", nothing)
    t1 = new_req(db, title="Hit")
    t2 = new_req(db, title="Other Hit")
    asyncio.run(worker.step(t1))
    asyncio.run(worker.step(t2))
    r1, r2 = db.get_request(t1), db.get_request(t2)
    assert r1["status"] == "monitoring" and r1["source"] == "album"
    albums = db.query("SELECT * FROM requests WHERE kind='album'")
    assert len(albums) == 1 and albums[0]["mbid"] == "rg-9"          # two songs → one album request
    assert r2["detail"]["album_request"] == albums[0]["id"]
    # album finishes and contains the song → song request completes
    m = env / "music"
    make_audio(str(m / "Band/The Album/01 Hit.mp3"), title="Hit", artist="Band", album="The Album")
    from app.services import library
    asyncio.run(library.scan())
    db.update_request(albums[0]["id"], status="completed")
    asyncio.run(worker.step(t1))
    assert db.get_request(t1)["status"] == "completed"
    # album finished without "Other Hit" (different version) → falls back to the song on its own
    asyncio.run(worker.step(t2))
    r2 = db.get_request(t2)
    assert r2["status"] == "queued" and r2["detail"]["no_album"]


def test_quality_first_order_and_lossless_only(env, monkeypatch):
    from app import worker
    from app.services import streamrip
    db, config = base(env, album_upgrade="off", slskd_enabled=True, slskd_api_key="k", ytdlp_enabled=True,
                      quality_first=True, source_order="lidarr,slskd,ytdlp,streamrip")
    monkeypatch.setattr(streamrip, "enabled", lambda: True)
    seen = []

    async def fake_next(r, reason=""):
        seen.append(list(r["detail"]["sources"]))
    monkeypatch.setattr(worker, "next_source", fake_next)
    rid = new_req(db, title="Song")
    asyncio.run(worker.start(db.get_request(rid)))
    assert seen[-1] == ["streamrip", "slskd_lossless", "slskd", "ytdlp"]   # lidarr only does albums
    rid = new_req(db, kind="album", mbid="rg-x", title="LP", detail={"lossless_only": True})
    asyncio.run(worker.start(db.get_request(rid)))
    assert seen[-1] == ["streamrip", "slskd_lossless"]


def test_slskd_lossless_only_ranking(env):
    from app.services import slskd
    base(env)
    resp = [{"username": "mp3", "hasFreeUploadSlot": True, "queueLength": 0, "uploadSpeed": 1,
             "files": [{"filename": "Band\\LP\\01 - A.mp3", "size": 1, "bitRate": 320}]},
            {"username": "hires", "hasFreeUploadSlot": True, "queueLength": 0, "uploadSpeed": 1,
             "files": [{"filename": "Band\\LP [24-96]\\01 - A.flac", "size": 1, "bitDepth": 24}]},
            {"username": "cd", "hasFreeUploadSlot": True, "queueLength": 0, "uploadSpeed": 1,
             "files": [{"filename": "Band\\LP\\01 - A.flac", "size": 1, "bitDepth": 16}]}]
    c = slskd.rank_album(resp, "Band", "LP", 1, lossless_only=True)
    assert [x["username"] for x in c] == ["hires", "cd"]


def test_lossless_album_replaces_lossy_copies(env, monkeypatch):
    from app.services import library, playlists, tagger
    db, config = base(env, autotag_on_import=False)
    m = env / "music"
    old = make_audio(str(m / "Band/LP (yt)/01 - Song.mp3"), title="Song", artist="Band", album="LP")
    asyncio.run(library.scan())
    old_id = db.one("SELECT id FROM tracks")["id"]
    pl = playlists.create(1, "P", [{"title": "Song", "artist": "Band"}])
    assert playlists.items(pl["id"])[0]["track_id"] == old_id
    stage = env / "stage"
    new = make_audio(str(stage / "01 - Song.flac"), title="Song", artist="Band", album="LP")
    paths = asyncio.run(tagger.import_album_files([new], "Band", "LP", None, str(stage)))
    assert not __import__("os").path.exists(old) and paths[0].endswith(".flac")
    new_id = db.one("SELECT id FROM tracks WHERE format='flac'")["id"]
    assert playlists.items(pl["id"])[0]["track_id"] == new_id           # playlist follows the better file
    assert library.stats()["tracks"] == 1


def test_ytmusic_find_song_is_version_aware(monkeypatch):
    from app.services import ytmusic

    class Fake:
        def search(self, q, filter=None, scope=None, limit=20):
            return [
                {"videoId": "remix", "title": "Levels (Skrillex Remix)", "artists": [{"name": "Avicii"}], "duration_seconds": 200, "album": {"name": "x", "id": "a"}},
                {"videoId": "cover", "title": "Levels", "artists": [{"name": "Some Cover Band"}], "duration_seconds": 200},
                {"videoId": "orig", "title": "Levels", "artists": [{"name": "Avicii"}], "duration_seconds": 202, "album": {"name": "Levels", "id": "MPREb_1"}},
            ]
    monkeypatch.setattr(ytmusic, "anon", lambda: Fake())
    hit = asyncio.run(ytmusic.find_song("Avicii", "Levels", 200))
    assert hit["videoId"] == "orig" and hit["album_id"] == "MPREb_1"
    hit = asyncio.run(ytmusic.find_song("Avicii", "Levels (Skrillex Remix)", 200))
    assert hit["videoId"] == "remix"


def test_auto_download_liked_songs_respects_user_switch(env, monkeypatch):
    from app import config, db
    from app.services import connections, playlists, sync
    db.connect()
    config.load()
    now = time.time()
    db.execute("INSERT INTO users (id, username, password_hash, role, created_at) VALUES (7,'kim','x','user',?)", (now,))
    config.set_many({"auto_request_likes": True})

    class Fake(sync.Adapter):
        async def likes(self):
            return [{"title": "Rare Song", "artist": "Obscure Band", "album": None, "duration": None, "isrc": None, "mbid": None,
                     "source_id": "v1", "cover": None}]
    monkeypatch.setattr(sync, "ADAPTERS", {"ytmusic": Fake})
    connections.save(7, "ytmusic", "kim", {})
    db.execute("UPDATE users SET auto_download=0 WHERE id=7")
    asyncio.run(sync.sync_user(7))
    assert db.query("SELECT * FROM requests") == []            # switched off for this user
    db.execute("UPDATE users SET auto_download=1 WHERE id=7")
    asyncio.run(sync.sync_user(7))
    reqs = db.query("SELECT * FROM requests")
    assert len(reqs) == 1 and reqs[0]["status"] == "queued" and reqs[0]["title"] == "Rare Song"   # no approval needed
    assert playlists.items(playlists.get_liked(7)["id"])[0]["request_id"] == reqs[0]["id"]


def test_admin_autodownload_api(client):
    H = {"X-Requested-With": "encore"}
    client.post("/api/auth/login", json={"username": "admin", "password": "adminpass123"})
    d = client.get("/api/admin/autodownload").json()
    assert d["settings"]["album_upgrade"] == "album_ep" and d["settings"]["quality_first"] is True
    uid = d["users"][0]["id"]
    d = client.put("/api/admin/autodownload", json={"settings": {"album_upgrade": "off", "auto_request_likes": True, "bogus": 1},
                                                    "users": {str(uid): False}}, headers=H).json()
    assert d["settings"]["album_upgrade"] == "off" and d["settings"]["auto_request_likes"] is True
    assert d["users"][0]["auto_download"] == 0
