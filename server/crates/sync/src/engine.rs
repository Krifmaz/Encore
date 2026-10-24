//! Two-way liked-songs sync + playlist push to media servers and services.
//!
//! Liked songs: one "Liked Songs" playlist per user. Every sync pulls likes from every connected service,
//! turns additions *and removals* (diffed against the last snapshot) into changes on that playlist, then
//! pushes the result back out to every service that supports it – like a song in Finamp and it's loved in
//! Plexamp; un-like it anywhere and it's gone everywhere. Playlists with sync targets are mirrored to them.
use crate::connections::{self, jellyfin_headers, plex_headers, subsonic_params, with_headers};
use encore_core::json::text as s;
use encore_db::{now, Row, RowExt};
use crate::{playlists, Ctx};
use encore_db::p;
use encore_http::{deezer, lastfm, listenbrainz, tidal};
use encore_library::matcher::{describe, LibraryIndex};

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

/// Library track ids ↔ a server's item ids (by file path, else by metadata).
#[derive(Default)]
pub struct ItemMap {
    /// server item id → library track id
    pub to_track: HashMap<String, i64>,
    /// library track id → server item id (the first one seen)
    pub to_item: HashMap<i64, String>,
}

impl ItemMap {
    /// Match every item of a server to the library: by path (whatever the server's mount prefix), else by title,
    /// artist, length and MusicBrainz id.
    fn new(st: &impl Ctx, items: &[Value]) -> Self {
        let lib = st.db().query("SELECT id, path, title, artist, album, duration, isrc, mb_trackid FROM tracks", p![]).unwrap_or_default();
        let root = format!("{}/", st.cfg().str("library_path").trim_end_matches('/'));
        let by_rel: HashMap<String, i64> = lib.iter().map(|t| { let p = t.s("path"); (p.strip_prefix(&root).unwrap_or(&p).to_lowercase(), t.i("id")) }).collect();
        let ix = LibraryIndex::new(lib);
        let mut m = ItemMap::default();
        for it in items {
            let path = s(&it["path"]).replace('\\', "/").to_lowercase();
            let parts: Vec<&str> = path.split('/').collect();
            // strip the server's own mount prefix by matching path suffixes
            let mut tid = if path.is_empty() { None } else { (0..parts.len()).find_map(|i| by_rel.get(&parts[i..].join("/")).copied()) };
            if tid.is_none() {
                let d = describe(&s(&it["title"]), &s(&it["artist"]), it["duration"].as_f64(), None, it["mbid"].as_str());
                tid = ix.find(&d).map(|(h, _)| h.i("id"));
            }
            if let Some(t) = tid {
                let id = s(&it["id"]);
                m.to_track.insert(id.clone(), t);
                m.to_item.entry(t).or_insert(id);
            }
        }
        m
    }
}

/// One connected service, able to read likes/playlists and (for some) push them.
pub struct Adapter<'a, S: Ctx> {
    /// the server
    st: &'a S,
    /// the connection's row (its `data` read)
    pub conn: Row,
    /// the connection's stored data (tokens, user)
    d: Value,
    /// the server's items matched to the library, once worked out
    map: Option<ItemMap>,
    /// every audio item on the server, once fetched
    items: Option<Vec<Value>>,
    /// lookups made on the service this run
    lookups: usize,
}

impl<'a, S: Ctx> Adapter<'a, S> {
    /// An adapter for connection `conn`.
    pub fn new(st: &'a S, conn: Row) -> Self {
        let d = conn["data"].clone();
        Adapter { st, conn, d, map: None, items: None, lookups: 0 }
    }
    /// The service's name.
    fn svc(&self) -> String {
        self.conn.s("service")
    }
    /// Whether likes and playlists can be sent to this service (not only read from it).
    pub fn can_push(&self) -> bool {
        ["jellyfin", "plex", "navidrome", "listenbrainz", "ytmusic"].contains(&self.svc().as_str())
    }
    /// A server address setting without its trailing slash.
    fn base(&self, key: &str) -> String {
        self.st.cfg().str(key).trim_end_matches('/').to_string()
    }

    // -- low-level calls
    /// GET `path` on the Jellyfin server as the signed-in user.
    async fn jf(&self, path: &str, q: &[(&str, String)]) -> anyhow::Result<Value> {
        let h = jellyfin_headers(self.d["token"].as_str(), &format!("encore-{}", self.conn.i("user_id")));
        Ok(with_headers(self.st.http().get(format!("{}{path}", self.base("jellyfin_url"))).query(q), &h).send().await?.error_for_status()?.json().await?)
    }
    /// GET `path` on the Plex server; its `MediaContainer`.
    async fn plex(&self, path: &str, q: &[(&str, String)]) -> anyhow::Result<Value> {
        let h = plex_headers(self.st, self.d["token"].as_str());
        let v: Value = with_headers(self.st.http().get(format!("{}{path}", self.base("plex_url"))).query(q), &h).send().await?.error_for_status()?.json().await?;
        Ok(v["MediaContainer"].clone())
    }
    /// Call Subsonic `method` on the Navidrome server (signed); its `subsonic-response`.
    async fn sub(&self, method: &str, q: &[(&str, String)]) -> anyhow::Result<Value> {
        let mut params = subsonic_params(self.d["user"].as_str().unwrap_or(""), self.d["password"].as_str().unwrap_or(""));
        params.extend(q.iter().map(|(k, v)| (*k, v.clone())));
        let v: Value = self.st.http().get(format!("{}/rest/{method}", self.base("navidrome_url"))).query(&params).send().await?.json().await?;
        let r = v["subsonic-response"].clone();
        if r["status"] != "ok" {
            anyhow::bail!("{}", r["error"]["message"].as_str().unwrap_or("Subsonic error"));
        }
        Ok(r)
    }
    /// The YouTube Music session data, refreshed when needed.
    async fn yt(&self) -> anyhow::Result<Value> {
        crate::ytmusic::fresh_data(self.st, &self.conn).await
    }

    // -- every audio item on a media server
    /// Every audio item on the media server (Jellyfin, Plex or Navidrome), fetched once per adapter.
    async fn all_items(&mut self) -> anyhow::Result<Vec<Value>> {
        if let Some(i) = &self.items {
            return Ok(i.clone());
        }
        let mut out = vec![];
        match self.svc().as_str() {
            "jellyfin" => {
                let mut start = 0;
                loop {
                    let d = self.jf("/Items", &[("userId", s(&self.d["user_id"])), ("IncludeItemTypes", "Audio".into()), ("Recursive", "true".into()),
                        ("Fields", "Path,ProviderIds".into()), ("StartIndex", start.to_string()), ("Limit", "2000".into())]).await?;
                    for i in d["Items"].as_array().cloned().unwrap_or_default() {
                        let artists = i["Artists"].as_array().map(|a| a.iter().map(s).collect::<Vec<_>>().join(", ")).filter(|a| !a.is_empty()).unwrap_or(s(&i["AlbumArtist"]));
                        out.push(json!({ "id": i["Id"], "path": i["Path"], "title": i["Name"], "artist": artists, "album": i["Album"],
                            "duration": i["RunTimeTicks"].as_f64().map(|t| t / 1e7), "mbid": i["ProviderIds"]["MusicBrainzTrack"], "fav": i["UserData"]["IsFavorite"] }));
                    }
                    start += 2000;
                    if start >= d["TotalRecordCount"].as_i64().unwrap_or(0) {
                        break;
                    }
                }
            }
            "plex" => {
                for sec in self.plex("/library/sections", &[]).await?["Directory"].as_array().cloned().unwrap_or_default() {
                    if sec["type"] != "artist" {
                        continue;
                    }
                    let mut start = 0;
                    loop {
                        let d = self.plex(&format!("/library/sections/{}/all", s(&sec["key"])), &[("type", "10".into()), ("X-Plex-Container-Start", start.to_string()), ("X-Plex-Container-Size", "2000".into())]).await?;
                        for t in d["Metadata"].as_array().cloned().unwrap_or_default() {
                            let artist = if t["originalTitle"].is_string() { t["originalTitle"].clone() } else { t["grandparentTitle"].clone() };
                            out.push(json!({ "id": s(&t["ratingKey"]), "path": t["Media"][0]["Part"][0]["file"], "title": t["title"], "artist": artist, "album": t["parentTitle"],
                                "duration": t["duration"].as_f64().map(|d| d / 1000.0), "rating": t["userRating"] }));
                        }
                        start += 2000;
                        if start >= d["totalSize"].as_i64().or(d["size"].as_i64()).unwrap_or(0) {
                            break;
                        }
                    }
                }
            }
            "navidrome" => {
                let mut off = 0;
                loop {
                    let d = self.sub("search3", &[("query", "".into()), ("songCount", "500".into()), ("songOffset", off.to_string()), ("artistCount", "0".into()), ("albumCount", "0".into())]).await?;
                    let songs = d["searchResult3"]["song"].as_array().cloned().unwrap_or_default();
                    out.extend(songs.iter().map(sub_song));
                    if songs.len() < 500 {
                        break;
                    }
                    off += 500;
                }
            }
            _ => {}
        }
        self.items = Some(out.clone());
        Ok(out)
    }

    /// The server's items matched to the library (worked out once).
    pub async fn item_map(&mut self) -> anyhow::Result<&ItemMap> {
        if self.map.is_none() {
            let items = self.all_items().await?;
            self.map = Some(ItemMap::new(self.st, &items));
        }
        Ok(self.map.as_ref().unwrap())
    }

    /// Liked songs on the service: rows with track_id (media servers) or source tracks to match.
    pub async fn likes(&mut self) -> anyhow::Result<Vec<Value>> {
        let svc = self.svc();
        let with_tid = |m: &ItemMap, items: Vec<Value>| -> Vec<Value> {
            items.into_iter().filter_map(|mut i| { let t = m.to_track.get(&s(&i["id"]))?; i["track_id"] = json!(t); Some(i) }).collect()
        };
        Ok(match svc.as_str() {
            "jellyfin" | "plex" => {
                let items = self.all_items().await?;
                let liked: Vec<Value> = items.into_iter().filter(|i| i["fav"].as_bool().unwrap_or(false) || i["rating"].as_f64().unwrap_or(0.0) >= 9.0).collect();
                with_tid(self.item_map().await?, liked)
            }
            "navidrome" => {
                let songs: Vec<Value> = self.sub("getStarred2", &[]).await?["starred2"]["song"].as_array().cloned().unwrap_or_default().iter().map(sub_song).collect();
                with_tid(self.item_map().await?, songs)
            }
            "listenbrainz" => {
                let (mut out, mut off) = (vec![], 0);
                loop {
                    let d: Value = listenbrainz::feedback(self.st, &s(&self.d["user"]), off).send().await?.json().await?;
                    let fb = d["feedback"].as_array().cloned().unwrap_or_default();
                    out.extend(fb.iter().map(|f| { let m = &f["track_metadata"]; json!({ "title": m["track_name"], "artist": m["artist_name"], "album": m["release_name"], "mbid": f["recording_mbid"] }) }));
                    if fb.len() < 1000 {
                        break out;
                    }
                    off += 1000;
                }
            }
            "spotify" => match connections::spotify_token(self.st, self.conn.i("user_id")).await {
                Some(tok) => self.st.spotify_api(&tok, "collection", "").await?["tracks"].as_array().cloned().unwrap_or_default(),
                None => vec![],
            },
            "deezer" => self.st.fetch_link(&format!("https://www.deezer.com/profile/{}/loved", s(&self.d["user_id"])), None).await?["tracks"].as_array().cloned().unwrap_or_default(),
            "lastfm" => {
                let (mut out, mut page) = (vec![], 1);
                while page <= 20 {
                    let d: Value = lastfm::loved_tracks(self.st, &self.st.cfg().str("lastfm_api_key"), &s(&self.d["user"]), page).send().await?.json().await?;
                    let lt = &d["lovedtracks"];
                    out.extend(lt["track"].as_array().cloned().unwrap_or_default().iter().map(|t| json!({ "title": t["name"], "artist": t["artist"]["name"],
                        "mbid": t["mbid"].as_str().filter(|m| !m.is_empty()) })));
                    if page >= lt["@attr"]["totalPages"].as_str().and_then(|x| x.parse().ok()).unwrap_or(1) {
                        break;
                    }
                    page += 1;
                }
                out
            }
            "ytmusic" => {
                let d = self.yt().await?;
                encore_http::ytmusic::browse_rows(self.st, &d, "FEmusic_liked_videos", 50).await?.into_iter().filter(|x| x["videoId"].is_string())
                    .map(|x| ytm_liked(&x)).collect()
            }
            "tidal" => self.tidal_likes().await?,
            _ => vec![],
        })
    }

    /// The songs liked on TIDAL.
    async fn tidal_likes(&self) -> anyhow::Result<Vec<Value>> {
        let Some(tok) = connections::tidal_token(self.st, self.conn.i("user_id")).await else { return Ok(vec![]) };
        let uid = self.d["user_id"].as_str().filter(|x| !x.is_empty()).unwrap_or("me").to_string();
        let country = self.d["country"].as_str().unwrap_or("US").to_string();
        let mut ids = vec![];
        let mut next = Some(format!("/userCollections/{uid}/relationships/tracks"));
        let mut first = true;
        while let Some(u) = next.filter(|_| ids.len() < 10000) {
            let mut rb = tidal::get(self.st, &tok, &u);
            if first {
                rb = rb.query(&[("countryCode", country.as_str()), ("locale", "en-US")]);
            }
            first = false;
            let page: Value = rb.send().await?.error_for_status()?.json().await?;
            ids.extend(page["data"].as_array().cloned().unwrap_or_default().iter().map(|x| s(&x["id"])));
            next = page["links"]["next"].as_str().map(String::from);
        }
        let mut out = vec![];
        for chunk in ids.chunks(20) {
            let d: Value = tidal::get(self.st, &tok, "/tracks")
                .query(&[("countryCode", country.as_str()), ("filter[id]", &chunk.join(",")), ("include", "artists")]).send().await?.json().await?;
            let names: HashMap<String, String> = d["included"].as_array().cloned().unwrap_or_default().iter().filter(|x| x["type"] == "artists")
                .map(|x| (s(&x["id"]), s(&x["attributes"]["name"]))).collect();
            for x in d["data"].as_array().cloned().unwrap_or_default() {
                let a = &x["attributes"];
                let artists: Vec<String> = x["relationships"]["artists"]["data"].as_array().cloned().unwrap_or_default().iter().filter_map(|r| names.get(&s(&r["id"])).cloned()).collect();
                let mut title = s(&a["title"]);
                let ver = s(&a["version"]);
                if !ver.is_empty() && !title.to_lowercase().contains(&ver.to_lowercase()) {
                    title = format!("{title} ({ver})");
                }
                out.push(json!({ "title": title, "artist": artists.join(", "), "isrc": a["isrc"], "source_id": x["id"] }));
            }
        }
        Ok(out)
    }

    /// YouTube video for a library track (cached; at most 150 searches per sync).
    async fn video_for(&mut self, tid: i64) -> Option<String> {
        let key = format!("ytm:{tid}");
        if let Ok(Some(r)) = self.st.db().one("SELECT value FROM sync_state WHERE connection_id=0 AND scope=?", p![key]) {
            let v = r.s("value");
            return if v == "-" { None } else { Some(v) };
        }
        if self.lookups >= 150 {
            return None;
        }
        self.lookups += 1;
        let t = self.st.db().one("SELECT title, artist, duration FROM tracks WHERE id=?", p![tid]).ok()??;
        let hit = encore_library::ytmusic::find_song(self.st, &t.s("artist"), &t.s("title"), Some(t.f("duration"))).await;
        let vid = hit.and_then(|h| h["videoId"].as_str().map(String::from));
        let _ = self.st.db().execute("INSERT OR REPLACE INTO sync_state VALUES (0,?,?)", p![key, vid.clone().unwrap_or("-".into())]);
        vid
    }

    /// Like (or unlike) library track `tid` on the service; whether it was sent.
    pub async fn set_like(&mut self, tid: i64, liked: bool) -> anyhow::Result<bool> {
        let svc = self.svc();
        if svc == "listenbrainz" {
            let Some(mbid) = self.st.db().one("SELECT mb_trackid FROM tracks WHERE id=?", p![tid])?.map(|r| r.s("mb_trackid")).filter(|m| !m.is_empty()) else { return Ok(false) };
            let r = listenbrainz::recording_feedback(self.st, &s(&self.d["token"]), &json!({ "recording_mbid": mbid, "score": if liked { 1 } else { 0 } })).send().await?;
            return Ok(r.status().is_success());
        }
        if svc == "ytmusic" {
            let Some(v) = self.video_for(tid).await else { return Ok(false) };
            let d = self.yt().await?;
            encore_http::ytmusic::rate(self.st, &d, &v, liked).await?;
            return Ok(true);
        }
        let Some(item) = self.item_map().await?.to_item.get(&tid).cloned() else { return Ok(false) };
        match svc.as_str() {
            "jellyfin" => {
                let h = jellyfin_headers(self.d["token"].as_str(), &format!("encore-{}", self.conn.i("user_id")));
                let m = if liked { reqwest::Method::POST } else { reqwest::Method::DELETE };
                let uid = s(&self.d["user_id"]);
                let mut r = with_headers(self.st.http().request(m.clone(), format!("{}/UserFavoriteItems/{item}", self.base("jellyfin_url"))).query(&[("userId", &uid)]), &h).send().await?;
                if r.status().as_u16() == 404 {
                    r = with_headers(self.st.http().request(m, format!("{}/Users/{uid}/FavoriteItems/{item}", self.base("jellyfin_url"))), &h).send().await?; // Jellyfin < 10.9
                }
                Ok(r.status().as_u16() < 400)
            }
            "plex" => {
                let h = plex_headers(self.st, self.d["token"].as_str());
                let r = with_headers(self.st.http().put(format!("{}/:/rate", self.base("plex_url")))
                    .query(&[("key", item.as_str()), ("identifier", "com.plexapp.plugins.library"), ("rating", if liked { "10" } else { "-1" })]), &h).send().await?;
                Ok(r.status().as_u16() < 400)
            }
            "navidrome" => {
                self.sub(if liked { "star" } else { "unstar" }, &[("id", item)]).await?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// The user's playlists on the service: [{id, name, url?}]
    pub async fn playlists(&mut self) -> anyhow::Result<Vec<Value>> {
        Ok(match self.svc().as_str() {
            "jellyfin" => self.jf("/Items", &[("userId", s(&self.d["user_id"])), ("IncludeItemTypes", "Playlist".into()), ("Recursive", "true".into())]).await?["Items"]
                .as_array().cloned().unwrap_or_default().iter().map(|p| json!({ "id": p["Id"], "name": p["Name"] })).collect(),
            "plex" => self.plex("/playlists", &[("playlistType", "audio".into())]).await?["Metadata"].as_array().cloned().unwrap_or_default().iter()
                .filter(|p| !p["smart"].as_bool().unwrap_or(false)).map(|p| json!({ "id": s(&p["ratingKey"]), "name": p["title"] })).collect(),
            "navidrome" => self.sub("getPlaylists", &[]).await?["playlists"]["playlist"].as_array().cloned().unwrap_or_default().iter()
                .map(|p| json!({ "id": p["id"], "name": p["name"] })).collect(),
            "spotify" => {
                let Some(tok) = connections::spotify_token(self.st, self.conn.i("user_id")).await else { return Ok(vec![]) };
                let (mut out, mut next) = (vec![], Some("https://api.spotify.com/v1/me/playlists?limit=50".to_string()));
                while let Some(u) = next {
                    let d: Value = self.st.http().get(&u).bearer_auth(&tok).send().await?.json().await?;
                    out.extend(d["items"].as_array().cloned().unwrap_or_default().iter().filter(|p| p.is_object())
                        .map(|p| json!({ "id": p["id"], "name": p["name"], "url": format!("https://open.spotify.com/playlist/{}", s(&p["id"])) })));
                    next = d["next"].as_str().map(String::from);
                }
                out
            }
            "deezer" => {
                let d: Value = deezer::user_playlists(self.st, s(&self.d["user_id"]), 200).send().await?.json().await?;
                d["data"].as_array().cloned().unwrap_or_default().iter().map(|p| json!({ "id": s(&p["id"]), "name": p["title"], "url": p["link"] })).collect()
            }
            "tidal" => {
                let Some(tok) = connections::tidal_token(self.st, self.conn.i("user_id")).await else { return Ok(vec![]) };
                let d: Value = tidal::get(self.st, &tok, "/playlists")
                    .query(&[("countryCode", self.d["country"].as_str().unwrap_or("US")), ("filter[r.owners.id]", &s(&self.d["user_id"]))]).send().await?.json().await?;
                d["data"].as_array().cloned().unwrap_or_default().iter().map(|p| json!({ "id": p["id"], "name": p["attributes"]["name"], "url": format!("https://tidal.com/playlist/{}", s(&p["id"])) })).collect()
            }
            "ytmusic" => {
                let d = self.yt().await?;
                encore_http::ytmusic::library_playlists(self.st, &d).await?
            }
            _ => vec![],
        })
    }

    /// Items of a playlist on a media server: [{id, title, artist, album}]
    pub async fn playlist_tracks(&mut self, rid: &str) -> anyhow::Result<Vec<Value>> {
        Ok(match self.svc().as_str() {
            "jellyfin" => self.jf(&format!("/Playlists/{rid}/Items"), &[("userId", s(&self.d["user_id"])), ("Fields", "Path".into())]).await?["Items"].as_array().cloned().unwrap_or_default()
                .iter().map(|i| json!({ "id": i["Id"], "entry": i["PlaylistItemId"], "title": i["Name"], "artist": i["AlbumArtist"], "album": i["Album"] })).collect(),
            "plex" => self.plex(&format!("/playlists/{rid}/items"), &[]).await?["Metadata"].as_array().cloned().unwrap_or_default()
                .iter().map(|i| json!({ "id": s(&i["ratingKey"]), "title": i["title"], "artist": i["grandparentTitle"], "album": i["parentTitle"] })).collect(),
            "navidrome" => self.sub("getPlaylist", &[("id", rid.into())]).await?["playlist"]["entry"].as_array().cloned().unwrap_or_default().iter().map(sub_song).collect(),
            _ => vec![],
        })
    }

    /// Mirror a playlist; → the remote id.
    pub async fn push_playlist(&mut self, name: &str, tids: &[i64], remote: Option<String>) -> anyhow::Result<Option<String>> {
        let svc = self.svc();
        match svc.as_str() {
            "listenbrainz" => return self.push_lb(name, tids, remote).await,
            "ytmusic" => return self.push_ytm(name, tids, remote).await,
            _ => {}
        }
        let map = self.item_map().await?;
        let ids: Vec<String> = tids.iter().filter_map(|t| map.to_item.get(t).cloned()).collect();
        match svc.as_str() {
            "jellyfin" => {
                let (base, uid) = (self.base("jellyfin_url"), s(&self.d["user_id"]));
                let h = jellyfin_headers(self.d["token"].as_str(), &format!("encore-{}", self.conn.i("user_id")));
                if let Some(rid) = &remote {
                    if let Ok(cur) = self.playlist_tracks(rid).await {
                        if cur.iter().map(|i| s(&i["id"])).collect::<Vec<_>>() == ids {
                            return Ok(remote);
                        }
                        let entries: Vec<String> = cur.iter().filter_map(|i| i["entry"].as_str().map(String::from)).collect();
                        for c in entries.chunks(100) {
                            with_headers(self.st.http().delete(format!("{base}/Playlists/{rid}/Items")).query(&[("entryIds", c.join(","))]), &h).send().await?;
                        }
                        for c in ids.chunks(100) {
                            with_headers(self.st.http().post(format!("{base}/Playlists/{rid}/Items")).query(&[("ids", c.join(",")), ("userId", uid.clone())]), &h).send().await?;
                        }
                        return Ok(remote);
                    }
                }
                let first: Vec<&String> = ids.iter().take(100).collect();
                let d: Value = with_headers(self.st.http().post(format!("{base}/Playlists")).json(&json!({ "Name": name, "Ids": first, "UserId": uid, "MediaType": "Audio" })), &h)
                    .send().await?.error_for_status()?.json().await?;
                let pid = s(&d["Id"]);
                for c in ids[ids.len().min(100)..].chunks(100) {
                    with_headers(self.st.http().post(format!("{base}/Playlists/{pid}/Items")).query(&[("ids", c.join(",")), ("userId", uid.clone())]), &h).send().await?;
                }
                Ok(Some(pid))
            }
            "plex" => {
                let base = self.base("plex_url");
                let h = plex_headers(self.st, self.d["token"].as_str());
                let mid = s(&self.plex("/identity", &[]).await?["machineIdentifier"]);
                let uri = |keys: &[String]| format!("server://{mid}/com.plexapp.plugins.library/library/metadata/{}", keys.join(","));
                if let Some(rid) = &remote {
                    if let Ok(cur) = self.playlist_tracks(rid).await {
                        if cur.iter().map(|i| s(&i["id"])).collect::<Vec<_>>() == ids {
                            return Ok(remote);
                        }
                        with_headers(self.st.http().delete(format!("{base}/playlists/{rid}/items")), &h).send().await?;
                        for c in ids.chunks(200) {
                            with_headers(self.st.http().put(format!("{base}/playlists/{rid}/items")).query(&[("uri", uri(c))]), &h).send().await?;
                        }
                        return Ok(remote);
                    }
                }
                if ids.is_empty() {
                    return Ok(None);
                }
                let d: Value = with_headers(self.st.http().post(format!("{base}/playlists")).query(&[("type", "audio".to_string()), ("title", name.into()), ("smart", "0".into()), ("uri", uri(&ids[..ids.len().min(200)]))]), &h)
                    .send().await?.error_for_status()?.json().await?;
                let pid = s(&d["MediaContainer"]["Metadata"][0]["ratingKey"]);
                for c in ids[ids.len().min(200)..].chunks(200) {
                    with_headers(self.st.http().put(format!("{base}/playlists/{pid}/items")).query(&[("uri", uri(c))]), &h).send().await?;
                }
                Ok(Some(pid))
            }
            "navidrome" => {
                let mut q: Vec<(&str, String)> = match &remote { Some(r) => vec![("playlistId", r.clone())], None => vec![("name", name.into())] };
                q.extend(ids.iter().map(|i| ("songId", i.clone())));
                match self.sub("createPlaylist", &q).await {
                    Ok(d) => Ok(remote.or(d["playlist"]["id"].as_str().map(String::from))),
                    Err(_) if remote.is_some() => Box::pin(self.push_playlist(name, tids, None)).await, // deleted on the server → recreate
                    Err(e) => Err(e),
                }
            }
            _ => Ok(None),
        }
    }

    /// Send playlist `name` to ListenBrainz (its songs by MusicBrainz id): replace the songs of `remote`, or create
    /// it; its id.
    async fn push_lb(&self, name: &str, tids: &[i64], remote: Option<String>) -> anyhow::Result<Option<String>> {
        let token = s(&self.d["token"]);
        let tracks: Vec<Value> = tids.iter().filter_map(|t| self.st.db().one("SELECT mb_trackid FROM tracks WHERE id=?", p![t]).ok().flatten())
            .map(|r| r.s("mb_trackid")).filter(|m| !m.is_empty()).map(|m| json!({ "identifier": format!("https://musicbrainz.org/recording/{m}") })).collect();
        if let Some(rid) = &remote {
            let r = listenbrainz::playlist(self.st, rid, Some(&token)).send().await?;
            if r.status().as_u16() == 200 {
                let n = r.json::<Value>().await?["playlist"]["track"].as_array().map(|a| a.len()).unwrap_or(0);
                if n > 0 {
                    listenbrainz::playlist_delete(self.st, &token, rid, &json!({ "index": 0, "count": n })).send().await?;
                }
                if !tracks.is_empty() {
                    listenbrainz::playlist_add(self.st, &token, rid, &json!({ "playlist": { "track": tracks } })).send().await?;
                }
                return Ok(remote);
            }
        }
        let d: Value = listenbrainz::playlist_create(self.st, &token, &json!({ "playlist": { "title": name, "track": tracks, "extension": { "https://musicbrainz.org/doc/jspf#playlist": { "public": false } } } }))
            .send().await?.error_for_status()?.json().await?;
        Ok(d["playlist_mbid"].as_str().map(String::from))
    }

    /// Send playlist `name` to YouTube Music: replace the songs of `remote` when they differ, or create it; its id.
    async fn push_ytm(&mut self, name: &str, tids: &[i64], remote: Option<String>) -> anyhow::Result<Option<String>> {
        let mut vids = vec![];
        for t in tids {
            if let Some(v) = self.video_for(*t).await {
                vids.push(v);
            }
        }
        let d = self.yt().await?;
        if let Some(rid) = &remote {
            if let Ok(cur) = encore_http::ytmusic::browse_rows(self.st, &d, &format!("VL{rid}"), 20).await {
                let have: Vec<&Value> = cur.iter().filter(|x| x["videoId"].is_string()).collect();
                if have.iter().map(|x| s(&x["videoId"])).collect::<Vec<_>>() == vids {
                    return Ok(remote);
                }
                let mut actions: Vec<Value> = have.iter().filter(|x| x["setVideoId"].is_string())
                    .map(|x| json!({ "action": "ACTION_REMOVE_VIDEO", "removedVideoId": x["videoId"], "setVideoId": x["setVideoId"] })).collect();
                actions.extend(vids.iter().map(|v| json!({ "action": "ACTION_ADD_VIDEO", "addedVideoId": v, "dedupeOption": "DEDUPE_OPTION_SKIP" })));
                encore_http::ytmusic::edit_playlist(self.st, &d, rid, actions).await?;
                return Ok(remote);
            }
        }
        encore_http::ytmusic::create_playlist(self.st, &d, name, &vids).await
    }
}

/// A liked YouTube Music row → a liked-songs track. A liked video (not a song) names its uploader's
/// channel as the artist: the artist and title are read from the video title then
/// ([`encore_library::video_title::parse`]), and the track says when the artist is only the uploader
/// (`artist_guessed`), so it is also matched by title and length.
fn ytm_liked(x: &Value) -> Value {
    let mut t = json!({ "title": x["title"], "artist": x["artist"], "album": x["album"], "duration": x["duration"], "source_id": x["videoId"], "cover": x["cover"] });
    if x["artist_is_channel"].as_bool() == Some(true) {
        let song = encore_library::video_title::parse(&s(&x["title"]), &s(&x["artist"]), "", "");
        t["source_title"] = x["title"].clone();
        (t["title"], t["artist"], t["artist_guessed"]) = (json!(song.title), json!(song.artist), json!(song.artist_guessed as i64));
    }
    t
}

/// A Subsonic song as Encore's server item.
fn sub_song(x: &Value) -> Value {
    json!({ "id": x["id"], "path": x["path"], "title": x["title"], "artist": x["artist"], "album": x["album"], "duration": x["duration"], "mbid": x["musicBrainzId"] })
}

// ---- snapshots ----------------------------------------------------------------------------------

/// What the last sync saw under `scope` (likes, a playlist) of connection `cid`.
fn snap(st: &impl Ctx, cid: i64, scope: &str) -> HashSet<String> {
    st.db().one("SELECT value FROM sync_state WHERE connection_id=? AND scope=?", p![cid, scope]).ok().flatten()
        .and_then(|r| serde_json::from_str::<Vec<String>>(&r.s("value")).ok()).unwrap_or_default().into_iter().collect()
}

/// Keep what this sync saw under `scope`.
fn save_snap(st: &impl Ctx, cid: i64, scope: &str, v: &HashSet<String>) {
    let mut v: Vec<&String> = v.iter().collect();
    v.sort();
    let _ = st.db().execute("INSERT OR REPLACE INTO sync_state VALUES (?,?,?)", p![cid, scope, json!(v).to_string()]);
}

/// What identifies a like across syncs: its library track, else the song itself.
fn like_key(r: &Row) -> String {
    if r.i("track_id") > 0 { format!("t:{}", r.i("track_id")) } else { playlists::identity(r, false) }
}

/// The likes a service lists that the last sync didn't see, in the order they go into Liked Songs. Services
/// list the newest like first (Spotify, Deezer, YouTube Music, Last.fm, ListenBrainz), so these go in oldest
/// first: a later like gets a later place, and Liked Songs (newest first) shows them in the service's order.
/// A song listed twice goes in once.
fn new_likes(rows: &[Row], prev: &HashSet<String>) -> Vec<Value> {
    let mut seen = HashSet::new();
    let mut out: Vec<Value> = rows.iter().filter(|r| {
        let k = like_key(r);
        !prev.contains(&k) && seen.insert(k)
    }).map(|r| Value::Object(r.clone())).collect();
    out.reverse();
    out
}

/// Record a connection's last error (cut to 220 characters).
fn set_error(st: &impl Ctx, cid: i64, msg: String) {
    let _ = st.db().execute("UPDATE connections SET last_error=? WHERE id=?", p![msg.chars().take(220).collect::<String>(), cid]);
}

// ---- engine -------------------------------------------------------------------------------------

/// Sync every connection of user `uid`: read likes and playlists into Encore, send them out where the service
/// takes them; what happened goes into `log`.
pub async fn sync_user(st: &impl Ctx, uid: i64, log: &mut Vec<String>) -> Value {
    let conns = connections::list_for(st, uid);
    let Ok(liked) = playlists::get_liked(st, uid) else { return json!({}) };
    let lid = liked.i("id");
    // the library's index of every song: kept while the songs are unchanged, built on the blocking pool
    let ix = st.library().songs.load(st.db()).await;
    let mut ads: Vec<Adapter<_>> = conns.into_iter().map(|c| Adapter::new(st, c)).collect();
    let (mut added_n, mut removed_n, mut pushed, mut pls, mut errors, mut requested) = (0, 0, 0, 0, 0, 0);
    let mut remote_now: HashMap<i64, HashSet<String>> = HashMap::new();
    let mut rows_by: HashMap<i64, HashMap<String, Row>> = HashMap::new();
    // 1. pull likes from every service, apply the diffs to Liked Songs
    for a in ads.iter_mut() {
        let cid = a.conn.i("id");
        if a.conn.i("sync_likes") == 0 {
            continue;
        }
        let rows = match a.likes().await {
            Ok(r) => r,
            Err(e) => {
                errors += 1;
                set_error(st, cid, format!("likes: {e}"));
                log.push(format!("{}: {e}", a.conn.s("service")));
                continue;
            }
        };
        let mut rows: Vec<Row> = rows.into_iter().filter_map(|v| v.as_object().cloned()).collect();
        for r in rows.iter_mut().filter(|r| r.i("track_id") == 0) {
            if let Some((h, _)) = ix.find(&playlists::item_desc(r)) {
                r.insert("track_id".into(), json!(h.i("id")));
            }
        }
        let prev = snap(st, cid, "likes");
        let new = new_likes(&rows, &prev);
        let keys: HashMap<String, Row> = rows.into_iter().map(|r| (like_key(&r), r)).collect();
        let now_keys: HashSet<String> = keys.keys().cloned().collect();
        if !new.is_empty() {
            if let Ok((n, _)) = playlists::add_tracks(st, lid, &new, &a.conn.s("service")) {
                added_n += n;
            }
        }
        let gone: HashSet<&String> = prev.difference(&now_keys).collect();
        if !gone.is_empty() {
            for it in playlists::items(st, lid) {
                if gone.contains(&like_key(&it)) {
                    let _ = st.db().execute("DELETE FROM playlist_items WHERE id=?", p![it.i("id")]);
                    removed_n += 1;
                }
            }
        }
        remote_now.insert(cid, now_keys);
        rows_by.insert(cid, keys);
    }
    // 1b. auto-download liked songs that aren't in the library yet
    if st.cfg().bool("auto_request_likes") {
        if let Ok(Some(owner)) = st.db().one("SELECT * FROM users WHERE id=? AND disabled=0 AND auto_download=1", p![uid]) {
            if let Ok(rep) = st.request_missing(&owner, lid, true, None) {
                if rep.requested > 0 {
                    st.db().log("info", &format!("Auto-download: {} liked song(s) for {}", rep.requested, owner.s("username")));
                    requested = rep.requested;
                }
            }
        }
    }
    // 2. push Liked Songs back out
    let liked_tracks: HashSet<i64> = playlists::items(st, lid).iter().map(|i| i.i("track_id")).filter(|t| *t > 0).collect();
    for a in ads.iter_mut() {
        let cid = a.conn.i("id");
        let Some(now_keys) = remote_now.get(&cid).cloned() else { continue };
        if !a.can_push() {
            save_snap(st, cid, "likes", &now_keys);
            continue;
        }
        if a.conn.s("service") == "ytmusic" {
            // remember the YouTube videos of matched likes, so they're never searched for again
            for (k, r) in rows_by.get(&cid).cloned().unwrap_or_default() {
                if let (Some(t), Some(v)) = (k.strip_prefix("t:"), r.get("source_id").and_then(|v| v.as_str())) {
                    let _ = st.db().execute("INSERT OR REPLACE INTO sync_state VALUES (0,?,?)", p![format!("ytm:{t}"), v]);
                }
            }
        }
        let mut have: HashSet<i64> = now_keys.iter().filter_map(|k| k.strip_prefix("t:").and_then(|x| x.parse().ok())).collect();
        let res: anyhow::Result<()> = async {
            for t in liked_tracks.difference(&have.clone()) {
                if a.set_like(*t, true).await? {
                    have.insert(*t);
                    pushed += 1;
                }
            }
            for t in have.clone().difference(&liked_tracks) {
                if a.set_like(*t, false).await? {
                    have.remove(t);
                    pushed += 1;
                }
            }
            Ok(())
        }.await;
        if let Err(e) = res {
            errors += 1;
            set_error(st, cid, format!("push likes: {e}"));
            log.push(format!("{}: {e}", a.conn.s("service")));
        }
        let mut snapv: HashSet<String> = now_keys.into_iter().filter(|k| !k.starts_with("t:")).collect();
        snapv.extend(have.iter().map(|t| format!("t:{t}")));
        save_snap(st, cid, "likes", &snapv);
    }
    // 3. mirror playlists to their targets
    // the user's own playlists and Liked Songs: someone else's added from a link is never mirrored
    for pl in st.db().query("SELECT * FROM playlists WHERE user_id=? AND kind IN ('playlist','liked')", p![uid]).unwrap_or_default() {
        let targets: Vec<String> = serde_json::from_str(&pl.s("sync_targets")).unwrap_or_default();
        if targets.is_empty() {
            continue;
        }
        let mut remote: serde_json::Map<String, Value> = serde_json::from_str(&pl.s("remote_ids")).unwrap_or_default();
        let tids: Vec<i64> = playlists::items(st, pl.i("id")).iter().map(|i| i.i("track_id")).filter(|t| *t > 0).collect();
        for svc in targets {
            let Some(a) = ads.iter_mut().find(|a| a.conn.s("service") == svc) else { continue };
            if a.conn.i("sync_playlists") == 0 || !a.can_push() {
                continue;
            }
            match a.push_playlist(&pl.s("name"), &tids, remote.get(&svc).and_then(|v| v.as_str()).map(String::from)).await {
                Ok(rid) => {
                    if let Some(r) = rid {
                        remote.insert(svc.clone(), json!(r));
                    }
                    pls += 1;
                }
                Err(e) => {
                    errors += 1;
                    set_error(st, a.conn.i("id"), format!("playlist '{}': {e}", pl.s("name")));
                    log.push(format!("{svc} playlist {}: {e}", pl.s("name")));
                }
            }
        }
        let _ = st.db().execute("UPDATE playlists SET remote_ids=? WHERE id=?", p![Value::Object(remote).to_string(), pl.i("id")]);
    }
    let t = now();
    for a in &ads {
        let cid = a.conn.i("id");
        let errored = st.db().one("SELECT 1 FROM connections WHERE id=? AND last_error IS NOT NULL", p![cid]).ok().flatten().is_some();
        if !errored || remote_now.contains_key(&cid) {
            let _ = st.db().execute("UPDATE connections SET last_sync=? WHERE id=?", p![t, cid]);
        }
    }
    let mut out = json!({ "liked_added": added_n, "liked_removed": removed_n, "pushed": pushed, "playlists": pls, "errors": errors });
    if requested > 0 {
        out["requested"] = json!(requested);
    }
    out
}

/// A playlist imported from a media server (source_url = "service:id").
pub async fn fetch_remote_playlist(st: &impl Ctx, pl: &Row) -> anyhow::Result<Value> {
    let src = pl.s("source_url");
    let (svc, rid) = src.split_once(':').ok_or_else(|| anyhow::anyhow!("not a media-server playlist"))?;
    let c = connections::get(st, pl.i("user_id"), svc).ok_or_else(|| anyhow::anyhow!("{svc} is no longer connected"))?;
    let mut a = Adapter::new(st, c);
    let rows = a.playlist_tracks(rid).await?;
    let map = a.item_map().await?;
    let mut tracks = vec![];
    for r in rows {
        let lib = map.to_track.get(&s(&r["id"])).and_then(|t| st.db().one("SELECT id, title, artist, album, duration, isrc, mb_trackid FROM tracks WHERE id=?", p![t]).ok().flatten());
        tracks.push(match lib {
            Some(t) => json!({ "title": t["title"], "artist": t["artist"], "album": t["album"], "duration": t["duration"], "isrc": t["isrc"], "mbid": t["mb_trackid"], "track_id": t["id"] }),
            None => json!({ "title": r["title"], "artist": r["artist"], "album": r["album"] }),
        });
    }
    Ok(json!({ "name": pl.s("name"), "tracks": tracks }))
}

/// Refresh followed playlists, re-match, then sync everyone (scheduled, or from Settings).
pub async fn sync_all(st: impl Ctx) {
    if !st.jobs().start("sync", json!({})) {
        return;
    }
    for pl in st.db().query("SELECT * FROM playlists WHERE follow=1", p![]).unwrap_or_default() {
        match playlists::refresh_followed(&st, &pl).await {
            Ok((added, _)) => {
                if added > 0 {
                    st.db().log("info", &format!("Playlist '{}': {added} new track(s) from {}", pl.s("name"), pl.s("source")));
                }
                if st.cfg().bool("auto_request_followed") {
                    if let Ok(Some(owner)) = st.db().one("SELECT * FROM users WHERE id=? AND disabled=0 AND auto_download=1", p![pl.i("user_id")]) {
                        if let Ok(rep) = st.request_missing(&owner, pl.i("id"), true, None) {
                            if rep.requested > 0 {
                                st.db().log("info", &format!("Auto-download: {} new track(s) from followed playlist '{}'", rep.requested, pl.s("name")));
                            }
                        }
                    }
                }
            }
            Err(e) => st.db().log("warning", &format!("Couldn't refresh playlist '{}': {e}", pl.s("name"))),
        }
    }
    // every unmatched playlist item against the library, on the blocking pool
    let st2 = st.clone();
    let _ = tokio::task::spawn_blocking(move || playlists::rematch(&st2, None, true)).await;
    for u in st.db().query("SELECT DISTINCT user_id FROM connections", p![]).unwrap_or_default() {
        let mut log = vec![];
        sync_user(&st, u.i("user_id"), &mut log).await;
    }
    st.jobs().set("sync", json!({ "running": false, "last": now() }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn like(track: i64) -> Row {
        json!({ "track_id": track, "title": format!("Song {track}") }).as_object().cloned().unwrap()
    }

    #[test]
    fn new_likes_go_in_oldest_first() {
        // a service lists 5 (newest) .. 1; 2 was there at the last sync, and 4 is listed twice
        let rows: Vec<Row> = [5, 4, 3, 4, 2, 1].into_iter().map(like).collect();
        let prev: HashSet<String> = ["t:2".to_string()].into();
        let got: Vec<i64> = new_likes(&rows, &prev).iter().map(|v| v["track_id"].as_i64().unwrap()).collect();
        assert_eq!(got, [1, 3, 4, 5]);
        // the same list again: nothing new
        let all: HashSet<String> = rows.iter().map(like_key).collect();
        assert!(new_likes(&rows, &all).is_empty());
    }
}
