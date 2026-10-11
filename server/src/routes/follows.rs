//! /api/follows: follow artists – new releases on MusicBrainz are requested automatically.
use crate::auth::{self, User};
use crate::db::{now, Row, RowExt};
use crate::error::{err, ApiResult};
use crate::routes::requests::{create_request, NewRequest};
use crate::state::AppState;
use crate::{jobs, library, mb, notify, p};
use axum::extract::{Path, State};
use axum::routing::{patch, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/follows", axum::routing::get(list).post(follow_route))
        .route("/api/follows/check", post(check_now))
        .route("/api/follows/library", post(follow_library).get(follow_library_status))
        .route("/api/follows/{fid}", patch(update).delete(unfollow))
}

const TYPES: [&str; 3] = ["Album", "EP", "Single"];

fn wanted(rg: &Value, types: &[String]) -> bool {
    // studio releases only: compilations, live albums, soundtracks, remixes … are skipped
    types.iter().any(|t| rg["type"] == t.as_str()) && rg["secondary"].as_array().is_none_or(|a| a.is_empty())
}

/// Out already (announced releases are picked up once they're released).
fn released(rg: &Value) -> bool {
    let d = rg["date"].as_str().unwrap_or("");
    if d.is_empty() {
        return true;
    }
    let mut parts: Vec<&str> = d.split('-').collect();
    while parts.len() < 3 {
        parts.push("01");
    }
    let full = format!("{}-{:0>2}-{:0>2}", parts[0], parts[1], parts[2]);
    full.as_str() <= crate::routes::discover::today().as_str()
}

fn request(st: &AppState, u: &Row, name: &str, rg: &Value) -> Option<i64> {
    let s = |k: &str| rg[k].as_str().map(String::from);
    let b = NewRequest { kind: "album".into(), mbid: s("id"), artist: Some(name.into()), title: s("title"), album: s("title"), year: s("year"), cover_url: s("cover"), ..Default::default() };
    let rid = create_request(st, u, &b, None, None).ok()?;
    let r = st.db.one("SELECT status, detail FROM requests WHERE id=?", p![rid]).ok()??;
    let mut d: Value = serde_json::from_str(&r.s("detail")).unwrap_or(json!({}));
    d["followed"] = json!(true);
    let _ = st.db.execute("UPDATE requests SET detail=? WHERE id=?", p![d.to_string(), rid]);
    let auto = st.cfg.bool("auto_request_follows") && (auth::is_admin(u) || u.b("auto_download"));
    if auto && r.s("status") == "pending" {
        let _ = st.db.execute("UPDATE requests SET status='queued', message='New release from a followed artist' WHERE id=?", p![rid]);
    }
    Some(rid)
}

/// Start following: existing releases count as seen (or are requested now with backfill).
pub async fn follow(st: &AppState, u: &Row, mbid: &str, name: Option<&str>, types: Option<Vec<String>>, backfill: bool) -> anyhow::Result<Value> {
    let mut types: Vec<String> = types.unwrap_or(vec!["Album".into(), "EP".into()]).into_iter().filter(|t| TYPES.contains(&t.as_str())).collect();
    if types.is_empty() {
        types = vec!["Album".into()];
    }
    let art = mb::artist(st, mbid).await?;
    if let Some(ex) = st.db.one("SELECT id FROM follows WHERE user_id=? AND mbid=?", p![u.i("id"), mbid])? {
        st.db.execute("UPDATE follows SET types=? WHERE id=?", p![json!(types).to_string(), ex.i("id")])?;
        return Ok(json!({ "id": ex.i("id"), "requested": 0 }));
    }
    let aname = art["name"].as_str().unwrap_or("").to_string();
    let name = name.filter(|n| !n.is_empty()).unwrap_or(&aname).to_string();
    let t = now();
    let fid = st.db.execute("INSERT INTO follows (user_id, mbid, name, types, seen, created_at, last_checked) VALUES (?,?,?,?,?,?,?)",
        p![u.i("id"), mbid, name, json!(types).to_string(), "[]", t, t])?;
    let (mut seen, mut requested) = (vec![], 0);
    for rg in art["release_groups"].as_array().cloned().unwrap_or_default() {
        if !released(&rg) {
            continue;
        }
        seen.push(rg["id"].clone());
        let id = rg["id"].as_str().unwrap_or("");
        if backfill && wanted(&rg, &types) && !library::has_release_group(&st.db, id) && !library::has_album(&st.db, &aname, rg["title"].as_str().unwrap_or("")) && request(st, u, &name, &rg).is_some() {
            requested += 1;
        }
    }
    st.db.execute("UPDATE follows SET seen=? WHERE id=?", p![json!(seen).to_string(), fid])?;
    st.db.log("info", &format!("{} followed {aname}{}", u.s("username"), if requested > 0 { format!(" ({requested} missing release(s) requested)") } else { String::new() }));
    Ok(json!({ "id": fid, "requested": requested }))
}

/// Releases that appeared since the last check; the wanted ones are requested.
pub async fn check(st: &AppState, f: &Row) -> usize {
    let Ok(Some(u)) = st.db.one("SELECT * FROM users WHERE id=? AND disabled=0", p![f.i("user_id")]) else { return 0 };
    let art = match mb::artist(st, &f.s("mbid")).await {
        Ok(a) => a,
        Err(e) => {
            let _ = st.db.execute("UPDATE follows SET last_error=?, last_checked=? WHERE id=?", p![e.to_string().chars().take(200).collect::<String>(), now(), f.i("id")]);
            return 0;
        }
    };
    let mut seen: HashSet<String> = serde_json::from_str::<Vec<String>>(&f.s("seen")).unwrap_or_default().into_iter().collect();
    let types: Vec<String> = serde_json::from_str(&f.s("types")).unwrap_or(vec!["Album".into(), "EP".into()]);
    let mut new = vec![];
    for rg in art["release_groups"].as_array().cloned().unwrap_or_default() {
        let id = rg["id"].as_str().unwrap_or("").to_string();
        if seen.contains(&id) || !released(&rg) {
            continue;
        }
        seen.insert(id.clone());
        if !wanted(&rg, &types) || library::has_release_group(&st.db, &id) {
            continue;
        }
        if request(st, &u, &f.s("name"), &rg).is_some() {
            new.push(rg);
        }
    }
    let mut s: Vec<&String> = seen.iter().collect();
    s.sort();
    let _ = st.db.execute("UPDATE follows SET seen=?, last_checked=?, last_error=NULL, found_count=found_count+?, last_found=COALESCE(?, last_found) WHERE id=?",
        p![json!(s).to_string(), now(), new.len(), if new.is_empty() { None } else { Some(now()) }, f.i("id")]);
    for rg in &new {
        let typ = rg["type"].as_str().unwrap_or("release");
        st.db.log("info", &format!("New release from {}: {} ({typ}) – requested", f.s("name"), rg["title"].as_str().unwrap_or("")));
        notify::send(st, &format!("🆕 New {} from {}", typ.to_lowercase(), f.s("name")), rg["title"].as_str().unwrap_or(""), rg["cover"].as_str()).await;
    }
    new.len()
}

pub async fn check_all(st: &AppState, uid: Option<i64>) -> usize {
    let rows = match uid {
        Some(u) => st.db.query("SELECT * FROM follows WHERE user_id=? ORDER BY last_checked", p![u]),
        None => st.db.query("SELECT * FROM follows ORDER BY last_checked", p![]),
    }.unwrap_or_default();
    let mut n = 0;
    for f in rows {
        n += check(st, &f).await;
    }
    n
}

fn row(f: &Row) -> Value {
    let mut v = json!({ "types": serde_json::from_str::<Value>(&f.s("types")).unwrap_or(json!([])),
        "known_releases": serde_json::from_str::<Vec<Value>>(&f.s("seen")).map(|a| a.len()).unwrap_or(0) });
    for k in ["id", "mbid", "name", "last_checked", "last_found", "found_count", "last_error", "created_at"] {
        v[k] = f.get(k).cloned().unwrap_or(Value::Null);
    }
    v
}

async fn list(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    Ok(Json(json!(st.db.query("SELECT * FROM follows WHERE user_id=? ORDER BY lower(name)", p![u.i("id")])?.iter().map(row).collect::<Vec<_>>())))
}

#[derive(Deserialize)]
struct FollowBody {
    mbid: String,
    name: Option<String>,
    types: Option<Vec<String>>,
    #[serde(default)]
    backfill: bool,
}

async fn follow_route(State(st): State<AppState>, User(u): User, Json(b): Json<FollowBody>) -> ApiResult<Json<Value>> {
    follow(&st, &u, &b.mbid, b.name.as_deref(), b.types, b.backfill).await.map(Json).map_err(|e| err(502, format!("MusicBrainz error: {e}")))
}

#[derive(Deserialize)]
struct FollowPatch {
    types: Option<Vec<String>>,
}

async fn update(State(st): State<AppState>, User(u): User, Path(fid): Path<i64>, Json(b): Json<FollowPatch>) -> ApiResult<Json<Value>> {
    st.db.one("SELECT 1 FROM follows WHERE id=? AND user_id=?", p![fid, u.i("id")])?.ok_or_else(|| err(404, "Not following"))?;
    if let Some(t) = b.types {
        let mut t: Vec<String> = t.into_iter().filter(|x| TYPES.contains(&x.as_str())).collect();
        if t.is_empty() {
            t = vec!["Album".into()];
        }
        st.db.execute("UPDATE follows SET types=? WHERE id=?", p![json!(t).to_string(), fid])?;
    }
    Ok(Json(row(&st.db.one("SELECT * FROM follows WHERE id=?", p![fid])?.unwrap())))
}

async fn unfollow(State(st): State<AppState>, User(u): User, Path(fid): Path<i64>) -> ApiResult<Json<Value>> {
    st.db.execute("DELETE FROM follows WHERE id=? AND user_id=?", p![fid, u.i("id")])?;
    Ok(Json(json!({ "ok": true })))
}

async fn check_now(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "new_releases": check_all(&st, Some(u.i("id"))).await })))
}

/// Follow every album artist in the library (background: MusicBrainz allows 1 request/second).
async fn follow_library(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    let job = format!("follow_library:{}", u.i("id"));
    if !jobs::start(&job, json!({})) {
        return Err(err(409, "Already running"));
    }
    tokio::spawn(async move {
        let rows = st.db.query("SELECT albumartist name, MAX(mb_albumartistid) mbid FROM tracks WHERE albumartist NOT IN ('', 'Unknown Artist', 'Various Artists') GROUP BY lower(albumartist)", p![]).unwrap_or_default();
        let mut have: HashSet<String> = st.db.query("SELECT mbid FROM follows WHERE user_id=?", p![u.i("id")]).unwrap_or_default().iter().map(|r| r.s("mbid")).collect();
        let (mut added, mut unmatched) = (0, vec![]);
        for r in rows {
            let name = r.s("name");
            let mut mbid = r.s("mbid");
            if mbid.is_empty() {
                let hits = mb::search_artists(&st, &name, 3).await.unwrap_or_default();
                let exact: Vec<&Value> = hits.iter().filter(|h| h["name"].as_str().unwrap_or("").to_lowercase() == name.to_lowercase() && h["score"].as_i64().unwrap_or(0) >= 95).collect();
                if exact.len() == 1 {
                    mbid = exact[0]["id"].as_str().unwrap_or("").to_string();
                }
            }
            if mbid.is_empty() {
                unmatched.push(name);
                continue;
            }
            if have.contains(&mbid) {
                continue;
            }
            if follow(&st, &u, &mbid, Some(&name), None, false).await.is_ok() {
                have.insert(mbid);
                added += 1;
            }
        }
        let n = unmatched.len();
        unmatched.truncate(50);
        jobs::set(&job, json!({ "running": false, "followed": added, "unmatched": unmatched, "unmatched_count": n }));
    });
    Ok(Json(json!({ "ok": true })))
}

async fn follow_library_status(User(u): User) -> Json<Value> {
    Json(jobs::get(&format!("follow_library:{}", u.i("id"))))
}
