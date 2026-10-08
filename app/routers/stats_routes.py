"""Fun stats: the home-screen numbers and the Leaderboard page."""
import random
import time

from fastapi import APIRouter, Depends

from .. import auth, db

router = APIRouter(prefix="/api/stats", tags=["stats"])


def _ranking() -> list[dict]:
    return db.query("""SELECT u.id, u.username,
                              (SELECT COUNT(*) FROM requests r WHERE r.user_id=u.id AND r.parent_id IS NULL) requests,
                              (SELECT COUNT(*) FROM requests r WHERE r.user_id=u.id AND r.status='completed') completed,
                              (SELECT COUNT(*) FROM plays p WHERE p.user_id=u.id) plays,
                              (SELECT COALESCE(SUM(seconds),0) FROM plays p WHERE p.user_id=u.id) listened,
                              (SELECT COUNT(*) FROM playlist_items i JOIN playlists pl ON pl.id=i.playlist_id WHERE pl.user_id=u.id AND pl.kind='liked') likes,
                              (SELECT COUNT(*) FROM playlists pl WHERE pl.user_id=u.id AND pl.kind!='liked') playlists
                       FROM users u WHERE u.disabled=0""")


def _facts() -> list[dict]:
    out = []
    lib = db.one("SELECT COUNT(*) n, COALESCE(SUM(duration),0) d, COALESCE(SUM(size),0) s FROM tracks")
    if not lib["n"]:
        return out
    longest = db.one("SELECT title, artist, duration FROM tracks ORDER BY duration DESC LIMIT 1")
    shortest = db.one("SELECT title, artist, duration FROM tracks WHERE duration > 0 ORDER BY duration LIMIT 1")
    oldest = db.one("SELECT album, albumartist, year FROM tracks WHERE year GLOB '[12][0-9][0-9][0-9]*' ORDER BY year LIMIT 1")
    lossless = db.one("SELECT ROUND(100.0 * SUM(CASE WHEN format IN ('flac','wav','alac','aiff','ape','wv') THEN 1 ELSE 0 END) / COUNT(*)) p FROM tracks")["p"]
    decade = db.one("""SELECT substr(year,1,3) || '0s' dec, COUNT(*) n FROM tracks WHERE year GLOB '[12][0-9][0-9][0-9]*'
                       GROUP BY dec ORDER BY n DESC LIMIT 1""")
    words = db.one("""SELECT COUNT(*) n FROM tracks WHERE lower(title) LIKE '%love%'""")["n"]
    if longest:
        out.append({"label": f"longest song – {longest['artist']}: {longest['title']}", "value": _dur(longest["duration"])})
    if shortest:
        out.append({"label": f"shortest song – {shortest['artist']}: {shortest['title']}", "value": _dur(shortest["duration"])})
    if oldest:
        out.append({"label": f"oldest release – {oldest['albumartist']}: {oldest['album']}", "value": str(oldest["year"])[:4]})
    out.append({"label": "of the library is lossless", "value": f"{int(lossless or 0)}%"})
    if decade:
        out.append({"label": "is the library's favourite decade", "value": decade["dec"]})
    out.append({"label": "songs with “love” in the title", "value": str(words)})
    out.append({"label": "floppy disks to hold the library", "value": f"{lib['s'] / 1_474_560:,.0f}"})
    out.append({"label": "times through the library to listen to a year non-stop", "value": f"{31_536_000 / max(lib['d'], 1):.1f}"})
    return out


def _dur(s: float) -> str:
    s = int(s or 0)
    return f"{s // 3600}:{s % 3600 // 60:02d}:{s % 60:02d}" if s >= 3600 else f"{s // 60}:{s % 60:02d}"


@router.get("/me")
def me(user=Depends(auth.current_user)):
    lib = db.one("SELECT COUNT(*) tracks, COALESCE(SUM(duration),0) duration, COUNT(DISTINCT albumartist) artists FROM tracks")
    p = db.one("SELECT COUNT(*) plays, COALESCE(SUM(seconds),0) listened FROM plays WHERE user_id=?", (user["id"],))
    top = db.one("SELECT artist, COUNT(*) n FROM plays WHERE user_id=? GROUP BY lower(artist) ORDER BY n DESC LIMIT 1", (user["id"],))
    ranking = sorted(_ranking(), key=lambda r: (r["requests"] + r["plays"]), reverse=True)
    rank = next((i + 1 for i, r in enumerate(ranking) if r["id"] == user["id"]), None)
    reqs = db.one("SELECT COUNT(*) n FROM requests WHERE user_id=? AND parent_id IS NULL", (user["id"],))["n"]
    facts = _facts()
    return {"library": lib, "me": {**p, "requests": reqs, "rank": rank if len(ranking) > 1 else None, "top_artist": top["artist"] if top else None},
            "fun": random.choice(facts) if facts else None}


@router.get("/leaderboard")
def leaderboard(user=Depends(auth.current_user)):
    users = _ranking()
    boards = {}
    for key, label in (("requests", "Most requests"), ("plays", "Most songs played"), ("listened", "Most time listening"),
                       ("likes", "Most liked songs"), ("playlists", "Playlist builders")):
        boards[key] = {"label": label, "rows": [{"username": u["username"], "value": u[key], "me": u["id"] == user["id"]}
                                                 for u in sorted(users, key=lambda u: u[key], reverse=True) if u[key]][:10]}
    week = time.time() - 7 * 86400
    return {
        "boards": boards,
        "top_songs": db.query("""SELECT p.title, p.artist, MAX(t.album_id) album_id, COUNT(*) plays FROM plays p LEFT JOIN tracks t ON t.id=p.track_id
                                 GROUP BY lower(p.artist), lower(p.title) ORDER BY plays DESC LIMIT 10"""),
        "top_artists": db.query("SELECT artist, COUNT(*) plays, COUNT(DISTINCT user_id) listeners FROM plays GROUP BY lower(artist) ORDER BY plays DESC LIMIT 10"),
        "this_week": db.query("""SELECT u.username, COUNT(*) plays FROM plays p JOIN users u ON u.id=p.user_id WHERE p.played_at > ?
                                 GROUP BY p.user_id ORDER BY plays DESC LIMIT 5""", (week,)),
        "library_artists": db.query("""SELECT MAX(albumartist) artist, COUNT(DISTINCT album_id) albums, COUNT(*) songs FROM tracks
                                       WHERE albumartist NOT IN ('', 'Unknown Artist', 'Various Artists') GROUP BY lower(albumartist) ORDER BY songs DESC LIMIT 10"""),
        "facts": _facts(),
    }
