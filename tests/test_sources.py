import asyncio
import json


def test_parse_text():
    from app.services.sources import parse_text
    d = parse_text("1. Daft Punk - One More Time\nAround the World by Daft Punk\nAvicii,Levels\n# comment\n")
    assert [(t["artist"], t["title"]) for t in d["tracks"]] == [("Daft Punk", "One More Time"), ("Daft Punk", "Around the World"), ("Avicii", "Levels")]


def test_jsonld_playlist(monkeypatch):
    from app.services import sources
    ld = {"@context": "https://schema.org", "@type": "MusicPlaylist", "name": "Obscure gems", "track": [
        {"@type": "MusicRecording", "name": "Song A", "byArtist": {"name": "Artist A"}, "duration": "PT3M21S", "isrcCode": "GBAAA0000001"},
        {"@type": "MusicRecording", "name": "Song B", "byArtist": [{"name": "X"}, {"name": "Y"}]}]}
    html = f'<html><script type="application/ld+json">{json.dumps(ld)}</script></html>'

    class R:
        status_code = 200
        text = html

    class C:
        async def get(self, *a, **k):
            return R()
    monkeypatch.setattr(sources, "client", lambda: C())
    d = asyncio.run(sources.web_jsonld("https://www.qobuz.com/us-en/playlists/x/123"))
    assert d["name"] == "Obscure gems"
    assert d["tracks"][0]["duration"] == 201 and d["tracks"][0]["isrc"] == "GBAAA0000001"
    assert d["tracks"][1]["artist"] == "X, Y"


def test_spotify_embed_fallback(monkeypatch):
    from app.services import sources
    data = {"props": {"pageProps": {"state": {"data": {"entity": {"name": "My List", "coverArt": {"sources": [{"url": "c.jpg"}]},
            "trackList": [{"title": "Song", "subtitle": "Artist, Other", "duration": 180000, "uri": "spotify:track:abc"}]}}}}}}
    html = f'<script id="__NEXT_DATA__" type="application/json">{json.dumps(data)}</script>'

    class R:
        status_code = 200
        text = html

    class C:
        async def get(self, *a, **k):
            return R()
    monkeypatch.setattr(sources, "client", lambda: C())
    d = asyncio.run(sources.spotify_embed("playlist", "xyz"))
    assert d["name"] == "My List" and d["tracks"][0] == {"title": "Song", "artist": "Artist, Other", "album": None, "duration": 180.0,
                                                          "isrc": None, "mbid": None, "source_id": "abc", "cover": None}


def test_detect():
    from app.services.sources import detect
    assert detect("https://open.spotify.com/playlist/37i9") == "spotify"
    assert detect("https://www.deezer.com/en/playlist/123") == "deezer"
    assert detect("https://tidal.com/browse/playlist/abc-def") == "tidal"
    assert detect("https://music.youtube.com/playlist?list=PL") == "ytmusic"
    assert detect("https://artist.bandcamp.com/album/x") == "ytdlp"
    assert detect("https://music.apple.com/us/playlist/x/pl.123") == "apple"
