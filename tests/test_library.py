import asyncio
import os

from conftest import make_audio


def test_scan_and_tag_roundtrip(env):
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

    tagger.mb.release = fake_release
    tagger.fetch_cover = no_cover
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
