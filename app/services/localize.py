"""Show song, album and artist names in your language (per user, off by default).

Sources, best first: MusicBrainz artist aliases for your locale, MusicBrainz pseudo-releases
(the official translated or transliterated tracklists, e.g. Japanese → English or romaji),
and finally automatic romanization for names in a non-Latin script. Files are never changed.
"""
import asyncio
import json
import re
import unicodedata

from .. import db
from . import musicbrainz as mb

LANGS = {"en": "English", "es": "Español", "fr": "Français", "de": "Deutsch", "it": "Italiano", "pt": "Português", "nl": "Nederlands",
         "pl": "Polski", "sv": "Svenska", "tr": "Türkçe", "ru": "Русский", "uk": "Українська", "ja": "日本語", "ko": "한국어",
         "zh": "中文", "ar": "العربية", "hi": "हिन्दी", "id": "Bahasa Indonesia", "vi": "Tiếng Việt", "th": "ไทย"}
ISO3 = {"en": "eng", "es": "spa", "fr": "fra", "de": "deu", "it": "ita", "pt": "por", "nl": "nld", "pl": "pol", "sv": "swe", "tr": "tur",
        "ru": "rus", "uk": "ukr", "ja": "jpn", "ko": "kor", "zh": "zho", "ar": "ara", "hi": "hin", "id": "ind", "vi": "vie", "th": "tha"}
LATIN_LANGS = {"en", "es", "fr", "de", "it", "pt", "nl", "pl", "sv", "tr", "id", "vi"}
state = {"running": False, "lang": None, "done": 0, "total": 0}


def prefs(user: dict) -> dict:
    try:
        p = json.loads(user.get("prefs") or "{}")
    except ValueError:
        p = {}
    return {"localize": bool(p.get("localize")), "lang": p.get("lang") or "en", "latin": p.get("latin", True)}


def key(p: dict) -> str:
    return p["lang"] + ("-latn" if p["latin"] else "")


def non_latin(s: str) -> bool:
    for ch in s or "":
        if ch.isalpha():
            name = unicodedata.name(ch, "")
            if name and not name.startswith("LATIN"):
                return True
    return False


def romanize(s: str) -> str:
    from anyascii import anyascii
    out = anyascii(s)
    return re.sub(r"\s{2,}", " ", out).strip()


def apply(rows: list[dict], user: dict, fields=("title", "artist", "album", "albumartist")) -> list[dict]:
    """Swap in localized names (originals kept as <field>_orig)."""
    p = prefs(user)
    if not p["localize"] or not rows:
        return rows
    lang = key(p)
    kind_of = {"title": "title", "artist": "artist", "albumartist": "artist", "album": "album", "name": "artist"}
    wanted: dict[str, set] = {}
    for r in rows:
        for f in fields:
            v = r.get(f)
            if v:
                wanted.setdefault(kind_of.get(f, f), set()).add(v)
    found: dict[tuple, str] = {}
    for kind, vals in wanted.items():
        vals = list(vals)
        for i in range(0, len(vals), 400):
            chunk = vals[i:i + 400]
            for row in db.query(f"SELECT original, value FROM loc WHERE kind=? AND lang=? AND original IN ({','.join('?' * len(chunk))})", (kind, lang, *chunk)):
                found[(kind, row["original"])] = row["value"]
    for r in rows:
        for f in fields:
            v = r.get(f)
            if not v:
                continue
            new = found.get((kind_of.get(f, f), v))
            if new is None and p["latin"] and non_latin(v):
                new = romanize(v)        # not looked up yet: romanize on the fly
            if new and new != v:
                r[f + "_orig"] = v
                r[f] = new
    return rows


def _save(kind: str, original: str, lang: str, value: str, source: str) -> None:
    if value and value != original:
        db.execute("INSERT OR REPLACE INTO loc (kind, original, lang, value, source) VALUES (?,?,?,?,?)", (kind, original, lang, value, source))


async def _artist(mbid: str, name: str, lang: str, latin: bool, k: str) -> None:
    try:
        a = await mb._get(f"artist/{mbid}", {"inc": "aliases"})
    except Exception:  # noqa: BLE001
        return
    aliases = a.get("aliases") or []
    loc = [x for x in aliases if (x.get("locale") or "").split("_")[0] == lang]
    loc.sort(key=lambda x: (not x.get("primary"), x.get("type") != "Artist name"))
    pick = next((x["name"] for x in loc if not latin or not non_latin(x["name"])), None)
    if not pick and latin:
        sort = a.get("sort-name") or ""
        if sort and not non_latin(sort) and "," in sort and non_latin(name):
            last, _, first = sort.partition(",")
            pick = f"{first.strip()} {last.strip()}".strip()
        elif sort and not non_latin(sort) and non_latin(name):
            pick = sort
    if pick:
        _save("artist", name, k, pick, "MusicBrainz alias")


async def _release(rel_id: str, lang: str, latin: bool, k: str) -> int:
    """Use the release's translated/transliterated pseudo-release tracklist."""
    try:
        r = await mb._get(f"release/{rel_id}", {"inc": "release-rels+recordings"})
    except Exception:  # noqa: BLE001
        return 0
    best = None
    for rel in r.get("relations") or []:
        if rel.get("type") != "transl-tracklisting" or not rel.get("release"):
            continue
        tr = rel["release"].get("text-representation") or {}
        script, language = tr.get("script"), tr.get("language")
        score = (2 if language == ISO3.get(lang) else 0) + (1 if latin and script == "Latn" else 0)
        if score and (not best or score > best[0]):
            best = (score, rel["release"]["id"])
    if not best:
        return 0
    try:
        p = await mb._get(f"release/{best[1]}", {"inc": "recordings"})
    except Exception:  # noqa: BLE001
        return 0
    _save("album", r.get("title", ""), k, p.get("title", ""), "MusicBrainz pseudo-release")
    n = 0
    for m1, m2 in zip(r.get("media") or [], p.get("media") or []):
        for t1, t2 in zip(m1.get("tracks") or [], m2.get("tracks") or []):
            _save("title", t1.get("title", ""), k, t2.get("title", ""), "MusicBrainz pseudo-release")
            n += 1
    return n


async def localize_library(lang: str, latin: bool) -> dict:
    """Look up localized names for everything in the library that isn't already in the target script/language."""
    if state["running"]:
        return state
    k = lang + ("-latn" if latin else "")
    state.update(running=True, lang=k, done=0, total=0)
    try:
        artists = db.query("""SELECT COALESCE(MAX(mb_albumartistid), MAX(mb_artistid)) mbid, albumartist name FROM tracks
                              GROUP BY albumartist""")
        albums = db.query("SELECT MAX(mb_albumid) rel, album FROM tracks WHERE mb_albumid IS NOT NULL AND mb_albumid != '' GROUP BY mb_albumid")
        need = lambda s: s and (non_latin(s) or lang not in LATIN_LANGS)  # noqa: E731
        artists = [a for a in artists if a["mbid"] and need(a["name"]) and not db.one("SELECT 1 FROM loc WHERE kind='artist' AND original=? AND lang=?", (a["name"], k))]
        albums = [a for a in albums if need(a["album"]) and not db.one("SELECT 1 FROM loc WHERE kind='album' AND original=? AND lang=?", (a["album"], k))]
        state["total"] = len(artists) + len(albums)
        for a in artists:
            await _artist(a["mbid"], a["name"], lang, latin, k)
            state["done"] += 1
        for a in albums:
            await _release(a["rel"], lang, latin, k)
            state["done"] += 1
        if latin:   # whatever MusicBrainz doesn't have: automatic romanization
            for kind, col in (("title", "title"), ("album", "album"), ("artist", "artist"), ("artist", "albumartist")):
                for row in db.query(f"SELECT DISTINCT {col} v FROM tracks WHERE {col} IS NOT NULL"):
                    v = row["v"]
                    if non_latin(v) and not db.one("SELECT 1 FROM loc WHERE kind=? AND original=? AND lang=?", (kind, v, k)):
                        _save(kind, v, k, romanize(v), "romanized")
                await asyncio.sleep(0)
        return {"looked_up": state["total"]}
    finally:
        state["running"] = False


async def localize_all_users() -> str:
    langs = set()
    for u in db.query("SELECT prefs FROM users WHERE disabled=0"):
        p = prefs(u)
        if p["localize"]:
            langs.add((p["lang"], p["latin"]))
    for lang, latin in langs:
        await localize_library(lang, latin)
    return f"{len(langs)} language setting(s) refreshed" if langs else "nobody has localized names switched on"
