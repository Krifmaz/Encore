#!/usr/bin/env python3
"""Test tone: a source for Encore's tests, written with the SDK. It searches a short built-in list of tones and
writes the one picked as a WAV file. `test` reports what it can see of its own process (whether a secret reached
its environment or its arguments, its working folder), so the tests can check the sandbox from inside."""
import math
import os
import struct
import sys
import time
import wave

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import encore_plugin as ep  # noqa: E402

TONES = [
    {"key": "tone:a440", "title": "A 440", "artist": "Test Tones", "album": "Tuning", "freq": 440.0},
    {"key": "tone:c262", "title": "Middle C", "artist": "Test Tones", "album": "Tuning", "freq": 261.63},
    {"key": "tone:e330", "title": "E 330", "artist": "Test Tones", "album": "Tuning", "freq": 329.63},
]


def status(req):
    return {"message": "Ready · three test tones", "signed_in": None}


def test(req):
    token = ep.setting(req, "token", "")
    leaked_env = bool(token) and any(token in v for v in os.environ.values())
    leaked_argv = bool(token) and any(token in a for a in sys.argv)
    ep.log("test ran")
    return {"message": f"token seen: {bool(token)} · in env: {leaked_env} · in argv: {leaked_argv} · argv: {' '.join(sys.argv[1:])} · cwd: {os.getcwd()} · home: {os.environ.get('HOME', '')}"}


def search(req):
    words = " ".join(str(req.get(k) or "") for k in ("query", "title", "album", "artist")).strip()
    out = []
    for t in TONES:
        hay = f"{t['title']} {t['artist']} {t['album']}"
        score = 100 * ep.contains(hay, words) if words else 50
        if words and score == 0:
            continue
        out.append(ep.result(key=t["key"], title=t["title"], artist=t["artist"], album=t["album"], format="wav",
                             bit_depth=16, sample_rate=int(ep.setting(req, "rate", "44100")), duration=float(ep.setting(req, "seconds", 1)),
                             score=score, payload={"freq": t["freq"]}))
    out.sort(key=lambda r: -(r["score"] or 0))
    return {"results": out[: int(req.get("limit") or 25)]}


def download(req):
    folder = req.get("folder") or "."
    freq = float((req.get("payload") or {}).get("freq") or 440.0)
    seconds = int(ep.setting(req, "seconds", 1))
    rate = int(ep.setting(req, "rate", "44100"))
    name = ep.safe_name(f"{req.get('title') or 'tone'}.wav")
    dest = os.path.join(folder, name)
    part = dest + ".part"
    try:
        with wave.open(part, "wb") as w:
            w.setnchannels(1)
            w.setsampwidth(2)
            w.setframerate(rate)
            for s in range(seconds):
                frames = b"".join(struct.pack("<h", int(12000 * math.sin(2 * math.pi * freq * (s * rate + i) / rate))) for i in range(rate))
                w.writeframes(frames)
                ep.progress(100.0 * (s + 1) / seconds, name)
                if ep.setting(req, "slow", False):
                    time.sleep(30)
        os.replace(part, dest)
    except ep.Cancelled:
        if os.path.exists(part):
            os.remove(part)
        raise
    return {"files": [ep.file_entry(dest, title=req.get("title"), artist=req.get("artist"), album=req.get("album"), track=1, duration=float(seconds))],
            "message": f"Wrote {name}"}


ep.run("test-tone", "0.1.0", kind="source", kinds=("track", "album"),
       handlers={"status": status, "test": test, "search": search, "download": download})
