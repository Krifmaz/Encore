"""Admin-only reverse proxy to the built-in services' web UIs: /svc/<name>/…"""
import asyncio
import time
from urllib.parse import urlsplit

import httpx
from fastapi import APIRouter, Request, WebSocket
from fastapi.responses import HTMLResponse, RedirectResponse, StreamingResponse
from starlette.background import BackgroundTask

from .. import auth, db
from ..services import embedded

router = APIRouter(include_in_schema=False)
HOP = {"connection", "keep-alive", "proxy-authenticate", "proxy-authorization", "te", "trailers", "transfer-encoding", "upgrade", "host", "content-length"}
_client: httpx.AsyncClient | None = None


def _admin_from_cookie(cookies) -> dict | None:
    token = cookies.get(auth.COOKIE)
    s = db.one("SELECT * FROM sessions WHERE token=?", (token,)) if token else None
    if not s or s["expires_at"] < time.time():
        return None
    return db.one("SELECT * FROM users WHERE id=? AND disabled=0 AND role='admin'", (s["user_id"],))


def _same_origin(request: Request) -> bool:
    """CSRF guard for the proxied UIs (they can't send Encore's header): the request must come from this site."""
    origin = request.headers.get("origin") or request.headers.get("referer")
    if not origin:
        return request.headers.get("sec-fetch-site", "same-origin") in ("same-origin", "none")
    return urlsplit(origin).netloc == request.headers.get("host")


def _upstream(name: str, path: str) -> str:
    port = embedded.SERVICES[name]["port"]
    prefix = "" if name == "qbittorrent" else embedded.base(name)   # qBittorrent has no URL base setting
    return f"http://127.0.0.1:{port}{prefix}/{path}"


@router.api_route("/svc/{name}", methods=["GET"])
async def svc_root(name: str):
    return RedirectResponse(f"/svc/{name}/")


@router.api_route("/svc/{name}/{path:path}", methods=["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"])
async def proxy(name: str, path: str, request: Request):
    global _client
    if name not in embedded.SERVICES or name not in embedded.wanted():
        return HTMLResponse("<h3>This built-in service isn't enabled.</h3><p>Set ENCORE_EMBEDDED in your compose file.</p>", 404)
    if not _admin_from_cookie(request.cookies):
        return RedirectResponse("/#/login") if request.method == "GET" else HTMLResponse("Admins only", 403)
    if request.method not in ("GET", "HEAD", "OPTIONS") and not _same_origin(request):
        return HTMLResponse("Cross-site request blocked", 403)
    _client = _client or httpx.AsyncClient(timeout=httpx.Timeout(300, connect=5), follow_redirects=False)
    headers = {k: v for k, v in request.headers.items() if k.lower() not in HOP and k.lower() != "cookie"}
    cookies = "; ".join(f"{k}={v}" for k, v in request.cookies.items() if k != auth.COOKIE)
    if cookies:
        headers["cookie"] = cookies
    headers["x-forwarded-for"] = request.client.host if request.client else "127.0.0.1"
    headers["x-forwarded-proto"] = request.url.scheme
    headers["x-forwarded-host"] = request.headers.get("host", "")
    url = _upstream(name, path) + (f"?{request.url.query}" if request.url.query else "")
    body = await request.body() if request.method in ("POST", "PUT", "PATCH", "DELETE") else None   # no chunked empty bodies on GET
    try:
        up = await _client.send(_client.build_request(request.method, url, headers=headers, content=body), stream=True)
    except httpx.HTTPError as e:
        st = embedded.state.get(name, {}).get("status", "stopped")
        return HTMLResponse(f"<h3>{embedded.SERVICES[name]['name']} isn't answering ({st}).</h3><p>{e}</p><p><a href='/#/admin/services'>Built-in services</a></p>", 502)
    out_headers = []
    for k, v in up.headers.multi_items():
        if k.lower() in HOP:
            continue
        if k.lower() == "location" and v.startswith(f"http://127.0.0.1:{embedded.SERVICES[name]['port']}"):
            v = v.split(str(embedded.SERVICES[name]["port"]), 1)[1]
            if name == "qbittorrent":
                v = f"/svc/qbittorrent{v}"
        if k.lower() == "x-frame-options":
            continue
        out_headers.append((k, v))
    resp = StreamingResponse(up.aiter_raw(), status_code=up.status_code, background=BackgroundTask(up.aclose))
    resp.raw_headers = [(k.lower().encode("latin-1"), v.encode("latin-1")) for k, v in out_headers]
    return resp


@router.websocket("/svc/{name}/{path:path}")
async def proxy_ws(ws: WebSocket, name: str, path: str):
    """Live updates (SignalR / websockets) for the proxied UIs."""
    from websockets.asyncio.client import connect
    if name not in embedded.SERVICES or name not in embedded.wanted() or not _admin_from_cookie(ws.cookies):
        await ws.close(code=4403)
        return
    origin = ws.headers.get("origin")
    if origin and urlsplit(origin).netloc != ws.headers.get("host"):
        await ws.close(code=4403)
        return
    target = _upstream(name, path).replace("http://", "ws://") + (f"?{ws.url.query}" if ws.url.query else "")
    cookies = "; ".join(f"{k}={v}" for k, v in ws.cookies.items() if k != auth.COOKIE)
    subprotocols = [p.strip() for p in ws.headers.get("sec-websocket-protocol", "").split(",") if p.strip()]
    try:
        async with connect(target, additional_headers={"cookie": cookies} if cookies else None, subprotocols=subprotocols or None,
                           max_size=None, open_timeout=10) as up:
            await ws.accept(subprotocol=up.subprotocol)

            async def down():
                async for msg in up:
                    if isinstance(msg, bytes):
                        await ws.send_bytes(msg)
                    else:
                        await ws.send_text(msg)

            async def upward():
                while True:
                    m = await ws.receive()
                    if m["type"] == "websocket.disconnect":
                        break
                    await up.send(m.get("bytes") if m.get("bytes") is not None else m.get("text", ""))
            done, pending = await asyncio.wait([asyncio.create_task(down()), asyncio.create_task(upward())], return_when=asyncio.FIRST_COMPLETED)
            for t in pending:
                t.cancel()
    except Exception:  # noqa: BLE001
        try:
            await ws.close(code=1011)
        except Exception:  # noqa: BLE001
            pass
