"""Search that understands sentences: "the song blinding lights by the weeknd", "radiohead album with creep",
"that song that goes is this the real life". Turns them into title/artist/kind, and falls back to Deezer
(good at fuzzy text) and LRCLIB (lyrics) to find what MusicBrainz can't from loose wording."""
import re

from .http import client

KIND_WORDS = {"song": "track", "songs": "track", "track": "track", "tracks": "track", "single": "track",
              "album": "album", "albums": "album", "ep": "album", "record": "album", "lp": "album",
              "artist": "artist", "band": "artist", "singer": "artist", "rapper": "artist", "group": "artist"}
FILLER = r"\b(play|find|search( for)?|look( for| up)?|get|download|i('m| am)? looking for|i want( to hear)?|can you|could you|please|" \
         r"the|a|an|that|which|called|named|titled|some|me|music|from|of)\b"
LYRIC_HINT = re.compile(r"\b(that goes|goes like|lyrics?|which goes|where (he|she|they) (says?|sings?))\b", re.I)


def parse(q: str) -> dict:
    """{'q': cleaned text, 'title', 'artist', 'kind', 'lyrics'} – any can be empty."""
    raw = q.strip()
    quoted = re.findall(r'"([^"]+)"', raw)
    text = raw.lower()
    lyrics = None
    m = LYRIC_HINT.search(text)
    if m:
        lyrics = raw[m.end():].strip(" ,.:\"'") or None
        text = text[:m.start()]
    kind = None
    for w, k in KIND_WORDS.items():
        if re.search(rf"\b{w}\b", text):
            kind = kind or k
            text = re.sub(rf"\b{w}\b", " ", text)
    title = artist = ""
    if " by " in f" {text} ":
        left, _, right = f" {text} ".partition(" by ")
        title, artist = left, right
    title = re.sub(r"\s+", " ", re.sub(FILLER, " ", title, flags=re.I)).strip(" ,.?!")
    artist = re.sub(r"\s+", " ", re.sub(r"\b(the artist|the band)\b", " ", artist)).strip(" ,.?!")
    clean = re.sub(r"\s+", " ", re.sub(FILLER, " ", text.replace('"', " "), flags=re.I)).strip(" ,.?!")
    if quoted:
        title = quoted[0]
    if artist:
        clean = f"{title} {artist}".strip()
    if lyrics and not clean:
        clean = lyrics
    return {"q": clean or raw, "title": title, "artist": artist, "kind": kind, "lyrics": lyrics,
            "sentence": len(raw.split()) >= 4 or bool(kind or artist or lyrics)}


async def deezer_tracks(q: str, limit: int = 6) -> list[tuple[str, str]]:
    """(title, artist) guesses from Deezer's forgiving full-text search."""
    try:
        r = await client().get("https://api.deezer.com/search/track", params={"q": q, "limit": limit})
        return [(t["title"], t["artist"]["name"]) for t in r.json().get("data", []) if t.get("title")]
    except Exception:  # noqa: BLE001
        return []


async def lyric_tracks(words: str, limit: int = 5) -> list[tuple[str, str]]:
    """(title, artist) of songs whose lyrics contain these words (LRCLIB)."""
    try:
        r = await client().get("https://lrclib.net/api/search", params={"q": words})
        seen, out = set(), []
        for t in r.json()[:20]:
            key = (t.get("trackName", "").lower(), t.get("artistName", "").lower())
            if key[0] and key not in seen:
                seen.add(key)
                out.append((t["trackName"], t["artistName"]))
        return out[:limit]
    except Exception:  # noqa: BLE001
        return []
