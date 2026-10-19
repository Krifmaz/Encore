//! A signed-in YouTube Music connection: its Google sign-in token, refreshed (and stored) before calls.
//! The web API itself is encore_http::ytmusic.
use crate::Ctx;
use serde_json::{json, Value};

/// Google sign-in tokens expire hourly: refresh (and store) before calls.
pub async fn fresh_data(st: &impl Ctx, conn: &encore_db::Row) -> anyhow::Result<Value> {
    use encore_db::RowExt;
    let mut d = conn["data"].clone();
    if d["mode"] != "oauth" || d["token"]["expires_at"].as_f64().unwrap_or(0.0) > encore_db::now() + 60.0 {
        return Ok(d);
    }
    let rt = d["token"]["refresh_token"].as_str().unwrap_or("").to_string();
    let t: Value = st.http().post("https://oauth2.googleapis.com/token").form(&[("client_id", st.cfg().str("ytmusic_client_id")), ("client_secret", st.cfg().str("ytmusic_client_secret")),
        ("refresh_token", rt), ("grant_type", "refresh_token".into())]).send().await?.json().await?;
    let Some(access) = t["access_token"].as_str() else { anyhow::bail!("YouTube Music sign-in expired – reconnect") };
    d["token"]["access_token"] = json!(access);
    d["token"]["expires_at"] = json!(encore_db::now() as i64 + t["expires_in"].as_i64().unwrap_or(3600));
    crate::connections::update_data(st, conn.i("id"), &d);
    Ok(d)
}
