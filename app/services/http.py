import asyncio

import httpx

from .. import __version__

# one client per event loop (the app's loop, plus any loop used by scripts/tests)
_clients: dict[int, httpx.AsyncClient] = {}
transport: httpx.AsyncBaseTransport | None = None  # tests swap in a fake-internet transport


def client() -> httpx.AsyncClient:
    try:
        key = id(asyncio.get_running_loop())
    except RuntimeError:
        key = 0
    c = _clients.get(key)
    if c is None or c.is_closed:
        c = httpx.AsyncClient(
            timeout=httpx.Timeout(30.0, connect=10.0),
            follow_redirects=True,
            transport=transport,
            headers={"User-Agent": f"Encore/{__version__} ( https://github.com/krifmaz/test )"},
        )
        _clients[key] = c
    return c


_prims: dict = {}


def loop_local(name: str, factory):
    """asyncio Locks/Semaphores bind to the loop that first uses them; keep one per loop."""
    try:
        key = (name, id(asyncio.get_running_loop()))
    except RuntimeError:
        key = (name, 0)
    if key not in _prims:
        _prims[key] = factory()
    return _prims[key]


async def close():
    try:
        key = id(asyncio.get_running_loop())
    except RuntimeError:
        return
    c = _clients.pop(key, None)
    if c:
        await c.aclose()


HINTS_401 = {
    "slskd": "API key rejected – check web.authentication.api_keys in slskd.yml (the key and its cidr must allow Encore's IP)",
    "Lidarr": "API key rejected – copy it from Lidarr → Settings → General → Security",
    "Jellyfin": "Token rejected – create an API key in Jellyfin → Dashboard → API Keys",
    "Plex": "Token rejected – use an X-Plex-Token from a signed-in Plex session",
}


def explain(service: str, url: str, status: int | None = None, body: str = "", exc: Exception | None = None) -> str:
    """Turn a failed request into a message that says what to fix."""
    import httpx as _h
    from urllib.parse import urlsplit
    host = urlsplit(url).netloc or url
    if exc is not None:
        if isinstance(exc, _h.ConnectError):
            msg = str(exc).lower()
            if "name or service not known" in msg or "nodename nor servname" in msg or "temporary failure in name resolution" in msg:
                return (f"Can't find host '{host.split(':')[0]}' – use the container/service name on the same Docker network "
                        f"(if {service} runs behind gluetun with network_mode: container:…, use the VPN container's name)")
            return f"Can't connect to {host} – is {service} running and listening on that port? ({exc})"
        if isinstance(exc, _h.TimeoutException):
            return f"{service} at {host} didn't answer in time – check the URL/port and that nothing (VPN, firewall) blocks it"
        return f"{service} request failed: {exc}"
    detail = " ".join((body or "").split())[:200]
    if status in (401, 403):
        return f"{status} from {service}: {HINTS_401.get(service, 'credentials rejected – check the API key/token')}" + (f" ({detail})" if detail else "")
    if status == 404:
        return f"404 from {service} at {url} – wrong URL or base path? (a reverse-proxy URL base must be included, e.g. http://lidarr:8686/lidarr)"
    return f"{service} returned HTTP {status}" + (f": {detail}" if detail else " with an empty response")
