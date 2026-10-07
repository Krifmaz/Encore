import asyncio
import json
import time

from conftest import make_audio


def setup_lib(env):
    from app import config, db
    from app.services import library
    db.connect()
    config.load()
    m = str(env / "music")
    make_audio(f"{m}/Avicii/True/01 Levels.mp3", title="Levels", artist="Avicii", album="True", track=1)
    make_audio(f"{m}/Avicii/Remixes/01 Levels (Skrillex Remix).mp3", title="Levels (Skrillex Remix)", artist="Avicii", album="Remixes", track=1)
    make_audio(f"{m}/Johnny Cash/Hurt.mp3", title="Hurt", artist="Johnny Cash", album="American IV", track=1)
    asyncio.run(library.scan())
    return db


def test_import_dedupes_but_keeps_versions(env):
    from app.services import playlists
    db = setup_lib(env)
    T = lambda t, a, **k: {"title": t, "artist": a, **k}  # noqa: E731
    res = playlists.create(1, "Mix", [
        T("Levels", "Avicii"), T("Levels - Radio Edit", "Avicii"), T("Levels (Skrillex Remix)", "Avicii"),
        T("LEVELS", "Avicii"),                      # duplicate of the first
        T("Hurt", "Nine Inch Nails"),               # original – NOT the Johnny Cash cover in the library
        T("Hurt", "Johnny Cash"),
        T("Levels", "Avicii", isrc="SE1234"),       # same song again
    ], "spotify")
    rows = playlists.items(res["id"])
    assert res["duplicates_skipped"] == 2
    titles = [(r["title"], r["artist"], bool(r["track_id"])) for r in rows]
    assert ("Levels", "Avicii", True) in titles
    assert ("Levels - Radio Edit", "Avicii", False) in titles           # edit isn't the album version
    assert ("Levels (Skrillex Remix)", "Avicii", True) in titles
    assert ("Hurt", "Nine Inch Nails", False) in titles                  # cover in library doesn't count
    assert ("Hurt", "Johnny Cash", True) in titles
    lev = db.one("SELECT id FROM tracks WHERE title='Levels'")["id"]
    remix = db.one("SELECT id FROM tracks WHERE title LIKE 'Levels (Sk%'")["id"]
    assert {r["track_id"] for r in rows if r["title"].startswith("Levels") and r["track_id"]} == {lev, remix}


def test_merge_and_request_missing(env):
    from app.services import playlists
    db = setup_lib(env)
    now = time.time()
    db.execute("INSERT INTO users (id, username, password_hash, role, created_at) VALUES (5,'u','x','admin',?)", (now,))
    a = playlists.create(5, "A", [{"title": "Levels", "artist": "Avicii"}, {"title": "Wake Me Up", "artist": "Avicii", "isrc": "X1"}])
    b = playlists.create(5, "B", [{"title": "Wake Me Up", "artist": "Avicii"}, {"title": "Hurt", "artist": "Johnny Cash"}])
    m = playlists.merge(5, [a["id"], b["id"]], "AB")
    assert m["total"] == 4 and m["added"] == 3 and m["duplicates_skipped"] == 1
    user = db.one("SELECT * FROM users WHERE id=5")
    assert playlists.request_missing(user, m["id"]) == 1
    req = db.query("SELECT * FROM requests")
    assert len(req) == 1 and req[0]["title"] == "Wake Me Up" and json.loads(req[0]["detail"]).get("isrc") == "X1"
    # second call doesn't request again
    assert playlists.request_missing(user, m["id"]) == 0


def test_liked_two_way_sync(env, monkeypatch):
    from app import db as _db
    from app.services import connections, playlists, sync
    db = setup_lib(env)
    lev = db.one("SELECT id FROM tracks WHERE title='Levels'")["id"]
    hurt = db.one("SELECT id FROM tracks WHERE title='Hurt'")["id"]
    remote = {"jellyfin": {lev}, "plex": set()}

    class Fake(sync.Adapter):
        can_push_likes = True

        async def likes(self):
            return [{"track_id": t} for t in remote[self.conn["service"]]]

        async def set_like(self, tid, liked):
            (remote[self.conn["service"]].add if liked else remote[self.conn["service"]].discard)(tid)
            return True

    monkeypatch.setattr(sync, "ADAPTERS", {"jellyfin": Fake, "plex": Fake})
    connections.save(1, "jellyfin", "me", {})
    connections.save(1, "plex", "me", {})

    asyncio.run(sync.sync_user(1))
    assert remote["plex"] == {lev}                      # liked in Finamp → loved in Plexamp
    liked = playlists.get_liked(1)
    assert {i["track_id"] for i in playlists.items(liked["id"])} == {lev}

    remote["plex"].add(hurt)                            # love in Plexamp
    asyncio.run(sync.sync_user(1))
    assert remote["jellyfin"] == {lev, hurt}

    remote["jellyfin"].discard(lev)                     # un-favourite in Finamp
    asyncio.run(sync.sync_user(1))
    assert remote["plex"] == {hurt} and remote["jellyfin"] == {hurt}

    _db.execute("DELETE FROM playlist_items WHERE playlist_id=? AND track_id=?", (liked["id"], hurt))  # unlike in Encore
    asyncio.run(sync.sync_user(1))
    assert remote["plex"] == set() and remote["jellyfin"] == set()
