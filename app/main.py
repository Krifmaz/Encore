import asyncio
import os
from contextlib import asynccontextmanager

from fastapi import FastAPI, Request
from fastapi.responses import FileResponse, JSONResponse
from fastapi.staticfiles import StaticFiles

from . import __version__, auth, config, db, worker
from .routers import admin, auth_routes, diagnostics, discover, library_routes, playlists_routes, requests_routes, system
from .services import http

STATIC = os.path.join(os.path.dirname(__file__), "static")


@asynccontextmanager
async def lifespan(app: FastAPI):
    db.connect()
    config.load()
    auth.ensure_admin()
    for p in (config.get("staging_path"), config.get("library_path")):
        try:
            os.makedirs(p, exist_ok=True)
        except OSError:
            pass
    task = asyncio.create_task(worker.loop()) if os.environ.get("ENCORE_WORKER", "1") != "0" else None
    yield
    worker._stop.set()
    if task:
        task.cancel()
    await http.close()


app = FastAPI(title="Encore", version=__version__, lifespan=lifespan, docs_url="/api/docs", redoc_url=None, openapi_url="/api/openapi.json")

for r in (auth_routes, admin, discover, requests_routes, library_routes, playlists_routes, system, diagnostics):
    app.include_router(r.router)


@app.middleware("http")
async def security_headers(request: Request, call_next):
    resp = await call_next(request)
    resp.headers.setdefault("X-Content-Type-Options", "nosniff")
    resp.headers.setdefault("X-Frame-Options", "DENY")
    resp.headers.setdefault("Referrer-Policy", "same-origin")
    if request.url.path.startswith("/api/"):
        resp.headers.setdefault("Cache-Control", "no-store")
    else:
        # app files: browsers must re-check on every load (cheap 304s), so updates show up immediately
        resp.headers.setdefault("Cache-Control", "no-cache")
    return resp


BUILD = os.environ.get("ENCORE_BUILD", "dev")


@app.get("/api/health")
def health():
    return {"ok": True, "version": __version__, "build": BUILD}


@app.exception_handler(404)
async def spa_fallback(request: Request, exc):
    if request.url.path.startswith("/api/"):
        return JSONResponse({"detail": getattr(exc, "detail", "Not found")}, status_code=404)
    return FileResponse(os.path.join(STATIC, "index.html"))


app.mount("/", StaticFiles(directory=STATIC, html=True), name="static")
