"""Version-aware track identity: tells originals, remixes, live takes, covers, edits… apart.

Everything that compares two tracks (playlist import, de-duplication, download verification,
syncing with media servers) goes through `compare()` so the rules are identical everywhere.
"""
import re
import unicodedata
from dataclasses import dataclass, field
from difflib import SequenceMatcher

# version markers → category. Categories in IGNORED don't change the song's identity.
_VERSION_PATTERNS: list[tuple[str, str]] = [
    (r"\bslowed\b|\breverb\b|\bsped[\s-]*up\b|\bnightcore\b|\b8d\b|\bdaycore\b|\bchopped\b", "altered"),
    (r"\bkaraoke\b", "karaoke"),
    (r"\ba[\s-]?cappella\b|\bacapella\b|\bvocals? only\b", "acappella"),
    (r"\binstrumental\b|\boff[\s-]?vocal\b", "instrumental"),
    (r"\bremix\b|\brmx\b|\bre-?work\b|\bbootleg\b|\bflip\b|\bvip\b|\bmash-?up\b|\bdub\b|\bclub mix\b|\b(?<!original )(?<!extended )(?<!radio )(?<!album )(?<!single )mix\b", "remix"),
    (r"\blive\b|\bin concert\b|\bbbc session\b|\bsession\b", "live"),
    (r"\bacoustic\b|\bunplugged\b|\bstripped\b|\bpiano version\b", "acoustic"),
    (r"\bdemo\b", "demo"),
    (r"\bcover\b|\btribute\b", "cover"),
    (r"\bextended\b", "extended"),
    (r"\bradio edit\b|\bsingle edit\b|\bshort edit\b|\b(?<!re-)edit\b", "edit"),
    (r"\bre-?recorded\b|\btaylor'?s version\b|\b20\d\d version\b|\bnew version\b", "rerecorded"),
    (r"\borchestral\b|\bsymphonic\b", "orchestral"),
    (r"\bclean\b|\bexplicit\b|\bcensored\b", "explicitness"),
    (r"\bremaster(?:ed)?\b|\bmono\b|\bstereo\b|\bdeluxe\b|\bbonus\b|\banniversary\b|\boriginal mix\b|\balbum version\b|\bsingle version\b|\bfrom .*\b|\bsoundtrack\b|\bofficial\b|\bmusic video\b|\blyric(s| video)?\b|\bvisuali[sz]er\b|\baudio\b|\bvideo\b|\bhd\b|\bhq\b|\b4k\b|\bmv\b", "ignored"),
]
IGNORED = {"ignored", "explicitness"}
_FEAT = re.compile(r"\s*[\(\[]?\b(?:feat\.?|ft\.?|featuring|with)\s+[^\)\]]*[\)\]]?", re.I)
_GROUP = re.compile(r"[\(\[\{]([^\)\]\}]*)[\)\]\}]")


def fold(s: str) -> str:
    s = unicodedata.normalize("NFKD", s or "")
    s = "".join(c for c in s if not unicodedata.combining(c)).lower()
    s = s.replace("&", " and ").replace("’", "'").replace("$", "s")
    s = re.sub(r"[^\w\s']", " ", s)
    s = s.replace("'", "")
    return " ".join(s.split())


@dataclass
class Desc:
    title: str = ""
    artist: str = ""
    album: str = ""
    length: float | None = None
    isrc: str | None = None
    mbid: str | None = None
    base: str = ""
    versions: frozenset = field(default_factory=frozenset)
    remixer: str = ""
    artists: tuple = ()


def split_artists(artist: str) -> tuple:
    parts = re.split(r"\s*(?:,|;|/|&|\+|\bx\b|\band\b|\bfeat\.?\b|\bft\.?\b|\bfeaturing\b|\bvs\.?\b|\bwith\b)\s*", artist or "", flags=re.I)
    out = [fold(p) for p in parts if fold(p)]
    return tuple(out) or (fold(artist),)


def parse_title(title: str) -> tuple[str, frozenset, str]:
    """→ (base title, version categories, remixer)."""
    t = title or ""
    chunks = _GROUP.findall(t)
    rest = _GROUP.sub(" ", t)
    # "Song - Live at X" / "Song - 2011 Remaster"
    if " - " in rest:
        head, tail = rest.split(" - ", 1)
        chunks.append(tail)
        rest = head
    rest = _FEAT.sub(" ", rest)
    versions, remixer = set(), ""
    for ch in chunks:
        low = ch.lower()
        if re.match(r"^\s*(feat|ft|featuring|with)\b", low):
            continue
        found = False
        for pat, cat in _VERSION_PATTERNS:
            if re.search(pat, low):
                found = True
                if cat not in IGNORED:
                    versions.add(cat)
                if cat == "remix" and not remixer:
                    m = re.search(r"^(.*?)\s*\b(?:remix|rmx|rework|re-work|bootleg|flip|vip|mix|dub)\b", low)
                    remixer = fold(m.group(1)) if m else ""
                    remixer = re.sub(r"\b(official|the|feat.*)\b", "", remixer).strip()
        if not found:
            # unknown parenthetical that isn't a version marker (e.g. "(Part 2)") is part of the title
            rest += " " + ch
    # tokens like "Song Remix" without brackets
    low_rest = rest.lower()
    for pat, cat in _VERSION_PATTERNS[:5]:
        if cat in ("remix", "instrumental", "karaoke", "altered") and re.search(rf"\s({pat})\s*$", " " + low_rest):
            versions.add(cat)
            rest = re.sub(rf"\s({pat})\s*$", "", " " + rest, flags=re.I)
    return fold(rest), frozenset(versions), remixer


def describe(title: str, artist: str, album: str = "", length: float | None = None,
             isrc: str | None = None, mbid: str | None = None) -> Desc:
    base, versions, remixer = parse_title(title)
    return Desc(title=title or "", artist=artist or "", album=album or "", length=length or None,
                isrc=(isrc or "").upper().replace("-", "") or None, mbid=mbid or None,
                base=base, versions=versions, remixer=remixer, artists=split_artists(artist))


def sim(a: str, b: str) -> float:
    if not a or not b:
        return 0.0
    if a == b:
        return 1.0
    return SequenceMatcher(None, a, b).ratio()


def _artist_score(a: Desc, b: Desc) -> float:
    best = 0.0
    for x in a.artists:
        for y in b.artists:
            best = max(best, sim(x, y), 1.0 if (x and y and (x in y or y in x) and min(len(x), len(y)) >= 4) else 0.0)
    return best


@dataclass
class Verdict:
    match: bool
    score: float
    reason: str


def compare(a: Desc, b: Desc, strict_duration: bool = True) -> Verdict:
    """Is b the same recording/version as a?"""
    if a.mbid and b.mbid and a.mbid == b.mbid:
        return Verdict(True, 1.0, "same MusicBrainz recording")
    if a.isrc and b.isrc and a.isrc == b.isrc:
        return Verdict(True, 0.98, "same ISRC")
    tsim = sim(a.base, b.base)
    if tsim < 0.85:
        return Verdict(False, tsim * 0.5, "different title")
    if a.versions != b.versions:
        diff = sorted((a.versions ^ b.versions))
        return Verdict(False, 0.3, f"different version ({', '.join(diff)})")
    if "remix" in a.versions and a.remixer and b.remixer and sim(a.remixer, b.remixer) < 0.7:
        return Verdict(False, 0.35, "different remix")
    asc = _artist_score(a, b)
    if asc < 0.75:
        return Verdict(False, 0.4, "different artist (cover?)")
    score = 0.55 * tsim + 0.35 * asc
    if a.length and b.length:
        d = abs(a.length - b.length)
        if d > (12 if strict_duration else 30):
            return Verdict(False, 0.45, f"length differs by {int(d)}s")
        score += 0.1 if d <= 4 else 0.05
    else:
        score += 0.05
    return Verdict(score >= 0.85, round(score, 3), "metadata match")


def key(d: Desc) -> str:
    """Grouping key – same key ⇒ same song & version (modulo fuzzy title)."""
    return "|".join([d.artists[0] if d.artists else "", d.base, ",".join(sorted(d.versions)), d.remixer])


# ---- library index ---------------------------------------------------------

class LibraryIndex:
    """In-memory index of library tracks for fast matching."""

    def __init__(self, rows: list[dict]):
        self.rows = rows
        self.by_mbid: dict[str, dict] = {}
        self.by_isrc: dict[str, dict] = {}
        self.by_word: dict[str, list[tuple[Desc, dict]]] = {}
        for r in rows:
            d = describe(r["title"], r["artist"], r.get("album") or "", r.get("duration"), r.get("isrc"), r.get("mb_trackid"))
            if d.mbid:
                self.by_mbid.setdefault(d.mbid, r)
            if d.isrc:
                self.by_isrc.setdefault(d.isrc, r)
            word = (d.base.split() or [""])[0]
            self.by_word.setdefault(word, []).append((d, r))

    @classmethod
    def load(cls) -> "LibraryIndex":
        from .. import db
        return cls(db.query("SELECT id, path, title, artist, album, albumartist, album_id, duration, isrc, mb_trackid, format, bitrate FROM tracks"))

    def find(self, want: Desc) -> tuple[dict | None, Verdict | None]:
        if want.mbid and want.mbid in self.by_mbid:
            return self.by_mbid[want.mbid], Verdict(True, 1.0, "same MusicBrainz recording")
        if want.isrc and want.isrc in self.by_isrc:
            return self.by_isrc[want.isrc], Verdict(True, 0.98, "same ISRC")
        word = (want.base.split() or [""])[0]
        best, best_v = None, None
        for d, r in self.by_word.get(word, []):
            v = compare(want, d)
            if v.match and (not best_v or v.score > best_v.score):
                best, best_v = r, v
        return best, best_v
