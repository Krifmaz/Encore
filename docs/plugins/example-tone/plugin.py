#!/usr/bin/env python3
"""Example tone: an example source for Encore plugin authors, written with the plugin SDK (encore_plugin.py,
copied beside this file from plugins/sdk/).

It "searches" a short built-in list of tones and "downloads" the one picked by writing a sine tone as a FLAC
file into the folder Encore gives it. No network, no account, nothing to install: Python 3.11 or newer and the
SDK. Copy the folder, rename the plugin and replace the list and the tone with your own service.

Run it the way Encore does, from the repository's root:

    python3 plugins/tools/plugin-check.py docs/plugins/example-tone --data /tmp/tone-data
    python3 plugins/tools/plugin-check.py docs/plugins/example-tone --data /tmp/tone-data \
        --track "The Example Tones|Concert A" --download 1 --folder /tmp/tone-files

docs/plugins/README.md walks through it; docs/plugins/CONTRACT.md is the protocol.
"""
import hashlib
import math
import os
import struct
import sys
import time
from array import array

# The SDK lives beside this file, in the plugin's folder and in its zip.
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import encore_plugin as ep  # noqa: E402

PLUGIN_ID = "example-tone"  # the manifest's [plugin] id: describe answers it as "name"
VERSION = "0.1.0"           # the manifest's version: Encore checks the two agree at install

# ---- the "service": a fixed list of albums of tones ----------------------------------------------------------

ARTIST = "The Example Tones"
ALBUMS = [
    {"id": "tuning-fork", "title": "Tuning Fork", "year": 2026,
     "tones": [("Concert A", 440.0), ("Middle C", 261.63), ("E Above Middle C", 329.63), ("G Above Middle C", 392.0)]},
    {"id": "test-signals", "title": "Test Signals", "year": 2026,
     "tones": [("Low Hum", 100.0), ("One Kilohertz", 1000.0), ("High Whistle", 4000.0)]},
]
RATES = (44100, 48000, 96000)  # the options of the "rate" setting
MAX_SECONDS = 600              # the "seconds" setting's max
MIN_SCORE = 50                 # results that fit the request less than this are left out


def find_album(album_id):
    return next((a for a in ALBUMS if a["id"] == album_id), None)


# ---- settings: read them with a default and a range, so a bad saved value never ends in a crash ---------------

def tone_seconds(req):
    """How long a tone lasts: as long as the song asked for when Encore knows its length (so Encore's length check
    passes), else the "seconds" setting."""
    length = req.get("length")
    if isinstance(length, (int, float)) and not isinstance(length, bool) and length > 0:
        seconds = float(length)
    else:
        try:
            seconds = float(ep.setting(req, "seconds", 30))
        except (TypeError, ValueError):
            ep.log("the Tone length setting is not a number; using 30 seconds")
            seconds = 30.0
    return max(1, min(MAX_SECONDS, round(seconds)))


def sample_rate(req):
    try:
        rate = int(ep.setting(req, "rate", "44100"))
    except (TypeError, ValueError):
        rate = 0
    if rate not in RATES:
        ep.log(f"the Sample rate setting is not one of {', '.join(map(str, RATES))}; using 44100")
        rate = 44100
    return rate


def fit(hay, wanted):
    """How well `hay` fits the words asked for, 0 to 100 (every word of the list fits an empty question)."""
    if not ep.norm(wanted):
        return 100
    return round(100 * ep.contains(hay, wanted))


# ---- the commands ------------------------------------------------------------------------------------------

def status(req):
    count = sum(len(a["tones"]) for a in ALBUMS)
    return {"message": f"Ready · {count} tones in {len(ALBUMS)} albums, written as FLAC on this server", "signed_in": None}


def test(req):
    # A real plugin signs in or asks its service here. This one writes a short tone in memory and checks that its
    # data folder (ENCORE_PLUGIN_DATA: caches, tokens) can be written.
    started = time.monotonic()
    frame = encode_frame(0, tone_block(440.0, 44100, 0, BLOCK, 44100), 44100)
    took = (time.monotonic() - started) * 1000
    probe = os.path.join(ep.data_dir(), ".write-check")
    try:
        with open(probe, "w", encoding="utf-8") as f:
            f.write("ok")
        os.remove(probe)
    except OSError as e:
        raise ep.PluginError(f"cannot write to its data folder {ep.data_dir()}: {e.strerror}", "internal") from e
    ep.log("test: frame written, data folder writable")
    return {"message": f"Wrote a test frame of {len(frame):,} bytes in {took:.0f} ms · the data folder can be written"}


def search(req):
    kind = req.get("kind") or "track"
    query = (req.get("query") or "").strip()  # what someone typed in the picker: search by it as typed
    try:
        limit = max(1, int(req.get("limit") or 25))
    except (TypeError, ValueError):
        limit = 25
    seconds, rate = tone_seconds(req), sample_rate(req)
    results = []
    if kind == "album":
        wanted = query or f"{req.get('artist') or ''} {req.get('album') or req.get('title') or ''}"
        for album in ALBUMS:
            score = fit(f"{album['title']} {ARTIST} sine tones", wanted)
            if score < MIN_SCORE:
                continue
            count = len(album["tones"])
            results.append(ep.result(
                key=album["id"], title=album["title"], artist=ARTIST, album=album["title"], format="flac",
                bit_depth=16, sample_rate=rate, tracks=count, duration=float(seconds * count),
                size=2 * rate * seconds * count, info=f"{count} sine tones · {album['year']}", score=score,
                payload={"album": album["id"], "seconds": seconds}))
    elif kind == "track":
        wanted = query or f"{req.get('artist') or ''} {req.get('title') or ''}"
        for album in ALBUMS:
            for number, (title, freq) in enumerate(album["tones"], 1):
                score = fit(f"{title} {ARTIST} {album['title']} {freq:g} hz sine tone", wanted)
                if score < MIN_SCORE:
                    continue
                results.append(ep.result(
                    key=f"{album['id']}/{number}", title=title, artist=ARTIST, album=album["title"], format="flac",
                    bit_depth=16, sample_rate=rate, tracks=1, duration=float(seconds), size=2 * rate * seconds,
                    info=f"A {freq:g} Hz sine tone", score=score,
                    payload={"album": album["id"], "track": number, "seconds": seconds}))
    else:
        # The manifest's kinds are track and album, so Encore asks for nothing else; answer clearly anyway.
        raise ep.PluginError(f"{PLUGIN_ID} searches songs and albums, not {kind}", "unsupported")
    results.sort(key=lambda r: -(r["score"] or 0))  # best first
    return {"results": results[:limit]}


def download(req):
    folder = req.get("folder")
    if not folder or not os.path.isdir(folder):
        raise ep.PluginError("Encore gave no download folder", "internal")
    payload = req.get("payload") or {}  # given back as it was in the search result
    album = find_album(payload.get("album"))
    if album is None:
        raise ep.PluginError("that tone is not in the list any more", "not_found")
    track = payload.get("track")
    if isinstance(track, str) and track.isdigit():
        track = int(track)
    if track is None:
        numbers = list(range(1, len(album["tones"]) + 1))  # an album: every tone of it
    elif isinstance(track, int) and 1 <= track <= len(album["tones"]):
        numbers = [track]
    else:
        raise ep.PluginError(f"there is no tone {track} on {album['title']}", "not_found")
    seconds = payload.get("seconds") if isinstance(payload.get("seconds"), int) else tone_seconds(req)
    seconds = max(1, min(MAX_SECONDS, seconds))
    rate = sample_rate(req)

    total = len(numbers) * seconds * rate
    done = [0, 0.0]  # samples written, when progress was last reported

    files = []
    for number in numbers:
        title, freq = album["tones"][number - 1]
        name = ep.safe_name(f"{number:02d} {title}.flac")
        dest = os.path.join(folder, name)
        before = done[0]

        def report(samples, name=name, before=before):
            done[0] = before + samples
            now = time.monotonic()
            if now - done[1] >= 1.0:  # about once a second
                done[1] = now
                ep.progress(100.0 * done[0] / total, name)

        tags = {
            "TITLE": title, "ARTIST": ARTIST, "ALBUMARTIST": ARTIST, "ALBUM": album["title"],
            "TRACKNUMBER": str(number), "TRACKTOTAL": str(len(album["tones"])), "DATE": str(album["year"]),
            "COMMENT": f"A {freq:g} Hz sine tone written by Encore's example plugin",
        }
        write_flac(dest, freq, seconds, rate, tags, report)
        ep.progress(100.0 * done[0] / total, name)
        files.append(ep.file_entry(dest, title=title, artist=ARTIST, album=album["title"], track=number, disc=1,
                                   duration=float(seconds), format="flac"))
    what = f"{len(files)} tones" if len(files) > 1 else "1 tone"
    return {"files": files, "message": f"Wrote {what} of {seconds} s at {rate / 1000:g} kHz"}


# ---- the tone ----------------------------------------------------------------------------------------------

BLOCK = 4096          # samples per FLAC frame
AMPLITUDE = 8192      # a quarter of full scale: -12 dBFS, comfortable to listen to


def tone_block(freq, rate, start, count, total):
    """Samples start .. start + count of a sine tone `total` samples long, as 16-bit integers. The first and last
    10 ms fade in and out, so the tone starts and stops without a click."""
    step = 2 * math.pi * freq / rate
    samples = [round(AMPLITUDE * math.sin(step * i)) for i in range(start, start + count)]
    fade = max(1, rate // 100)
    if start < fade or start + count > total - fade:
        for j in range(count):
            gain = min(1.0, (start + j) / fade, (total - 1 - start - j) / fade)
            if gain < 1.0:
                samples[j] = round(samples[j] * gain)
    return array("h", samples)


# ---- a FLAC writer: verbatim (uncompressed) frames, mono, 16 bits ------------------------------------------
# Only what this example needs from the FLAC format (https://www.rfc-editor.org/rfc/rfc9639): the stream info
# block, a Vorbis comment block with the tags, and frames whose one subframe holds the samples as they are.
# The file is larger than flac would make it, and every player reads it.

RATE_CODES = {44100: 0b1001, 48000: 0b1010, 96000: 0b1011}  # anything else: "see the stream info" (0)


def _crc8_table():
    table = []
    for i in range(256):
        c = i
        for _ in range(8):
            c = ((c << 1) ^ 0x07) & 0xFF if c & 0x80 else (c << 1) & 0xFF
        table.append(c)
    return table


def _crc16_table():
    table = []
    for i in range(256):
        c = i << 8
        for _ in range(8):
            c = ((c << 1) ^ 0x8005) & 0xFFFF if c & 0x8000 else (c << 1) & 0xFFFF
        table.append(c)
    return table


CRC8, CRC16 = _crc8_table(), _crc16_table()


def crc8(data):
    crc = 0
    for b in data:
        crc = CRC8[crc ^ b]
    return crc


def crc16(data):
    crc = 0
    for b in data:
        crc = ((crc << 8) & 0xFFFF) ^ CRC16[(crc >> 8) ^ b]
    return crc


def coded_number(n):
    """A frame number as FLAC writes it: in the UTF-8 pattern, up to 36 bits."""
    if n < 0x80:
        return bytes([n])
    extra = next(k for k, limit in ((1, 1 << 11), (2, 1 << 16), (3, 1 << 21), (4, 1 << 26), (5, 1 << 31), (6, 1 << 36)) if n < limit)
    tail = []
    for _ in range(extra):
        tail.append(0x80 | (n & 0x3F))
        n >>= 6
    return bytes([((0xFF << (7 - extra)) & 0xFF) | n] + tail[::-1])


def metadata_block(kind, body, last):
    return bytes([(0x80 if last else 0) | kind]) + len(body).to_bytes(3, "big") + body


def stream_info(rate, total):
    body = struct.pack(">HH", BLOCK, BLOCK)  # smallest and largest block size
    body += bytes(6)  # smallest and largest frame size: 0, not given
    # sample rate (20 bits), channels - 1 (3), bits per sample - 1 (5), samples in all (36)
    body += ((rate << 44) | (0 << 41) | (15 << 36) | total).to_bytes(8, "big")
    body += bytes(16)  # the MD5 of the samples: written at the end, once it is known
    return metadata_block(0, body, last=False)


def vorbis_comment(tags):
    vendor = f"Encore {PLUGIN_ID} {VERSION}".encode()
    body = struct.pack("<I", len(vendor)) + vendor + struct.pack("<I", len(tags))
    for key, value in tags.items():
        entry = f"{key}={value}".encode()
        body += struct.pack("<I", len(entry)) + entry
    return metadata_block(4, body, last=True)


def encode_frame(number, samples, rate):
    """One frame: the header, one verbatim subframe, the CRC-16. `samples` is an array of 16-bit integers."""
    big = array("h", samples)
    if sys.byteorder == "little":
        big.byteswap()  # FLAC stores samples big-endian
    head = bytearray(b"\xff\xf8")  # sync code, fixed block size
    head.append(0b0111_0000 | RATE_CODES.get(rate, 0))  # the block size follows the frame number; sample rate
    head.append(0b0000_1000)  # mono, 16 bits per sample
    head += coded_number(number)
    head += struct.pack(">H", len(samples) - 1)
    head.append(crc8(head))
    frame = bytes(head) + b"\x02" + big.tobytes()  # subframe header: verbatim, no wasted bits
    return frame + struct.pack(">H", crc16(frame))


def write_flac(dest, freq, seconds, rate, tags, report):
    """Write the tone to `dest` through `dest.part`, renamed when the file is whole: a download Encore stops
    (SIGTERM, which the SDK turns into ep.Cancelled) leaves no half file under the real name."""
    total = seconds * rate
    part = dest + ".part"
    md5 = hashlib.md5()  # of the samples, little-endian, as the stream info wants it
    try:
        with open(part, "wb") as f:
            f.write(b"fLaC")
            f.write(stream_info(rate, total))
            f.write(vorbis_comment(tags))
            for number, start in enumerate(range(0, total, BLOCK)):
                samples = tone_block(freq, rate, start, min(BLOCK, total - start), total)
                little = array("h", samples)
                if sys.byteorder != "little":
                    little.byteswap()
                md5.update(little.tobytes())
                f.write(encode_frame(number, samples, rate))
                report(start + len(samples))
            f.seek(4 + 4 + 18)  # "fLaC", the block header, then 18 bytes into the stream info
            f.write(md5.digest())
        os.replace(part, dest)
    except BaseException:  # ep.Cancelled included: remove the part file, then let the SDK answer
        try:
            os.remove(part)
        except OSError:
            pass
        raise


ep.run(PLUGIN_ID, VERSION, kind="source", kinds=("track", "album"),
       handlers={"status": status, "test": test, "search": search, "download": download})
