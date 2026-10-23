//! Notifications (Discord, ntfy, generic webhook) and media-server refresh.
use crate::Ctx;
use serde_json::json;

/// Tell the owner about a request: a Discord webhook embed and/or an ntfy message, as the settings name them.
pub async fn send(st: &impl Ctx, title: &str, message: &str, image: Option<&str>) {
    let hook = st.cfg().str("discord_webhook");
    if !hook.is_empty() {
        let mut embed = json!({ "title": title, "description": message, "color": 0x8B5CF6 });
        if let Some(i) = image.filter(|i| !i.is_empty()) {
            embed["thumbnail"] = json!({ "url": i });
        }
        if let Err(e) = st.http().post(&hook).json(&json!({ "username": st.cfg().str("app_name"), "embeds": [embed] })).send().await {
            st.db().log("warning", &format!("Discord notification failed: {e}"));
        }
    }
    let ntfy = st.cfg().str("ntfy_url");
    if !ntfy.is_empty() {
        // title/tags as query parameters: HTTP headers can't carry emoji or accented characters
        let mut rb = st.http().post(&ntfy).query(&[("title", title), ("tags", "musical_note")]).body(message.to_string());
        let tok = st.cfg().str("ntfy_token");
        if !tok.is_empty() {
            rb = rb.bearer_auth(tok);
        }
        if let Err(e) = rb.send().await {
            st.db().log("warning", &format!("ntfy notification failed: {e}"));
        }
    }
    let wh = st.cfg().str("webhook_url");
    if !wh.is_empty() {
        if let Err(e) = st.http().post(&wh).json(&json!({ "title": title, "message": message, "image": image })).send().await {
            st.db().log("warning", &format!("Webhook notification failed: {e}"));
        }
    }
}

/// Ask the media servers to scan for the new music: each one whose connection plugin (navidrome, jellyfin,
/// plex) is installed and switched on, with its address and sign-in set.
pub async fn refresh_media_servers(st: &impl Ctx) -> Vec<&'static str> {
    let mut done = vec![];
    let on = |id: &str| st.plugins().enabled(id);
    let (nu, nus) = (st.cfg().str("navidrome_url"), st.cfg().str("navidrome_user"));
    if on("navidrome") && !nu.is_empty() && !nus.is_empty() {
        let salt = hex::encode(rand::random::<[u8; 6]>());
        let token = format!("{:x}", md5::compute(format!("{}{salt}", st.cfg().str("navidrome_password"))));
        match st.http().get(format!("{}/rest/startScan", nu.trim_end_matches('/')))
            .query(&[("u", nus.as_str()), ("t", &token), ("s", &salt), ("v", "1.16.1"), ("c", "encore"), ("f", "json")]).send().await {
            Ok(_) => done.push("Navidrome"),
            Err(e) => st.db().log("warning", &format!("Navidrome scan failed: {e}")),
        }
    }
    let (ju, jk) = (st.cfg().str("jellyfin_url"), st.cfg().str("jellyfin_api_key"));
    if on("jellyfin") && !ju.is_empty() && !jk.is_empty() {
        match st.http().post(format!("{}/Library/Refresh", ju.trim_end_matches('/'))).header("X-Emby-Token", jk).send().await {
            Ok(_) => done.push("Jellyfin"),
            Err(e) => st.db().log("warning", &format!("Jellyfin refresh failed: {e}")),
        }
    }
    let (pu, pt) = (st.cfg().str("plex_url"), st.cfg().str("plex_token"));
    if on("plex") && !pu.is_empty() && !pt.is_empty() {
        match st.http().get(format!("{}/library/sections/all/refresh", pu.trim_end_matches('/'))).query(&[("X-Plex-Token", pt)]).send().await {
            Ok(_) => done.push("Plex"),
            Err(e) => st.db().log("warning", &format!("Plex refresh failed: {e}")),
        }
    }
    done
}
