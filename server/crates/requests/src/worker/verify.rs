//! What a download is checked against: the release's track list (MusicBrainz, or the catalogue it was
//! found in), used to rank candidates, to tell a complete download from a partial one and to tag the files.
use super::*;

/// The release a request is for, with its tracks (looked up and kept on the request when missing).
pub(super) async fn release_info(st: &impl Ctx, r: &mut Req) -> Option<Value> {
    if r.d.get("release").is_none() && r.s("kind") == "album" {
        if r.s("mbid").is_empty() && r.d["ext"].is_object() {
            // not in MusicBrainz: track list from Deezer/Apple/…
            r.d["release"] = match encore_library::catalog::tracklist(st, &r.d["ext"]).await {
                Ok(t) if !t.is_empty() => json!({ "id": null, "tracks": t }),
                Ok(_) => Value::Null,
                Err(e) => {
                    st.db().log("warning", &format!("Tracklist lookup failed for {}: {e}", r.label()));
                    Value::Null
                }
            };
        } else if !r.s("mbid").is_empty() {
            let mbid = r.s("mbid");
            let lookup = async {
                let Some(rid) = mb::best_release_for_group(st, &mbid, None).await? else { return Ok(Value::Null) };
                let rel = mb::release(st, &rid).await?;
                let tracks: Vec<Value> = rel["tracks"].as_array().cloned().unwrap_or_default().iter()
                    .map(|t| json!({ "title": t["title"], "artist": t["artist"], "length": t["length"], "recording_id": t["recording_id"], "isrc": t["isrc"] })).collect();
                Ok(json!({ "id": rel["id"], "tracks": tracks }))
            };
            // while MusicBrainz's line is long the message says so ("Looking up the album's songs – waiting for
            // MusicBrainz …"), then it is what it was
            let (status, before) = (r.s("status"), r.s("message"));
            let res: anyhow::Result<Value> = with_steps(st, r.id, &status, "Looking up the album's songs", &before, lookup).await;
            r.d["release"] = res.unwrap_or_else(|e| {
                st.db().log("warning", &format!("MusicBrainz release lookup failed: {e}"));
                Value::Null
            });
        }
    }
    r.d.get("release").filter(|v| !v.is_null()).cloned()
}

/// How many tracks the release has.
pub(super) fn n_tracks(rel: &Option<Value>) -> Option<usize> {
    rel.as_ref().and_then(|r| r["tracks"].as_array().map(|a| a.len()))
}

/// Track list/year from Deezer or Apple for releases MusicBrainz doesn't have (used to tag the files).
pub(super) fn source_meta(r: &Req) -> Option<Value> {
    if !r.s("mbid").is_empty() || !r.d["ext"].is_object() {
        return None;
    }
    Some(json!({ "tracks": r.d["release"]["tracks"].as_array().cloned().unwrap_or_default(), "year": r.s("year"), "upc": r.d["ext"]["upc"] }))
}
