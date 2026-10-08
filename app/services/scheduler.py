"""Scheduled tasks: one place to switch background jobs on/off, set their intervals and run them now."""
import asyncio
import time
import traceback

from .. import config, db

# key, name, description, enabled getter/setter, minutes getter/setter
def _flag(key):
    return (lambda: bool(config.get(key)), lambda v: config.set_many({key: bool(v)}))


def _minutes(key, scale=1):
    return (lambda: int(config.get(key)) * scale, lambda v: config.set_many({key: max(1, int(v) // scale)}))


def _interval_as_switch(key, scale=1, default=60):
    """Old settings where 0 meant "off": enabled = interval > 0."""
    def get_on():
        return int(config.get(key)) > 0

    def set_on(v):
        if not v:
            config.set_many({key: 0})
        elif int(config.get(key)) <= 0:
            config.set_many({key: default // scale})
    return get_on, set_on


TASKS = [
    {"key": "scan", "name": "Scan the library", "desc": "Pick up files added, changed or removed outside Encore.",
     "on": _interval_as_switch("scan_interval_minutes"), "every": _minutes("scan_interval_minutes")},
    {"key": "sync", "name": "Sync likes & playlists", "desc": "Two-way liked songs and playlist sync with Jellyfin, Plex, Navidrome, ListenBrainz, Spotify…",
     "on": _interval_as_switch("sync_interval_minutes", default=15), "every": _minutes("sync_interval_minutes")},
    {"key": "follows", "name": "Check followed artists", "desc": "Look for new releases from artists on the Following page and request them.",
     "on": _interval_as_switch("follow_check_hours", 60, 720), "every": _minutes("follow_check_hours", 60)},
    {"key": "lossy", "name": "Lossless upgrades", "desc": "Re-check albums you only have as MP3/YouTube rips for a lossless copy.",
     "on": _flag("upgrade_lossy"), "every": _minutes("task_lossy_minutes")},
    {"key": "discover", "name": "Auto-discover", "desc": "Request the top songs of each user's ListenBrainz Weekly Exploration (count set in Auto-download).",
     "on": (lambda: int(config.get("discover_auto_count") or 0) > 0,
            lambda v: config.set_many({"discover_auto_count": (int(config.get("discover_auto_count") or 0) or 3) if v else 0})),
     "every": _minutes("task_discover_minutes")},
    {"key": "lyrics", "name": "Fetch missing lyrics", "desc": "Download lyrics (.lrc) from LRCLIB for songs that don't have any yet.",
     "on": _flag("task_lyrics_enabled"), "every": _minutes("task_lyrics_minutes")},
    {"key": "artwork", "name": "Find missing artwork", "desc": "Look up and embed cover art for albums without any.",
     "on": _flag("task_artwork_enabled"), "every": _minutes("task_artwork_minutes")},
    {"key": "retag", "name": "Re-tag albums added to MusicBrainz", "desc": "Albums tagged from Deezer/Apple/Discogs get proper MusicBrainz tags once they appear there.",
     "on": _flag("task_retag_enabled"), "every": _minutes("task_retag_minutes")},
    {"key": "verify", "name": "Verify audio fingerprints", "desc": "Fingerprint unchecked songs with AcoustID and flag wrong audio.",
     "on": _flag("task_verify_enabled"), "every": _minutes("task_verify_minutes")},
    {"key": "cleanup", "name": "Clean up", "desc": "Remove finished downloads older than the cleanup age from the list and delete leftover staging files.",
     "on": _flag("task_cleanup_enabled"), "every": _minutes("task_cleanup_minutes")},
]
BY_KEY = {t["key"]: t for t in TASKS}
_running: dict[str, asyncio.Task] = {}
_boot = time.time()


async def _cleanup() -> str:
    import os
    import shutil
    days = int(config.get("task_cleanup_days") or 14)
    cutoff = time.time() - days * 86400
    n = db.one("SELECT COUNT(*) n FROM requests WHERE status IN ('completed','denied','cancelled') AND updated_at < ?", (cutoff,))["n"]
    db.execute("DELETE FROM requests WHERE status IN ('completed','denied','cancelled') AND updated_at < ?", (cutoff,))
    stage = config.get("staging_path")
    removed = 0
    active = {f"request-{r['id']}" for r in db.query("SELECT id FROM requests WHERE status IN ('queued','searching','downloading','monitoring','importing')")}
    for name in os.listdir(stage) if os.path.isdir(stage) else []:
        p = os.path.join(stage, name)
        if name.startswith("request-") and name not in active and os.path.getmtime(p) < time.time() - 3600:
            shutil.rmtree(p, ignore_errors=True)
            removed += 1
    return f"{removed} leftover download folder(s) removed" + (f", {n} old request(s) cleared" if n else "")


async def _run(key: str) -> str:
    from .. import worker
    if key == "scan":
        from . import library
        if library.state["scanning"]:
            return "already scanning"
        r = await library.scan()
        return (r or {}).get("message") or "done"
    if key == "sync":
        from . import sync
        await sync.sync_all()
        return "done"
    if key == "follows":
        from . import follows
        return f"{await follows.check_all()} new release(s)"
    if key == "lossy":
        return f"{await worker.queue_lossy_upgrades()} upgrade(s) queued"
    if key == "discover":
        from . import discovery
        return f"{await discovery.auto_discover()} song(s) requested"
    if key == "lyrics":
        from . import lyrics
        r = await lyrics.fetch_missing(limit=150)
        return f"lyrics found for {r['found']} of {r['checked']}"
    if key == "artwork":
        from ..routers import library_routes
        return await library_routes.find_missing_artwork()
    if key == "retag":
        return f"{await worker.retag_external()} album(s) re-tagged"
    if key == "verify":
        from . import acoustid, verify
        if not acoustid.available():
            raise RuntimeError("needs an AcoustID key and fpcalc")
        if verify.state["running"]:
            return "already running"
        await verify.verify_library(only_new=True)
        return verify.state.get("message") or f"{verify.state.get('verified', 0)} verified, {verify.state.get('mismatch', 0)} wrong"
    if key == "cleanup":
        return await _cleanup()
    raise ValueError(key)


def _state(key: str) -> dict:
    return db.one("SELECT * FROM task_runs WHERE key=?", (key,)) or {}


def run_now(key: str) -> bool:
    if key not in BY_KEY:
        raise KeyError(key)
    t = _running.get(key)
    if t and not t.done():
        return False

    async def go():
        start = time.time()
        db.execute("INSERT OR REPLACE INTO task_runs (key, last_run, duration, result, error) VALUES (?,?,?,?,?)",
                   (key, start, None, "running…", None))
        try:
            res, err = await _run(key), None
        except Exception as e:  # noqa: BLE001
            traceback.print_exc()
            res, err = None, str(e)[:300]
        db.execute("UPDATE task_runs SET duration=?, result=?, error=? WHERE key=?", (time.time() - start, res, err, key))
    _running[key] = asyncio.create_task(go())
    return True


def tick() -> None:
    """Called by the worker loop every few seconds: start every task that is due."""
    now = time.time()
    for t in TASKS:
        if not t["on"][0]():
            continue
        every = max(1, t["every"][0]()) * 60
        last = _state(t["key"]).get("last_run") or (_boot - every + _first_delay(t["key"]))
        if now - last >= every:
            run_now(t["key"])


def _first_delay(key: str) -> float:
    """After a restart, stagger the first runs instead of starting everything at once."""
    return {"scan": 30, "sync": 120, "follows": 600, "cleanup": 900}.get(key, 1800)


def listing() -> list[dict]:
    out = []
    for t in TASKS:
        st = _state(t["key"])
        every = t["every"][0]()
        running = bool(_running.get(t["key"]) and not _running[t["key"]].done())
        last = st.get("last_run")
        out.append({"key": t["key"], "name": t["name"], "desc": t["desc"], "enabled": t["on"][0](), "minutes": every,
                    "last_run": last, "duration": st.get("duration"), "result": st.get("result"), "error": st.get("error"),
                    "running": running, "next_run": (last or _boot) + every * 60 if t["on"][0]() else None,
                    "locked": any(config.env_locked(k) for k in _keys(t["key"]))})
    return out


def _keys(key: str) -> list[str]:
    return {"scan": ["scan_interval_minutes"], "sync": ["sync_interval_minutes"], "follows": ["follow_check_hours"],
            "lossy": ["upgrade_lossy"], "discover": ["discover_auto_count"]}.get(key, [])


def update(key: str, enabled: bool | None = None, minutes: int | None = None) -> dict:
    t = BY_KEY[key]
    if minutes is not None:
        t["every"][1](max(5, min(int(minutes), 60 * 24 * 30)))
    if enabled is not None:
        t["on"][1](enabled)
    return next(x for x in listing() if x["key"] == key)
