import httpx

from .. import __version__

_client: httpx.AsyncClient | None = None


def client() -> httpx.AsyncClient:
    global _client
    if _client is None:
        _client = httpx.AsyncClient(
            timeout=httpx.Timeout(30.0, connect=10.0),
            follow_redirects=True,
            headers={"User-Agent": f"Encore/{__version__} ( https://github.com/krifmaz/test )"},
        )
    return _client


async def close():
    if _client:
        await _client.aclose()
