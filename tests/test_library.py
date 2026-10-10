import asyncio
import os

from conftest import make_audio


def test_scan_and_tag_roundtrip(env, monkeypatch):
    from app import config, db
    from app.services import library, tagger
    db.connect()
    config.load()
    music = str(env / "music")
    for i, t in enumerate(["Intro", "Second Song", "Third"], 1):
        make_audio(f"{music}/messy folder/{i:02d} {t}.mp3", title=t.lower(), artist="Band", album="LP", track=i)
    make_audio(f"{music}/flac/one.flac", title="Flac Song", artist="Other", album="F", track=1)
    asyncio.run(library.scan())
    s = library.stats()
    assert s["tracks"] == 4 and s["albums"] == 2
    assert library.has_album("band", "lp")

    rel = {
        "id": "rel-1", "title": "LP", "artist": "The Band", "artist_id": "art-1", "date": "2001-05-01", "year": "2001",
        "country": "XW", "status": "Official", "release_group_id": "rg-1", "type": "Album", "label": "Lbl", "catalog": "",
        "barcode": "", "disc_count": 1, "genre": "Rock", "cover": None,
        "tracks": [
            {"disc": 1, "track": 1, "title": "Intro", "artist": "The Band", "artist_id": "art-1", "length": 2, "recording_id": "rec-1", "track_id": "t1", "isrc": None},
            {"disc": 1, "track": 2, "title": "Second Song", "artist": "The Band", "artist_id": "art-1", "length": 2, "recording_id": "rec-2", "track_id": "t2", "isrc": None},
            {"disc": 1, "track": 3, "title": "Third", "artist": "The Band", "artist_id": "art-1", "length": 2, "recording_id": "rec-3", "track_id": "t3", "isrc": "USABC0000001"},
        ],
    }

    async def fake_release(rid):
        return rel

    async def no_cover(*a):
        return None

    monkeypatch.setattr(tagger.mb, "release", fake_release)
    monkeypatch.setattr(tagger, "fetch_cover", no_cover)
    album_id = db.one("SELECT album_id FROM tracks WHERE album='LP'")["album_id"]
    pv = asyncio.run(tagger.preview([t["path"] for t in db.query("SELECT path FROM tracks WHERE album_id=?", (album_id,))], "rel-1"))
    assert pv["confidence"] > 0.85
    new = asyncio.run(tagger.retag_album(album_id, "rel-1"))
    assert sorted(os.path.relpath(p, music) for p in new) == [
        "The Band/LP (2001)/01 - Intro.mp3", "The Band/LP (2001)/02 - Second Song.mp3", "The Band/LP (2001)/03 - Third.mp3"]
    assert not os.path.exists(f"{music}/messy folder")
    t = db.one("SELECT * FROM tracks WHERE title='Third'")
    assert t["mb_albumid"] == "rel-1" and t["mb_releasegroupid"] == "rg-1" and t["albumartist"] == "The Band"
    assert library.has_release_group("rg-1")


def test_render_path_multi_disc(env):
    from app import config, db
    from app.services import tagger
    db.connect()
    config.load()
    p = tagger.render_path({"albumartist": "AC/DC", "album": "Live?", "year": "1992", "track": 3, "disc": 2, "disccount": 2, "title": "T.N.T."}, ".flac")
    assert p.endswith("AC_DC/Live_ (1992)/2-03 - T.N.T.flac")


def test_slskd_ranking(env):
    from app import config, db
    from app.services import slskd
    db.connect()
    config.load()
    responses = [
        {"username": "good", "hasFreeUploadSlot": True, "queueLength": 0, "uploadSpeed": 5_000_000, "files": [
            {"filename": f"Music\\Band\\Band - LP (2001) [FLAC]\\0{i} - Song {i}.flac", "size": 100, "bitRate": None} for i in range(1, 4)]},
        {"username": "mp3", "hasFreeUploadSlot": True, "queueLength": 0, "uploadSpeed": 1_000_000, "files": [
            {"filename": f"Band\\LP\\0{i}.mp3", "size": 100, "bitRate": 320} for i in range(1, 4)]},
        {"username": "wrongcount", "hasFreeUploadSlot": True, "queueLength": 0, "uploadSpeed": 9_000_000, "files": [
            {"filename": "Band\\LP\\01.flac", "size": 100}]},
        {"username": "lowbr", "hasFreeUploadSlot": True, "queueLength": 0, "uploadSpeed": 1, "files": [
            {"filename": f"Band\\LP\\0{i}.mp3", "size": 100, "bitRate": 128} for i in range(1, 4)]},
    ]
    c = slskd.rank_album(responses, "Band", "LP", 3)
    assert [x["username"] for x in c] == ["good", "mp3"]
    t = slskd.rank_track(responses, "Band", "Song 2", None)
    assert t[0]["files"][0]["filename"].endswith("02 - Song 2.flac")


def test_map_files_by_title_without_numbers():
    from app.services.tagger import map_files
    files = [{"title": "Beta", "track": None, "duration": 200}, {"title": "Alpha", "track": None, "duration": 100}]
    tracks = [{"disc": 1, "track": 1, "title": "Alpha", "length": 100}, {"disc": 1, "track": 2, "title": "Beta", "length": 200}]
    pairs, conf = map_files(files, tracks)
    assert pairs[0][1]["title"] == "Beta" and pairs[1][1]["title"] == "Alpha"
    assert conf > 0.8


def test_organize_moves_files_and_fills_tags(env, monkeypatch):
    from app import config, db
    from app.services import library, organize
    db.connect()
    config.load()
    music = str(env / "music")
    make_audio(f"{music}/dump/01 - Opening.mp3", title="Opening", artist="Band", album="Record", track=1)
    make_audio(f"{music}/dump/Loner - Lost Song.mp3", title="", artist="", album="", track=0)
    asyncio.run(library.scan())

    async def fake_deezer(artist, title):
        return {"album": "Found Album", "year": "2020", "track": 4} if title == "Lost Song" else {}
    monkeypatch.setattr(organize, "_deezer", fake_deezer)
    msg = asyncio.run(organize.organize_library())
    assert "2 moved" in msg
    paths = sorted(r["path"][len(music):] for r in db.query("SELECT path FROM tracks"))
    assert paths[0].startswith("/Band/Record") and paths[0].endswith("01 - Opening.mp3")
    assert paths[1].startswith("/Loner/Found Album (2020)") and paths[1].endswith("04 - Lost Song.mp3")
    assert not os.path.exists(f"{music}/dump")
    assert db.one("SELECT title FROM tracks WHERE path LIKE '%Lost Song%'")["title"] == "Lost Song"


def test_sentence_search_parsing():
    from app.services.nlsearch import parse
    p = parse("the song blinding lights by the weeknd")
    assert (p["title"], p["artist"], p["kind"]) == ("blinding lights", "the weeknd", "track")
    assert parse("that song that goes is this the real life")["lyrics"] == "is this the real life"
    assert parse("play some music by daft punk")["q"] == "daft punk"
    assert not parse("Blinding Lights")["sentence"]


def test_share_link_plays_only_that_song(client, env):
    import asyncio as aio

    from app import db
    from app.services import library
    music = str(env / "music")
    make_audio(f"{music}/A/B/01 - One.mp3", title="One", artist="A", album="B", track=1)
    make_audio(f"{music}/A/B/02 - Two.mp3", title="Two", artist="A", album="B", track=2)
    aio.run(library.scan())
    one = db.one("SELECT id FROM tracks WHERE title='One'")["id"]
    H = {"X-Requested-With": "encore"}
    client.post("/api/auth/login", json={"username": "admin", "password": "adminpass123"})
    tok = client.post(f"/api/library/tracks/{one}/share", headers=H).json()["token"]
    client.cookies.clear()
    assert client.get(f"/api/share/{tok}").json()["title"] == "One"
    assert client.get(f"/api/share/{tok}/stream").status_code == 200
    assert client.get(f"/s/{tok}").status_code == 200
    assert client.get("/api/share/nope").status_code == 404
    # the link opens nothing else
    assert client.get(f"/api/library/tracks/{one}/stream").status_code == 401
    assert client.get("/api/library/tracks?limit=5").status_code == 401


def test_fix_song_splits_version_and_remove_artist(client, env):
    import asyncio as aio

    from app import db
    from app.services import library
    music = str(env / "music")
    make_audio(f"{music}/Picco/BE (2026)/01 - BE a mASTer.mp3", title="BE a mASTer", artist="Picco", album="BE", track=1)
    make_audio(f"{music}/Picco/BE (2026)/01 - BE a mASTer (2).mp3", title="BE a mASTer", artist="Picco", album="BE", track=1)
    make_audio(f"{music}/Wrong Guy/X/01 - Oops.mp3", title="Oops", artist="Wrong Guy", album="X", track=1)
    aio.run(library.scan())
    H = {"X-Requested-With": "encore"}
    client.post("/api/auth/login", json={"username": "admin", "password": "adminpass123"})
    tid = db.one("SELECT id FROM tracks WHERE path LIKE '%(2).mp3'")["id"]
    r = client.post(f"/api/library/tracks/{tid}/fix", json={"values": {"title": "BE a mASTer (Sped Up)", "album": "BE a mASTer (Sped Up)", "albumartist": "Picco"}}, headers=H)
    assert r.status_code == 200, r.text
    p = db.one("SELECT path, title FROM tracks WHERE id=?", (tid,))
    assert p["title"] == "BE a mASTer (Sped Up)" and "BE a mASTer (Sped Up)" in p["path"] and os.path.isfile(p["path"])
    assert db.one("SELECT COUNT(*) n FROM tracks WHERE title='BE a mASTer'")["n"] == 1
    # an artist added by mistake goes away completely
    r = client.delete("/api/library/artists?name=Wrong%20Guy", headers=H).json()
    assert r["removed"] == 1 and not os.path.exists(f"{music}/Wrong Guy")


def test_fingerprint_similarity():
    from app.services.acoustid import similarity
    import random
    rnd = random.Random(1)
    a = [rnd.getrandbits(32) for _ in range(300)]
    assert similarity(a, a) == 1.0
    assert similarity(a, a[3:]) == 1.0                       # offset alignment
    assert similarity(a, [rnd.getrandbits(32) for _ in range(300)]) < 0.6
