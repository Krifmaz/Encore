#!/usr/bin/env python3
"""Run an Encore plugin the way Encore does, from its folder: protocol 1 (docs/plugins/CONTRACT.md).

    python3 plugin-check.py <plugin folder>                         describe, status and test
    python3 plugin-check.py <folder> --track "Artist|Song|Album"    and a song search
    python3 plugin-check.py <folder> --album "Artist|Album" --download 1 --folder /tmp/encore-check
    python3 plugin-check.py <folder> --query "typed words" --set format=mp3 --only search

It reads plugin.toml and says what Encore would refuse in it (the rules of an uploaded plugin,
docs/contracts/plugin-manifest.md), then starts the plugin as Encore does: `<interpreter> <script> <command>`,
one JSON request on stdin, a cleared environment with ENCORE_PLUGIN_DATA, the manifest's time limits, and
SIGTERM, then SIGKILL after 10 seconds, to its process group when it runs over. Every answer is checked against
the contract: the describe fields, the shape of search results, downloaded files inside the folder.

Exit status 0 when every command it ran answered ok and as the contract says, 1 otherwise (an error answer, a
broken answer, a manifest Encore would refuse), 2 for a usage error.
Python 3.11 or newer, nothing else.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time

try:
    import tomllib
except ModuleNotFoundError:  # Python 3.10 and older
    sys.exit("plugin-check needs Python 3.11 or newer (for tomllib)")

PROTOCOL = 1
COMMANDS = ("describe", "status", "test", "search", "download", "update")
ERROR_CODES = ("auth", "not_found", "network", "rate_limited", "unsupported", "cancelled", "internal")
KINDS = ("source", "client", "tool", "playback", "connection")
REQUEST_KINDS = ("track", "album", "url")
SETTING_TYPES = ("text", "password", "number", "bool", "select", "checks", "textarea", "path", "url")
INTERPRETERS = ("python3", "node", "sh")
DEFAULT_TIMEOUTS = {"describe": 10, "status": 15, "test": 30, "search": 60, "download": 3600, "update": 600}
# the SDK's AUDIO_EXT: the files a download may deliver
AUDIO_EXT = ("flac", "mp3", "m4a", "alac", "ogg", "opus", "wav", "aiff", "aif", "ape", "wv", "aac", "webm")
STDOUT_CAP = 16 * 1024 * 1024
PAYLOAD_CAP = 64 * 1024
KILL_AFTER = 10

# every key Encore reads; anything else is refused (an array of tables is checked per entry)
KNOWN = {
    "plugin": {"id", "name", "version", "description", "kind", "protocol", "homepage", "license", "commands", "kinds",
               "lossless", "needs_sign_in", "min_encore", "builtin", "settings_on"},
    "sources": {"id", "name", "picker"},
    "run": {"script", "interpreter", "shell", "env", "timeouts"},
    "update": {"from", "github", "tag", "asset", "sums", "auto", "pypi", "prerelease", "tags", "pin", "pin_reports"},
    "install": {"verify", "version_from", "timeout", "max_mb", "remove", "check"},
    "binary": {"arch", "asset", "url", "sha256", "sums", "unpack", "bin"},
    "python": {"python", "packages", "bin", "overrides"},
    "download": {"when", "url", "sha256", "sums", "path", "max_mb", "magic"},
    "settings": {"key", "label", "type", "options", "default", "min", "max", "help", "when", "unit", "step", "full",
                 "advanced", "part"},
    "permissions": {"network", "filesystem", "paths", "memory", "cpu", "processes", "open_files"},
    "client": {"source", "protocol", "chosen_by"},
    "migration": {"from_tool", "auto_key", "adopt", "in_use"},
}
RESULT_FIELDS = {
    "key": (str,), "title": (str,), "artist": (str,), "album": (str,), "subtitle": (str,), "format": (str, type(None)),
    "bitrate": (int,), "bit_depth": (int,), "sample_rate": (int,), "quality": (str,), "lossless": (bool,),
    "tracks": (int, type(None)), "size": (int, type(None)), "duration": (int, float, type(None)),
    "isrc": (str, type(None)), "cover": (str, type(None)), "info": (str,), "score": (int, float, type(None)),
    "url": (str, type(None)), "payload": (dict,),
}
ID_RE = re.compile(r"^[a-z0-9](?:[a-z0-9-]{0,38}[a-z0-9])?$")
KEY_RE = re.compile(r"^[a-z][a-z0-9_]{0,63}$")
VERSION_RE = re.compile(r"^[0-9A-Za-z][0-9A-Za-z.+_-]{0,39}$")
ENV_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")
HOST_RE = re.compile(r"^(\*\.)?[a-z0-9][a-z0-9.-]*$")

COLOR = sys.stdout.isatty() and not os.environ.get("NO_COLOR")


def paint(text: str, code: str) -> str:
    return f"\033[{code}m{text}\033[0m" if COLOR else text


def ok(text: str = "ok") -> str:
    return paint(text, "32")


def bad(text: str = "failed") -> str:
    return paint(text, "31")


def faint(text: str) -> str:
    return paint(text, "2")


# ---- the manifest ------------------------------------------------------------------------------------

def find_manifest(folder: str) -> str:
    """plugin.toml at the folder's root, or in its one top folder (as in an unpacked zip)."""
    p = os.path.join(folder, "plugin.toml")
    if os.path.isfile(p):
        return p
    subs = [d for d in os.listdir(folder) if os.path.isdir(os.path.join(folder, d)) and not d.startswith(".")]
    if len(subs) == 1 and os.path.isfile(os.path.join(folder, subs[0], "plugin.toml")):
        return os.path.join(folder, subs[0], "plugin.toml")
    raise SystemExit(f"no plugin.toml in {folder}")


def rel_ok(p) -> bool:
    return isinstance(p, str) and bool(p) and not p.startswith("/") and "\\" not in p and all(
        x not in ("", ".", "..") for x in p.split("/"))


def check_manifest(m: dict, root: str) -> list[str]:
    """What Encore would refuse in an uploaded plugin's manifest (the main rules of manifest.rs)."""
    p: list[str] = []
    for table, value in m.items():
        if table not in KNOWN:
            p.append(f"unknown table [{table}]")
            continue
        entries = value if isinstance(value, list) else [value]
        for e in entries:
            if not isinstance(e, dict):
                p.append(f"[{table}] must be a table")
                continue
            for k in e:
                if k not in KNOWN[table]:
                    p.append(f"[{table}] unknown key {k!r}")
            if table == "run" and isinstance(e.get("timeouts"), dict):
                for c, secs in e["timeouts"].items():
                    if c not in COMMANDS:
                        p.append(f"[run.timeouts] {c!r} is not a command")
                    if not isinstance(secs, int) or isinstance(secs, bool) or not 1 <= secs <= 86400:
                        p.append(f"[run.timeouts] {c}: 1 to 86400 seconds")
    pl = m.get("plugin")
    if not isinstance(pl, dict):
        return p + ["[plugin] is missing"]
    for k in ("id", "name", "version", "kind", "protocol"):
        if k not in pl:
            p.append(f"[plugin] {k} is missing")
    pid = pl.get("id", "")
    if not isinstance(pid, str) or not ID_RE.match(pid):
        p.append(f"[plugin] id {pid!r}: a-z, 0-9 and dashes (at most 40)")
    v = pl.get("version", "")
    if not isinstance(v, str) or not VERSION_RE.match(v) or not any(c.isdigit() for c in v):
        p.append(f"[plugin] version {v!r}: a version number such as 0.1.0")
    if pl.get("kind") not in KINDS:
        p.append(f"[plugin] kind {pl.get('kind')!r}: one of {', '.join(KINDS)}")
    if pl.get("protocol") != PROTOCOL:
        p.append(f"[plugin] protocol {pl.get('protocol')!r}: Encore speaks protocol {PROTOCOL}")
    commands = pl.get("commands", [])
    if not isinstance(commands, list) or any(c not in COMMANDS for c in commands) or len(set(commands)) != len(commands):
        p.append(f"[plugin] commands: each once, from {', '.join(COMMANDS)}")
        commands = []
    if "describe" not in commands:
        p.append("[plugin] commands: a plugin program answers describe")
    kinds = pl.get("kinds", [])
    if pl.get("kind") == "source":
        for c in ("search", "download"):
            if c not in commands:
                p.append(f"[plugin] commands: a source answers {c}")
        if not kinds:
            p.append("[plugin] kinds: a source lists the requests it takes")
    elif kinds:
        p.append("[plugin] kinds: only a source takes requests")
    if any(k not in REQUEST_KINDS for k in kinds):
        p.append(f"[plugin] kinds: from {', '.join(REQUEST_KINDS)}")
    for k in ("builtin", "settings_on"):
        if k in pl:
            p.append(f"[plugin] {k}: only for the catalog's plugins")
    for t in ("migration", "python", "download"):
        if t in m:
            p.append(f"[{t}]: only for the catalog's plugins")
    run = m.get("run") if isinstance(m.get("run"), dict) else {}
    script = run.get("script")
    if not rel_ok(script):
        p.append("[run] script: the program, a path inside the plugin's folder")
    elif not os.path.isfile(os.path.join(root, script)):
        p.append(f"[run] script: {script} is not in the plugin's folder")
    interp = run.get("interpreter")
    if interp is not None and interp not in INTERPRETERS:
        p.append(f"[run] interpreter {interp!r}: one of {', '.join(INTERPRETERS)}, or none")
    if interp is None and rel_ok(script) and os.path.isfile(os.path.join(root, script)) and not os.access(os.path.join(root, script), os.X_OK):
        p.append(f"[run] script: {script} is not executable and no interpreter is named")
    if run.get("shell", False) is not False:
        p.append("[run] shell: must be absent or false")
    for k in (run.get("env") or {}):
        if not ENV_RE.match(k) or k.startswith(("LD_", "ENCORE_")) or k in ("PATH", "HOME", "TMPDIR", "PYTHONPATH", "PYTHONHOME", "NODE_OPTIONS"):
            p.append(f"[run] env {k}: not a variable a plugin may set")
    keys = set()
    settings = m.get("settings", [])
    for s in settings if isinstance(settings, list) else []:
        key = s.get("key", "")
        at = f"[[settings]] {key}"
        if not isinstance(key, str) or not KEY_RE.match(key):
            p.append(f"[[settings]] key {key!r}: a-z, 0-9 and underscores, starting with a letter")
        if key in keys:
            p.append(f"{at}: listed twice")
        keys.add(key)
        if not s.get("label"):
            p.append(f"{at}: label is missing")
        t = s.get("type")
        if t not in SETTING_TYPES:
            p.append(f"{at}: type {t!r} is not one of {', '.join(SETTING_TYPES)}")
            continue
        opts = s.get("options", [])
        if t in ("select", "checks"):
            if not opts or any(not (isinstance(o, list) and len(o) == 2 and all(isinstance(x, str) for x in o)) for o in opts):
                p.append(f"{at}: options are [value, label] pairs of text")
                opts = []
        elif opts:
            p.append(f"{at}: only a select or checks has options")
        if "default" in s:
            d = s["default"]
            values = [o[0] for o in opts]
            if t == "number" and (not isinstance(d, (int, float)) or isinstance(d, bool)):
                p.append(f"{at}: default is a number")
            elif t == "number" and (("min" in s and d < s["min"]) or ("max" in s and d > s["max"])):
                p.append(f"{at}: default is outside min and max")
            elif t == "bool" and not isinstance(d, bool):
                p.append(f"{at}: default is true or false")
            elif t == "select" and str(d) not in values:
                p.append(f"{at}: default {d!r} is not one of the options")
            elif t not in ("number", "bool", "select", "checks") and not isinstance(d, str):
                p.append(f"{at}: default is text")
    perms = m.get("permissions", {})
    net = perms.get("network", False)
    if not (isinstance(net, bool) or (isinstance(net, list) and all(isinstance(h, str) and HOST_RE.match(h) for h in net))):
        p.append("[permissions] network: a list of host names, or true / false")
    if perms.get("filesystem", "folder") != "folder":
        p.append('[permissions] filesystem: "folder"')
    for g in perms.get("paths", []):
        if not (isinstance(g, dict) and g.get("setting") == "library_path" and g.get("access") == "read"):
            p.append('[permissions] paths: an uploaded plugin may only read the library ({ setting = "library_path", access = "read" })')
    up = m.get("update")
    if isinstance(up, dict) and (up.get("github") or up.get("from") == "github"):
        if not re.match(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$", str(up.get("github", ""))):
            p.append("[update] github: owner/repo")
        if str(up.get("tag", "v{version}")).count("{version}") != 1:
            p.append("[update] tag: has {version} once")
        if not up.get("asset") and "binary" not in m:
            p.append("[update] asset: the plugin's zip in each release")
        if up.get("asset") and not up.get("sums"):
            p.append("[update] sums: the zip's sha256 must be checked (SHA256SUMS or \"github\")")
    for b in m.get("binary", []):
        if not re.match(r"^[0-9a-fA-F]{64}$", str(b.get("sha256", ""))):
            p.append(f"[[binary]] {b.get('arch')}: an uploaded plugin's program has a fixed sha256")
    return p


# ---- running a command -------------------------------------------------------------------------------

class Run:
    """One command's run: its answer, its log, its exit status and how long it took."""

    def __init__(self):
        self.answer: dict | None = None
        self.stdout_lines: list[str] = []
        self.log: list[str] = []
        self.progress: list[tuple[float, str]] = []
        self.status: int | None = None
        self.seconds = 0.0
        self.timed_out = False


def program_argv(m: dict, root: str) -> list[str]:
    run = m.get("run", {})
    script = os.path.join(root, run.get("script", ""))
    interp = run.get("interpreter")
    if interp in ("python3", "python"):
        return [sys.executable, script]
    if interp == "node":
        node = shutil.which("node") or shutil.which("nodejs")
        if not node:
            raise SystemExit("the plugin runs with node, which is not installed here")
        return [node, script]
    if interp == "sh":
        return ["/bin/sh", script]
    return [script]


def plugin_env(m: dict, root: str, data: str, scratch: str) -> dict:
    pl = m["plugin"]
    env = {
        "ENCORE_PLUGIN_DATA": data,
        "ENCORE_PLUGIN_ID": pl.get("id", ""),
        "ENCORE_PLUGIN_VERSION": pl.get("version", ""),
        "ENCORE_VERSION": os.environ.get("ENCORE_VERSION", "0.32.0"),
        "ENCORE_PROTOCOL": str(PROTOCOL),
        "PATH": "/usr/local/bin:/usr/bin:/bin",
        "HOME": os.path.join(scratch, "home"),
        "TMPDIR": os.path.join(scratch, "tmp"),
        "XDG_CONFIG_HOME": os.path.join(scratch, "home", ".config"),
        "XDG_CACHE_HOME": os.path.join(scratch, "home", ".cache"),
        "XDG_DATA_HOME": os.path.join(scratch, "home", ".local", "share"),
        "LANG": "C.UTF-8",
        "PYTHONUNBUFFERED": "1",
        "PYTHONDONTWRITEBYTECODE": "1",
        "PYTHONIOENCODING": "utf-8",
    }
    if os.environ.get("TZ"):
        env["TZ"] = os.environ["TZ"]
    if m.get("permissions", {}).get("network", False):
        for k in ("HTTP_PROXY", "HTTPS_PROXY", "NO_PROXY", "http_proxy", "https_proxy", "no_proxy", "SSL_CERT_FILE", "SSL_CERT_DIR"):
            if os.environ.get(k):
                env[k] = os.environ[k]
    for k, v in (m.get("run", {}).get("env") or {}).items():
        env[k] = str(v).replace("{data}", data).replace("{plugin}", root).replace("{bin}", "").replace("{bin_dir}", "")
    for d in (env["HOME"], env["TMPDIR"], env["XDG_CONFIG_HOME"], env["XDG_CACHE_HOME"], env["XDG_DATA_HOME"], data):
        os.makedirs(d, exist_ok=True)
    return env


def stop_group(proc: subprocess.Popen) -> None:
    """SIGTERM to the plugin's process group, SIGKILL when it is still there after KILL_AFTER seconds."""
    try:
        os.killpg(proc.pid, signal.SIGTERM)
    except (ProcessLookupError, PermissionError):
        return
    try:
        proc.wait(KILL_AFTER)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except (ProcessLookupError, PermissionError):
            pass
        proc.wait()


def run_command(argv: list[str], command: str, request: dict | None, env: dict, cwd: str, timeout: int, show_log: bool) -> Run:
    r = Run()
    started = time.monotonic()
    proc = subprocess.Popen(argv + [command], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            env=env, cwd=cwd, start_new_session=True)

    def feed():
        try:
            if request is not None:
                proc.stdin.write(json.dumps(request, ensure_ascii=False).encode("utf-8"))
        except (BrokenPipeError, OSError):
            pass
        finally:
            try:
                proc.stdin.close()
            except OSError:
                pass

    out = bytearray()

    def read_out():
        while chunk := proc.stdout.read(65536):
            if len(out) < STDOUT_CAP:
                out.extend(chunk[: STDOUT_CAP - len(out)])

    last_shown = [-100.0, ""]

    def read_err():
        for raw in iter(proc.stderr.readline, b""):
            line = raw.decode("utf-8", "replace").rstrip("\r\n")
            m = re.match(r"^progress\s+(-?[0-9.]+)(?:\s+(.*))?$", line)
            if m:
                try:
                    pct = max(0.0, min(100.0, float(m.group(1))))
                except ValueError:
                    r.log.append(line)
                    continue
                text = m.group(2) or ""
                r.progress.append((pct, text))
                if show_log and (pct - last_shown[0] >= 10 or pct >= 100 or text != last_shown[1]):
                    last_shown[0], last_shown[1] = pct, text
                    print(faint(f"    progress {pct:5.1f}%  {text}"), flush=True)
                continue
            text = line[4:] if line.startswith("log ") else line
            r.log.append(text)
            if show_log:
                print(faint(f"    log  {text}"), flush=True)

    threads = [threading.Thread(target=f, daemon=True) for f in (feed, read_out, read_err)]
    for t in threads:
        t.start()
    try:
        proc.wait(timeout)
    except subprocess.TimeoutExpired:
        r.timed_out = True
        stop_group(proc)
    for t in threads:
        t.join(KILL_AFTER)
    r.status = proc.returncode
    r.seconds = time.monotonic() - started
    r.stdout_lines = out.decode("utf-8", "replace").splitlines()
    for line in reversed(r.stdout_lines):
        line = line.strip()
        if line.startswith("{"):
            try:
                v = json.loads(line)
            except ValueError:
                continue
            if isinstance(v, dict):
                r.answer = v
                break
    return r


# ---- checking the answers ----------------------------------------------------------------------------

def check_envelope(r: Run, command: str, timeout: int) -> list[str]:
    if r.timed_out:
        return [f"did not answer within {timeout} seconds (stopped with SIGTERM)"]
    if r.answer is None:
        last = r.log[-1] if r.log else "no output"
        return [f"no JSON answer on stdout (exit status {r.status}): {last}"]
    a = r.answer
    p = []
    if not isinstance(a.get("ok"), bool):
        p.append('"ok" is missing or not true/false')
    if a.get("ok") is False:
        if not isinstance(a.get("error"), str) or not a.get("error"):
            p.append('an error answer has no "error" text')
        if a.get("code") not in ERROR_CODES:
            p.append(f'"code" {a.get("code")!r} is not one of {", ".join(ERROR_CODES)}')
        if "retry_after" in a and not isinstance(a["retry_after"], (int, float)):
            p.append('"retry_after" is a number of seconds')
    elif a.get("ok") is True and r.status not in (0, None):
        p.append(f"answered ok but exited with status {r.status}")
    return p


def check_describe(a: dict, m: dict) -> list[str]:
    pl = m["plugin"]
    p = []
    if a.get("protocol") != PROTOCOL:
        p.append(f"protocol {a.get('protocol')!r}, Encore speaks {PROTOCOL}")
    if a.get("name") != pl.get("id"):
        p.append(f"name {a.get('name')!r} is not the manifest's id {pl.get('id')!r}")
    if a.get("version") != pl.get("version"):
        p.append(f"version {a.get('version')!r} is not the manifest's {pl.get('version')!r}")
    if a.get("kind") != pl.get("kind"):
        p.append(f"kind {a.get('kind')!r} is not the manifest's {pl.get('kind')!r}")
    answered = a.get("commands") if isinstance(a.get("commands"), list) else []
    missing = [c for c in pl.get("commands", []) if c not in answered]
    if missing:
        p.append(f"the manifest lists {', '.join(missing)}, which the program does not answer")
    return p


def check_results(a: dict, req: dict) -> list[str]:
    results = a.get("results")
    if not isinstance(results, list):
        return ['"results" is not a list']
    p = []
    keys = set()
    for i, res in enumerate(results, 1):
        if not isinstance(res, dict):
            p.append(f"result {i} is not an object")
            continue
        for f, types in RESULT_FIELDS.items():
            if f not in res:
                if f in ("key", "title", "payload"):
                    p.append(f"result {i}: {f} is missing")
                continue
            if not isinstance(res[f], types) or (bool not in types and isinstance(res[f], bool)):
                p.append(f"result {i}: {f} is {type(res[f]).__name__}")
        if res.get("key") in keys:
            p.append(f"result {i}: key {res.get('key')!r} is used twice")
        keys.add(res.get("key"))
        if isinstance(res.get("score"), (int, float)) and not 0 <= res["score"] <= 100:
            p.append(f"result {i}: score {res['score']} is outside 0 to 100")
        if len(json.dumps(res.get("payload", {}))) > PAYLOAD_CAP:
            p.append(f"result {i}: payload is larger than {PAYLOAD_CAP // 1024} KB")
    if isinstance(req.get("limit"), int) and len(results) > req["limit"]:
        p.append(f"{len(results)} results, more than the limit of {req['limit']}")
    return p


def check_files(a: dict, folder: str) -> list[str]:
    files = a.get("files")
    if not isinstance(files, list) or not files:
        return ['"files" is missing or empty']
    p = []
    root = os.path.realpath(folder)
    for i, f in enumerate(files, 1):
        path = f.get("path") if isinstance(f, dict) else None
        if not isinstance(path, str) or not path:
            p.append(f"file {i}: no path")
            continue
        full = os.path.realpath(path if os.path.isabs(path) else os.path.join(folder, path))
        if os.path.commonpath([root, full]) != root:
            p.append(f"file {i}: {path} is outside the download folder")
        elif not os.path.isfile(full):
            p.append(f"file {i}: {path} is not there")
        elif os.path.splitext(full)[1].lstrip(".").lower() not in AUDIO_EXT:
            p.append(f"file {i}: {os.path.basename(full)} is not an audio file")
        elif full.endswith(".part"):
            p.append(f"file {i}: {path} is a .part file")
    for k in ("cover_url", "source_url", "message"):
        if k in a and a[k] is not None and not isinstance(a[k], str):
            p.append(f'"{k}" is not text')
    return p


# ---- output ------------------------------------------------------------------------------------------

def size_text(n) -> str:
    if not isinstance(n, (int, float)):
        return ""
    for unit in ("B", "KB", "MB", "GB"):
        if n < 1024 or unit == "GB":
            return f"{n:.0f} {unit}" if unit == "B" else f"{n:.1f} {unit}"
        n /= 1024
    return ""


def time_text(s) -> str:
    if not isinstance(s, (int, float)):
        return ""
    s = int(round(s))
    return f"{s // 3600}:{s // 60 % 60:02d}:{s % 60:02d}" if s >= 3600 else f"{s // 60}:{s % 60:02d}"


def print_results(results: list) -> None:
    for i, r in enumerate(results, 1):
        if not isinstance(r, dict):
            continue
        score = r.get("score")
        line = " · ".join(x for x in (r.get("title") or "", r.get("artist") or "", r.get("album") or "") if x)
        extra = "  ".join(x for x in (r.get("quality") or "", size_text(r.get("size")), time_text(r.get("duration")),
                                     f"{r['tracks']} files" if isinstance(r.get("tracks"), int) and r["tracks"] > 1 else "") if x)
        print(f"  {i:>3}  {score if score is not None else '':>5}  {line}")
        if extra or r.get("info"):
            print(faint(f"              {extra}{'  ' if extra and r.get('info') else ''}{r.get('info') or ''}"))


def report(command: str, r: Run, problems: list[str], summary: str) -> None:
    a = r.answer or {}
    if problems:
        status = bad()
    elif a.get("ok") is False:
        status = paint(f"error {a.get('code')}", "33")
    else:
        status = ok()
    print(f"{command:<10} {status}  {faint(f'{r.seconds:.1f} s')}  {summary}")
    for p in problems:
        print(bad(f"    ! {p}"))
    if (problems or a.get("ok") is False) and r.log:
        for line in r.log[-5:]:
            print(faint(f"    log  {line}"))


# ---- main --------------------------------------------------------------------------------------------

def parse_value(raw: str, setting: dict | None):
    t = (setting or {}).get("type")
    if t == "number":
        try:
            return int(raw)
        except ValueError:
            return float(raw)
    if t == "bool":
        return raw.strip().lower() in ("1", "true", "yes", "on")
    return raw


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Run an Encore plugin the way Encore does (protocol 1).")
    ap.add_argument("plugin", help="the plugin's folder (with plugin.toml)")
    ap.add_argument("--track", metavar="ARTIST|TITLE[|ALBUM]", help="search for a song")
    ap.add_argument("--album", metavar="ARTIST|ALBUM", help="search for an album")
    ap.add_argument("--query", help="search for typed words, as the picker does")
    ap.add_argument("--url", help="a pasted link (kind url)")
    ap.add_argument("--length", type=float, help="the song's length in seconds")
    ap.add_argument("--tracks", type=int, help="the album's number of songs")
    ap.add_argument("--isrc", help="the song's ISRC")
    ap.add_argument("--lossless-only", action="store_true", help="only lossless results")
    ap.add_argument("--limit", type=int, default=25, help="the most results wanted (25)")
    ap.add_argument("--download", type=int, metavar="N", help="download result N of the search")
    ap.add_argument("--folder", help="where the download goes (a new temporary folder when not given)")
    ap.add_argument("--data", help="ENCORE_PLUGIN_DATA (default: data/ in the plugin's folder)")
    ap.add_argument("--set", action="append", default=[], metavar="KEY=VALUE", help="a setting (the manifest's defaults otherwise)")
    ap.add_argument("--settings", metavar="FILE", help="settings from a JSON file")
    ap.add_argument("--only", help="comma-separated commands to run (describe,status,test,search,download,update)")
    ap.add_argument("--json", action="store_true", help="print the raw answers")
    ap.add_argument("--quiet", action="store_true", help="no log and progress lines while it runs")
    args = ap.parse_args(argv)

    if not os.path.isdir(args.plugin):
        print(f"{args.plugin} is not a folder", file=sys.stderr)
        return 2
    path = find_manifest(args.plugin)
    root = os.path.dirname(os.path.abspath(path))
    try:
        with open(path, "rb") as f:
            m = tomllib.load(f)
    except (OSError, tomllib.TOMLDecodeError) as e:
        print(f"plugin.toml   {bad()}  {e}")
        return 1
    problems = check_manifest(m, root)
    pl = m.get("plugin", {})
    print(f"plugin.toml   {bad() if problems else ok()}  {pl.get('id')} {pl.get('version')} ({pl.get('kind')}, protocol {pl.get('protocol')})")
    for p in problems:
        print(bad(f"    ! {p}"))
    if not isinstance(m.get("run"), dict) or not m["run"].get("script"):
        return 1
    failed = bool(problems)

    # the settings Encore would send: the defaults, then the file, then --set
    by_key = {s.get("key"): s for s in m.get("settings", []) if isinstance(s, dict)}
    settings = {k: s["default"] for k, s in by_key.items() if "default" in s}
    if args.settings:
        with open(args.settings, encoding="utf-8") as f:
            settings.update(json.load(f))
    for item in args.set:
        if "=" not in item:
            print(f"--set {item}: KEY=VALUE", file=sys.stderr)
            return 2
        k, v = item.split("=", 1)
        if k not in by_key:
            print(faint(f"    note: {k} is not a setting of this plugin"))
        settings[k] = parse_value(v, by_key.get(k))

    search_req = None
    if args.track:
        parts = args.track.split("|")
        if len(parts) < 2:
            print("--track: ARTIST|TITLE[|ALBUM]", file=sys.stderr)
            return 2
        search_req = {"kind": "track", "artist": parts[0].strip(), "title": parts[1].strip(), "album": (parts[2] if len(parts) > 2 else "").strip(), "query": ""}
    elif args.album:
        parts = args.album.split("|")
        if len(parts) != 2:
            print("--album: ARTIST|ALBUM", file=sys.stderr)
            return 2
        search_req = {"kind": "album", "artist": parts[0].strip(), "title": parts[1].strip(), "album": parts[1].strip(), "query": ""}
    elif args.query:
        search_req = {"kind": "track", "artist": "", "title": "", "album": "", "query": args.query}
    elif args.url:
        search_req = {"kind": "url", "artist": "", "title": "", "album": "", "query": "", "url": args.url}
    if search_req is not None:
        search_req.update({"length": args.length, "track_count": args.tracks, "lossless_only": args.lossless_only, "limit": args.limit})
        if args.isrc:
            search_req["isrc"] = args.isrc

    commands = [c for c in pl.get("commands", []) if c in COMMANDS]
    wanted = ["describe", "status", "test"]
    if search_req is not None:
        wanted.append("search")
    if args.download:
        wanted.append("download")
    if args.only:
        wanted = [c.strip() for c in args.only.split(",") if c.strip()]
        bad_names = [c for c in wanted if c not in COMMANDS]
        if bad_names:
            print(f"--only: {', '.join(bad_names)} is not a command", file=sys.stderr)
            return 2
    if "download" in wanted and "search" not in wanted:
        print("--download needs a search (--track, --album, --query or --url)", file=sys.stderr)
        return 2
    if "search" in wanted and search_req is None:
        print("a search needs --track, --album, --query or --url", file=sys.stderr)
        return 2

    data = os.path.abspath(args.data or os.path.join(root, "data"))
    scratch = tempfile.mkdtemp(prefix="plugin-check-")
    timeouts = {**DEFAULT_TIMEOUTS, **(m.get("run", {}).get("timeouts") or {})}
    try:
        argv0 = program_argv(m, root)
        env = plugin_env(m, root, data, scratch)
        results: list = []
        for command in wanted:
            if command not in commands and command != "describe":
                print(f"{command:<10} {faint('skipped')}  the manifest does not list it")
                continue
            if command == "describe":
                req = None
            elif command == "search":
                req = {**search_req, "settings": settings}
            elif command == "download":
                if not 1 <= args.download <= len(results):
                    print(f"download   {bad()}  there is no result {args.download} ({len(results)} found)")
                    failed = True
                    continue
                pick = results[args.download - 1]
                folder = os.path.abspath(args.folder or tempfile.mkdtemp(prefix="plugin-download-"))
                os.makedirs(folder, exist_ok=True)
                if os.listdir(folder):
                    print(faint(f"    note: {folder} is not empty (Encore gives a plugin an empty folder)"))
                req = {"kind": search_req["kind"], "artist": search_req.get("artist", ""), "title": search_req.get("title", ""),
                       "album": search_req.get("album", ""), "key": pick.get("key"), "payload": pick.get("payload", {}),
                       "folder": folder, "settings": settings}
            elif command == "update":
                req = {"settings": settings, "version": pl.get("version")}
            else:
                req = {"settings": settings}
            r = run_command(argv0, command, req, env, data, timeouts.get(command, 60), not args.quiet)
            problems = check_envelope(r, command, timeouts.get(command, 60))
            a = r.answer or {}
            summary = a.get("error", "") if a.get("ok") is False else ""
            if not problems and a.get("ok"):
                if command == "describe":
                    problems = check_describe(a, m)
                    summary = f"{a.get('name')} {a.get('version')}, commands {' '.join(a.get('commands') or [])}"
                elif command == "search":
                    problems = check_results(a, req)
                    results = [x for x in a.get("results", []) if isinstance(x, dict)]
                    summary = f"{len(results)} result{'s' if len(results) != 1 else ''}"
                elif command == "download":
                    problems = check_files(a, req["folder"])
                    n = len(a.get("files") or [])
                    summary = f"{n} file{'s' if n != 1 else ''} in {req['folder']}" + (f" · {a['message']}" if a.get("message") else "")
                else:
                    if not isinstance(a.get("message", ""), str):
                        problems = ['"message" is not text']
                    summary = str(a.get("message", ""))
            elif command == "describe" and a.get("ok") is False:
                problems.append("describe must answer: Encore runs it at install")
            report(command, r, problems, summary)
            if args.json and r.answer is not None:
                print(json.dumps(r.answer, ensure_ascii=False, indent=2))
            if command == "search" and results and not problems:
                print_results(results)
            if command == "download" and a.get("ok") and not problems:
                for f in a.get("files", []):
                    print(f"    {f.get('path')}  {faint(size_text(f.get('size')))}")
            if problems or (r.answer or {}).get("ok") is not True:
                failed = True
    finally:
        shutil.rmtree(scratch, ignore_errors=True)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
