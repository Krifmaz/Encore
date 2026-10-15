//! Giving back to the databases Encore leans on (Settings → Contribute).
//!
//! - AcoustID: songs whose MusicBrainz recording the audio itself confirmed (acoustid::check_file,
//!   "MusicBrainz recording confirmed by audio") are queued with their fingerprint and sent in batches of up
//!   to 50 with the server's client key and the admin's own user key. Each fingerprint + recording is sent
//!   once; the submission ids are kept and their status checked later.
//! - MusicBrainz seeding: an album found only on Deezer / Apple Music opens the MusicBrainz release editor
//!   with everything filled in (GET /api/contribute/mb-seed/{service}-{id} serves a small page that posts
//!   the form to musicbrainz.org/release/add).
//! - ISRCs: with a MusicBrainz account connected (OAuth2, scope submit_isrc), ISRCs from the files of
//!   audio-confirmed recordings that MusicBrainz lacks are submitted in batches.
use crate::auth::{Admin, MaybeUser, User};
use crate::db::{now, RowExt};
use crate::error::{err, ApiResult};
use crate::p;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

const ACOUSTID_SUBMIT: &str = "https://api.acoustid.org/v2/submit";
const ACOUSTID_STATUS: &str = "https://api.acoustid.org/v2/submission_status";
const MB: &str = "https://musicbrainz.org";
/// AcoustID takes at most this many fingerprints per call
pub const BATCH: usize = 50;
/// ISRCs per MusicBrainz POST
const ISRC_BATCH: usize = 100;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/contribute", get(status))
        .route("/api/contribute/acoustid/run", post(run_acoustid))
        .route("/api/contribute/isrc/run", post(run_isrc))
        .route("/api/contribute/mb/connect", post(mb_connect))
        .route("/api/contribute/mb/callback", get(mb_callback))
        .route("/api/contribute/mb/disconnect", post(mb_disconnect))
        .route("/api/contribute/mb-seed-link", post(seed_link))
        .route("/api/contribute/mb-seed/{key}", get(seed_page))
}

/// What the last runs said, for the Contribute page (kept in memory: a restart starts with a clean slate).
static LAST: Mutex<Option<Value>> = Mutex::new(None);

fn note(kind: &str, ok: bool, message: &str) {
    let mut l = LAST.lock().unwrap();
    let mut v = l.take().unwrap_or_else(|| json!({}));
    v[kind] = json!({ "at": now(), "ok": ok, "message": message });
    *l = Some(v);
}

fn last(kind: &str) -> Value {
    LAST.lock().unwrap().as_ref().map(|v| v[kind].clone()).unwrap_or(Value::Null)
}

fn is_mbid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36 && b.iter().enumerate().all(|(i, c)| if [8, 13, 18, 23].contains(&i) { *c == b'-' } else { c.is_ascii_hexdigit() })
}

/// "us-abc-12-34567" → "USABC1234567" when it is a valid ISRC.
pub fn clean_isrc(s: &str) -> Option<String> {
    let c: String = s.chars().filter(|c| !matches!(c, '-' | ' ')).collect::<String>().to_uppercase();
    let b = c.as_bytes();
    (b.len() == 12 && b[..2].iter().all(|x| x.is_ascii_uppercase()) && b[2..5].iter().all(|x| x.is_ascii_alphanumeric()) && b[5..].iter().all(|x| x.is_ascii_digit()))
        .then_some(c)
}

// ---- AcoustID -------------------------------------------------------------------------------------

/// Queue a fingerprint after a verification (acoustid::check_file's `submit`: only present when the audio
/// confirmed the recording). Never queues the same fingerprint + recording twice.
pub fn queue_acoustid(st: &AppState, track_id: Option<i64>, res: &Value) {
    let s = &res["submit"];
    let (Some(mbid), Some(fp), Some(dur)) = (s["mbid"].as_str(), s["fingerprint"].as_str(), s["duration"].as_f64()) else { return };
    if res["status"] != "verified" || !is_mbid(mbid) || fp.is_empty() || dur < 1.0 {
        return;
    }
    let hash = hex::encode(Sha256::digest(fp.as_bytes()));
    if let Some(t) = track_id {
        // the same song verified again (a library re-check): already queued or sent for this recording
        if st.db.one("SELECT 1 FROM contrib_acoustid WHERE track_id=? AND mbid=?", p![t, mbid]).ok().flatten().is_some() {
            return;
        }
    }
    let _ = st.db.execute("INSERT OR IGNORE INTO contrib_acoustid (track_id, fp_hash, fingerprint, duration, mbid, status, created_at) VALUES (?,?,?,?,?,'queued',?)",
        p![track_id, hash, fp, dur.round() as i64, mbid, now()]);
}

/// The form fields of one AcoustID submit call (indexes .0 … .49).
pub fn acoustid_form(client: &str, user: &str, items: &[(String, i64, String)]) -> Vec<(String, String)> {
    let mut f = vec![("client".to_string(), client.to_string()), ("clientversion".to_string(), crate::VERSION.to_string()),
                     ("user".to_string(), user.to_string()), ("format".to_string(), "json".to_string())];
    for (i, (fp, dur, mbid)) in items.iter().take(BATCH).enumerate() {
        f.push((format!("duration.{i}"), dur.to_string()));
        f.push((format!("fingerprint.{i}"), fp.clone()));
        f.push((format!("mbid.{i}"), mbid.clone()));
    }
    f
}

/// {"submissions":[{"index":"0","id":123,"status":"pending"}]} → (index, id, status), by position when
/// the index is missing.
pub fn parse_submissions(d: &Value) -> Vec<(usize, i64, String)> {
    d["submissions"].as_array().cloned().unwrap_or_default().iter().enumerate().filter_map(|(n, s)| {
        let idx = s["index"].as_str().and_then(|x| x.parse().ok()).or(s["index"].as_u64().map(|x| x as usize)).unwrap_or(n);
        Some((idx, s["id"].as_i64()?, s["status"].as_str().unwrap_or("pending").to_string()))
    }).collect()
}

async fn pace() {
    // AcoustID allows 3 requests a second
    static NEXT: tokio::sync::Mutex<Option<std::time::Instant>> = tokio::sync::Mutex::const_new(None);
    let mut n = NEXT.lock().await;
    if let Some(t) = *n {
        let now = std::time::Instant::now();
        if t > now {
            tokio::time::sleep(t - now).await;
        }
    }
    *n = Some(std::time::Instant::now() + Duration::from_millis(400));
}

fn acoustid_ready(st: &AppState) -> Result<(String, String), &'static str> {
    let (client, user) = (st.cfg.str("acoustid_api_key"), st.cfg.str("acoustid_user_key"));
    if !st.cfg.bool("contribute_acoustid") {
        return Err("Fingerprint submissions are off");
    }
    if client.is_empty() {
        return Err("Add the AcoustID API key under Apps & sources → Verification first");
    }
    if user.is_empty() {
        return Err("Add your AcoustID user key to send fingerprints");
    }
    Ok((client, user))
}

/// Send queued fingerprints (up to `calls` batches), then check on earlier submissions. → (sent, imported)
pub async fn acoustid_pass(st: &AppState, calls: usize) -> anyhow::Result<(usize, usize)> {
    let (client, user) = acoustid_ready(st).map_err(|e| anyhow::anyhow!(e))?;
    let mut sent = 0;
    for _ in 0..calls {
        let rows = st.db.query("SELECT id, fingerprint, duration, mbid FROM contrib_acoustid WHERE status='queued' ORDER BY id LIMIT ?", p![BATCH as i64])?;
        if rows.is_empty() {
            break;
        }
        let items: Vec<(String, i64, String)> = rows.iter().map(|r| (r.s("fingerprint"), r.i("duration"), r.s("mbid"))).collect();
        pace().await;
        let d: Value = st.http.post(ACOUSTID_SUBMIT).form(&acoustid_form(&client, &user, &items)).send().await?.json().await?;
        if d["status"] != "ok" {
            let msg = d["error"]["message"].as_str().unwrap_or("AcoustID error").to_string();
            // a bad key fails every item the same way: stop and say so, leave the queue as it is
            anyhow::bail!("AcoustID did not take the fingerprints: {msg}");
        }
        let subs = parse_submissions(&d);
        let t = now();
        for (n, r) in rows.iter().enumerate() {
            match subs.iter().find(|s| s.0 == n) {
                Some((_, id, status)) => {
                    st.db.execute("UPDATE contrib_acoustid SET status=?, submission_id=?, submitted_at=?, error=NULL WHERE id=?",
                        p![if status == "imported" { "imported" } else { "submitted" }, id, t, r.i("id")])?;
                }
                None => {
                    st.db.execute("UPDATE contrib_acoustid SET status='error', error='no submission id in the answer', submitted_at=? WHERE id=?", p![t, r.i("id")])?;
                }
            }
        }
        sent += rows.len();
    }
    // submissions are imported within minutes; look at the ones older than a minute
    let mut imported = 0;
    let pending = st.db.query("SELECT id, submission_id FROM contrib_acoustid WHERE status='submitted' AND submission_id IS NOT NULL AND submitted_at < ?
        AND (checked_at IS NULL OR checked_at < ?) ORDER BY submitted_at LIMIT 200", p![now() - 60.0, now() - 600.0])?;
    for chunk in pending.chunks(BATCH) {
        let mut q: Vec<(String, String)> = vec![("client".into(), client.clone()), ("clientversion".into(), crate::VERSION.to_string()), ("format".into(), "json".into())];
        q.extend(chunk.iter().map(|r| ("id".to_string(), r.i("submission_id").to_string())));
        pace().await;
        let d: Value = st.http.get(ACOUSTID_STATUS).query(&q).send().await?.json().await?;
        if d["status"] != "ok" {
            anyhow::bail!("AcoustID status check failed: {}", d["error"]["message"].as_str().unwrap_or("error"));
        }
        let t = now();
        for s in d["submissions"].as_array().cloned().unwrap_or_default() {
            let Some(sid) = s["id"].as_i64() else { continue };
            let Some(row) = chunk.iter().find(|r| r.i("submission_id") == sid) else { continue };
            if s["status"] == "imported" {
                imported += 1;
                st.db.execute("UPDATE contrib_acoustid SET status='imported', acoustid=?, checked_at=? WHERE id=?", p![s["result"]["id"], t, row.i("id")])?;
            } else {
                st.db.execute("UPDATE contrib_acoustid SET checked_at=? WHERE id=?", p![t, row.i("id")])?;
            }
        }
    }
    Ok((sent, imported))
}

// ---- MusicBrainz OAuth2 + ISRCs -------------------------------------------------------------------

fn mb_redirect_uri(st: &AppState) -> Option<String> {
    let base = st.cfg.str("base_url").trim_end_matches('/').to_string();
    (!base.is_empty()).then(|| format!("{base}/api/contribute/mb/callback"))
}

fn mb_client_param() -> String {
    format!("encore-{}", crate::VERSION)
}

/// A fresh access token for the connected MusicBrainz account (refreshed when it is about to run out).
async fn mb_token(st: &AppState) -> anyhow::Result<String> {
    let refresh = st.cfg.str("mb_oauth_refresh_token");
    if refresh.is_empty() {
        anyhow::bail!("Connect a MusicBrainz account first");
    }
    let access = st.cfg.str("mb_oauth_access_token");
    if !access.is_empty() && st.cfg.float("mb_oauth_expires_at") > now() + 120.0 {
        return Ok(access);
    }
    let r = st.http.post(format!("{MB}/oauth2/token")).form(&[("grant_type", "refresh_token"), ("refresh_token", refresh.as_str()),
        ("client_id", &st.cfg.str("mb_client_id")), ("client_secret", &st.cfg.str("mb_client_secret"))]).send().await?;
    let code = r.status().as_u16();
    let d: Value = r.json().await.unwrap_or(Value::Null);
    let Some(tok) = d["access_token"].as_str() else {
        if code == 400 || code == 401 {
            // revoked or the app was deleted: the connection is gone
            st.cfg.set_many(&json!({ "mb_oauth_access_token": "", "mb_oauth_refresh_token": "" }));
        }
        anyhow::bail!("MusicBrainz did not renew the sign-in (HTTP {code}): {}", d["error_description"].as_str().or(d["error"].as_str()).unwrap_or("no reason given"));
    };
    let mut set = json!({ "mb_oauth_access_token": tok, "mb_oauth_expires_at": now() + d["expires_in"].as_f64().unwrap_or(3600.0) });
    if let Some(rt) = d["refresh_token"].as_str() {
        set["mb_oauth_refresh_token"] = json!(rt);
    }
    st.cfg.set_many(&set);
    Ok(tok.to_string())
}

/// The XML body MusicBrainz takes for ISRC submissions: one <recording> per recording, its ISRCs inside.
pub fn isrc_xml(items: &[(String, String)]) -> String {
    let mut by: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (rec, isrc) in items {
        let v = by.entry(rec.as_str()).or_default();
        if !v.contains(&isrc.as_str()) {
            v.push(isrc.as_str());
        }
    }
    let mut x = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<metadata xmlns=\"http://musicbrainz.org/ns/mmd-2.0#\"><recording-list>");
    for (rec, isrcs) in by {
        x.push_str(&format!("<recording id=\"{}\"><isrc-list count=\"{}\">", xml_esc(rec), isrcs.len()));
        for i in isrcs {
            x.push_str(&format!("<isrc id=\"{}\"/>", xml_esc(i)));
        }
        x.push_str("</isrc-list></recording>");
    }
    x.push_str("</recording-list></metadata>");
    x
}

fn xml_esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Look for audio-confirmed recordings whose file carries an ISRC MusicBrainz doesn't list (a few per pass:
/// each costs a MusicBrainz lookup). → number queued
fn http_status(e: &anyhow::Error) -> Option<u16> {
    e.downcast_ref::<reqwest::Error>().and_then(|r| r.status()).map(|s| s.as_u16())
}

/// An error about one row (a 4xx other than rate limiting, an answer that isn't JSON), not about
/// MusicBrainz as a whole.
fn row_error(e: &anyhow::Error) -> bool {
    match e.downcast_ref::<reqwest::Error>() {
        Some(r) if r.is_decode() => true,
        Some(r) => r.status().is_some_and(|s| s.is_client_error() && s.as_u16() != 429),
        None => false,
    }
}

pub async fn isrc_scan(st: &AppState, limit: usize) -> anyhow::Result<usize> {
    let rows = st.db.query("SELECT t.id, t.mb_trackid, t.isrc FROM tracks t WHERE t.verify_status='verified' AND t.verify_detail='MusicBrainz recording confirmed by audio'
        AND t.isrc IS NOT NULL AND t.isrc != '' AND t.mb_trackid IS NOT NULL AND t.mb_trackid != ''
        AND NOT EXISTS (SELECT 1 FROM contrib_isrc c WHERE c.recording=t.mb_trackid) ORDER BY t.id DESC LIMIT ?", p![limit as i64])?;
    let mut queued = 0;
    for r in rows {
        let rec = r.s("mb_trackid");
        let t = now();
        let Some(isrc) = clean_isrc(&r.s("isrc")).filter(|_| is_mbid(&rec)) else {
            st.db.execute("INSERT OR IGNORE INTO contrib_isrc (recording, isrc, track_id, status, error, created_at) VALUES (?,?,?,'skipped','not a valid ISRC or recording id',?)",
                p![rec, r.s("isrc"), r.i("id"), t])?;
            continue;
        };
        let d = match st.mb.get(st, &format!("recording/{rec}"), &[("inc", "isrcs".into())]).await {
            Ok(d) => d,
            // this recording's own problem (deleted, merged, a broken answer): noted, the next one is tried
            Err(e) if row_error(&e) => {
                st.db.execute("INSERT OR IGNORE INTO contrib_isrc (recording, isrc, track_id, status, error, created_at) VALUES (?,?,?,'skipped',?,?)",
                    p![rec, isrc, r.i("id"), format!("MusicBrainz recording lookup: {e}").chars().take(200).collect::<String>(), t])?;
                continue;
            }
            // MusicBrainz down or rate limiting: the next pass tries again
            Err(e) => return Err(e),
        };
        let have: Vec<String> = d["isrcs"].as_array().cloned().unwrap_or_default().iter().filter_map(|x| x.as_str().map(String::from)).collect();
        // only a "not found" means no recording has it; any other failure ends the pass (nothing is queued)
        let others: Vec<String> = match st.mb.get(st, &format!("isrc/{isrc}"), &[]).await {
            Ok(v) => v["recordings"].as_array().cloned().unwrap_or_default().iter().filter_map(|r| r["id"].as_str().map(String::from)).collect(),
            Err(e) if http_status(&e) == Some(404) => vec![],
            Err(e) => return Err(e),
        };
        let (status, why) = if have.contains(&isrc) {
            ("skipped", Some("already in MusicBrainz"))
        } else if !others.is_empty() {
            // on another recording already: a person should look at that, not a robot
            ("skipped", Some("MusicBrainz has this ISRC on another recording"))
        } else {
            queued += 1;
            ("queued", None)
        };
        st.db.execute("INSERT OR IGNORE INTO contrib_isrc (recording, isrc, track_id, status, error, created_at) VALUES (?,?,?,?,?,?)", p![rec, isrc, r.i("id"), status, why, t])?;
    }
    Ok(queued)
}

/// Send queued ISRCs. → number sent
pub async fn isrc_submit(st: &AppState) -> anyhow::Result<usize> {
    let rows = st.db.query("SELECT recording, isrc FROM contrib_isrc WHERE status='queued' ORDER BY created_at LIMIT ?", p![ISRC_BATCH as i64])?;
    if rows.is_empty() {
        return Ok(0);
    }
    let token = mb_token(st).await?;
    let items: Vec<(String, String)> = rows.iter().map(|r| (r.s("recording"), r.s("isrc"))).collect();
    let r = st.http.post(format!("{MB}/ws/2/recording")).query(&[("client", mb_client_param())]).bearer_auth(token)
        .header("Content-Type", "application/xml; charset=utf-8").body(isrc_xml(&items)).send().await?;
    let code = r.status().as_u16();
    let body = r.text().await.unwrap_or_default();
    let t = now();
    if (200..300).contains(&code) {
        for (rec, isrc) in &items {
            st.db.execute("UPDATE contrib_isrc SET status='submitted', submitted_at=?, error=NULL WHERE recording=? AND isrc=?", p![t, rec, isrc])?;
        }
        st.db.log("info", &format!("Sent {} ISRC(s) to MusicBrainz", items.len()));
        return Ok(items.len());
    }
    let why = body.split("<text>").nth(1).and_then(|x| x.split("</text>").next()).unwrap_or("").trim().to_string();
    if code == 400 {
        // MusicBrainz turned the batch down: mark them so the same batch isn't retried forever
        for (rec, isrc) in &items {
            st.db.execute("UPDATE contrib_isrc SET status='error', error=? WHERE recording=? AND isrc=?", p![format!("HTTP 400 {why}").trim().to_string(), rec, isrc])?;
        }
    }
    anyhow::bail!("MusicBrainz did not take the ISRCs (HTTP {code}){}", if why.is_empty() { String::new() } else { format!(": {why}") })
}

/// Background loop: fingerprints every few minutes, ISRCs every half hour.
pub async fn worker(st: AppState) {
    tokio::time::sleep(Duration::from_secs(90)).await;
    let mut tick: u64 = 0;
    loop {
        if acoustid_ready(&st).is_ok() {
            match acoustid_pass(&st, 4).await {
                Ok((s, i)) => if s + i > 0 { note("acoustid", true, &format!("{s} sent, {i} imported")) },
                Err(e) => note("acoustid", false, &e.to_string()),
            }
        }
        if tick % 6 == 0 && st.cfg.bool("contribute_isrc") && !st.cfg.str("mb_oauth_refresh_token").is_empty() {
            let r = async { let q = isrc_scan(&st, 20).await?; let n = isrc_submit(&st).await?; anyhow::Ok((q, n)) }.await;
            match r {
                Ok((q, n)) => if q + n > 0 { note("isrc", true, &format!("{q} found, {n} sent")) },
                Err(e) => note("isrc", false, &e.to_string()),
            }
        }
        tick += 1;
        tokio::time::sleep(Duration::from_secs(300)).await;
    }
}

// ---- routes: status, run now, connect --------------------------------------------------------------

fn counts(st: &AppState, table: &str) -> Value {
    let mut out = json!({});
    for r in st.db.query(&format!("SELECT status, COUNT(*) n FROM {table} GROUP BY status"), p![]).unwrap_or_default() {
        out[r.s("status")] = json!(r.i("n"));
    }
    out
}

async fn status(State(st): State<AppState>, Admin(_): Admin) -> Json<Value> {
    let ac = counts(&st, "contrib_acoustid");
    let ic = counts(&st, "contrib_isrc");
    let n = |v: &Value, k: &str| v[k].as_i64().unwrap_or(0);
    let recent: Vec<Value> = st.db.query("SELECT c.status, c.mbid, c.acoustid, c.error, c.submitted_at, c.created_at, t.title, t.artist FROM contrib_acoustid c
        LEFT JOIN tracks t ON t.id=c.track_id ORDER BY c.id DESC LIMIT 10", p![]).unwrap_or_default().into_iter().map(serde_json::Value::Object).collect();
    Json(json!({
        "acoustid": {
            "enabled": st.cfg.bool("contribute_acoustid"),
            "has_client_key": !st.cfg.str("acoustid_api_key").is_empty(),
            "has_user_key": !st.cfg.str("acoustid_user_key").is_empty(),
            "can_fingerprint": crate::acoustid::can_fingerprint(),
            "ready": acoustid_ready(&st).is_ok(),
            "why_not": acoustid_ready(&st).err(),
            "queued": n(&ac, "queued"), "submitted": n(&ac, "submitted"), "imported": n(&ac, "imported"), "failed": n(&ac, "error"),
            "last": last("acoustid"),
            "recent": recent,
        },
        "isrc": {
            "enabled": st.cfg.bool("contribute_isrc"),
            "has_client": !st.cfg.str("mb_client_id").is_empty() && !st.cfg.str("mb_client_secret").is_empty(),
            "connected": !st.cfg.str("mb_oauth_refresh_token").is_empty(),
            "account": st.cfg.str("mb_oauth_account"),
            "redirect_uri": mb_redirect_uri(&st),
            "queued": n(&ic, "queued"), "submitted": n(&ic, "submitted"), "skipped": n(&ic, "skipped"), "failed": n(&ic, "error"),
            "last": last("isrc"),
        },
    }))
}

async fn run_acoustid(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    acoustid_ready(&st).map_err(|e| err(400, e))?;
    match acoustid_pass(&st, 10).await {
        Ok((s, i)) => {
            let msg = format!("{s} sent, {i} imported");
            note("acoustid", true, &msg);
            Ok(Json(json!({ "ok": true, "sent": s, "imported": i, "message": msg })))
        }
        Err(e) => {
            note("acoustid", false, &e.to_string());
            Err(err(502, e.to_string()))
        }
    }
}

async fn run_isrc(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    if st.cfg.str("mb_oauth_refresh_token").is_empty() {
        return Err(err(400, "Connect a MusicBrainz account first"));
    }
    let r = async { let q = isrc_scan(&st, 40).await?; let n = isrc_submit(&st).await?; anyhow::Ok((q, n)) }.await;
    match r {
        Ok((q, n)) => {
            let msg = format!("{q} found, {n} sent");
            note("isrc", true, &msg);
            Ok(Json(json!({ "ok": true, "found": q, "sent": n, "message": msg })))
        }
        Err(e) => {
            note("isrc", false, &e.to_string());
            Err(err(502, e.to_string()))
        }
    }
}

async fn mb_connect(State(st): State<AppState>, Admin(u): Admin) -> ApiResult<Json<Value>> {
    let cid = st.cfg.str("mb_client_id");
    if cid.is_empty() || st.cfg.str("mb_client_secret").is_empty() {
        return Err(err(400, "Add the MusicBrainz application's client ID and secret first"));
    }
    let redirect = mb_redirect_uri(&st).ok_or_else(|| err(400, "Set the Public URL in Settings → General first (the sign-in needs a callback address)"))?;
    let state = crate::auth::random_token(24);
    st.cache_put(&format!("mboauth:{state}"), json!({ "user_id": u.i("id") }), 900);
    let q = [("response_type", "code"), ("client_id", cid.as_str()), ("redirect_uri", redirect.as_str()), ("scope", "submit_isrc tag rating"),
             ("state", state.as_str()), ("access_type", "offline"), ("approval_prompt", "force")];
    let url = format!("{MB}/oauth2/authorize?{}", q.iter().map(|(k, v)| format!("{k}={}", urlencoding::encode(v))).collect::<Vec<_>>().join("&"));
    Ok(Json(json!({ "url": url })))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct CallbackQ {
    code: String,
    state: String,
    error: String,
}

fn back(msg: &str, ok: bool) -> Response {
    let k = if ok { "mb" } else { "error" };
    Redirect::to(&format!("/#/settings?s=contribute&{k}={}", urlencoding::encode(&msg.chars().take(200).collect::<String>()))).into_response()
}

async fn mb_callback(State(st): State<AppState>, MaybeUser(u): MaybeUser, Query(q): Query<CallbackQ>) -> Response {
    if !q.error.is_empty() {
        return back(if q.error == "access_denied" { "You did not allow Encore – connect again and approve the access" } else { &q.error }, false);
    }
    let Some(u) = u.filter(crate::auth::is_admin) else { return back("Sign in to Encore as an admin in this browser first, then connect again", false) };
    let key = format!("mboauth:{}", q.state);
    if st.cache_get(&key).filter(|p| p["user_id"].as_i64() == Some(u.i("id"))).is_none() {
        return back("The sign-in expired or was started in another browser – connect again", false);
    }
    st.cache_put(&key, Value::Null, 0);
    let Some(redirect) = mb_redirect_uri(&st) else { return back("Set the Public URL first", false) };
    let r = st.http.post(format!("{MB}/oauth2/token")).form(&[("grant_type", "authorization_code"), ("code", q.code.as_str()), ("redirect_uri", redirect.as_str()),
        ("client_id", &st.cfg.str("mb_client_id")), ("client_secret", &st.cfg.str("mb_client_secret"))]).send().await;
    let d: Value = match r {
        Ok(r) => { let code = r.status().as_u16(); let d: Value = r.json().await.unwrap_or(Value::Null); if d["access_token"].is_null() {
            return back(&format!("MusicBrainz did not accept the sign-in (HTTP {code}): {}", d["error_description"].as_str().or(d["error"].as_str()).unwrap_or("check the client ID, secret and callback URL")), false);
        } d }
        Err(e) => return back(&format!("Could not reach MusicBrainz: {e}"), false),
    };
    let tok = d["access_token"].as_str().unwrap_or("").to_string();
    let who: Value = match st.http.get(format!("{MB}/oauth2/userinfo")).bearer_auth(&tok).send().await { Ok(r) => r.json().await.unwrap_or(Value::Null), Err(_) => Value::Null };
    let account = who["sub"].as_str().unwrap_or("").to_string();
    st.cfg.set_many(&json!({ "mb_oauth_access_token": tok, "mb_oauth_refresh_token": d["refresh_token"].as_str().unwrap_or(""),
        "mb_oauth_expires_at": now() + d["expires_in"].as_f64().unwrap_or(3600.0), "mb_oauth_account": account }));
    st.db.log("info", &format!("{} connected the MusicBrainz account {account} for ISRC submissions", u.s("username")));
    if d["refresh_token"].as_str().unwrap_or("").is_empty() {
        return back("MusicBrainz gave no refresh token – submissions stop after an hour. Connect again", false);
    }
    back("connected", true)
}

async fn mb_disconnect(State(st): State<AppState>, Admin(_): Admin) -> Json<Value> {
    st.cfg.set_many(&json!({ "mb_oauth_access_token": "", "mb_oauth_refresh_token": "", "mb_oauth_expires_at": 0.0, "mb_oauth_account": "" }));
    Json(json!({ "ok": true }))
}

// ---- MusicBrainz release editor seeding ------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SeedTrack {
    pub title: String,
    pub artist: String,
    pub length_ms: Option<i64>,
    pub disc: i64,
    pub number: i64,
}

#[derive(Debug, Clone, Default)]
pub struct SeedAlbum {
    pub title: String,
    /// credited artists in order (name, MusicBrainz id when known)
    pub artists: Vec<(String, Option<String>)>,
    /// MusicBrainz primary type ("album", "ep", "single") and secondary types ("compilation")
    pub types: Vec<String>,
    pub date: String,
    pub label: String,
    pub catalog_number: String,
    pub barcode: String,
    pub tracks: Vec<SeedTrack>,
    /// (url, MusicBrainz release-url link type id)
    pub urls: Vec<(String, u32)>,
    /// "Deezer", "Apple Music"
    pub sources: Vec<String>,
}

/// Release-url link types: free streaming (Deezer), streaming (Apple Music), purchase for download (iTunes).
pub const LINK_FREE_STREAMING: u32 = 85;
pub const LINK_STREAMING: u32 = 980;
pub const LINK_PURCHASE_DOWNLOAD: u32 = 74;

/// (ISO 639-3 language, ISO 15924 script) from the titles – only when the guess is a safe one.
pub fn guess_language(titles: &[&str]) -> (Option<&'static str>, Option<&'static str>) {
    let all: String = titles.join(" ");
    let letters: Vec<char> = all.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.is_empty() {
        return (None, None);
    }
    let count = |f: &dyn Fn(u32) -> bool| letters.iter().filter(|c| f(**c as u32)).count();
    let total = letters.len();
    let kana = count(&|c| (0x3040..=0x30FF).contains(&c));
    let hangul = count(&|c| (0xAC00..=0xD7AF).contains(&c) || (0x1100..=0x11FF).contains(&c));
    let han = count(&|c| (0x4E00..=0x9FFF).contains(&c));
    let cyr = count(&|c| (0x0400..=0x04FF).contains(&c));
    let greek = count(&|c| (0x0370..=0x03FF).contains(&c));
    let latin = count(&|c| c < 0x250);
    if kana > 0 && (kana + han) * 2 >= total {
        return (Some("jpn"), Some("Jpan"));
    }
    if hangul * 2 >= total {
        return (Some("kor"), Some("Kore"));
    }
    if han * 2 >= total {
        return (None, Some("Hani"));
    }
    if cyr * 2 >= total {
        return (None, Some("Cyrl"));
    }
    if greek * 2 >= total {
        return (Some("ell"), Some("Grek"));
    }
    if latin * 10 >= total * 9 {
        // English only when common English words make up a fair share of the words
        const EN: [&str; 24] = ["the", "you", "i", "me", "my", "love", "of", "in", "on", "and", "to", "a", "it", "is", "we", "your", "be", "all", "for", "with", "don't", "it's", "i'm", "no"];
        let words: Vec<String> = all.to_lowercase().split(|c: char| !(c.is_alphanumeric() || c == '\'')).filter(|w| !w.is_empty()).map(String::from).collect();
        let en = words.iter().filter(|w| EN.contains(&w.as_str())).count();
        return (if !words.is_empty() && en * 6 >= words.len() { Some("eng") } else { None }, Some("Latn"));
    }
    (None, None)
}

/// Join phrases for a list of credited names: "A", "A & B", "A, B & C".
fn joins(n: usize) -> Vec<&'static str> {
    (0..n).map(|i| if i + 1 == n { "" } else if i + 2 == n { " & " } else { ", " }).collect()
}

/// The release editor's seeding fields (https://musicbrainz.org/doc/Development/Release_Editor_Seeding).
pub fn seed_fields(a: &SeedAlbum) -> Vec<(String, String)> {
    let mut f: Vec<(String, String)> = vec![];
    let mut put = |k: String, v: String| { if !v.is_empty() { f.push((k, v)); } };
    put("name".into(), a.title.clone());
    for t in &a.types {
        put("type".into(), t.clone());
    }
    put("status".into(), "official".into());
    put("packaging".into(), "None".into());
    let titles: Vec<&str> = std::iter::once(a.title.as_str()).chain(a.tracks.iter().map(|t| t.title.as_str())).collect();
    let (lang, script) = guess_language(&titles);
    put("language".into(), lang.unwrap_or("").into());
    put("script".into(), script.unwrap_or("").into());
    let j = joins(a.artists.len());
    for (i, (name, mbid)) in a.artists.iter().enumerate() {
        put(format!("artist_credit.names.{i}.name"), name.clone());
        put(format!("artist_credit.names.{i}.artist.name"), name.clone());
        put(format!("artist_credit.names.{i}.mbid"), mbid.clone().unwrap_or_default());
        put(format!("artist_credit.names.{i}.join_phrase"), j[i].into());
    }
    let d: Vec<&str> = a.date.get(..10.min(a.date.len())).unwrap_or("").split('-').collect();
    for (k, v) in ["year", "month", "day"].iter().zip(d.iter()) {
        if let Ok(n) = v.parse::<u32>() {
            if n > 0 {
                put(format!("events.0.date.{k}"), n.to_string());
            }
        }
    }
    if !a.date.is_empty() {
        put("events.0.country".into(), "XW".into());
    }
    put("labels.0.name".into(), a.label.clone());
    put("labels.0.catalog_number".into(), a.catalog_number.clone());
    put("barcode".into(), a.barcode.clone());
    let mut discs: Vec<i64> = a.tracks.iter().map(|t| t.disc.max(1)).collect();
    discs.sort();
    discs.dedup();
    let album_artist = a.artists.iter().map(|x| x.0.as_str()).collect::<Vec<_>>().join(" & ").to_lowercase();
    for (m, disc) in discs.iter().enumerate() {
        put(format!("mediums.{m}.format"), "Digital Media".into());
        let mut ts: Vec<&SeedTrack> = a.tracks.iter().filter(|t| t.disc.max(1) == *disc).collect();
        ts.sort_by_key(|t| t.number);
        for (n, t) in ts.iter().enumerate() {
            let pre = format!("mediums.{m}.track.{n}");
            put(format!("{pre}.name"), t.title.clone());
            put(format!("{pre}.number"), if t.number > 0 { t.number.to_string() } else { (n + 1).to_string() });
            if let Some(ms) = t.length_ms.filter(|ms| *ms > 0) {
                put(format!("{pre}.length"), ms.to_string());
            }
            // a song by someone else than the album artist gets its own credit; the album's credit otherwise
            let same = t.artist.is_empty() || t.artist.to_lowercase() == album_artist;
            if same {
                for (i, (name, mbid)) in a.artists.iter().enumerate() {
                    put(format!("{pre}.artist_credit.names.{i}.name"), name.clone());
                    put(format!("{pre}.artist_credit.names.{i}.artist.name"), name.clone());
                    put(format!("{pre}.artist_credit.names.{i}.mbid"), mbid.clone().unwrap_or_default());
                    put(format!("{pre}.artist_credit.names.{i}.join_phrase"), j[i].into());
                }
            } else {
                let known = a.artists.iter().find(|(n, _)| n.to_lowercase() == t.artist.to_lowercase()).and_then(|x| x.1.clone());
                put(format!("{pre}.artist_credit.names.0.name"), t.artist.clone());
                put(format!("{pre}.artist_credit.names.0.artist.name"), t.artist.clone());
                put(format!("{pre}.artist_credit.names.0.mbid"), known.unwrap_or_default());
            }
        }
    }
    for (i, (url, kind)) in a.urls.iter().enumerate() {
        put(format!("urls.{i}.url"), url.clone());
        put(format!("urls.{i}.link_type"), kind.to_string());
    }
    let mut noteline = format!("Seeded from Encore using {} metadata", if a.sources.is_empty() { "Deezer/Apple".to_string() } else { a.sources.join(" and ") });
    let mut seen = vec![];
    for (u, _) in &a.urls {
        if !seen.contains(u) {
            noteline.push('\n');
            noteline.push_str(u);
            seen.push(u.clone());
        }
    }
    put("edit_note".into(), noteline);
    f
}

/// The page the browser opens: posts the seeded form to MusicBrainz at once (a button when scripts are off).
pub fn seed_html(a: &SeedAlbum, fields: &[(String, String)]) -> String {
    use crate::routes::html_escape as e;
    let inputs: String = fields.iter().map(|(k, v)| format!("<input type=\"hidden\" name=\"{}\" value=\"{}\">", e(k), e(v))).collect::<Vec<_>>().join("\n");
    format!(r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Add to MusicBrainz</title><meta name="referrer" content="no-referrer">
<style>body{{font-family:system-ui,sans-serif;background:#111;color:#eee;display:grid;place-items:center;min-height:100vh;margin:0;padding:16px;box-sizing:border-box}}main{{max-width:520px}}button{{font:inherit;padding:10px 18px;border-radius:8px;border:0;background:#ba478f;color:#fff;cursor:pointer}}p{{color:#bbb}}</style></head>
<body><main><h1>Add “{title}” to MusicBrainz</h1><p>Opening the MusicBrainz release editor with the album filled in from {src}. Check every tab before you enter the edit: you are the editor, Encore only typed it in.</p>
<form id="seed" method="post" action="{MB}/release/add" accept-charset="utf-8">
{inputs}
<button type="submit">Open the release editor</button></form></main>
<script>document.getElementById('seed').submit();</script></body></html>"#,
        title = e(&a.title), src = e(&if a.sources.is_empty() { "Deezer/Apple".to_string() } else { a.sources.join(" and ") }))
}

fn s(v: &Value) -> String {
    match v { Value::String(s) => s.clone(), Value::Null => String::new(), x => x.to_string() }
}

/// Deezer's / Apple's data for an album, as a seed. `artist` = (name, MusicBrainz id) from the artist page.
pub async fn load_seed(st: &AppState, deezer: Option<&str>, apple: Option<&str>, artist: (&str, Option<&str>)) -> anyhow::Result<SeedAlbum> {
    let mut a = SeedAlbum::default();
    let mbid_for = |name: &str| -> Option<String> {
        let (an, am) = artist;
        am.filter(|m| is_mbid(m) && crate::routes::catalog::norm(name) == crate::routes::catalog::norm(an)).map(String::from)
    };
    if let Some(id) = deezer.filter(|i| i.chars().all(|c| c.is_ascii_digit()) && !i.is_empty()) {
        let d: Value = st.http.get(format!("https://api.deezer.com/album/{id}")).send().await?.error_for_status()?.json().await?;
        if !d["error"].is_null() {
            anyhow::bail!("Deezer: {}", d["error"]["message"].as_str().unwrap_or("album not found"));
        }
        a.title = s(&d["title"]);
        let main: Vec<String> = d["contributors"].as_array().cloned().unwrap_or_default().iter()
            .filter(|c| c["role"].as_str().is_none_or(|r| r == "Main")).map(|c| s(&c["name"])).filter(|n| !n.is_empty()).collect();
        let names = if main.is_empty() { vec![s(&d["artist"]["name"])] } else { main };
        a.artists = names.iter().map(|n| (n.clone(), mbid_for(n))).collect();
        let rt = s(&d["record_type"]).to_lowercase();
        a.types = match rt.as_str() { "ep" => vec!["ep".into()], "single" => vec!["single".into()], "compile" => vec!["album".into(), "compilation".into()], _ => vec!["album".into()] };
        a.date = s(&d["release_date"]);
        a.label = s(&d["label"]);
        a.barcode = s(&d["upc"]);
        let link = { let l = s(&d["link"]); if l.is_empty() { format!("https://www.deezer.com/album/{id}") } else { l } };
        a.urls.push((link, LINK_FREE_STREAMING));
        a.sources.push("Deezer".into());
        let tl = crate::routes::catalog::tracklist(st, &json!({ "deezer": id })).await?;
        a.tracks = tl.iter().map(|t| SeedTrack { title: s(&t["title"]), artist: s(&t["artist"]),
            length_ms: t["length"].as_f64().map(|x| (x * 1000.0).round() as i64), disc: t["disc"].as_i64().unwrap_or(1), number: t["track"].as_i64().unwrap_or(0) }).collect();
    }
    if let Some(id) = apple.filter(|i| i.chars().all(|c| c.is_ascii_digit()) && !i.is_empty()) {
        let d: Value = st.http.get("https://itunes.apple.com/lookup").query(&[("id", id), ("entity", "song"), ("limit", "300")]).send().await?.error_for_status()?.json().await?;
        let res = d["results"].as_array().cloned().unwrap_or_default();
        let coll = res.iter().find(|x| x["wrapperType"] == "collection").cloned().unwrap_or(Value::Null);
        let url = { let u = s(&coll["collectionViewUrl"]); let u = u.split('?').next().unwrap_or("").to_string(); if u.is_empty() { format!("https://music.apple.com/album/{id}") } else { u } };
        a.urls.push((url.clone(), LINK_STREAMING));
        a.urls.push((url, LINK_PURCHASE_DOWNLOAD));
        a.sources.push("Apple Music".into());
        if a.title.is_empty() {
            let raw = s(&coll["collectionName"]);
            let kind = crate::routes::catalog::kind_of("", &raw);
            a.title = raw.trim_end_matches(" - Single").trim_end_matches(" - EP").to_string();
            a.types = vec![match kind { "EP" => "ep", "Single" => "single", _ => "album" }.into()];
            let name = s(&coll["artistName"]);
            a.artists = vec![(name.clone(), mbid_for(&name))];
            a.date = s(&coll["releaseDate"]).chars().take(10).collect();
            a.tracks = res.iter().filter(|x| x["wrapperType"] == "track").map(|x| SeedTrack { title: s(&x["trackName"]), artist: s(&x["artistName"]),
                length_ms: x["trackTimeMillis"].as_i64(), disc: x["discNumber"].as_i64().unwrap_or(1), number: x["trackNumber"].as_i64().unwrap_or(0) }).collect();
        }
    }
    if a.title.is_empty() {
        anyhow::bail!("Neither Deezer nor Apple Music had this album");
    }
    Ok(a)
}

/// HMAC-SHA256 (RFC 2104) – signs seed links so the phone's browser can open them without a session.
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let (mut ipad, mut opad) = ([0x36u8; 64], [0x5cu8; 64]);
    for i in 0..64 {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let inner = Sha256::new().chain_update(ipad).chain_update(msg).finalize();
    Sha256::new().chain_update(opad).chain_update(inner).finalize().into()
}

/// How long a signed seed link works.
const SEED_LINK_SECS: i64 = 900;

fn seed_sig(secret: &str, key: &str, q: &SeedQ, exp: i64) -> String {
    let msg = format!("mbseed\n{key}\n{}\n{}\n{}\n{exp}", q.apple, q.artist, q.artist_name);
    hex::encode(hmac_sha256(secret.as_bytes(), msg.as_bytes()))
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub struct SeedQ {
    /// an Apple Music id next to a Deezer key (both pages get linked)
    apple: String,
    /// the artist's MusicBrainz id and name (from the artist page) for the credits
    artist: String,
    artist_name: String,
    exp: i64,
    sig: String,
}

/// "deezer-123" / "apple-456" → (deezer, apple)
fn parse_key(key: &str, q: &SeedQ) -> Option<(Option<String>, Option<String>)> {
    let (svc, id) = key.split_once('-')?;
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let apple = Some(q.apple.clone()).filter(|a| !a.is_empty() && a.chars().all(|c| c.is_ascii_digit()));
    match svc {
        "deezer" => Some((Some(id.to_string()), apple)),
        "apple" => Some((None, Some(id.to_string()))),
        _ => None,
    }
}

async fn seed_page(State(st): State<AppState>, MaybeUser(u): MaybeUser, Path(key): Path<String>, Query(q): Query<SeedQ>) -> ApiResult<Response> {
    if u.is_none() {
        // the phone app opens this in the browser, which has no Encore session: a signed link instead
        // never longer than the 15 minutes a link is made for, and never with an empty key
        let (t, secret) = (now() as i64, st.cfg.str("api_key"));
        let ok = !secret.is_empty() && q.exp > t && q.exp <= t + SEED_LINK_SECS && crate::auth::ct_eq(&seed_sig(&secret, &key, &q, q.exp), &q.sig);
        if !ok {
            return Err(err(401, "This link has expired – open Add to MusicBrainz again from Encore"));
        }
    }
    let (deezer, apple) = parse_key(&key, &q).ok_or_else(|| err(400, "Unknown album"))?;
    let artist = (q.artist_name.as_str(), Some(q.artist.as_str()).filter(|a| !a.is_empty()));
    let a = load_seed(&st, deezer.as_deref(), apple.as_deref(), artist).await.map_err(|e| err(502, format!("Could not load the album: {e}")))?;
    let fields = seed_fields(&a);
    let mut resp = Html(seed_html(&a, &fields)).into_response();
    // the form posts to musicbrainz.org; nothing else is loaded
    resp.headers_mut().insert("content-security-policy", axum::http::HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; form-action https://musicbrainz.org"));
    Ok(resp)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct LinkBody {
    key: String,
    apple: String,
    artist: String,
    artist_name: String,
}

/// A link to the seed page that works for 15 minutes without signing in (the phone opens it in the browser).
async fn seed_link(State(st): State<AppState>, User(_): User, Json(b): Json<LinkBody>) -> ApiResult<Json<Value>> {
    let q = SeedQ { apple: b.apple, artist: b.artist, artist_name: b.artist_name, ..Default::default() };
    parse_key(&b.key, &q).ok_or_else(|| err(400, "Unknown album"))?;
    let secret = st.cfg.str("api_key");
    if secret.is_empty() {
        return Err(err(503, "No server key to sign the link with"));
    }
    let exp = now() as i64 + SEED_LINK_SECS;
    let sig = seed_sig(&secret, &b.key, &q, exp);
    let url = format!("/api/contribute/mb-seed/{}?apple={}&artist={}&artist_name={}&exp={exp}&sig={sig}", urlencoding::encode(&b.key),
        urlencoding::encode(&q.apple), urlencoding::encode(&q.artist), urlencoding::encode(&q.artist_name));
    Ok(Json(json!({ "url": url })))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MBID: &str = "b1a9c0e9-d987-4042-ae91-78d6a3267d69";

    #[test]
    fn acoustid_batch_form() {
        let items: Vec<(String, i64, String)> = (0..60).map(|i| (format!("FP{i}"), 200 + i, MBID.to_string())).collect();
        let f = acoustid_form("client-key", "user-key", &items);
        let get = |k: &str| f.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone());
        assert_eq!(get("client").as_deref(), Some("client-key"));
        assert_eq!(get("user").as_deref(), Some("user-key"));
        assert_eq!(get("format").as_deref(), Some("json"));
        assert_eq!(get("fingerprint.0").as_deref(), Some("FP0"));
        assert_eq!(get("duration.49").as_deref(), Some("249"));
        assert_eq!(get("mbid.49").as_deref(), Some(MBID));
        // at most 50 per call
        assert!(get("fingerprint.50").is_none());
        assert_eq!(f.iter().filter(|(k, _)| k.starts_with("fingerprint.")).count(), BATCH);
    }

    #[test]
    fn acoustid_answer() {
        let d = json!({ "status": "ok", "submissions": [{ "index": "1", "id": 22, "status": "pending" }, { "index": "0", "id": 21, "status": "imported" }] });
        assert_eq!(parse_submissions(&d), vec![(1, 22, "pending".to_string()), (0, 21, "imported".to_string())]);
        let single = json!({ "status": "ok", "submissions": [{ "id": 5, "status": "pending" }] });
        assert_eq!(parse_submissions(&single), vec![(0, 5, "pending".to_string())]);
    }

    #[test]
    fn queues_only_confirmed_recordings_once() {
        let _env = crate::TEST_ENV.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("encore-contrib-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("ENCORE_CONFIG_DIR", &dir);
        let st = crate::build_state().unwrap();
        let sure = json!({ "status": "verified", "submit": { "mbid": MBID, "fingerprint": "AQAA", "duration": 201.4 } });
        queue_acoustid(&st, Some(1), &sure);
        queue_acoustid(&st, Some(1), &sure);
        queue_acoustid(&st, Some(2), &sure); // same fingerprint + recording from another copy: still once
        queue_acoustid(&st, Some(3), &json!({ "status": "verified", "detail": "audio matches “A – B”" })); // a title match: nothing to send
        queue_acoustid(&st, Some(4), &json!({ "status": "mismatch", "submit": { "mbid": MBID, "fingerprint": "AQAB", "duration": 100 } }));
        let rows = st.db.query("SELECT * FROM contrib_acoustid", p![]).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].i("duration"), 201);
        assert_eq!(rows[0].s("status"), "queued");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn isrc_body() {
        let x = isrc_xml(&[(MBID.into(), "USABC1234567".into()), (MBID.into(), "USABC1234568".into()), (MBID.into(), "USABC1234567".into()),
                            ("00000000-0000-0000-0000-000000000001".into(), "GBXYZ0000001".into())]);
        assert!(x.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(x.contains("<metadata xmlns=\"http://musicbrainz.org/ns/mmd-2.0#\"><recording-list>"));
        assert!(x.contains(&format!("<recording id=\"{MBID}\"><isrc-list count=\"2\"><isrc id=\"USABC1234567\"/><isrc id=\"USABC1234568\"/></isrc-list></recording>")));
        assert!(x.contains("<recording id=\"00000000-0000-0000-0000-000000000001\"><isrc-list count=\"1\"><isrc id=\"GBXYZ0000001\"/></isrc-list></recording>"));
        assert_eq!(clean_isrc("us-abc-12-34567").as_deref(), Some("USABC1234567"));
        assert_eq!(clean_isrc("nope"), None);
        assert!(is_mbid(MBID));
        assert!(!is_mbid("not-an-id"));
    }

    fn album() -> SeedAlbum {
        SeedAlbum {
            title: "Days Before Rodeo".into(),
            artists: vec![("Travis Scott".into(), Some(MBID.into())), ("Young Thug".into(), None)],
            types: vec!["album".into()],
            date: "2014-08-18".into(),
            label: "Grand Hustle".into(),
            catalog_number: String::new(),
            barcode: "0602547012345".into(),
            tracks: vec![
                SeedTrack { title: "The Prayer".into(), artist: "Travis Scott & Young Thug".into(), length_ms: Some(184000), disc: 1, number: 1 },
                SeedTrack { title: "Mamacita".into(), artist: "Rich Homie Quan".into(), length_ms: Some(220500), disc: 1, number: 2 },
                SeedTrack { title: "Bonus".into(), artist: String::new(), length_ms: None, disc: 2, number: 1 },
            ],
            urls: vec![("https://www.deezer.com/album/1".into(), LINK_FREE_STREAMING), ("https://music.apple.com/album/2".into(), LINK_STREAMING),
                       ("https://music.apple.com/album/2".into(), LINK_PURCHASE_DOWNLOAD)],
            sources: vec!["Deezer".into(), "Apple Music".into()],
        }
    }

    #[test]
    fn seed_form_fields() {
        let f = seed_fields(&album());
        let get = |k: &str| f.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str());
        assert_eq!(get("name"), Some("Days Before Rodeo"));
        assert_eq!(get("type"), Some("album"));
        assert_eq!(get("status"), Some("official"));
        assert_eq!(get("packaging"), Some("None"));
        assert_eq!(get("barcode"), Some("0602547012345"));
        assert_eq!(get("labels.0.name"), Some("Grand Hustle"));
        assert!(get("labels.0.catalog_number").is_none()); // empty fields are left out
        assert_eq!(get("events.0.date.year"), Some("2014"));
        assert_eq!(get("events.0.date.month"), Some("8"));
        assert_eq!(get("events.0.date.day"), Some("18"));
        assert_eq!(get("events.0.country"), Some("XW"));
        assert_eq!(get("artist_credit.names.0.name"), Some("Travis Scott"));
        assert_eq!(get("artist_credit.names.0.mbid"), Some(MBID));
        assert_eq!(get("artist_credit.names.0.join_phrase"), Some(" & "));
        assert_eq!(get("artist_credit.names.1.name"), Some("Young Thug"));
        assert!(get("artist_credit.names.1.mbid").is_none());
        assert_eq!(get("mediums.0.format"), Some("Digital Media"));
        assert_eq!(get("mediums.0.track.0.name"), Some("The Prayer"));
        assert_eq!(get("mediums.0.track.0.length"), Some("184000"));
        assert_eq!(get("mediums.0.track.0.artist_credit.names.0.mbid"), Some(MBID)); // the album's credit
        assert_eq!(get("mediums.0.track.1.artist_credit.names.0.name"), Some("Rich Homie Quan"));
        assert!(get("mediums.0.track.1.artist_credit.names.0.mbid").is_none());
        assert_eq!(get("mediums.1.track.0.name"), Some("Bonus"));
        assert!(get("mediums.1.track.0.length").is_none());
        assert_eq!(get("urls.0.url"), Some("https://www.deezer.com/album/1"));
        assert_eq!(get("urls.0.link_type"), Some("85"));
        assert_eq!(get("urls.1.link_type"), Some("980"));
        assert_eq!(get("urls.2.link_type"), Some("74"));
        let note = get("edit_note").unwrap();
        assert!(note.starts_with("Seeded from Encore using Deezer and Apple Music metadata"));
        assert_eq!(note.matches("https://music.apple.com/album/2").count(), 1);
        assert_eq!(get("script"), Some("Latn"));
        let html = seed_html(&album(), &f);
        assert!(html.contains("action=\"https://musicbrainz.org/release/add\""));
        assert!(html.contains("name=\"mediums.0.track.0.name\" value=\"The Prayer\""));
    }

    #[test]
    fn language_guess() {
        assert_eq!(guess_language(&["Love of my life", "You and I", "The Night"]), (Some("eng"), Some("Latn")));
        assert_eq!(guess_language(&["Rodeo", "Antidote", "Pornography"]), (None, Some("Latn")));
        assert_eq!(guess_language(&["ちゅきちゅきポコポコポン", "夜に駆ける"]), (Some("jpn"), Some("Jpan")));
        assert_eq!(guess_language(&["봄날", "피 땀 눈물"]), (Some("kor"), Some("Kore")));
        assert_eq!(guess_language(&["Кино", "Группа крови"]), (None, Some("Cyrl")));
    }

    #[test]
    fn signed_links() {
        // RFC 4231 test case 2
        assert_eq!(hex::encode(hmac_sha256(b"Jefe", b"what do ya want for nothing?")), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
        let q = SeedQ { artist: MBID.into(), artist_name: "Travis Scott".into(), ..Default::default() };
        let a = seed_sig("secret", "deezer-1", &q, 100);
        assert_eq!(a, seed_sig("secret", "deezer-1", &q, 100));
        assert_ne!(a, seed_sig("secret", "deezer-2", &q, 100));
        assert_ne!(a, seed_sig("secret", "deezer-1", &q, 101));
        assert_ne!(a, seed_sig("other", "deezer-1", &q, 100));
        assert_eq!(parse_key("deezer-12", &SeedQ { apple: "34".into(), ..Default::default() }), Some((Some("12".into()), Some("34".into()))));
        assert_eq!(parse_key("apple-34", &SeedQ::default()), Some((None, Some("34".into()))));
        assert_eq!(parse_key("bandcamp-1", &SeedQ::default()), None);
        assert_eq!(parse_key("deezer-1x", &SeedQ::default()), None);
    }
}
