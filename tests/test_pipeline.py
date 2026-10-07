import asyncio
import json
import os
import time

from conftest import make_audio


def test_album_request_via_slskd(env, monkeypatch):
    from app import config, db, worker
    from app.services import lidarr, musicbrainz as mb, notify, slskd, tagger
    db.connect()
    config.load()
    dl = env / "slskd"
    config.set_many({"slskd_enabled": True, "slskd_api_key": "k", "slskd_download_path": str(dl),
                     "ytdlp_enabled": False, "source_order": "lidarr,slskd,ytdlp"})
    titles = ["One", "Two"]
    remote = [f"Share\\Band - Record\\0{i} - {t}.flac" for i, t in enumerate(titles, 1)]

    async def search(q, timeout_s=None):
        return [{"username": "peer", "hasFreeUploadSlot": True, "queueLength": 0, "uploadSpeed": 1,
                 "files": [{"filename": f, "size": 10} for f in remote]}]

    enq = []

    async def enqueue(user, files):
        enq.append(user)
        for i, f in enumerate(files):  # "download" them
            make_audio(str(dl / "Band - Record" / f["filename"].split("\\")[-1]), title=titles[i].lower(), artist="band", album="record", track=i + 1)

    async def user_downloads(user):
        return [{"id": str(i), "filename": f, "state": "Completed, Succeeded", "size": 10, "bytesTransferred": 10} for i, f in enumerate(remote)]

    rel = {"id": "r1", "title": "Record", "artist": "Band", "artist_id": "a1", "date": "1999", "year": "1999", "country": "",
           "status": "Official", "release_group_id": "rg1", "type": "Album", "label": "", "catalog": "", "barcode": "",
           "disc_count": 1, "genre": "", "cover": None,
           "tracks": [{"disc": 1, "track": i, "title": t, "artist": "Band", "artist_id": "a1", "length": 2,
                       "recording_id": f"rec{i}", "track_id": f"t{i}", "isrc": None} for i, t in enumerate(titles, 1)]}

    async def best(rgid, n=None):
        return "r1"

    async def release(rid):
        return rel

    async def nothing(*a, **k):
        return None

    async def none_list(*a, **k):
        return []

    monkeypatch.setattr(slskd, "search", search)
    monkeypatch.setattr(slskd, "enqueue", enqueue)
    monkeypatch.setattr(slskd, "user_downloads", user_downloads)
    monkeypatch.setattr(mb, "best_release_for_group", best)
    monkeypatch.setattr(mb, "release", release)
    monkeypatch.setattr(tagger, "fetch_cover", nothing)
    monkeypatch.setattr(notify, "send", nothing)
    monkeypatch.setattr(notify, "refresh_media_servers", none_list)
    assert not lidarr.enabled()

    now = time.time()
    rid = db.execute("INSERT INTO requests (user_id, kind, mbid, artist, title, album, status, detail, created_at, updated_at) VALUES (1,'album','rg1','Band','Record','Record','queued','{}',?,?)", (now, now))

    async def run():
        for _ in range(4):
            await worker.step(rid)
            if db.get_request(rid)["status"] == "completed":
                break
    asyncio.run(run())
    r = db.get_request(rid)
    assert r["status"] == "completed", (r["status"], r["message"], r["detail"])
    assert enq == ["peer"]
    music = env / "music"
    assert os.path.isfile(music / "Band" / "Record (1999)" / "01 - One.flac")
    t = db.one("SELECT * FROM tracks WHERE title='Two'")
    assert t["mb_releasegroupid"] == "rg1"
    # downloaded folder cleaned up
    assert not (dl / "Band - Record").exists()
    # a second request for the same album is detected as already available
    rid2 = db.execute("INSERT INTO requests (user_id, kind, mbid, artist, title, album, status, detail, created_at, updated_at) VALUES (1,'album','rg1','Band','Record','Record','queued','{}',?,?)", (now, now))
    asyncio.run(worker.step(rid2))
    assert db.get_request(rid2)["message"] == "Already in library"


def test_failed_source_falls_through(env, monkeypatch):
    from app import config, db, worker
    from app.services import notify, slskd
    db.connect()
    config.load()
    config.set_many({"slskd_enabled": True, "slskd_api_key": "k", "ytdlp_enabled": False, "album_upgrade": "off"})

    async def search(q, timeout_s=None):
        return []

    async def nothing(*a, **k):
        return None
    monkeypatch.setattr(slskd, "search", search)
    monkeypatch.setattr(notify, "send", nothing)
    now = time.time()
    rid = db.execute("INSERT INTO requests (user_id, kind, mbid, artist, title, status, detail, created_at, updated_at) VALUES (1,'track',NULL,'X','Y','queued',?,?,?)", (json.dumps({}), now, now))
    asyncio.run(worker.step(rid))
    r = db.get_request(rid)
    assert r["status"] == "failed" and "no suitable results" in r["message"]
