"""Built-in services (all-in-one image): slskd, Lidarr, Prowlarr, qBittorrent and SABnzbd run inside
the Encore container, configured and wired together automatically, so nothing else needs installing.

  ENCORE_EMBEDDED=all            (or a list: slskd,lidarr,prowlarr,qbittorrent,sabnzbd)

Each service listens on 127.0.0.1 only; its web UI is reachable through Encore at /svc/<name>/
for admins. Data lives in <config>/services/<name>. Wiring done on first start:
  • Encore → slskd and Lidarr (URLs + API keys)
  • Lidarr → /music root folder, qBittorrent + SABnzbd as download clients
  • Prowlarr → Lidarr (indexers you add in Prowlarr sync to Lidarr)
What's left for you: Soulseek login, indexers in Prowlarr, a Usenet server in SABnzbd.
"""
import asyncio
import configparser
import os
import re
import secrets
import shutil
import sys
import time
import xml.etree.ElementTree as ET

from .. import config, db

ROOT = os.environ.get("ENCORE_EMBEDDED_ROOT", "/opt/encore-services")
SERVICES = {
    "slskd": {"name": "slskd", "what": "Soulseek client", "port": 5030, "bin": "slskd/slskd"},
    "lidarr": {"name": "Lidarr", "what": "album monitoring (torrents & Usenet via Prowlarr)", "port": 8686, "bin": "lidarr/Lidarr/Lidarr"},
    "prowlarr": {"name": "Prowlarr", "what": "indexer manager for Lidarr", "port": 9696, "bin": "prowlarr/Prowlarr/Prowlarr"},
    "qbittorrent": {"name": "qBittorrent", "what": "torrent client", "port": 8090, "bin": "bin/qbittorrent-nox"},
    "sabnzbd": {"name": "SABnzbd", "what": "Usenet client", "port": 8085, "bin": "sabnzbd/SABnzbd.py"},
}
ORDER = ["qbittorrent", "sabnzbd", "slskd", "lidarr", "prowlarr"]
state: dict[str, dict] = {}
_procs: dict[str, asyncio.subprocess.Process] = {}
_tasks: list[asyncio.Task] = []
_stopping = False


def wanted() -> list[str]:
    raw = (os.environ.get("ENCORE_EMBEDDED") or "").strip().lower()
    if not raw or raw in ("0", "false", "no", "off"):
        return []
    names = list(SERVICES) if raw in ("1", "all", "true", "yes", "on") else [n.strip() for n in raw.split(",")]
    return [n for n in ORDER if n in names]


def base(name: str) -> str:
    return f"/svc/{name}"


def data_dir(name: str) -> str:
    d = os.path.join(db.DATA_DIR, "services", name)
    os.makedirs(d, exist_ok=True)
    return d


def binary(name: str) -> str | None:
    p = os.path.join(ROOT, SERVICES[name]["bin"])
    return p if os.path.exists(p) else None


def url(name: str) -> str:
    return f"http://127.0.0.1:{SERVICES[name]['port']}{base(name) if name != 'qbittorrent' else ''}"


def _downloads() -> str:
    return os.environ.get("ENCORE_DOWNLOADS_PATH", "/downloads")


# ---- config files -----------------------------------------------------------------------------

def _xml_config(name: str) -> str:
    """Lidarr / Prowlarr config.xml: localhost only, auth handled by Encore (External), fixed API key."""
    path = os.path.join(data_dir(name), "config.xml")
    root = ET.parse(path).getroot() if os.path.exists(path) else ET.Element("Config")
    want = {"BindAddress": "127.0.0.1", "Port": str(SERVICES[name]["port"]), "UrlBase": base(name), "EnableSsl": "False",
            "LaunchBrowser": "False", "AuthenticationMethod": "External", "AuthenticationRequired": "DisabledForLocalAddresses",
            "UpdateMechanism": "Docker", "AnalyticsEnabled": "False", "Branch": "master", "LogLevel": "info", "InstanceName": SERVICES[name]["name"]}
    for k, v in want.items():
        el = root.find(k)
        if el is None:
            el = ET.SubElement(root, k)
        el.text = v
    key = root.find("ApiKey")
    if key is None or not (key.text or "").strip():
        key = key if key is not None else ET.SubElement(root, "ApiKey")
        key.text = secrets.token_hex(16)
    ET.ElementTree(root).write(path, encoding="utf-8", xml_declaration=False)
    return key.text


def _slskd_config() -> str:
    path = os.path.join(data_dir("slskd"), "slskd.yml")
    key = None
    if os.path.exists(path):
        m = re.search(r"^\s+key:\s*(\S+)", open(path).read(), re.M)
        key = m.group(1) if m else None
    key = key or secrets.token_hex(24)
    user = config.get("embedded_slsk_username") or ""
    pw = config.get("embedded_slsk_password") or ""
    dl = os.path.join(_downloads(), "slskd")
    yml = f"""# written by Encore – edit Soulseek login in Encore → Settings → Built-in services
web:
  port: {SERVICES['slskd']['port']}
  ip_address: 127.0.0.1
  url_base: {base('slskd')}
  https:
    disabled: true
  authentication:
    disabled: true
    api_keys:
      encore:
        key: {key}
        role: administrator
        cidr: 127.0.0.1/32,::1/128
directories:
  downloads: {dl}
  incomplete: {dl}-incomplete
shares:
  directories:
    - {config.get('library_path')}
soulseek:
  listen_port: {int(config.get('embedded_slsk_port') or 50300)}
"""
    if user and pw:
        yml += f"  username: {_yaml_str(user)}\n  password: {_yaml_str(pw)}\n"
    with open(path, "w") as fh:
        fh.write(yml)
    os.makedirs(dl, exist_ok=True)
    os.makedirs(dl + "-incomplete", exist_ok=True)
    return key


def _yaml_str(s: str) -> str:
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def _qbit_config() -> None:
    d = os.path.join(data_dir("qbittorrent"), "qBittorrent", "config")
    os.makedirs(d, exist_ok=True)
    path = os.path.join(d, "qBittorrent.conf")
    cp = configparser.RawConfigParser()
    cp.optionxform = str
    if os.path.exists(path):
        cp.read(path)
    tor = os.path.join(_downloads(), "torrents")
    for sec, vals in {
        "LegalNotice": {"Accepted": "true"},
        "Preferences": {"WebUI\\Address": "127.0.0.1", "WebUI\\Port": str(SERVICES["qbittorrent"]["port"]), "WebUI\\LocalHostAuth": "false",
                        "WebUI\\HostHeaderValidation": "false", "WebUI\\CSRFProtection": "false", "WebUI\\ClickjackingProtection": "false",
                        "WebUI\\ReverseProxySupportEnabled": "true", "WebUI\\TrustedReverseProxiesList": "127.0.0.1",
                        "Downloads\\SavePath": tor, "Downloads\\TempPathEnabled": "true", "Downloads\\TempPath": tor + "/incomplete"},
        "BitTorrent": {"Session\\DefaultSavePath": tor, "Session\\TempPathEnabled": "true", "Session\\TempPath": tor + "/incomplete",
                       "Session\\Port": str(int(config.get("embedded_torrent_port") or 6881))},
    }.items():
        if not cp.has_section(sec):
            cp.add_section(sec)
        for k, v in vals.items():
            cp.set(sec, k, v)
    with open(path, "w") as fh:
        cp.write(fh, space_around_delimiters=False)
    os.makedirs(tor + "/incomplete", exist_ok=True)


def _sab_config() -> str:
    path = os.path.join(data_dir("sabnzbd"), "sabnzbd.ini")
    key = None
    if os.path.exists(path):
        m = re.search(r"^api_key\s*=\s*(\S+)", open(path).read(), re.M)
        key = m.group(1) if m else None
    if not key:
        key = secrets.token_hex(16)
        u = os.path.join(_downloads(), "usenet")
        with open(path, "w") as fh:
            fh.write(f"""__version__ = 19
__encoding__ = utf-8
[misc]
host = 127.0.0.1
port = {SERVICES['sabnzbd']['port']}
url_base = {base('sabnzbd')}
api_key = {key}
nzb_key = {secrets.token_hex(16)}
download_dir = {u}/incomplete
complete_dir = {u}/complete
host_whitelist = 127.0.0.1, localhost
inet_exposure = 4
check_new_rel = 0
auto_browser = 0
[categories]
[[*]]
name = *
dir = ""
[[music]]
name = music
dir = music
""")
        os.makedirs(f"{u}/incomplete", exist_ok=True)
        os.makedirs(f"{u}/complete", exist_ok=True)
    return key


def _command(name: str) -> list[str]:
    b = binary(name)
    d = data_dir(name)
    if name == "slskd":
        return [b, "--app-dir", d, "--config", os.path.join(d, "slskd.yml")]
    if name in ("lidarr", "prowlarr"):
        return [b, "-nobrowser", f"-data={d}"]
    if name == "qbittorrent":
        return [b, f"--profile={d}", "--confirm-legal-notice"]
    py = os.path.join(ROOT, "sabnzbd", "venv", "bin", "python")
    return [py if os.path.exists(py) else sys.executable, "-OO", b, "-f", os.path.join(d, "sabnzbd.ini"), "-b", "0", "--disable-file-log"]


def prepare(name: str) -> dict:
    """Write config files and point Encore at the service. Returns the keys Encore needs."""
    keys = {}
    if name == "slskd":
        keys["slskd"] = _slskd_config()
        config.set_many({"slskd_enabled": True, "slskd_url": url("slskd"), "slskd_api_key": keys["slskd"],
                         "slskd_download_path": os.path.join(_downloads(), "slskd")})
    elif name in ("lidarr", "prowlarr"):
        keys[name] = _xml_config(name)
        if name == "lidarr":
            config.set_many({"lidarr_enabled": True, "lidarr_url": url("lidarr"), "lidarr_api_key": keys["lidarr"]})
    elif name == "qbittorrent":
        _qbit_config()
    elif name == "sabnzbd":
        keys["sabnzbd"] = _sab_config()
    return keys


# ---- running ----------------------------------------------------------------------------------

async def _healthy(name: str, timeout: float = 120) -> bool:
    from .http import client
    path = {"slskd": "/api/v0/application", "lidarr": "/ping", "prowlarr": "/ping", "qbittorrent": "/api/v2/app/version",
            "sabnzbd": "/api?mode=version&output=json"}[name]
    headers = {"X-API-Key": config.get("slskd_api_key")} if name == "slskd" else {}
    end = time.time() + timeout
    while time.time() < end and not _stopping:
        p = _procs.get(name)
        if p and p.returncode is not None:
            return False
        try:
            r = await client().get(url(name) + path, headers=headers, timeout=5)
            if r.status_code < 500:
                return True
        except Exception:  # noqa: BLE001
            pass
        await asyncio.sleep(1.5)
    return False


async def _supervise(name: str) -> None:
    st = state.setdefault(name, {})
    backoff = 2
    while not _stopping:
        try:
            prepare(name)
        except Exception as e:  # noqa: BLE001
            st.update(status="error", error=f"config: {e}")
            db.log("error", f"Built-in {name}: could not write its config: {e}")
            return
        log = open(os.path.join(data_dir(name), "encore-supervisor.log"), "ab")
        env = {**os.environ, "HOME": data_dir(name), "TMPDIR": data_dir(name)}
        try:
            proc = await asyncio.create_subprocess_exec(*_command(name), stdout=log, stderr=asyncio.subprocess.STDOUT, env=env, cwd=data_dir(name))
        except OSError as e:
            st.update(status="error", error=str(e))
            db.log("error", f"Built-in {name} failed to start: {e}")
            return
        _procs[name] = proc
        st.update(status="starting", pid=proc.pid, started=time.time(), error=None)
        if await _healthy(name):
            st.update(status="running")
            backoff = 2
            asyncio.create_task(_wire_when_ready())
        rc = await proc.wait()
        log.close()
        if _stopping:
            break
        st.update(status="restarting", error=f"exited with code {rc}", restarts=st.get("restarts", 0) + 1)
        db.log("warning", f"Built-in {name} exited ({rc}); restarting in {backoff}s")
        await asyncio.sleep(backoff)
        backoff = min(backoff * 2, 300)


async def start() -> None:
    global _stopping
    _stopping = False
    for name in wanted():
        if not binary(name):
            state[name] = {"status": "missing", "error": f"not installed in this image ({os.path.join(ROOT, SERVICES[name]['bin'])}) – use the :aio image"}
            continue
        _tasks.append(asyncio.create_task(_supervise(name)))


async def stop() -> None:
    global _stopping
    _stopping = True
    for name, p in list(_procs.items()):
        if p.returncode is None:
            p.terminate()
    for name, p in list(_procs.items()):
        try:
            await asyncio.wait_for(p.wait(), 20)
        except asyncio.TimeoutError:
            p.kill()
    for t in _tasks:
        t.cancel()
    _tasks.clear()


async def restart(name: str) -> None:
    p = _procs.get(name)
    if p and p.returncode is None:
        p.terminate()      # the supervisor starts it again


def listing() -> list[dict]:
    out = []
    for name in SERVICES:
        on = name in wanted()
        st = state.get(name, {})
        out.append({"key": name, **{k: SERVICES[name][k] for k in ("name", "what", "port")}, "enabled": on,
                    "installed": bool(binary(name)), "status": st.get("status") if on else "off", "error": st.get("error"),
                    "pid": st.get("pid"), "uptime": time.time() - st["started"] if st.get("status") == "running" and st.get("started") else None,
                    "restarts": st.get("restarts", 0), "ui": f"{base(name)}/" if on else None, "wired": st.get("wired")})
    return out


def logs(name: str, lines: int = 200) -> str:
    p = os.path.join(data_dir(name), "encore-supervisor.log")
    try:
        with open(p, "rb") as fh:
            fh.seek(0, 2)
            fh.seek(max(0, fh.tell() - 64 * 1024))
            return "\n".join(fh.read().decode("utf-8", "replace").splitlines()[-lines:])
    except OSError:
        return ""


# ---- wiring the services together ----------------------------------------------------------------

_wire_lock: asyncio.Lock | None = None


async def _wire_when_ready() -> None:
    global _wire_lock
    _wire_lock = _wire_lock or asyncio.Lock()
    async with _wire_lock:
        await wire()


def _schema_fill(schema: dict, values: dict) -> dict:
    for f in schema.get("fields", []):
        if f.get("name") in values:
            f["value"] = values[f["name"]]
    return schema


async def wire() -> dict:
    """Connect Lidarr ↔ download clients ↔ Prowlarr. Safe to run again: existing entries are kept."""
    from .http import client
    done = {}
    ws = wanted()
    if "lidarr" in ws and state.get("lidarr", {}).get("status") == "running":
        h = {"X-Api-Key": _xml_config("lidarr")}
        api = url("lidarr") + "/api/v1"
        try:
            lib = config.get("library_path")
            roots = (await client().get(f"{api}/rootfolder", headers=h)).json()
            if not any(r.get("path", "").rstrip("/") == lib.rstrip("/") for r in roots):
                qp = (await client().get(f"{api}/qualityprofile", headers=h)).json()
                mp = (await client().get(f"{api}/metadataprofile", headers=h)).json()
                r = await client().post(f"{api}/rootfolder", headers=h, json={"name": "Music", "path": lib, "defaultQualityProfileId": qp[0]["id"] if qp else 1,
                                                                             "defaultMetadataProfileId": mp[0]["id"] if mp else 1, "defaultMonitorOption": "none",
                                                                             "defaultNewItemMonitorOption": "none", "defaultTags": []})
                done["root_folder"] = r.status_code < 300
            have = {c.get("implementation") for c in (await client().get(f"{api}/downloadclient", headers=h)).json()}
            schemas = {s.get("implementation"): s for s in (await client().get(f"{api}/downloadclient/schema", headers=h)).json()}
            clients = []
            if "qbittorrent" in ws and state.get("qbittorrent", {}).get("status") == "running" and "QBittorrent" not in have:
                clients.append(("QBittorrent", "qBittorrent (built-in)", {"host": "127.0.0.1", "port": SERVICES["qbittorrent"]["port"], "musicCategory": "lidarr"}))
            if "sabnzbd" in ws and state.get("sabnzbd", {}).get("status") == "running" and "Sabnzbd" not in have:
                clients.append(("Sabnzbd", "SABnzbd (built-in)", {"host": "127.0.0.1", "port": SERVICES["sabnzbd"]["port"], "urlBase": base("sabnzbd"),
                                                                   "apiKey": _sab_config(), "musicCategory": "music"}))
            for impl, name, vals in clients:
                if impl not in schemas:
                    continue
                body = _schema_fill(dict(schemas[impl]), vals)
                body.update(name=name, enable=True, priority=1)
                r = await client().post(f"{api}/downloadclient", headers=h, json=body)
                done[impl] = r.status_code < 300
                if r.status_code >= 300:
                    db.log("warning", f"Built-in services: adding {name} to Lidarr failed: {r.text[:200]}")
        except Exception as e:  # noqa: BLE001
            db.log("warning", f"Built-in services: wiring Lidarr failed: {e}")
    if "prowlarr" in ws and "lidarr" in ws and state.get("prowlarr", {}).get("status") == "running" and state.get("lidarr", {}).get("status") == "running":
        h = {"X-Api-Key": _xml_config("prowlarr")}
        api = url("prowlarr") + "/api/v1"
        try:
            apps = (await client().get(f"{api}/applications", headers=h)).json()
            if not any(a.get("implementation") == "Lidarr" for a in apps):
                schema = next(s for s in (await client().get(f"{api}/applications/schema", headers=h)).json() if s.get("implementation") == "Lidarr")
                body = _schema_fill(schema, {"prowlarrUrl": url("prowlarr"), "baseUrl": url("lidarr"), "apiKey": _xml_config("lidarr")})
                body.update(name="Lidarr (built-in)", syncLevel="fullSync")
                r = await client().post(f"{api}/applications", headers=h, json=body)
                done["prowlarr_lidarr"] = r.status_code < 300
                if r.status_code >= 300:
                    db.log("warning", f"Built-in services: linking Prowlarr to Lidarr failed: {r.text[:200]}")
        except Exception as e:  # noqa: BLE001
            db.log("warning", f"Built-in services: wiring Prowlarr failed: {e}")
    for k in done:
        db.log("info", f"Built-in services: {k} set up")
    for name in ws:
        if state.get(name, {}).get("status") == "running":
            state[name]["wired"] = True
    return done


def remove_data(name: str) -> None:
    """Forget a service's data (used by the reset button)."""
    shutil.rmtree(data_dir(name), ignore_errors=True)
