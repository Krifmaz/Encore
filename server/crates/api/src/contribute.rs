//! Settings → Contribute through the API: the status, "send now", connecting a MusicBrainz account
//! (OAuth2, scope submit_isrc), and the release editor seeding page (GET /api/contribute/mb-seed/{service}-{id}
//! serves a small page that posts the form to musicbrainz.org/release/add, also through a signed link the
//! phone opens without a session). The submissions themselves and the album data of a seed are
//! encore_library::contribute.
use crate::enrich::enrich;
use crate::auth::{Admin, MaybeUser, User};
use encore_db::{now, RowExt};
use encore_core::error::{err, ApiResult};
use encore_db::p;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use encore_http::mb;
use encore_library::contribute::{self, SeedAlbum};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

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

fn mb_redirect_uri(st: &AppState) -> Option<String> {
    let base = st.cfg.str("base_url").trim_end_matches('/').to_string();
    (!base.is_empty()).then(|| format!("{base}/api/contribute/mb/callback"))
}

// ---- routes: status, run now, connect --------------------------------------------------------------

fn counts(st: &AppState, table: &str) -> Value {
    let mut out = json!({});
    for r in st.db.query(&format!("SELECT status, COUNT(*) n FROM {table} GROUP BY status"), p![]).unwrap_or_default() {
        out[r.s("status")] = json!(r.i("n"));
    }
    out
}

async fn status(State(st): State<AppState>, Admin(viewer): Admin) -> Json<Value> {
    let ac = counts(&st, "contrib_acoustid");
    let ic = counts(&st, "contrib_isrc");
    let n = |v: &Value, k: &str| v[k].as_i64().unwrap_or(0);
    let recent: Vec<Value> = st.db.query("SELECT c.status, c.mbid, c.acoustid, c.error, c.submitted_at, c.created_at, t.title, t.artist FROM contrib_acoustid c
        LEFT JOIN tracks t ON t.id=c.track_id ORDER BY c.id DESC LIMIT 10", p![]).unwrap_or_default().into_iter().map(serde_json::Value::Object).collect();
    let mut v = json!({
        "acoustid": {
            "enabled": st.cfg.bool("contribute_acoustid"),
            "has_client_key": !st.cfg.str("acoustid_api_key").is_empty(),
            "has_user_key": !st.cfg.str("acoustid_user_key").is_empty(),
            "can_fingerprint": encore_library::acoustid::can_fingerprint(),
            "ready": contribute::acoustid_ready(&st).is_ok(),
            "why_not": contribute::acoustid_ready(&st).err(),
            "queued": n(&ac, "queued"), "submitted": n(&ac, "submitted"), "imported": n(&ac, "imported"), "failed": n(&ac, "error"),
            "last": st.library.contribute.last("acoustid"),
            "recent": recent,
        },
        "isrc": {
            "enabled": st.cfg.bool("contribute_isrc"),
            "has_client": !st.cfg.str("mb_client_id").is_empty() && !st.cfg.str("mb_client_secret").is_empty(),
            "connected": !st.cfg.str("mb_oauth_refresh_token").is_empty(),
            "account": st.cfg.str("mb_oauth_account"),
            "redirect_uri": mb_redirect_uri(&st),
            "queued": n(&ic, "queued"), "submitted": n(&ic, "submitted"), "skipped": n(&ic, "skipped"), "failed": n(&ic, "error"),
            "last": st.library.contribute.last("isrc"),
        },
    });
    enrich(&st, Some(&viewer), &mut v).await;
    Json(v)
}

async fn run_acoustid(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    contribute::acoustid_ready(&st).map_err(|e| err(400, e))?;
    match contribute::acoustid_pass(&st, 10).await {
        Ok((s, i)) => {
            let msg = format!("{s} sent, {i} imported");
            st.library.contribute.note("acoustid", true, &msg);
            Ok(Json(json!({ "ok": true, "sent": s, "imported": i, "message": msg })))
        }
        Err(e) => {
            st.library.contribute.note("acoustid", false, &e.to_string());
            Err(err(502, e.to_string()))
        }
    }
}

async fn run_isrc(State(st): State<AppState>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    if st.cfg.str("mb_oauth_refresh_token").is_empty() {
        return Err(err(400, "Connect a MusicBrainz account first"));
    }
    let r = async { let q = contribute::isrc_scan(&st, 40).await?; let n = contribute::isrc_submit(&st).await?; anyhow::Ok((q, n)) }.await;
    match r {
        Ok((q, n)) => {
            let msg = format!("{q} found, {n} sent");
            st.library.contribute.note("isrc", true, &msg);
            Ok(Json(json!({ "ok": true, "found": q, "sent": n, "message": msg })))
        }
        Err(e) => {
            st.library.contribute.note("isrc", false, &e.to_string());
            Err(err(502, e.to_string()))
        }
    }
}

async fn mb_connect(State(st): State<AppState>, Admin(u): Admin, headers: axum::http::HeaderMap) -> ApiResult<Json<Value>> {
    let cid = st.cfg.str("mb_client_id");
    if cid.is_empty() || st.cfg.str("mb_client_secret").is_empty() {
        return Err(err(400, "Add the MusicBrainz application's client ID and secret first"));
    }
    let redirect = mb_redirect_uri(&st).ok_or_else(|| err(400, "Set the Public URL in Settings → General first (the sign-in needs a callback address)"))?;
    // a single-use state for this admin (crate::signin): the callback links the account for them only
    let url = crate::signin::start(&st, u.i("id"), "musicbrainz", &headers, |state, _| {
        let q = [("response_type", "code"), ("client_id", cid.as_str()), ("redirect_uri", redirect.as_str()), ("scope", "submit_isrc tag rating"),
                 ("state", state), ("access_type", "offline"), ("approval_prompt", "force")];
        Ok(format!("{}/oauth2/authorize?{}", mb::MAIN, q.iter().map(|(k, v)| format!("{k}={}", urlencoding::encode(v))).collect::<Vec<_>>().join("&")))
    }).map_err(|e| err(400, e.to_string()))?;
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

/// MusicBrainz comes back here. The account is linked for the admin who started the sign-in (its single-use
/// `state`); a browser that is not signed in as that admin is asked first (crate::signin).
async fn mb_callback(State(st): State<AppState>, MaybeUser(u): MaybeUser, Query(q): Query<CallbackQ>) -> Response {
    crate::signin::callback(&st, u, "musicbrainz", &q.state, &q.error, &q.code, |r| match r {
        Ok(()) => back("connected", true),
        Err(e) => back(&e, false),
    }).await
}

/// The callback's code exchanged for MusicBrainz tokens → (the account's name, the token answer).
pub async fn mb_exchange(st: &AppState, code: &str) -> anyhow::Result<(String, Value)> {
    let redirect = mb_redirect_uri(st).ok_or_else(|| anyhow::anyhow!("Set the Public URL first"))?;
    let r = mb::oauth_token(st, &[("grant_type", "authorization_code"), ("code", code), ("redirect_uri", redirect.as_str()),
        ("client_id", &st.cfg.str("mb_client_id")), ("client_secret", &st.cfg.str("mb_client_secret"))]).send().await
        .map_err(|e| anyhow::anyhow!("Could not reach MusicBrainz: {e}"))?;
    let status = r.status().as_u16();
    let d: Value = r.json().await.unwrap_or(Value::Null);
    if d["access_token"].is_null() {
        anyhow::bail!("MusicBrainz did not accept the sign-in (HTTP {status}): {}", d["error_description"].as_str().or(d["error"].as_str()).unwrap_or("check the client ID, secret and callback URL"));
    }
    let tok = d["access_token"].as_str().unwrap_or("");
    let who: Value = match mb::userinfo(st, tok).send().await { Ok(r) => r.json().await.unwrap_or(Value::Null), Err(_) => Value::Null };
    Ok((who["sub"].as_str().unwrap_or("").to_string(), d))
}

/// Saves the MusicBrainz account `account` (token answer `d`) as the server's, for the admin `u` who started
/// the sign-in. An error when `u` is no longer an admin, or when MusicBrainz gave no refresh token (saved
/// anyway: it works for an hour).
pub fn mb_link(st: &AppState, u: &encore_db::Row, account: &str, d: &Value) -> Result<(), String> {
    if !encore_config::perms::is_admin(u) {
        return Err("Only an admin can connect the server's MusicBrainz account".into());
    }
    st.cfg.set_many(&json!({ "mb_oauth_access_token": d["access_token"].as_str().unwrap_or(""), "mb_oauth_refresh_token": d["refresh_token"].as_str().unwrap_or(""),
        "mb_oauth_expires_at": now() + d["expires_in"].as_f64().unwrap_or(3600.0), "mb_oauth_account": account }));
    st.db.log("info", &format!("{} connected the MusicBrainz account {account} for ISRC submissions", u.s("username")));
    if d["refresh_token"].as_str().unwrap_or("").is_empty() {
        return Err("MusicBrainz gave no refresh token – submissions stop after an hour. Connect again".into());
    }
    Ok(())
}

async fn mb_disconnect(State(st): State<AppState>, Admin(_): Admin) -> Json<Value> {
    st.cfg.set_many(&json!({ "mb_oauth_access_token": "", "mb_oauth_refresh_token": "", "mb_oauth_expires_at": 0.0, "mb_oauth_account": "" }));
    Json(json!({ "ok": true }))
}

// ---- MusicBrainz release editor seeding ------------------------------------------------------------

/// The page the browser opens: posts the seeded form to MusicBrainz at once (a button when scripts are off).
pub fn seed_html(a: &SeedAlbum, fields: &[(String, String)]) -> String {
    use encore_core::text::html_escape as e;
    let inputs: String = fields.iter().map(|(k, v)| format!("<input type=\"hidden\" name=\"{}\" value=\"{}\">", e(k), e(v))).collect::<Vec<_>>().join("\n");
    format!(r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Add to MusicBrainz</title><meta name="referrer" content="no-referrer">
<style>body{{font-family:system-ui,sans-serif;background:#111;color:#eee;display:grid;place-items:center;min-height:100vh;margin:0;padding:16px;box-sizing:border-box}}main{{max-width:520px}}button{{font:inherit;padding:10px 18px;border-radius:8px;border:0;background:#ba478f;color:#fff;cursor:pointer}}p{{color:#bbb}}</style></head>
<body><main><h1>Add “{title}” to MusicBrainz</h1><p>Opening the MusicBrainz release editor with the album filled in from {src}. Check every tab before you enter the edit: you are the editor, Encore only typed it in.</p>
<form id="seed" method="post" action="{main}/release/add" accept-charset="utf-8">
{inputs}
<button type="submit">Open the release editor</button></form></main>
<script>document.getElementById('seed').submit();</script></body></html>"#,
        main = mb::MAIN, title = e(&a.title), src = e(&if a.sources.is_empty() { "Deezer/Apple".to_string() } else { a.sources.join(" and ") }))
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
    let a = contribute::load_seed(&st, deezer.as_deref(), apple.as_deref(), artist).await.map_err(|e| err(502, format!("Could not load the album: {e}")))?;
    let fields = contribute::seed_fields(&a);
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
    use encore_library::acoustid::queue_acoustid;
    use encore_library::contribute::{seed_fields, SeedTrack, LINK_FREE_STREAMING, LINK_PURCHASE_DOWNLOAD, LINK_STREAMING};

    const MBID: &str = "b1a9c0e9-d987-4042-ae91-78d6a3267d69";

    #[test]
    fn queues_only_confirmed_recordings_once() {
        let dir = std::env::temp_dir().join(format!("encore-contrib-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = crate::test_support::state_in(&dir);
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
