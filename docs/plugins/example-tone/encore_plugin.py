"""Encore plugin SDK, protocol 1. One file, no third-party packages, Python 3.11+.

Copy this file next to your plugin's program and write the handlers:

    import encore_plugin as ep

    def status(req):   return {"message": "Ready"}
    def test(req):     return {"message": "Signed in as ..."}
    def search(req):   return {"results": [ep.result(key=..., title=..., payload=...)]}
    def download(req): return {"files": [ep.file_entry(path, title=...)]}

    ep.run("my-plugin", "0.1.0", kind="source", kinds=("track", "album"),
           handlers={"status": status, "test": test, "search": search, "download": download})

How Encore talks to a plugin (docs/CONTRACT.md has the full contract):
  * it runs `<program> <command>` with one JSON object on stdin,
  * reads one JSON object from stdout (`{"ok": true, ...}` or `{"ok": false, "error": "...", "code": "..."}`),
  * and shows stderr lines as the plugin's log; `progress <0-100> [text]` lines drive the progress bar.
"""
from __future__ import annotations

import hashlib
import json
import os
import re
import signal
import sys
import time
import traceback
import urllib.error
import urllib.parse
import urllib.request

PROTOCOL = 1
COMMANDS = ("describe", "status", "test", "search", "download", "update")
ERROR_CODES = ("auth", "not_found", "network", "rate_limited", "unsupported", "cancelled", "internal")
AUDIO_EXT = ("flac", "mp3", "m4a", "alac", "ogg", "opus", "wav", "aiff", "aif", "ape", "wv", "aac", "webm")
LOSSLESS = ("flac", "alac", "wav", "aiff", "aif", "ape", "wv")

_NAME = "plugin"
_VERSION = "0"


class PluginError(Exception):
    """A failure the user should read as it is. `code` is one of ERROR_CODES. `retry_after` (seconds)
    tells Encore when a rate-limited call may be tried again."""

    def __init__(self, message: str, code: str = "internal", retry_after: float | None = None):
        super().__init__(message)
        self.code = code if code in ERROR_CODES else "internal"
        self.retry_after = retry_after


class Cancelled(Exception):
    """Encore stopped the plugin (SIGTERM): partial files are being cleaned up."""


# ---- talking to Encore ---------------------------------------------------------------------------

def progress(percent: float, message: str | None = None) -> None:
    """Report download progress (0-100) and, optionally, what is happening right now."""
    pct = max(0.0, min(100.0, float(percent)))
    line = f"progress {pct:.1f}" + (f" {message}" if message else "")
    print(line, file=sys.stderr, flush=True)


def log(message: str) -> None:
    """A line for the plugin's log in Encore (the last one is the error shown when the plugin crashes)."""
    print(f"log {message}", file=sys.stderr, flush=True)


def setting(req: dict, key: str, default=None):
    """A setting's effective value (Encore sends the saved values; empty counts as unset)."""
    v = (req.get("settings") or {}).get(key)
    return default if v is None or v == "" else v


def data_dir() -> str:
    """The plugin's own writable folder (kept across updates): caches, tokens, cookies."""
    d = os.environ.get("ENCORE_PLUGIN_DATA") or os.path.join(os.path.dirname(os.path.abspath(sys.argv[0])), "data")
    os.makedirs(d, exist_ok=True)
    return d


def user_agent() -> str:
    return f"Encore-plugin/{_NAME} {_VERSION} (+https://github.com/Krifmaz/Encore)"


# ---- HTTP (urllib, with retries) -----------------------------------------------------------------

def http(url: str, *, timeout: float = 30, headers: dict | None = None, data: bytes | None = None, method: str | None = None, retries: int = 3):
    """Open a URL; retries connection errors, 5xx and 429. Raises PluginError on a final failure."""
    h = {"User-Agent": user_agent(), "Accept": "*/*"}
    h.update(headers or {})
    last: Exception | None = None
    for attempt in range(retries):
        try:
            req = urllib.request.Request(url, data=data, headers=h, method=method)
            return urllib.request.urlopen(req, timeout=timeout)
        except urllib.error.HTTPError as e:
            if e.code in (401, 403):
                raise PluginError(f"{_host(url)} refused the request (HTTP {e.code}): check the sign-in", "auth") from e
            if e.code == 404:
                raise PluginError(f"{_host(url)} has no such item (HTTP 404)", "not_found") from e
            if e.code == 429:
                wait = _retry_after(e.headers.get("Retry-After"))
                if attempt + 1 < retries:
                    time.sleep(min(wait, 30))
                    continue
                raise PluginError(f"{_host(url)} is rate-limiting this server (HTTP 429)", "rate_limited", retry_after=wait) from e
            if e.code >= 500 and attempt + 1 < retries:
                last = e
                time.sleep(1.5 * (attempt + 1))
                continue
            raise PluginError(f"{_host(url)} answered HTTP {e.code}", "network") from e
        except (urllib.error.URLError, TimeoutError, ConnectionError, OSError) as e:
            last = e
            if attempt + 1 < retries:
                time.sleep(1.5 * (attempt + 1))
                continue
    raise PluginError(f"could not reach {_host(url)}: {last}", "network")


def http_json(url: str, **kw):
    with http(url, **kw) as r:
        raw = r.read()
    try:
        return json.loads(raw.decode("utf-8", "replace"))
    except ValueError as e:
        raise PluginError(f"{_host(url)} did not answer with JSON", "network") from e


def http_text(url: str, **kw) -> str:
    with http(url, **kw) as r:
        return r.read().decode("utf-8", "replace")


def download_file(url: str, dest: str, *, expected_size: int | None = None, md5: str | None = None, timeout: float = 60,
                  on_progress=None, retries: int = 3, headers: dict | None = None) -> int:
    """Stream a URL to `dest` (through `dest.part`), check the size and MD5 when known, return the byte
    count. `on_progress(done_bytes, total_bytes_or_None)` is called about once a second."""
    part = dest + ".part"
    os.makedirs(os.path.dirname(os.path.abspath(dest)), exist_ok=True)
    for attempt in range(retries):
        done = 0
        digest = hashlib.md5() if md5 else None
        try:
            with http(url, timeout=timeout, headers=headers, retries=1) as r, open(part, "wb") as f:
                total = expected_size
                if total is None:
                    cl = r.headers.get("Content-Length")
                    total = int(cl) if cl and cl.isdigit() else None
                last = 0.0
                while True:
                    chunk = r.read(256 * 1024)
                    if not chunk:
                        break
                    f.write(chunk)
                    done += len(chunk)
                    if digest:
                        digest.update(chunk)
                    now = time.monotonic()
                    if on_progress and now - last >= 1.0:
                        last = now
                        on_progress(done, total)
            if expected_size is not None and done != expected_size:
                raise PluginError(f"short download: {done} of {expected_size} bytes", "network")
            if digest and digest.hexdigest().lower() != md5.lower():
                raise PluginError("the file arrived corrupted (MD5 mismatch)", "network")
            os.replace(part, dest)
            if on_progress:
                on_progress(done, expected_size if expected_size is not None else done)
            return done
        except PluginError as e:
            _rm(part)
            if e.code == "network" and attempt + 1 < retries:
                log(f"retrying {os.path.basename(dest)}: {e}")
                continue
            raise
        except Cancelled:
            _rm(part)
            raise
        except OSError as e:
            _rm(part)
            raise PluginError(f"could not write {dest}: {e}", "internal") from e
    raise PluginError("download failed", "network")


# ---- matching and naming helpers (the same ideas as Encore's picker) ------------------------------

def norm(s: str) -> str:
    return " ".join(re.sub(r"[^0-9a-z]+", " ", (s or "").lower()).split())


def contains(hay: str, needle: str) -> float:
    """Share of `needle`'s words found in `hay` (0-1); 0 for an empty needle."""
    words = norm(needle).split()
    if not words:
        return 0.0
    h = f" {norm(hay)} "
    return sum(1 for w in words if f" {w} " in h) / len(words)


def is_lossless(fmt: str | None) -> bool:
    return (fmt or "").lower() in LOSSLESS


def quality(fmt: str | None, bitrate: int = 0, bit_depth: int = 0, sample_rate: int = 0) -> str:
    """The quality label Encore shows: 'FLAC 24-bit/96kHz', 'MP3 320k', 'OPUS'."""
    f = (fmt or "").lower()
    if not f:
        return ""
    if is_lossless(f):
        bits = f" {bit_depth}-bit" if bit_depth else ""
        khz = f"/{sample_rate / 1000:g}kHz" if sample_rate and sample_rate != 44100 else ""
        return f"{f.upper()}{bits}{khz}"
    return f"{f.upper()} {bitrate}k" if bitrate else f.upper()


def safe_name(name: str) -> str:
    """A file name Encore can import on any filesystem: no path parts, no control or reserved characters."""
    name = os.path.basename(name.replace("\\", "/"))
    name = re.sub(r'[\x00-\x1f<>:"|?*]', "_", name).strip(" .")
    return name or "untitled"


def result(*, key: str, title: str, payload: dict, artist: str = "", album: str = "", subtitle: str | None = None,
           format: str | None = None, bitrate: int = 0, bit_depth: int = 0, sample_rate: int = 0, lossless: bool | None = None,
           tracks: int | None = 1, size: int | None = None, duration: float | None = None, isrc: str | None = None,
           cover: str | None = None, info: str = "", score: float | None = None, url: str | None = None) -> dict:
    """One search result in the shape Encore's picker and request worker expect."""
    fmt = (format or "").lower() or None
    ll = is_lossless(fmt) if lossless is None else bool(lossless)
    return {
        "key": key, "title": title, "artist": artist, "album": album,
        "subtitle": subtitle if subtitle is not None else " · ".join(x for x in (artist, album) if x),
        "format": fmt, "bitrate": int(bitrate or 0), "bit_depth": int(bit_depth or 0), "sample_rate": int(sample_rate or 0),
        "quality": quality(fmt, bitrate, bit_depth, sample_rate), "lossless": ll,
        "tracks": tracks, "size": size, "duration": duration, "isrc": isrc, "cover": cover, "info": info,
        "score": score, "url": url, "payload": payload,
    }


def file_entry(path: str, *, title: str | None = None, artist: str | None = None, album: str | None = None,
               track: int | None = None, disc: int | None = None, duration: float | None = None, format: str | None = None,
               size: int | None = None, isrc: str | None = None) -> dict:
    """One produced file, with whatever tags the source knows (Encore tags and verifies it anyway)."""
    if size is None:
        try:
            size = os.path.getsize(path)
        except OSError:
            size = None
    fmt = (format or os.path.splitext(path)[1].lstrip(".")).lower() or None
    return {"path": os.path.abspath(path), "title": title, "artist": artist, "album": album, "track": track, "disc": disc,
            "duration": duration, "format": fmt, "size": size, "isrc": isrc}


# ---- the entry point -----------------------------------------------------------------------------

def run(name: str, version: str, *, handlers: dict, kind: str = "source", kinds=("track", "album"), needs_sign_in: bool = False) -> None:
    global _NAME, _VERSION
    _NAME, _VERSION = name, version
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except (AttributeError, ValueError):
            pass
    try:
        signal.signal(signal.SIGTERM, _on_term)
    except (AttributeError, ValueError, OSError):
        pass
    cmd = sys.argv[1] if len(sys.argv) > 1 else ""
    if cmd == "describe":
        cmds = ["describe"] + [c for c in COMMANDS if c in handlers and c != "describe"]
        _emit({"ok": True, "name": name, "version": version, "protocol": PROTOCOL, "kind": kind, "kinds": list(kinds),
               "commands": cmds, "needs_sign_in": needs_sign_in})
        return
    if cmd not in handlers or cmd not in COMMANDS:
        _emit({"ok": False, "error": f"{name} does not support '{cmd or '(no command)'}'", "code": "unsupported"})
        sys.exit(2)
    try:
        req = _read_request()
        out = handlers[cmd](req) or {}
        if not isinstance(out, dict):
            raise PluginError(f"the {cmd} handler returned {type(out).__name__}, not a dict", "internal")
        out.setdefault("ok", True)
        _emit(out)
    except PluginError as e:
        out = {"ok": False, "error": str(e), "code": e.code}
        if e.retry_after is not None:
            out["retry_after"] = e.retry_after
        _emit(out)
    except (Cancelled, KeyboardInterrupt):
        _emit({"ok": False, "error": "cancelled", "code": "cancelled"})
        sys.exit(130)
    except Exception as e:  # noqa: BLE001 - the JSON envelope is the contract; the traceback goes to the log
        traceback.print_exc(file=sys.stderr)
        _emit({"ok": False, "error": f"{type(e).__name__}: {e}", "code": "internal"})
        sys.exit(1)


def _read_request() -> dict:
    raw = ""
    try:
        if not sys.stdin.isatty():
            raw = sys.stdin.read()
    except (OSError, ValueError):
        raw = ""
    if not raw.strip():
        return {}
    try:
        req = json.loads(raw)
    except ValueError as e:
        raise PluginError(f"the request on stdin is not JSON: {e}", "internal") from e
    if not isinstance(req, dict):
        raise PluginError("the request on stdin must be a JSON object", "internal")
    return req


def _emit(obj: dict) -> None:
    sys.stdout.write(json.dumps(obj, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def _on_term(_sig, _frame):
    raise Cancelled()


def _host(url: str) -> str:
    return urllib.parse.urlsplit(url).hostname or url


def _retry_after(v) -> float:
    try:
        return float(v)
    except (TypeError, ValueError):
        return 5.0


def _rm(path: str) -> None:
    try:
        os.remove(path)
    except OSError:
        pass
