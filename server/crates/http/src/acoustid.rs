//! AcoustID: which recordings a Chromaprint fingerprint belongs to, and fingerprint submissions.
use crate::Ctx;
use reqwest::RequestBuilder;
use serde_json::{json, Value};

/// The AcoustID fingerprint lookup.
pub const LOOKUP: &str = "https://api.acoustid.org/v2/lookup";
/// Where fingerprints are submitted.
pub const SUBMIT: &str = "https://api.acoustid.org/v2/submit";
/// Where a submission's status is asked.
pub const SUBMISSION_STATUS: &str = "https://api.acoustid.org/v2/submission_status";
/// A track id every AcoustID database has: looking it up only proves the client key.
const KNOWN_TRACK: &str = "9ff43b6a-4f16-427c-93c2-92307ca505e0";

/// Send fingerprints (the form of one submit call: client, user, duration.N, fingerprint.N, mbid.N …).
pub fn submit(st: &impl Ctx, form: &[(String, String)]) -> RequestBuilder {
    st.http().post(SUBMIT).form(form)
}

/// The status of earlier submissions (client, format and one `id` per submission).
pub fn submission_status(st: &impl Ctx, q: &[(String, String)]) -> RequestBuilder {
    st.http().get(SUBMISSION_STATUS).query(q)
}

/// The address of a lookup that only works with a valid client key (for the key tests).
pub fn check_key_url(key: &str) -> String {
    reqwest::Url::parse_with_params(LOOKUP, &[("client", key), ("trackid", KNOWN_TRACK)]).map(|u| u.to_string()).unwrap_or_else(|_| LOOKUP.to_string())
}

/// The request of [`check_key_url`].
pub fn check_key(st: &impl Ctx, key: &str) -> RequestBuilder {
    st.http().get(check_key_url(key))
}

/// → [{acoustid, score, recordings:[{id,title,artist,length,release_groups,release_group_ids}]}], best first.
pub async fn lookup(st: &impl Ctx, duration: f64, fp: &str) -> anyhow::Result<Vec<Value>> {
    // AcoustID allows 3 requests a second
    crate::take_slot(&st.pace().acoustid, std::time::Duration::from_millis(340)).await;
    let d: Value = st.http().post(LOOKUP).form(&[("client", st.cfg().str("acoustid_api_key")), ("duration", (duration as i64).to_string()),
        ("fingerprint", fp.to_string()), ("meta", "recordings releasegroups compress".into())]).send().await?.json().await?;
    if d["status"] != "ok" {
        anyhow::bail!("{}", d["error"]["message"].as_str().unwrap_or("AcoustID error"));
    }
    let mut out: Vec<Value> = d["results"].as_array().into_iter().flatten().map(|res| {
        let recs: Vec<Value> = res["recordings"].as_array().into_iter().flatten().map(|rec| {
            let artist: String = rec["artists"].as_array().map(|a| a.iter().fold(String::new(), |mut s, x| {
                // name then joinphrase ("A", " & ", "B", ""), appended in place
                s.push_str(x["name"].as_str().unwrap_or(""));
                s.push_str(x["joinphrase"].as_str().unwrap_or(""));
                s
            })).unwrap_or_default();
            let groups = rec["releasegroups"].as_array().cloned().unwrap_or_default();
            json!({ "id": rec["id"], "title": rec["title"].as_str().unwrap_or(""), "artist": artist, "length": rec["duration"],
                "release_groups": groups.iter().take(3).map(|g| g["title"].clone()).collect::<Vec<_>>(),
                "release_group_ids": groups.iter().filter_map(|g| g["id"].as_str()).collect::<Vec<_>>() })
        }).collect();
        json!({ "acoustid": res["id"], "score": res["score"].as_f64().unwrap_or(0.0), "recordings": recs })
    }).collect();
    out.sort_by(|a, b| b["score"].as_f64().partial_cmp(&a["score"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
    Ok(out)
}
