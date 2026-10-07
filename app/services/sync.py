"""Two-way liked-songs sync + playlist push to media servers (Jellyfin/Finamp, Plex/Plexamp, Navidrome, ListenBrainz).

Liked songs: Encore keeps one "Liked Songs" playlist per user. Every sync pulls likes from every connected
service, turns additions *and removals* (diffed against the last snapshot) into changes on that playlist, then
pushes the result back out to every service that supports it. So liking a song in Finamp makes it loved in
Plexamp, and un-liking it anywhere removes it everywhere.

Playlists: Encore is the source of truth for playlists with sync targets; they're mirrored to each target.
"""
import asyncio
import json
import time
import traceback

from .. import config, db
from . import connections, matcher, playlists, sources
from .http import client, loop_local

state = {"running": False, "last": None}


# ---- library ↔ server item mapping ------------------------------------------------

def _rel(path: str) -> str:
    lib = config.get("library_path").rstrip("/") + "/"
    return path[len(lib):] if path.startswith(lib) else path


class ItemMap:
    """Maps library track ids ↔ a media server's item ids (by file path, falling back to metadata)."""

    def __init__(self, server_items: list[dict]):
        self.lib = db.query("SELECT id, path, title, artist, album, duration, isrc, mb_trackid FROM tracks")
        by_rel = {}
        for t in self.lib:
            by_rel[_rel(t["path"]).lower()] = t["id"]
        index = matcher.LibraryIndex(self.lib)
        self.to_track: dict[str, int] = {}
        self.to_item: dict[int, str] = {}
        for it in server_items:
            tid = None
            p = (it.get("path") or "").replace("\\", "/").lower()
            if p:
                # strip the server's own mount prefix by matching path suffixes
                parts = p.split("/")
                for i in range(len(parts)):
                    tid = by_rel.get("/".join(parts[i:]))
                    if tid:
                        break
            if not tid:
                hit, _ = index.find(matcher.describe(it.get("title"), it.get("artist"), it.get("album") or "", it.get("duration"), mbid=it.get("mbid")))
                tid = hit["id"] if hit else None
            if tid:
                self.to_track[str(it["id"])] = tid
                self.to_item.setdefault(tid, str(it["id"]))


# ---- adapters ----------------------------------------------------------------------

class Adapter:
    can_push_likes = False
    can_push_playlists = False

    def __init__(self, conn: dict):
        self.conn = conn
        self.d = conn["data"]
        self._map: ItemMap | None = None

    async def item_map(self) -> ItemMap:
        if self._map is None:
            self._map = ItemMap(await self.all_items())
        return self._map

    async def all_items(self) -> list[dict]:
        return []

    async def likes(self) -> list[dict]:
        """→ list of {track_id?} or source tracks (T dicts)"""
        return []

    async def set_like(self, track_id: int, liked: bool) -> bool:
        return False

    async def playlists(self) -> list[dict]:
        return []

    async def playlist_tracks(self, remote_id: str) -> list[dict]:
        return []

    async def push_playlist(self, name: str, track_ids: list[int], remote_id: str | None) -> str | None:
        return None


class Jellyfin(Adapter):
    can_push_likes = can_push_playlists = True

    def __init__(self, conn):
        super().__init__(conn)
        self.url = config.get("jellyfin_url").rstrip("/")
        self.h = connections.jellyfin_auth_header(self.d["token"], f"encore-{conn['user_id']}")
        self.uid = self.d["user_id"]

    async def _get(self, path, **params):
        r = await client().get(self.url + path, headers=self.h, params=params)
        r.raise_for_status()
        return r.json()

    async def _items(self, **params) -> list[dict]:
        out, start = [], 0
        while True:
            d = await self._get("/Items", userId=self.uid, IncludeItemTypes="Audio", Recursive="true",
                                Fields="Path,ProviderIds", StartIndex=start, Limit=2000, **params)
            for i in d.get("Items", []):
                out.append({"id": i["Id"], "path": i.get("Path"), "title": i.get("Name"),
                            "artist": ", ".join(i.get("Artists") or []) or i.get("AlbumArtist"), "album": i.get("Album"),
                            "duration": (i.get("RunTimeTicks") or 0) / 1e7 or None,
                            "mbid": (i.get("ProviderIds") or {}).get("MusicBrainzTrack")})
            start += 2000
            if start >= d.get("TotalRecordCount", 0):
                return out

    async def all_items(self):
        return await self._items()

    async def likes(self):
        m = await self.item_map()
        return [{"track_id": m.to_track[str(i["id"])]} | i for i in await self._items(Filters="IsFavorite") if str(i["id"]) in m.to_track]

    async def set_like(self, track_id, liked):
        item = (await self.item_map()).to_item.get(track_id)
        if not item:
            return False
        method = "POST" if liked else "DELETE"
        r = await client().request(method, f"{self.url}/UserFavoriteItems/{item}", headers=self.h, params={"userId": self.uid})
        if r.status_code == 404:  # Jellyfin < 10.9
            r = await client().request(method, f"{self.url}/Users/{self.uid}/FavoriteItems/{item}", headers=self.h)
        return r.status_code < 400

    async def playlists(self):
        d = await self._get("/Items", userId=self.uid, IncludeItemTypes="Playlist", Recursive="true")
        return [{"id": p["Id"], "name": p["Name"]} for p in d.get("Items", [])]

    async def playlist_tracks(self, remote_id):
        d = await self._get(f"/Playlists/{remote_id}/Items", userId=self.uid, Fields="Path")
        return d.get("Items", [])

    async def push_playlist(self, name, track_ids, remote_id):
        m = await self.item_map()
        ids = [m.to_item[t] for t in track_ids if t in m.to_item]
        c = client()
        if remote_id:
            try:
                cur = await self.playlist_tracks(remote_id)
            except Exception:  # noqa: BLE001
                remote_id, cur = None, []
            if remote_id:
                if [i["Id"] for i in cur] == ids:
                    return remote_id
                entries = [i.get("PlaylistItemId") for i in cur if i.get("PlaylistItemId")]
                for k in range(0, len(entries), 100):
                    await c.delete(f"{self.url}/Playlists/{remote_id}/Items", headers=self.h, params={"entryIds": ",".join(entries[k:k + 100])})
                for k in range(0, len(ids), 100):
                    await c.post(f"{self.url}/Playlists/{remote_id}/Items", headers=self.h, params={"ids": ",".join(ids[k:k + 100]), "userId": self.uid})
                return remote_id
        r = await c.post(f"{self.url}/Playlists", headers=self.h, json={"Name": name, "Ids": ids[:100], "UserId": self.uid, "MediaType": "Audio"})
        r.raise_for_status()
        pid = r.json()["Id"]
        for k in range(100, len(ids), 100):
            await c.post(f"{self.url}/Playlists/{pid}/Items", headers=self.h, params={"ids": ",".join(ids[k:k + 100]), "userId": self.uid})
        return pid


class Plex(Adapter):
    can_push_likes = can_push_playlists = True
    LOVED = 10  # Plexamp's "love" = 5 stars

    def __init__(self, conn):
        super().__init__(conn)
        self.url = config.get("plex_url").rstrip("/")
        self.h = connections.plex_headers(self.d["token"])
        self._mid = None

    async def _get(self, path, **params):
        r = await client().get(self.url + path, headers=self.h, params=params)
        r.raise_for_status()
        return r.json().get("MediaContainer", {})

    async def machine_id(self):
        if not self._mid:
            self._mid = (await self._get("/identity"))["machineIdentifier"]
        return self._mid

    async def all_items(self):
        out = []
        for sec in (await self._get("/library/sections")).get("Directory", []):
            if sec.get("type") != "artist":
                continue
            start = 0
            while True:
                d = await self._get(f"/library/sections/{sec['key']}/all", type=10, **{
                    "X-Plex-Container-Start": start, "X-Plex-Container-Size": 2000})
                for t in d.get("Metadata", []):
                    part = ((t.get("Media") or [{}])[0].get("Part") or [{}])[0]
                    out.append({"id": t["ratingKey"], "path": part.get("file"), "title": t.get("title"),
                                "artist": t.get("originalTitle") or t.get("grandparentTitle"), "album": t.get("parentTitle"),
                                "duration": (t.get("duration") or 0) / 1000 or None, "rating": t.get("userRating")})
                start += 2000
                if start >= d.get("totalSize", d.get("size", 0)):
                    break
        self._items_cache = out
        return out

    async def likes(self):
        m = await self.item_map()
        items = getattr(self, "_items_cache", None) or await self.all_items()
        return [{"track_id": m.to_track[str(i["id"])]} | i for i in items if (i.get("rating") or 0) >= self.LOVED - 1 and str(i["id"]) in m.to_track]

    async def set_like(self, track_id, liked):
        item = (await self.item_map()).to_item.get(track_id)
        if not item:
            return False
        r = await client().put(f"{self.url}/:/rate", headers=self.h, params={
            "key": item, "identifier": "com.plexapp.plugins.library", "rating": self.LOVED if liked else -1})
        return r.status_code < 400

    async def playlists(self):
        d = await self._get("/playlists", playlistType="audio")
        return [{"id": p["ratingKey"], "name": p["title"]} for p in d.get("Metadata", []) if not p.get("smart")]

    async def playlist_tracks(self, remote_id):
        return (await self._get(f"/playlists/{remote_id}/items")).get("Metadata", [])

    def _uri(self, mid, keys):
        return f"server://{mid}/com.plexapp.plugins.library/library/metadata/{','.join(keys)}"

    async def push_playlist(self, name, track_ids, remote_id):
        m = await self.item_map()
        keys = [m.to_item[t] for t in track_ids if t in m.to_item]
        mid = await self.machine_id()
        c = client()
        if remote_id:
            try:
                cur = [str(i["ratingKey"]) for i in await self.playlist_tracks(remote_id)]
            except Exception:  # noqa: BLE001
                remote_id, cur = None, None
            if remote_id:
                if cur == keys:
                    return remote_id
                await c.delete(f"{self.url}/playlists/{remote_id}/items", headers=self.h)
                for k in range(0, len(keys), 200):
                    await c.put(f"{self.url}/playlists/{remote_id}/items", headers=self.h, params={"uri": self._uri(mid, keys[k:k + 200])})
                return remote_id
        if not keys:
            return None
        r = await c.post(f"{self.url}/playlists", headers=self.h, params={
            "type": "audio", "title": name, "smart": 0, "uri": self._uri(mid, keys[:200])})
        r.raise_for_status()
        pid = r.json()["MediaContainer"]["Metadata"][0]["ratingKey"]
        for k in range(200, len(keys), 200):
            await c.put(f"{self.url}/playlists/{pid}/items", headers=self.h, params={"uri": self._uri(mid, keys[k:k + 200])})
        return pid


class Navidrome(Adapter):
    can_push_likes = can_push_playlists = True

    def __init__(self, conn):
        super().__init__(conn)
        self.url = config.get("navidrome_url").rstrip("/")

    async def _get(self, method, params=None):
        p = connections.subsonic_params(self.d["user"], self.d["password"])
        p.update(params or {})
        r = await client().get(f"{self.url}/rest/{method}", params=p)
        d = r.json()["subsonic-response"]
        if d.get("status") != "ok":
            raise RuntimeError((d.get("error") or {}).get("message", "Subsonic error"))
        return d

    @staticmethod
    def _song(s):
        return {"id": s["id"], "path": s.get("path"), "title": s.get("title"), "artist": s.get("artist"),
                "album": s.get("album"), "duration": s.get("duration"), "mbid": s.get("musicBrainzId")}

    async def all_items(self):
        out, off = [], 0
        while True:
            d = await self._get("search3", {"query": "", "songCount": 500, "songOffset": off, "artistCount": 0, "albumCount": 0})
            songs = d.get("searchResult3", {}).get("song", [])
            out += [self._song(s) for s in songs]
            if len(songs) < 500:
                return out
            off += 500

    async def likes(self):
        m = await self.item_map()
        songs = (await self._get("getStarred2")).get("starred2", {}).get("song", [])
        return [{"track_id": m.to_track[s["id"]]} | self._song(s) for s in songs if s["id"] in m.to_track]

    async def set_like(self, track_id, liked):
        item = (await self.item_map()).to_item.get(track_id)
        if not item:
            return False
        await self._get("star" if liked else "unstar", {"id": item})
        return True

    async def playlists(self):
        d = await self._get("getPlaylists")
        return [{"id": p["id"], "name": p["name"]} for p in d.get("playlists", {}).get("playlist", [])]

    async def playlist_tracks(self, remote_id):
        return (await self._get("getPlaylist", {"id": remote_id})).get("playlist", {}).get("entry", [])

    async def push_playlist(self, name, track_ids, remote_id):
        m = await self.item_map()
        ids = [m.to_item[t] for t in track_ids if t in m.to_item]
        params = [("name", name)] if not remote_id else [("playlistId", remote_id)]
        params += [("songId", i) for i in ids]
        p = connections.subsonic_params(self.d["user"], self.d["password"])
        r = await client().get(f"{self.url}/rest/createPlaylist", params=list(p.items()) + params)
        d = r.json()["subsonic-response"]
        if d.get("status") != "ok":
            if remote_id:  # playlist was deleted on the server → recreate
                return await self.push_playlist(name, track_ids, None)
            raise RuntimeError((d.get("error") or {}).get("message", "Subsonic error"))
        return remote_id or (d.get("playlist") or {}).get("id")


class ListenBrainz(Adapter):
    can_push_likes = can_push_playlists = True
    API = "https://api.listenbrainz.org/1"

    def _h(self):
        return {"Authorization": f"Token {self.d['token']}"}

    async def likes(self):
        out, off = [], 0
        while True:
            r = await client().get(f"{self.API}/feedback/user/{self.d['user']}/get-feedback",
                                   params={"score": 1, "count": 1000, "offset": off, "metadata": "true"})
            fb = r.json().get("feedback", [])
            for f in fb:
                md = f.get("track_metadata") or {}
                out.append(sources.T(md.get("track_name"), md.get("artist_name"), md.get("release_name"), mbid=f.get("recording_mbid")))
            if len(fb) < 1000:
                return out
            off += 1000

    async def set_like(self, track_id, liked):
        t = db.one("SELECT mb_trackid FROM tracks WHERE id=?", (track_id,))
        if not t or not t["mb_trackid"]:
            return False
        r = await client().post(f"{self.API}/feedback/recording-feedback", headers=self._h(),
                                json={"recording_mbid": t["mb_trackid"], "score": 1 if liked else 0})
        return r.status_code < 400

    async def push_playlist(self, name, track_ids, remote_id):
        mbids = [r["mb_trackid"] for r in (db.one("SELECT mb_trackid FROM tracks WHERE id=?", (t,)) for t in track_ids) if r and r["mb_trackid"]]
        tracks = [{"identifier": f"https://musicbrainz.org/recording/{m}"} for m in mbids]
        c = client()
        if remote_id:
            r = await c.get(f"{self.API}/playlist/{remote_id}", headers=self._h())
            if r.status_code == 200:
                n = len(r.json()["playlist"].get("track", []))
                if n:
                    await c.post(f"{self.API}/playlist/{remote_id}/item/delete", headers=self._h(), json={"index": 0, "count": n})
                if tracks:
                    await c.post(f"{self.API}/playlist/{remote_id}/item/add", headers=self._h(), json={"playlist": {"track": tracks}})
                return remote_id
        r = await c.post(f"{self.API}/playlist/create", headers=self._h(), json={"playlist": {
            "title": name, "track": tracks,
            "extension": {"https://musicbrainz.org/doc/jspf#playlist": {"public": False}}}})
        r.raise_for_status()
        return r.json().get("playlist_mbid")


class Spotify(Adapter):
    async def likes(self):
        tok = await connections.spotify_token(self.conn["user_id"])
        return (await sources.spotify_api(tok, "collection", ""))["tracks"] if tok else []

    async def playlists(self):
        tok = await connections.spotify_token(self.conn["user_id"])
        out, nxt = [], "https://api.spotify.com/v1/me/playlists?limit=50"
        while nxt and tok:
            d = (await client().get(nxt, headers={"Authorization": f"Bearer {tok}"})).json()
            out += [{"id": p["id"], "name": p["name"], "url": f"https://open.spotify.com/playlist/{p['id']}"} for p in d.get("items", []) if p]
            nxt = d.get("next")
        return out


class Tidal(Adapter):
    async def _get(self, path, params=None):
        tok = await connections.tidal_token(self.conn["user_id"])
        r = await client().get("https://openapi.tidal.com/v2" + path if path.startswith("/") else path,
                               headers={"Authorization": f"Bearer {tok}", "Accept": "application/vnd.api+json"}, params=params)
        r.raise_for_status()
        return r.json()

    async def likes(self):
        tok = await connections.tidal_token(self.conn["user_id"])
        if not tok:
            return []
        uid, country = self.d.get("user_id") or "me", self.d.get("country", "US")
        ids, nxt, params = [], f"/userCollections/{uid}/relationships/tracks", {"countryCode": country, "locale": "en-US"}
        while nxt and len(ids) < 10000:
            page = await self._get(nxt, params)
            ids += [x["id"] for x in page.get("data", [])]
            nxt, params = (page.get("links") or {}).get("next"), None
            if nxt and nxt.startswith("/v2/"):
                nxt = nxt[3:]
        return await _tidal_tracks(tok, ids, country) if ids else []

    async def playlists(self):
        uid, country = self.d.get("user_id"), self.d.get("country", "US")
        d = await self._get("/playlists", {"countryCode": country, "filter[r.owners.id]": uid})
        return [{"id": p["id"], "name": p["attributes"].get("name"), "url": f"https://tidal.com/playlist/{p['id']}"} for p in d.get("data", [])]


async def _tidal_tracks(token: str, ids: list[str], country: str) -> list[dict]:
    out = []
    h = {"Authorization": f"Bearer {token}", "Accept": "application/vnd.api+json"}
    for i in range(0, len(ids), 20):
        chunk = ids[i:i + 20]
        r = await client().get("https://openapi.tidal.com/v2/tracks", headers=h,
                               params={"countryCode": country, "filter[id]": ",".join(chunk), "include": "artists"})
        d = r.json()
        names = {x["id"]: x.get("attributes", {}).get("name", "") for x in d.get("included", []) if x.get("type") == "artists"}
        for x in d.get("data", []):
            a = x.get("attributes", {})
            arts = [names.get(r_["id"], "") for r_ in (x.get("relationships", {}).get("artists") or {}).get("data", [])]
            title = a.get("title", "")
            if a.get("version") and a["version"].lower() not in title.lower():
                title += f" ({a['version']})"
            out.append(sources.T(title, ", ".join(filter(None, arts)), None, sources._iso_dur(a.get("duration")), a.get("isrc"), source_id=x["id"]))
    return out


class YTMusic(Adapter):
    """YouTube Music account: two-way likes (👍) and playlist push."""
    can_push_likes = can_push_playlists = True
    MAX_LOOKUPS = 150  # song searches per sync, keeps us well clear of rate limits

    def __init__(self, conn):
        super().__init__(conn)
        from . import ytmusic
        self.m = ytmusic
        self.yt = ytmusic.for_connection(conn)
        self.lookups = 0

    async def likes(self):
        d = await self.m.run(self.yt.get_liked_songs, 5000)
        out = []
        for x in d.get("tracks", []):
            t = self.m._t(x)
            out.append(t)
        # remember videoIds of liked songs we can match, so pushes don't need searches
        return out

    async def _vid(self, track_id):
        cached = self.m._cache_get(track_id)
        if cached:
            return None if cached == "-" else cached
        if self.lookups >= self.MAX_LOOKUPS:
            return None
        self.lookups += 1
        return await self.m.video_for_track(track_id)

    async def set_like(self, track_id, liked):
        vid = await self._vid(track_id)
        if not vid:
            return False
        await self.m.run(self.yt.rate_song, vid, "LIKE" if liked else "INDIFFERENT")
        return True

    async def playlists(self):
        rows = await self.m.run(self.yt.get_library_playlists, None)
        return [{"id": p["playlistId"], "name": p["title"], "url": f"https://music.youtube.com/playlist?list={p['playlistId']}"}
                for p in rows if p.get("playlistId") not in ("LM", "SE")]

    async def push_playlist(self, name, track_ids, remote_id):
        vids = []
        for t in track_ids:
            v = await self._vid(t)
            if v:
                vids.append(v)
        if remote_id:
            try:
                cur = await self.m.run(self.yt.get_playlist, remote_id, None)
                have = [x for x in cur.get("tracks", []) if x.get("videoId")]
                if [x["videoId"] for x in have] == vids:
                    return remote_id
                if have:
                    await self.m.run(self.yt.remove_playlist_items, remote_id, have)
                if vids:
                    await self.m.run(self.yt.add_playlist_items, remote_id, vids, None, True)
                return remote_id
            except Exception:  # noqa: BLE001
                pass
        res = await self.m.run(self.yt.create_playlist, name, "Synced by Encore", "PRIVATE", vids)
        return res if isinstance(res, str) else None


class Deezer(Adapter):
    async def likes(self):
        return (await sources.deezer(f"https://www.deezer.com/profile/{self.d['user_id']}/loved"))["tracks"]

    async def playlists(self):
        d = (await client().get(f"https://api.deezer.com/user/{self.d['user_id']}/playlists?limit=200")).json()
        return [{"id": str(p["id"]), "name": p["title"], "url": p.get("link")} for p in d.get("data", [])]


class LastFM(Adapter):
    async def likes(self):
        out, page = [], 1
        while page <= 20:
            d = (await client().get("https://ws.audioscrobbler.com/2.0/", params={
                "method": "user.getlovedtracks", "user": self.d["user"], "api_key": config.get("lastfm_api_key"),
                "format": "json", "limit": 1000, "page": page})).json()
            lt = d.get("lovedtracks", {})
            for t in lt.get("track", []):
                out.append(sources.T(t.get("name"), (t.get("artist") or {}).get("name"), mbid=t.get("mbid") or None))
            if page >= int(lt.get("@attr", {}).get("totalPages", 1)):
                break
            page += 1
        return out


ADAPTERS = {"jellyfin": Jellyfin, "plex": Plex, "navidrome": Navidrome, "listenbrainz": ListenBrainz, "ytmusic": YTMusic,
            "spotify": Spotify, "tidal": Tidal, "deezer": Deezer, "lastfm": LastFM}


def adapter(conn: dict) -> Adapter:
    return ADAPTERS[conn["service"]](conn)


# ---- snapshots ---------------------------------------------------------------------

def _snap(conn_id: int, scope: str) -> set:
    r = db.one("SELECT value FROM sync_state WHERE connection_id=? AND scope=?", (conn_id, scope))
    return set(json.loads(r["value"])) if r else set()


def _save_snap(conn_id: int, scope: str, values: set) -> None:
    db.execute("INSERT OR REPLACE INTO sync_state VALUES (?,?,?)", (conn_id, scope, json.dumps(sorted(values, key=str))))


def _like_key(row: dict) -> str:
    return f"t:{row['track_id']}" if row.get("track_id") else playlists.identity(row)


# ---- engine ------------------------------------------------------------------------

async def sync_user(user_id: int, log: list | None = None) -> dict:
    log = log if log is not None else []
    conns = [c for c in connections.list_for(user_id) if c["service"] in ADAPTERS]
    liked = playlists.get_liked(user_id)
    index = matcher.LibraryIndex.load()
    adapters = {c["id"]: adapter(c) for c in conns}
    stats = {"liked_added": 0, "liked_removed": 0, "pushed": 0, "playlists": 0, "errors": 0}

    # 1. pull likes from every service and apply diffs to Encore's Liked Songs
    remote_now: dict[int, set] = {}
    liked_rows: dict[int, dict] = {}
    for c in conns:
        if not c["sync_likes"]:
            continue
        a = adapters[c["id"]]
        try:
            rows = await a.likes()
        except Exception as e:  # noqa: BLE001
            stats["errors"] += 1
            db.execute("UPDATE connections SET last_error=? WHERE id=?", (f"likes: {str(e)[:200]}", c["id"]))
            log.append(f"{c['service']}: {e}")
            continue
        need = [r for r in rows if not r.get("track_id")]
        playlists.match_rows(need, index)
        keys = {_like_key(r): r for r in rows}
        prev = _snap(c["id"], "likes")
        new_keys = set(keys) - prev
        gone = prev - set(keys)
        if new_keys:
            added, _ = playlists.add_tracks(liked["id"], [{**keys[k], "cover": keys[k].get("cover")} for k in new_keys], c["service"], index)
            stats["liked_added"] += added
        if gone:
            for it in playlists.items(liked["id"]):
                if _like_key(it) in gone:
                    db.execute("DELETE FROM playlist_items WHERE id=?", (it["id"],))
                    stats["liked_removed"] += 1
        remote_now[c["id"]] = set(keys)
        liked_rows[c["id"]] = keys

    # 1b. auto-download liked songs that aren't in the library yet
    if config.get("auto_request_likes"):
        owner = db.one("SELECT * FROM users WHERE id=? AND disabled=0 AND auto_download=1", (user_id,))
        if owner:
            n = playlists.request_missing(owner, liked["id"], auto=True)
            if n:
                db.log("info", f"Auto-download: {n} liked song(s) for {owner['username']}")
                stats["requested"] = n

    # 2. push Encore's liked songs back out to services that accept likes
    liked_tracks = {it["track_id"] for it in playlists.items(liked["id"]) if it["track_id"]}
    for c in conns:
        a = adapters[c["id"]]
        if not (c["sync_likes"] and a.can_push_likes) or c["id"] not in remote_now:
            continue
        have = {int(k[2:]) for k in remote_now[c["id"]] if k.startswith("t:")}
        if c["service"] == "ytmusic":
            # keep YT videoIds of matched likes so we never search for them again
            from . import ytmusic as _ytm
            for k, row in liked_rows.get(c["id"], {}).items():
                if k.startswith("t:") and row.get("source_id"):
                    _ytm._cache_set(int(k[2:]), row["source_id"])
        try:
            for tid in liked_tracks - have:
                if await a.set_like(tid, True):
                    have.add(tid)
                    stats["pushed"] += 1
            for tid in have - liked_tracks:
                if await a.set_like(tid, False):
                    have.discard(tid)
                    stats["pushed"] += 1
        except Exception as e:  # noqa: BLE001
            stats["errors"] += 1
            db.execute("UPDATE connections SET last_error=? WHERE id=?", (f"push likes: {str(e)[:200]}", c["id"]))
            log.append(f"{c['service']}: {e}")
        snap = {k for k in remote_now[c["id"]] if not k.startswith("t:")} | {f"t:{t}" for t in have}
        _save_snap(c["id"], "likes", snap)
    for c in conns:
        if c["id"] in remote_now and not adapters[c["id"]].can_push_likes:
            _save_snap(c["id"], "likes", remote_now[c["id"]])

    # 3. mirror playlists to their targets (liked songs too, as a playlist, for servers without favourites)
    by_service = {c["service"]: c for c in conns}
    for pl in db.query("SELECT * FROM playlists WHERE user_id=?", (user_id,)):
        targets = json.loads(pl["sync_targets"] or "[]")
        if not targets:
            continue
        remote = json.loads(pl.get("remote_ids") or "{}")
        tids = [it["track_id"] for it in playlists.items(pl["id"]) if it["track_id"]]
        for svc in targets:
            c = by_service.get(svc)
            if not c or not c["sync_playlists"] or not adapters[c["id"]].can_push_playlists:
                continue
            try:
                rid = await adapters[c["id"]].push_playlist(pl["name"], tids, remote.get(svc))
                if rid:
                    remote[svc] = str(rid)
                stats["playlists"] += 1
            except Exception as e:  # noqa: BLE001
                stats["errors"] += 1
                db.execute("UPDATE connections SET last_error=? WHERE id=?", (f"playlist '{pl['name']}': {str(e)[:180]}", c["id"]))
                log.append(f"{svc} playlist {pl['name']}: {e}")
        db.execute("UPDATE playlists SET remote_ids=? WHERE id=?", (json.dumps(remote), pl["id"]))

    now = time.time()
    for c in conns:
        if not db.one("SELECT last_error FROM connections WHERE id=? AND last_error IS NOT NULL", (c["id"],)) or c["id"] in remote_now:
            db.execute("UPDATE connections SET last_sync=? WHERE id=?", (now, c["id"]))
    return stats


async def fetch_remote_playlist(pl: dict) -> dict:
    """Read a playlist that was imported from a media server (source_url = 'service:id')."""
    svc, rid = pl["source_url"].split(":", 1)
    c = connections.get(pl["user_id"], svc)
    if not c:
        raise RuntimeError(f"{svc} is no longer connected")
    a = adapter(c)
    m = await a.item_map()
    rows = await a.playlist_tracks(rid)
    tracks = []
    for r in rows:
        key = str(r.get("Id") or r.get("ratingKey") or r.get("id"))
        tid = m.to_track.get(key)
        t = db.one("SELECT id, title, artist, album, duration, isrc, mb_trackid FROM tracks WHERE id=?", (tid,)) if tid else None
        if t:
            tracks.append({"title": t["title"], "artist": t["artist"], "album": t["album"], "duration": t["duration"],
                           "isrc": t["isrc"], "mbid": t["mb_trackid"], "track_id": t["id"]})
        else:
            tracks.append(sources.T(r.get("Name") or r.get("title"), r.get("AlbumArtist") or r.get("grandparentTitle") or r.get("artist"),
                                    r.get("Album") or r.get("parentTitle") or r.get("album")))
    return {"name": pl["name"], "tracks": tracks}


async def sync_all() -> None:
    lock = loop_local("sync", asyncio.Lock)
    if lock.locked():
        return
    async with lock:
        state["running"] = True
        try:
            # refresh followed playlists first so new tracks get synced in the same pass
            for pl in db.query("SELECT * FROM playlists WHERE follow=1"):
                try:
                    added, _ = await playlists.refresh_followed(pl)
                    if added:
                        db.log("info", f"Playlist '{pl['name']}': {added} new track(s) from {pl['source']}")
                    if config.get("auto_request_followed"):
                        owner = db.one("SELECT * FROM users WHERE id=? AND disabled=0 AND auto_download=1", (pl["user_id"],))
                        if owner:
                            n = playlists.request_missing(owner, pl["id"], auto=True)
                            if n:
                                db.log("info", f"Auto-download: {n} new track(s) from followed playlist '{pl['name']}'")
                except Exception as e:  # noqa: BLE001
                    db.log("warning", f"Couldn't refresh playlist '{pl['name']}': {e}")
            playlists.rematch()
            for u in db.query("SELECT DISTINCT user_id FROM connections"):
                try:
                    await sync_user(u["user_id"])
                except Exception:  # noqa: BLE001
                    traceback.print_exc()
        finally:
            state.update(running=False, last=time.time())

