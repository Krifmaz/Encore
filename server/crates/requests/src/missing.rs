//! Requesting what a playlist lacks ("Download missing", the auto-download rules). Playlists and liked
//! songs live in encore_sync::playlists; the server hands this to the sync as
//! `encore_sync::Ctx::request_missing`.
use crate::create::{create_request, NewRequest};
use crate::Ctx;
use encore_db::{p, Row, RowExt};
use encore_sync::playlists::{opt, MissingFailure, MissingReport, REREQUEST};
use serde_json::{json, Value};

/// How long the auto-download waits before it asks again for a song whose request failed (seconds): 15
/// minutes after its first failed attempt, twice as long after each one after that (30 minutes, an hour, 2
/// hours …), at most a day. It used to ask again at every sync – every 15 minutes, for days, for a song no
/// source has.
pub fn auto_retry_after(attempts: u64) -> f64 {
    (900.0 * 2f64.powi(attempts.saturating_sub(1).min(16) as i32)).min(86_400.0)
}

/// Request every item that's not in the library yet. auto=true (auto-download rules) skips approval.
/// An item whose request is gone, failed, cancelled or denied is requested again; one whose request is
/// pending, running or done counts as in progress; when someone already asked for the song (409) the item
/// is linked to that request instead of being skipped.
pub fn request_missing(st: &impl Ctx, u: &Row, pid: i64, auto: bool, album_mode: Option<String>) -> anyhow::Result<MissingReport> {
    let mut rep = MissingReport::default();
    let rows = st.db().query("SELECT i.*, r.status AS request_status, r.updated_at AS request_updated_at,
                                   COALESCE(json_extract(r.detail, '$.attempts'), 1) AS request_attempts
                            FROM playlist_items i LEFT JOIN requests r ON r.id=i.request_id
                            WHERE i.playlist_id=? ORDER BY i.position, i.id", p![pid])?;
    let t = encore_db::now();
    // once the weekly limit is hit the rest of the rows are reported, not requested
    let mut limit: Option<String> = None;
    for r in rows {
        if r.i("track_id") > 0 {
            continue;
        }
        let status = r.s("request_status");
        if r.i("request_id") > 0 && !status.is_empty() && !REREQUEST.contains(&status.as_str()) {
            rep.in_progress += 1;
            continue;
        }
        // the auto-download asks again for a failed song only once its wait is over ([`auto_retry_after`]);
        // "Download missing" asks at once
        if auto && status == "failed" && t - r.f("request_updated_at") < auto_retry_after(r.i("request_attempts").max(1) as u64) {
            continue;
        }
        let what = || match r.s("title") { t if !t.is_empty() => t, _ => r.s("artist") };
        if let Some(why) = &limit {
            rep.failed.push(MissingFailure { title: what(), reason: why.clone() });
            continue;
        }
        let b = NewRequest { kind: "track".into(), mbid: opt(&r, "mbid"), artist: opt(&r, "artist"), title: opt(&r, "title"), album: opt(&r, "album"),
            cover_url: opt(&r, "cover_url"), length: Some(r.f("duration")).filter(|d| *d > 0.0), isrc: opt(&r, "isrc"), album_mode: album_mode.clone(), ..Default::default() };
        let extra = if auto { Some(json!({ "auto": true })) } else { None };
        let rid = match create_request(st, u, &b, None, None) {
            Ok(id) => id,
            // someone already asked for this song: the item follows that request
            Err(e) if e.status.as_u16() == 409 => {
                match e.extra.as_ref().and_then(|x| x["id"].as_i64()) {
                    Some(id) => {
                        st.db().execute("UPDATE playlist_items SET request_id=? WHERE id=?", p![id, r.i("id")])?;
                        rep.in_progress += 1;
                    }
                    None => rep.failed.push(MissingFailure { title: what(), reason: e.detail }),
                }
                continue;
            }
            // the weekly limit stops the requests (what was requested so far stays); with nothing requested yet it is an error
            Err(e) if e.status.as_u16() == 429 => {
                if rep.requested == 0 && rep.in_progress == 0 {
                    anyhow::bail!("{}", e.detail);
                }
                rep.failed.push(MissingFailure { title: what(), reason: e.detail.clone() });
                limit = Some(e.detail);
                continue;
            }
            // one row that can't be requested (no artist or title, say) must not stop the others
            Err(e) => {
                tracing::info!("playlist {pid}: couldn't request a missing track ({}): {}", what(), e.detail);
                rep.failed.push(MissingFailure { title: what(), reason: e.detail });
                continue;
            }
        };
        if let Some(x) = extra {
            let d = st.db().one("SELECT detail FROM requests WHERE id=?", p![rid])?.map(|r| r.s("detail")).unwrap_or("{}".into());
            let mut d: Value = serde_json::from_str(&d).unwrap_or(json!({}));
            d["auto"] = x["auto"].clone();
            st.db().execute("UPDATE requests SET detail=? WHERE id=?", p![d.to_string(), rid])?;
        }
        if auto || encore_config::perms::can(st.cfg(), u, "auto_approve_playlists") {
            st.db().execute("UPDATE requests SET status='queued', message=? WHERE id=? AND status='pending'",
                p![if auto { "Auto-download" } else { "Auto-approved (playlist)" }, rid])?;
            crate::events::emit(st, rid);
        }
        st.db().execute("UPDATE playlist_items SET request_id=? WHERE id=?", p![rid, r.i("id")])?;
        rep.requested += 1;
    }
    Ok(rep)
}

