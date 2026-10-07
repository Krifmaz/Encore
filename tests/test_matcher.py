import pytest

from app.services.matcher import compare, describe as D, parse_title

CASES = [
    ("Blinding Lights", "The Weeknd", "Blinding Lights - Chromatics Remix", "The Weeknd", False),
    ("Blinding Lights", "The Weeknd", "Blinding Lights", "The Weeknd", True),
    ("Hurt", "Nine Inch Nails", "Hurt", "Johnny Cash", False),                       # cover
    ("Wonderwall - Remastered 2014", "Oasis", "Wonderwall", "Oasis", True),          # remaster = same song
    ("Strobe (Radio Edit)", "deadmau5", "Strobe", "deadmau5", False),
    ("Strobe (Original Mix)", "deadmau5", "Strobe", "deadmau5", True),
    ("Levels (Skrillex Remix)", "Avicii", "Levels (Nicky Romero Remix)", "Avicii", False),
    ("Levels (Skrillex Remix)", "Avicii", "Levels - Skrillex Remix", "Avicii", True),
    ("Love Story (Taylor's Version)", "Taylor Swift", "Love Story", "Taylor Swift", False),
    ("Sicko Mode (feat. Drake)", "Travis Scott", "SICKO MODE", "Travis Scott, Drake", True),
    ("Creep - Acoustic", "Radiohead", "Creep (Live)", "Radiohead", False),
    ("Song Part 2", "X", "Song (Part 2)", "X", True),
    ("Smells Like Teen Spirit", "Nirvana", "Smells Like Teen Spirit (Slowed + Reverb)", "Nirvana", False),
    ("Hey Jude", "The Beatles", "Hey Jude", "Beatles", True),
    ("Beyoncé", "Artist", "Beyonce", "Artist", True),
    ("Bad Guy", "Billie Eilish", "Bad Guy (Official Music Video)", "Billie Eilish", True),
    ("One More Time", "Daft Punk", "One More Time (Instrumental)", "Daft Punk", False),
]


@pytest.mark.parametrize("a,aa,b,bb,expected", CASES)
def test_version_aware_compare(a, aa, b, bb, expected):
    assert compare(D(a, aa), D(b, bb)).match is expected


def test_duration_and_ids():
    assert not compare(D("Song", "X", length=200), D("Song", "X", length=320)).match
    assert compare(D("Totally different", "X", isrc="USABC1234567"), D("Other", "Y", isrc="US-ABC-12-34567")).match
    assert compare(D("a", "b", mbid="m1"), D("c", "d", mbid="m1")).match


def test_parse_remixer():
    assert parse_title("Clarity (Tiësto Remix) [feat. Foxes]") == ("clarity", frozenset({"remix"}), "tiesto")
    assert parse_title("Strobe (Extended Mix)")[1] == frozenset({"extended"})
