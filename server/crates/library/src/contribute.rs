//! Giving back to the databases Encore leans on (Settings → Contribute); the routes are in the server's API.
//!
//! - AcoustID: songs whose MusicBrainz recording the audio itself confirmed (acoustid::check_file,
//!   "MusicBrainz recording confirmed by audio") are queued with their fingerprint and sent in batches of up
//!   to 50 with the server's client key and the admin's own user key. Each fingerprint + recording is sent
//!   once; the submission ids are kept and their status checked later.
//! - MusicBrainz seeding: an album found only on Deezer / Apple Music opens the MusicBrainz release editor
//!   with everything filled in ([`load_seed`], [`seed_fields`]; the API serves the page that posts the form).
//! - ISRCs: with a MusicBrainz account connected (OAuth2, scope submit_isrc), ISRCs from the files of
//!   audio-confirmed recordings that MusicBrainz lacks are submitted in batches.
//!
//! [`worker`] runs both submissions in the background.
use crate::Ctx;
use encore_core::ids::is_mbid;
use encore_core::json::text as s;
use encore_core::text::html_escape;
use encore_db::{now, p, RowExt};
use encore_http::{acoustid, apple, deezer, mb};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// AcoustID takes at most this many fingerprints per call
pub const BATCH: usize = 50;
/// ISRCs per MusicBrainz POST
const ISRC_BATCH: usize = 100;

/// What the contributions keep while the server runs. Clones share it.
#[derive(Clone, Default)]
pub struct Contrib {
    /// what the last runs said (Contribute page; a restart starts with a clean slate)
    last: Arc<Mutex<Option<Value>>>,
    /// when the next AcoustID call may go out
    next: Arc<tokio::sync::Mutex<Option<Instant>>>,
}

impl Contrib {
    /// Keep what a run of `kind` ("acoustid", "isrc") said, for the Contribute page.
    pub fn note(&self, kind: &str, ok: bool, message: &str) {
        let mut l = self.last.lock().unwrap();
        let mut v = l.take().unwrap_or_else(|| json!({}));
        v[kind] = json!({ "at": now(), "ok": ok, "message": message });
        *l = Some(v);
    }

    /// What the last run of `kind` said (null before the first).
    pub fn last(&self, kind: &str) -> Value {
        self.last.lock().unwrap().as_ref().map(|v| v[kind].clone()).unwrap_or(Value::Null)
    }

    /// Wait for the next AcoustID slot: AcoustID allows 3 requests a second.
    async fn pace(&self) {
        let mut n = self.next.lock().await;
        if let Some(t) = *n {
            let now = Instant::now();
            if t > now {
                tokio::time::sleep(t - now).await;
            }
        }
        *n = Some(Instant::now() + Duration::from_millis(400));
    }
}

/// "us-abc-12-34567" → "USABC1234567" when it is a valid ISRC.
pub fn clean_isrc(s: &str) -> Option<String> {
    let c: String = s.chars().filter(|c| !matches!(c, '-' | ' ')).collect::<String>().to_uppercase();
    let b = c.as_bytes();
    (b.len() == 12 && b[..2].iter().all(|x| x.is_ascii_uppercase()) && b[2..5].iter().all(|x| x.is_ascii_alphanumeric()) && b[5..].iter().all(|x| x.is_ascii_digit()))
        .then_some(c)
}

// ---- AcoustID -------------------------------------------------------------------------------------

/// The form fields of one AcoustID submit call (indexes .0 … .49).
pub fn acoustid_form(client: &str, user: &str, items: &[(String, i64, String)]) -> Vec<(String, String)> {
    let mut f = vec![("client".to_string(), client.to_string()), ("clientversion".to_string(), encore_core::VERSION.to_string()),
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

/// The keys AcoustID submissions need (client, user), or why they can't go out.
pub fn acoustid_ready(st: &impl Ctx) -> Result<(String, String), &'static str> {
    let (client, user) = (st.cfg().str("acoustid_api_key"), st.cfg().str("acoustid_user_key"));
    if !st.cfg().bool("contribute_acoustid") {
        return Err("Fingerprint submissions are off");
    }
    if client.is_empty() {
        return Err("Add the AcoustID API key under Settings → Library → Verification & duplicates first");
    }
    if user.is_empty() {
        return Err("Add your AcoustID user key to send fingerprints");
    }
    Ok((client, user))
}

/// Send queued fingerprints (up to `calls` batches), then check on earlier submissions. → (sent, imported)
pub async fn acoustid_pass(st: &impl Ctx, calls: usize) -> anyhow::Result<(usize, usize)> {
    let (client, user) = acoustid_ready(st).map_err(|e| anyhow::anyhow!(e))?;
    let mut sent = 0;
    for _ in 0..calls {
        let rows = st.db().query("SELECT id, fingerprint, duration, mbid FROM contrib_acoustid WHERE status='queued' ORDER BY id LIMIT ?", p![BATCH as i64])?;
        if rows.is_empty() {
            break;
        }
        let items: Vec<(String, i64, String)> = rows.iter().map(|r| (r.s("fingerprint"), r.i("duration"), r.s("mbid"))).collect();
        st.library().contribute.pace().await;
        let d: Value = acoustid::submit(st, &acoustid_form(&client, &user, &items)).send().await?.json().await?;
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
                    st.db().execute("UPDATE contrib_acoustid SET status=?, submission_id=?, submitted_at=?, error=NULL WHERE id=?",
                        p![if status == "imported" { "imported" } else { "submitted" }, id, t, r.i("id")])?;
                }
                None => {
                    st.db().execute("UPDATE contrib_acoustid SET status='error', error='no submission id in the answer', submitted_at=? WHERE id=?", p![t, r.i("id")])?;
                }
            }
        }
        sent += rows.len();
    }
    // submissions are imported within minutes; look at the ones older than a minute
    let mut imported = 0;
    let pending = st.db().query("SELECT id, submission_id FROM contrib_acoustid WHERE status='submitted' AND submission_id IS NOT NULL AND submitted_at < ?
        AND (checked_at IS NULL OR checked_at < ?) ORDER BY submitted_at LIMIT 200", p![now() - 60.0, now() - 600.0])?;
    for chunk in pending.chunks(BATCH) {
        let mut q: Vec<(String, String)> = vec![("client".into(), client.clone()), ("clientversion".into(), encore_core::VERSION.to_string()), ("format".into(), "json".into())];
        q.extend(chunk.iter().map(|r| ("id".to_string(), r.i("submission_id").to_string())));
        st.library().contribute.pace().await;
        let d: Value = acoustid::submission_status(st, &q).send().await?.json().await?;
        if d["status"] != "ok" {
            anyhow::bail!("AcoustID status check failed: {}", d["error"]["message"].as_str().unwrap_or("error"));
        }
        let t = now();
        for s in d["submissions"].as_array().cloned().unwrap_or_default() {
            let Some(sid) = s["id"].as_i64() else { continue };
            let Some(row) = chunk.iter().find(|r| r.i("submission_id") == sid) else { continue };
            if s["status"] == "imported" {
                imported += 1;
                st.db().execute("UPDATE contrib_acoustid SET status='imported', acoustid=?, checked_at=? WHERE id=?", p![s["result"]["id"], t, row.i("id")])?;
            } else {
                st.db().execute("UPDATE contrib_acoustid SET checked_at=? WHERE id=?", p![t, row.i("id")])?;
            }
        }
    }
    Ok((sent, imported))
}

// ---- MusicBrainz OAuth2 + ISRCs -------------------------------------------------------------------

/// How Encore names itself to MusicBrainz's edit calls.
pub fn mb_client_param() -> String {
    format!("encore-{}", encore_core::VERSION)
}

/// A fresh access token for the connected MusicBrainz account (refreshed when it is about to run out).
async fn mb_token(st: &impl Ctx) -> anyhow::Result<String> {
    let refresh = st.cfg().str("mb_oauth_refresh_token");
    if refresh.is_empty() {
        anyhow::bail!("Connect a MusicBrainz account first");
    }
    let access = st.cfg().str("mb_oauth_access_token");
    if !access.is_empty() && st.cfg().float("mb_oauth_expires_at") > now() + 120.0 {
        return Ok(access);
    }
    let r = mb::oauth_token(st, &[("grant_type", "refresh_token"), ("refresh_token", refresh.as_str()),
        ("client_id", &st.cfg().str("mb_client_id")), ("client_secret", &st.cfg().str("mb_client_secret"))]).send().await?;
    let code = r.status().as_u16();
    let d: Value = r.json().await.unwrap_or(Value::Null);
    let Some(tok) = d["access_token"].as_str() else {
        if code == 400 || code == 401 {
            // revoked or the app was deleted: the connection is gone
            st.cfg().set_many(&json!({ "mb_oauth_access_token": "", "mb_oauth_refresh_token": "" }));
        }
        anyhow::bail!("MusicBrainz did not renew the sign-in (HTTP {code}): {}", d["error_description"].as_str().or(d["error"].as_str()).unwrap_or("no reason given"));
    };
    let mut set = json!({ "mb_oauth_access_token": tok, "mb_oauth_expires_at": now() + d["expires_in"].as_f64().unwrap_or(3600.0) });
    if let Some(rt) = d["refresh_token"].as_str() {
        set["mb_oauth_refresh_token"] = json!(rt);
    }
    st.cfg().set_many(&set);
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
        x.push_str(&format!("<recording id=\"{}\"><isrc-list count=\"{}\">", html_escape(rec), isrcs.len()));
        for i in isrcs {
            x.push_str(&format!("<isrc id=\"{}\"/>", html_escape(i)));
        }
        x.push_str("</isrc-list></recording>");
    }
    x.push_str("</recording-list></metadata>");
    x
}

/// The HTTP status of a failed call, when it got that far.
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

/// Look for audio-confirmed recordings whose file carries an ISRC MusicBrainz doesn't list (a few per pass:
/// each costs a MusicBrainz lookup). → number queued
pub async fn isrc_scan(st: &impl Ctx, limit: usize) -> anyhow::Result<usize> {
    let rows = st.db().query("SELECT t.id, t.mb_trackid, t.isrc FROM tracks t WHERE t.verify_status='verified' AND t.verify_detail='MusicBrainz recording confirmed by audio'
        AND t.isrc IS NOT NULL AND t.isrc != '' AND t.mb_trackid IS NOT NULL AND t.mb_trackid != ''
        AND NOT EXISTS (SELECT 1 FROM contrib_isrc c WHERE c.recording=t.mb_trackid) ORDER BY t.id DESC LIMIT ?", p![limit as i64])?;
    let mut queued = 0;
    for r in rows {
        let rec = r.s("mb_trackid");
        let t = now();
        let Some(isrc) = clean_isrc(&r.s("isrc")).filter(|_| is_mbid(&rec)) else {
            st.db().execute("INSERT OR IGNORE INTO contrib_isrc (recording, isrc, track_id, status, error, created_at) VALUES (?,?,?,'skipped','not a valid ISRC or recording id',?)",
                p![rec, r.s("isrc"), r.i("id"), t])?;
            continue;
        };
        let d = match st.mb().get(st, &format!("recording/{rec}"), &[("inc", "isrcs".into())]).await {
            Ok(d) => d,
            // this recording's own problem (deleted, merged, a broken answer): noted, the next one is tried
            Err(e) if row_error(&e) => {
                st.db().execute("INSERT OR IGNORE INTO contrib_isrc (recording, isrc, track_id, status, error, created_at) VALUES (?,?,?,'skipped',?,?)",
                    p![rec, isrc, r.i("id"), format!("MusicBrainz recording lookup: {e}").chars().take(200).collect::<String>(), t])?;
                continue;
            }
            // MusicBrainz down or rate limiting: the next pass tries again
            Err(e) => return Err(e),
        };
        let have: Vec<String> = d["isrcs"].as_array().cloned().unwrap_or_default().iter().filter_map(|x| x.as_str().map(String::from)).collect();
        // only a "not found" means no recording has it; any other failure ends the pass (nothing is queued)
        let others: Vec<String> = match st.mb().get(st, &format!("isrc/{isrc}"), &[]).await {
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
        st.db().execute("INSERT OR IGNORE INTO contrib_isrc (recording, isrc, track_id, status, error, created_at) VALUES (?,?,?,?,?,?)", p![rec, isrc, r.i("id"), status, why, t])?;
    }
    Ok(queued)
}

/// Send queued ISRCs. → number sent
pub async fn isrc_submit(st: &impl Ctx) -> anyhow::Result<usize> {
    let rows = st.db().query("SELECT recording, isrc FROM contrib_isrc WHERE status='queued' ORDER BY created_at LIMIT ?", p![ISRC_BATCH as i64])?;
    if rows.is_empty() {
        return Ok(0);
    }
    let token = mb_token(st).await?;
    let items: Vec<(String, String)> = rows.iter().map(|r| (r.s("recording"), r.s("isrc"))).collect();
    let r = mb::submit_isrcs(st, &token, &mb_client_param(), isrc_xml(&items)).send().await?;
    let code = r.status().as_u16();
    let body = r.text().await.unwrap_or_default();
    let t = now();
    if (200..300).contains(&code) {
        for (rec, isrc) in &items {
            st.db().execute("UPDATE contrib_isrc SET status='submitted', submitted_at=?, error=NULL WHERE recording=? AND isrc=?", p![t, rec, isrc])?;
        }
        st.db().log("info", &format!("Sent {} ISRC(s) to MusicBrainz", items.len()));
        return Ok(items.len());
    }
    let why = body.split("<text>").nth(1).and_then(|x| x.split("</text>").next()).unwrap_or("").trim().to_string();
    if code == 400 {
        // MusicBrainz turned the batch down: mark them so the same batch isn't retried forever
        for (rec, isrc) in &items {
            st.db().execute("UPDATE contrib_isrc SET status='error', error=? WHERE recording=? AND isrc=?", p![format!("HTTP 400 {why}").trim().to_string(), rec, isrc])?;
        }
    }
    anyhow::bail!("MusicBrainz did not take the ISRCs (HTTP {code}){}", if why.is_empty() { String::new() } else { format!(": {why}") })
}

/// Background loop: fingerprints every few minutes, ISRCs every half hour.
pub async fn worker(st: impl Ctx) {
    tokio::time::sleep(Duration::from_secs(90)).await;
    let c = st.library().contribute.clone();
    let mut tick: u64 = 0;
    loop {
        if acoustid_ready(&st).is_ok() {
            match acoustid_pass(&st, 4).await {
                Ok((s, i)) => if s + i > 0 { c.note("acoustid", true, &format!("{s} sent, {i} imported")) },
                Err(e) => c.note("acoustid", false, &e.to_string()),
            }
        }
        if tick.is_multiple_of(6) && st.cfg().bool("contribute_isrc") && !st.cfg().str("mb_oauth_refresh_token").is_empty() {
            let r = async { let q = isrc_scan(&st, 20).await?; let n = isrc_submit(&st).await?; anyhow::Ok((q, n)) }.await;
            match r {
                Ok((q, n)) => if q + n > 0 { c.note("isrc", true, &format!("{q} found, {n} sent")) },
                Err(e) => c.note("isrc", false, &e.to_string()),
            }
        }
        tick += 1;
        tokio::time::sleep(Duration::from_secs(300)).await;
    }
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

/// Deezer's / Apple's data for an album, as a seed. `artist` = (name, MusicBrainz id) from the artist page.
pub async fn load_seed(st: &impl Ctx, deezer: Option<&str>, apple: Option<&str>, artist: (&str, Option<&str>)) -> anyhow::Result<SeedAlbum> {
    let mut a = SeedAlbum::default();
    let mbid_for = |name: &str| -> Option<String> {
        let (an, am) = artist;
        am.filter(|m| is_mbid(m) && crate::catalog::title_key(name) == crate::catalog::title_key(an)).map(String::from)
    };
    if let Some(id) = deezer.filter(|i| i.chars().all(|c| c.is_ascii_digit()) && !i.is_empty()) {
        let d: Value = deezer::album(st, id).send().await?.error_for_status()?.json().await?;
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
        let tl = crate::catalog::tracklist(st, &json!({ "deezer": id })).await?;
        a.tracks = tl.iter().map(|t| SeedTrack { title: s(&t["title"]), artist: s(&t["artist"]),
            length_ms: t["length"].as_f64().map(|x| (x * 1000.0).round() as i64), disc: t["disc"].as_i64().unwrap_or(1), number: t["track"].as_i64().unwrap_or(0) }).collect();
    }
    if let Some(id) = apple.filter(|i| i.chars().all(|c| c.is_ascii_digit()) && !i.is_empty()) {
        let d: Value = apple::lookup(st, id, "song", Some(300)).send().await?.error_for_status()?.json().await?;
        let res = d["results"].as_array().cloned().unwrap_or_default();
        let coll = res.iter().find(|x| x["wrapperType"] == "collection").cloned().unwrap_or(Value::Null);
        let url = { let u = s(&coll["collectionViewUrl"]); let u = u.split('?').next().unwrap_or("").to_string(); if u.is_empty() { format!("https://music.apple.com/album/{id}") } else { u } };
        a.urls.push((url.clone(), LINK_STREAMING));
        a.urls.push((url, LINK_PURCHASE_DOWNLOAD));
        a.sources.push("Apple Music".into());
        if a.title.is_empty() {
            let raw = s(&coll["collectionName"]);
            let kind = crate::catalog::kind_of("", &raw);
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

    #[test]
    fn language_guess() {
        assert_eq!(guess_language(&["Love of my life", "You and I", "The Night"]), (Some("eng"), Some("Latn")));
        assert_eq!(guess_language(&["Rodeo", "Antidote", "Pornography"]), (None, Some("Latn")));
        assert_eq!(guess_language(&["ちゅきちゅきポコポコポン", "夜に駆ける"]), (Some("jpn"), Some("Jpan")));
        assert_eq!(guess_language(&["봄날", "피 땀 눈물"]), (Some("kor"), Some("Kore")));
        assert_eq!(guess_language(&["Кино", "Группа крови"]), (None, Some("Cyrl")));
    }
}
