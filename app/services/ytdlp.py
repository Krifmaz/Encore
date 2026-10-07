"""yt-dlp wrapper: YouTube, SoundCloud, Bandcamp, Mixcloud and ~1800 other sites."""
import asyncio
import os
import tempfile

import yt_dlp

from .. import config


def enabled() -> bool:
    return bool(config.get("ytdlp_enabled"))


def version() -> str:
    return yt_dlp.version.__version__


def _base_opts(outdir: str, progress=None, playlist=False) -> dict:
    fmt = config.get("ytdlp_format")
    opts = {
        "format": "bestaudio/best",
        "outtmpl": os.path.join(outdir, "%(playlist_index|)s%(playlist_index& - |)s%(artist,uploader|Unknown)s - %(track,title)s.%(ext)s"),
        "noplaylist": not playlist,
        "quiet": True,
        "no_warnings": True,
        "noprogress": True,
        "ignoreerrors": playlist,
        "writethumbnail": True,
        "restrictfilenames": False,
        "windowsfilenames": True,
        "postprocessors": [],
        "retries": 5,
        "fragment_retries": 5,
    }
    if fmt != "best":
        pp = {"key": "FFmpegExtractAudio", "preferredcodec": fmt}
        if fmt in ("mp3", "m4a"):
            pp["preferredquality"] = config.get("ytdlp_quality") or "0"
        opts["postprocessors"].append(pp)
    opts["postprocessors"] += [
        {"key": "FFmpegMetadata", "add_metadata": True},
        {"key": "EmbedThumbnail", "already_have_thumbnail": False},
    ]
    if fmt in ("opus", "best", "flac"):
        # thumbnails can't always be embedded into these containers; keep it non-fatal
        opts["postprocessors"] = [p for p in opts["postprocessors"] if p["key"] != "EmbedThumbnail"]
        opts["writethumbnail"] = False
    cookies = config.get("ytdlp_cookies")
    if cookies:
        fd, path = tempfile.mkstemp(suffix=".txt", prefix="encore-cookies-")
        with os.fdopen(fd, "w") as fh:
            fh.write(cookies)
        opts["cookiefile"] = path
    if progress:
        opts["progress_hooks"] = [progress]
    return opts


def _download(url: str, outdir: str, progress=None, playlist=False) -> list[dict]:
    os.makedirs(outdir, exist_ok=True)
    before = set(os.listdir(outdir))
    with yt_dlp.YoutubeDL(_base_opts(outdir, progress, playlist)) as ydl:
        info = ydl.extract_info(url, download=True)
    entries = (info or {}).get("entries") or [info] if info else []
    entries = [e for e in entries if e]
    after = [f for f in os.listdir(outdir) if f not in before]
    audio = [os.path.join(outdir, f) for f in after if f.rsplit(".", 1)[-1].lower() in ("mp3", "m4a", "opus", "ogg", "flac", "webm", "wav", "aac")]
    for f in after:
        if f.rsplit(".", 1)[-1].lower() in ("jpg", "jpeg", "webp", "png"):
            try:
                os.remove(os.path.join(outdir, f))
            except OSError:
                pass
    meta = []
    for e in entries:
        meta.append({
            "title": e.get("track") or e.get("title"),
            "artist": e.get("artist") or e.get("creator") or e.get("uploader"),
            "album": e.get("album"),
            "duration": e.get("duration"),
            "webpage_url": e.get("webpage_url"),
            "thumbnail": e.get("thumbnail"),
        })
    return [{"files": sorted(audio), "entries": meta}]


async def download(url: str, outdir: str, progress=None, playlist=False) -> dict:
    res = await asyncio.to_thread(_download, url, outdir, progress, playlist)
    return res[0]


def _search(query: str, n: int = 5) -> list[dict]:
    with yt_dlp.YoutubeDL({"quiet": True, "no_warnings": True, "extract_flat": True, "skip_download": True}) as ydl:
        info = ydl.extract_info(f"ytsearch{n}:{query}", download=False)
    return [
        {"url": e.get("url") or f"https://www.youtube.com/watch?v={e.get('id')}", "title": e.get("title"), "duration": e.get("duration"), "channel": e.get("channel") or e.get("uploader")}
        for e in (info or {}).get("entries", []) if e
    ]


async def search(query: str, n: int = 5) -> list[dict]:
    return await asyncio.to_thread(_search, query, n)


async def find_best(artist: str, title: str, length: float | None) -> str | None:
    """Find the YouTube video that best matches a track (prefers 'Topic' auto-generated uploads)."""
    results = await search(f"{artist} - {title} audio", 6)
    best, best_score = None, -1e9
    for r in results:
        s = 0.0
        name = (r["title"] or "").lower()
        if title.lower() in name:
            s += 20
        if artist.lower() in name or artist.lower() in (r["channel"] or "").lower():
            s += 10
        if (r["channel"] or "").endswith("- Topic"):
            s += 15
        for bad in ("live", "cover", "karaoke", "remix", "reaction", "8d", "slowed", "sped up", "nightcore", "instrumental"):
            if bad in name and bad not in title.lower():
                s -= 15
        if length and r.get("duration"):
            diff = abs(r["duration"] - length)
            s += 15 if diff < 4 else (5 if diff < 12 else -20)
        if s > best_score:
            best, best_score = r["url"], s
    return best


async def probe(url: str) -> dict:
    def _p():
        with yt_dlp.YoutubeDL({"quiet": True, "no_warnings": True, "extract_flat": "in_playlist", "skip_download": True}) as ydl:
            return ydl.extract_info(url, download=False)
    info = await asyncio.to_thread(_p)
    entries = info.get("entries")
    return {
        "title": info.get("title"),
        "uploader": info.get("uploader") or info.get("artist"),
        "thumbnail": info.get("thumbnail") or ((info.get("thumbnails") or [{}])[-1].get("url")),
        "is_playlist": entries is not None,
        "count": len(list(entries)) if entries is not None else 1,
        "extractor": info.get("extractor_key"),
        "duration": info.get("duration"),
    }
