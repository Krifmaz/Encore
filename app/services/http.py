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
