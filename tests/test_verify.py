import asyncio
import os

import pytest
from conftest import make_audio


def lib(env):
    from app import config, db
    db.connect()
    config.load()
    return db, config


def test_duplicate_finder_is_version_aware(env):
    from app.services import library, verify
    db, _ = lib(env)
    m = str(env / "music")
    make_audio(f"{m}/A/Album/01 Song.mp3", title="Song", artist="A", album="Album", track=1, seconds=3)
    make_audio(f"{m}/A/Singles/A - Song.flac", title="Song", artist="A", album="Song", track=1, seconds=3)
    make_audio(f"{m}/A/Singles/A - Song (Club Remix).mp3", title="Song (Club Remix)", artist="A", album="Song", track=1, seconds=3)
    make_audio(f"{m}/B/Covers/Song.mp3", title="Song", artist="B", album="Covers", track=1, seconds=3)
    asyncio.run(library.scan())
    groups = verify.find_duplicates()
    assert len(groups) == 1
    g = groups[0]
    assert sorted(t["relpath"] for t in g["tracks"]) == ["A/Album/01 Song.mp3", "A/Singles/A - Song.flac"]
    keep = next(t for t in g["tracks"] if t["id"] == g["keep"])
    assert keep["format"] == "flac"                                  # lossless copy wins
    verify.delete_tracks([t["id"] for t in g["tracks"] if t["id"] != g["keep"]], g["keep"])
    assert not os.path.exists(f"{m}/A/Album/01 Song.mp3") and verify.find_duplicates() == []


def _fake_identify(title, artist, rec_id="r-1", length=3):
    async def ident(path):
        return {"duration": length, "results": [{"acoustid": "ac-1", "score": 0.95, "recordings": [
            {"id": rec_id, "title": title, "artist": artist, "length": length, "release_groups": []}]}]}
    return ident


def test_check_file_detects_wrong_version(env, monkeypatch):
    from app.services import acoustid, verify
    lib(env)
    monkeypatch.setattr(acoustid, "identify", _fake_identify("Levels (Skrillex Remix)", "Avicii"))
    res = asyncio.run(verify.check_file("x.mp3", "Levels", "Avicii", 3))
    assert res["status"] == "mismatch" and "remix" in res["detail"]
    res = asyncio.run(verify.check_file("x.mp3", "Levels (Skrillex Remix)", "Avicii", 3))
    assert res["status"] == "verified"
    res = asyncio.run(verify.check_file("x.mp3", "Whatever", "Avicii", 3, recording_id="r-1"))
    assert res["status"] == "verified"


def test_import_single_rejects_and_dedupes(env, monkeypatch):
    from app.services import acoustid, library, musicbrainz, tagger
    db, config = lib(env)
    config.set_many({"acoustid_api_key": "k"})
    monkeypatch.setattr(acoustid, "available", lambda: True)

    async def no_rec(*a, **k):
        return []
    monkeypatch.setattr(musicbrainz, "search_recordings", no_rec)
    stage = env / "staging"
    # wrong audio → rejected and deleted
    monkeypatch.setattr(acoustid, "identify", _fake_identify("Totally Different", "Someone"))
    bad = make_audio(str(stage / "bad.mp3"), title="Levels", artist="Avicii")
    with pytest.raises(tagger.VerificationFailed):
        asyncio.run(tagger.import_single(bad, "Avicii", "Levels", None, str(stage)))
    assert not os.path.exists(bad)
    # right audio → imported and marked verified
    monkeypatch.setattr(acoustid, "identify", _fake_identify("Levels", "Avicii"))
    good = make_audio(str(stage / "good.mp3"), title="Levels", artist="Avicii")
    p, note = asyncio.run(tagger.import_single(good, "Avicii", "Levels", None, str(stage)))
    assert os.path.exists(p) and db.one("SELECT verify_status FROM tracks WHERE path=?", (p,))["verify_status"] == "verified"
    # same song again (same quality) → not imported twice
    again = make_audio(str(stage / "again.mp3"), title="Levels", artist="Avicii")
    p2, note2 = asyncio.run(tagger.import_single(again, "Avicii", "Levels", None, str(stage)))
    assert p2 == p and "already in library" in note2 and not os.path.exists(again)
    # lossless copy → replaces the mp3
    flac = make_audio(str(stage / "hq.flac"), title="Levels", artist="Avicii")
    p3, note3 = asyncio.run(tagger.import_single(flac, "Avicii", "Levels", None, str(stage)))
    assert p3.endswith(".flac") and "replaced" in note3 and not os.path.exists(p)
    assert library.stats()["tracks"] == 1
