//! Tag albums from MusicBrainz, Deezer, Apple Music or Discogs, or by audio fingerprint; cover-art choices.
//! Every provider returns releases shaped like mb::release(), so preview → retag works the same.
//! Release ids carry their provider: "deezer:123", "apple:456", "discogs:r789"; plain UUIDs are MusicBrainz.
use crate::catalog::dur;
use encore_core::json::text as s;
use crate::Ctx;
use crate::acoustid;
use encore_http::{apple, coverartarchive, deezer, discogs, json_any_status as getj, mb};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;

/// The places the tagger searches for a release: (id, name).
pub const PROVIDERS: [(&str, &str); 5] = [("musicbrainz", "MusicBrainz"), ("deezer", "Deezer"), ("apple", "Apple Music"), ("discogs", "Discogs"), ("fingerprint", "Audio fingerprint (AcoustID)")];


/// First letter upper case, the rest lower case.
fn title_case(x: &str) -> String {
    let mut c = x.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase()).unwrap_or_default()
}

/// A release as the tagger lists it.
#[allow(clippy::too_many_arguments)]
fn rel(id: &str, title: &str, artist: &str, date: &str, tracks: Vec<Value>, provider: &str, extra: Value) -> Value {
    let discs: std::collections::HashSet<i64> = tracks.iter().map(|t| t["disc"].as_i64().unwrap_or(1)).collect();
    let g = |k: &str| extra.get(k).cloned().unwrap_or_else(|| json!(""));
    json!({ "id": id, "title": title, "artist": artist, "artist_id": null, "date": date, "year": date.chars().take(4).collect::<String>(),
        "country": g("country"), "status": "Official", "release_group_id": null, "type": g("type"), "label": g("label"), "catalog": g("catalog"),
        "barcode": g("barcode"), "disc_count": discs.len().max(1), "genre": g("genre"), "tracks": tracks, "cover": extra.get("cover").cloned().unwrap_or(Value::Null),
        "cover_url": extra.get("cover_big").or(extra.get("cover")).cloned().unwrap_or(Value::Null), "provider": provider })
}

/// A track of such a release.
fn trk(title: &str, artist: &str, track: i64, disc: i64, length: f64, isrc: Value) -> Value {
    json!({ "title": title, "artist": artist, "artist_id": null, "track": track, "disc": disc.max(1), "length": length, "recording_id": null, "track_id": null, "isrc": isrc })
}

/// Search `provider` for releases of `album` by `artist` (`fingerprint`: by the files' fingerprints; anything else: MusicBrainz).
pub async fn search(st: &impl Ctx, provider: &str, artist: &str, album: &str, n: usize, paths: &[PathBuf]) -> anyhow::Result<Vec<Value>> {
    match provider {
        "deezer" => {
            let q = if artist.is_empty() { album.to_string() } else { format!("artist:\"{artist}\" album:\"{album}\"") };
            let mut data = getj(deezer::search(st, "album", &q, 20)).await?["data"].as_array().cloned().unwrap_or_default();
            if data.is_empty() {
                data = getj(deezer::search(st, "album", &format!("{artist} {album}"), 20)).await?["data"].as_array().cloned().unwrap_or_default();
            }
            Ok(data.iter().map(|a| json!({ "id": format!("deezer:{}", a["id"]), "title": s(&a["title"]), "artist": s(&a["artist"]["name"]), "date": s(&a["release_date"]),
                "country": "", "track_count": a["nb_tracks"].as_i64().unwrap_or(0), "formats": "Digital", "type": title_case(&s(&a["record_type"])),
                "label": "", "score": 90, "cover": a["cover_medium"], "provider": "Deezer" })).collect())
        }
        "apple" => {
            let d = getj(apple::search(st, &format!("{artist} {album}"), "album", 20)).await?;
            Ok(d["results"].as_array().into_iter().flatten().filter(|a| !a["collectionId"].is_null()).map(|a| json!({
                "id": format!("apple:{}", a["collectionId"]), "title": s(&a["collectionName"]), "artist": s(&a["artistName"]),
                "date": s(&a["releaseDate"]).chars().take(10).collect::<String>(), "country": s(&a["country"]), "track_count": a["trackCount"].as_i64().unwrap_or(0),
                "formats": "Digital", "type": "", "label": s(&a["copyright"]).chars().take(60).collect::<String>(), "score": 90,
                "cover": s(&a["artworkUrl100"]).replace("100x100bb", "300x300bb"), "provider": "Apple Music" })).collect())
        }
        "discogs" => {
            if st.cfg().str("discogs_token").is_empty() {
                anyhow::bail!("Add a Discogs token in Settings to search Discogs");
            }
            let d: Value = discogs::search(st, &[("artist", artist), ("release_title", album), ("type", "release"), ("per_page", "20")]).send().await?.json().await?;
            Ok(d["results"].as_array().into_iter().flatten().map(|x| {
                let join = |v: &Value, n: usize| v.as_array().map(|a| a.iter().take(n).map(s).collect::<Vec<_>>().join(", ")).unwrap_or_default();
                json!({ "id": format!("discogs:r{}", x["id"]), "title": s(&x["title"]), "artist": "", "date": s(&x["year"]), "country": s(&x["country"]),
                    "track_count": 0, "formats": join(&x["format"], 9), "type": "", "label": join(&x["label"], 2), "score": 80, "cover": x["thumb"], "provider": "Discogs" })
            }).collect())
        }
        "fingerprint" => by_fingerprint(st, paths, n).await,
        _ => {
            let a = if artist == "Unknown Artist" { "" } else { artist };
            let mut out = mb::search_releases(st, a, album, 20).await?;
            for r in out.iter_mut() {
                r["provider"] = json!("MusicBrainz");
                r["formats"] = json!("");
                r["label"] = json!("");
            }
            Ok(out)
        }
    }
}

/// Identify the songs by their audio (names don't matter), then find the releases most of them are on.
async fn by_fingerprint(st: &impl Ctx, paths: &[PathBuf], n: usize) -> anyhow::Result<Vec<Value>> {
    if !acoustid::available(st) {
        anyhow::bail!("Audio fingerprinting needs an AcoustID key (Settings) and fpcalc");
    }
    let mut votes: HashMap<String, usize> = HashMap::new();
    for p in paths.iter().take(30) {
        let Some((d, fp)) = acoustid::fingerprint(p).await else { continue };
        let Ok(res) = encore_http::acoustid::lookup(st, d, &fp).await else { continue };
        let Some(best) = res.first().filter(|b| b["score"].as_f64().unwrap_or(0.0) >= 0.7) else { continue };
        let rgs: std::collections::HashSet<String> = best["recordings"].as_array().into_iter().flatten()
            .flat_map(|r| r["release_group_ids"].as_array().cloned().unwrap_or_default()).filter_map(|g| g.as_str().map(String::from)).collect();
        for g in rgs {
            *votes.entry(g).or_default() += 1;
        }
    }
    let mut ranked: Vec<(String, usize)> = votes.into_iter().collect();
    ranked.sort_by_key(|x| std::cmp::Reverse(x.1));
    let mut out = vec![];
    for (rg, hits) in ranked.into_iter().take(6) {
        let Some(rid) = mb::best_release_for_group(st, &rg, Some(n as i64)).await? else { continue };
        let r = mb::release(st, &rid).await?;
        out.push(json!({ "id": rid, "title": r["title"], "artist": r["artist"], "date": r["date"], "country": r["country"],
            "track_count": r["tracks"].as_array().map(|a| a.len()).unwrap_or(0), "formats": "", "type": r["type"], "label": r["label"],
            "score": (100 * hits / paths.len().max(1)), "cover": r["cover"], "provider": format!("Fingerprint · {hits}/{} songs", paths.len()) }));
    }
    Ok(out)
}

/// A release by its id (`deezer:…`, `apple:…`, `discogs:…`, else a MusicBrainz id) with its tracks.
pub async fn release(st: &impl Ctx, id: &str) -> anyhow::Result<Value> {
    let (kind, rid) = id.split_once(':').unwrap_or(("", id));
    match kind {
        "deezer" => {
            let a = getj(deezer::album(st, rid)).await?;
            let tr = getj(deezer::album_tracks(st, rid, 500)).await?["data"].as_array().cloned().unwrap_or_default();
            let mut isrcs: HashMap<String, Value> = HashMap::new();
            if tr.first().is_some_and(|t| t["isrc"].is_null()) {
                // older albums: ISRCs only on each track's own page (5 at a time)
                let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(5));
                let mut set = tokio::task::JoinSet::new();
                for t in tr.iter().take(60) {
                    let (st, sem, id) = (st.clone(), sem.clone(), t["id"].to_string());
                    set.spawn(async move {
                        let _p = sem.acquire().await;
                        let isrc = getj(deezer::track(&st, &id)).await.map(|d| d["isrc"].clone()).unwrap_or(Value::Null);
                        (id, isrc)
                    });
                }
                while let Some(Ok((id, isrc))) = set.join_next().await {
                    isrcs.insert(id, isrc);
                }
            }
            let tracks = tr.iter().enumerate().map(|(i, t)| {
                let isrc = if t["isrc"].is_null() { isrcs.get(&t["id"].to_string()).cloned().unwrap_or(Value::Null) } else { t["isrc"].clone() };
                trk(&s(&t["title"]), &s(&t["artist"]["name"]), t["track_position"].as_i64().unwrap_or(i as i64 + 1), t["disk_number"].as_i64().unwrap_or(1),
                    t["duration"].as_f64().unwrap_or(0.0), isrc)
            }).collect();
            Ok(rel(id, &s(&a["title"]), &s(&a["artist"]["name"]), &s(&a["release_date"]), tracks, "Deezer", json!({
                "label": s(&a["label"]), "barcode": s(&a["upc"]), "genre": s(&a["genres"]["data"][0]["name"]), "type": title_case(&s(&a["record_type"])),
                "cover": a["cover_medium"], "cover_big": a["cover_xl"] })))
        }
        "apple" => {
            let r = getj(apple::lookup(st, rid, "song", Some(300))).await?["results"].as_array().cloned().unwrap_or_default();
            let col = r.iter().find(|x| x["wrapperType"] == "collection").cloned().unwrap_or_default();
            let tracks = r.iter().filter(|x| x["wrapperType"] == "track").map(|x| trk(&s(&x["trackName"]), &s(&x["artistName"]), x["trackNumber"].as_i64().unwrap_or(0),
                x["discNumber"].as_i64().unwrap_or(1), x["trackTimeMillis"].as_f64().unwrap_or(0.0) / 1000.0, Value::Null)).collect();
            let art = s(&col["artworkUrl100"]);
            Ok(rel(id, &s(&col["collectionName"]), &s(&col["artistName"]), &s(&col["releaseDate"]).chars().take(10).collect::<String>(), tracks, "Apple Music", json!({
                "genre": s(&col["primaryGenreName"]), "label": s(&col["copyright"]).chars().take(80).collect::<String>(), "country": s(&col["country"]),
                "cover": art.replace("100x100bb", "500x500bb"), "cover_big": art.replace("100x100bb", "3000x3000bb") })))
        }
        "discogs" => {
            let path = match rid.strip_prefix('m') { Some(m) => format!("masters/{m}"), None => format!("releases/{}", &rid[1..]) };
            let d: Value = discogs::get(st, &path).send().await?.json().await?;
            let names = |v: &Value| v.as_array().map(|a| a.iter().map(|x| s(&x["name"])).collect::<Vec<_>>().join(", ")).unwrap_or_default();
            let artist = names(&d["artists"]);
            let tracks = d["tracklist"].as_array().into_iter().flatten().filter(|t| t["type_"].as_str().unwrap_or("track") == "track").enumerate()
                .map(|(i, t)| { let a = names(&t["artists"]); trk(&s(&t["title"]), if a.is_empty() { &artist } else { &a }, i as i64 + 1, 1, dur(&t["duration"]).as_f64().unwrap_or(0.0), Value::Null) }).collect();
            let imgs = d["images"].as_array().cloned().unwrap_or_default();
            let img = imgs.iter().find(|i| i["type"] == "primary").or(imgs.first()).map(|i| i["uri"].clone()).unwrap_or(Value::Null);
            let labels = d["labels"].as_array().cloned().unwrap_or_default();
            let barcode = d["identifiers"].as_array().into_iter().flatten().find(|i| i["type"] == "Barcode").map(|i| s(&i["value"])).unwrap_or_default();
            let date = if d["released"].is_null() { s(&d["year"]) } else { s(&d["released"]) };
            Ok(rel(id, &s(&d["title"]), &artist, &date, tracks, "Discogs", json!({
                "label": labels.iter().take(2).map(|l| s(&l["name"])).collect::<Vec<_>>().join(", "), "catalog": labels.iter().take(2).map(|l| s(&l["catno"])).collect::<Vec<_>>().join(", "),
                "barcode": barcode, "country": s(&d["country"]), "genre": s(&d["genres"][0]), "cover": img, "cover_big": img })))
        }
        _ => {
            let mut r = mb::release(st, id).await?;
            r["provider"] = json!("MusicBrainz");
            Ok(r)
        }
    }
}

/// Cover art choices from every catalogue (Cover Art Archive, Deezer, Apple Music, YouTube Music, Discogs).
pub async fn art_candidates(st: &impl Ctx, artist: &str, album: &str, rgid: Option<&str>, release_id: Option<&str>) -> Vec<Value> {
    let caa = async {
        let mut ids: Vec<(&str, String)> = vec![];
        if let Some(r) = release_id.filter(|x| !x.is_empty()) {
            ids.push(("release", r.into()));
        }
        if let Some(g) = rgid.filter(|x| !x.is_empty()) {
            ids.push(("release-group", g.into()));
        }
        if ids.is_empty() {
            let q = format!("release:\"{}\" AND artist:\"{}\"", mb::esc(album), mb::esc(artist));
            for g in mb::search_albums(st, &q, 4).await.unwrap_or_default() {
                ids.push(("release-group", s(&g["id"])));
            }
        }
        let mut out = vec![];
        for (kind, i) in ids.into_iter().take(4) {
            let Ok(d) = getj(coverartarchive::images(st, kind, &i)).await else { continue };
            for img in d["images"].as_array().into_iter().flatten().take(6) {
                if img["front"].as_bool().unwrap_or(false) || img["types"].as_array().is_some_and(|t| t.iter().any(|x| x == "Front")) {
                    let thumb = if img["thumbnails"]["500"].is_string() { img["thumbnails"]["500"].clone() } else { img["image"].clone() };
                    out.push(json!({ "url": img["image"], "thumb": thumb, "source": "Cover Art Archive", "label": kind.replace('-', " ") }));
                }
            }
        }
        out
    };
    let deezer = async {
        getj(deezer::search(st, "album", &format!("{artist} {album}"), 8)).await.map(|d| d["data"].as_array().cloned().unwrap_or_default()
            .iter().filter(|a| a["cover_xl"].is_string()).map(|a| json!({ "url": a["cover_xl"], "thumb": a["cover_medium"], "source": "Deezer",
                "label": format!("{} – {}", s(&a["artist"]["name"]), s(&a["title"])), "size": "1000×1000" })).collect::<Vec<_>>()).unwrap_or_default()
    };
    let apple = async {
        getj(apple::search(st, &format!("{artist} {album}"), "album", 8)).await
            .map(|d| d["results"].as_array().into_iter().flatten().filter(|a| a["artworkUrl100"].is_string()).map(|a| {
                let u = s(&a["artworkUrl100"]);
                json!({ "url": u.replace("100x100bb", "3000x3000bb"), "thumb": u.replace("100x100bb", "300x300bb"), "source": "Apple Music",
                    "label": format!("{} – {}", s(&a["artistName"]), s(&a["collectionName"])), "size": "up to 3000×3000" })
            }).collect::<Vec<_>>()).unwrap_or_default()
    };
    let ytm = async {
        encore_http::ytmusic::search(st, &format!("{artist} {album}"), false).await.unwrap_or_default().iter().take(6).filter_map(|a| {
            let th = a["cover"].as_str()?;
            let big = match th.split_once("=w") { Some((b, _)) => format!("{b}=w1200-h1200-l90-rj"), None => th.to_string() };
            Some(json!({ "url": big, "thumb": th, "source": "YouTube Music", "label": a["title"], "size": "1200×1200" }))
        }).collect::<Vec<_>>()
    };
    let discogs = async {
        if st.cfg().str("discogs_token").is_empty() {
            return vec![];
        }
        let Ok(r) = discogs::search(st, &[("artist", artist), ("release_title", album), ("type", "release"), ("per_page", "6")]).send().await else { return vec![] };
        r.json::<Value>().await.map(|d| d["results"].as_array().into_iter().flatten()
            .filter(|x| x["cover_image"].as_str().is_some_and(|c| !c.contains("spacer.gif")))
            .map(|x| json!({ "url": x["cover_image"], "thumb": if x["thumb"].is_string() { x["thumb"].clone() } else { x["cover_image"].clone() }, "source": "Discogs", "label": x["title"] }))
            .collect()).unwrap_or_default()
    };
    let (a, b, c, d, e) = tokio::join!(caa, deezer, apple, ytm, discogs);
    let mut seen = std::collections::HashSet::new();
    [a, b, c, d, e].into_iter().flatten().filter(|x| x["url"].as_str().is_some_and(|u| seen.insert(u.to_string()))).collect()
}
