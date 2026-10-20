//! Last.fm's API: read calls with the server's key (similar artists and songs, a listener's top lists) and
//! signed write calls (sessions, now playing, scrobbles).
use crate::Ctx;
use reqwest::RequestBuilder;
use serde_json::Value;

pub const API: &str = "https://ws.audioscrobbler.com/2.0/";

/// Last.fm request signature: every parameter (except format) sorted by name, then the shared secret, md5.
pub fn sig(params: &[(&str, String)], secret: &str) -> String {
    let mut p: Vec<&(&str, String)> = params.iter().filter(|(k, _)| *k != "format" && *k != "callback").collect();
    p.sort_by_key(|(k, _)| *k);
    let s: String = p.iter().map(|(k, v)| format!("{k}{v}")).collect::<String>() + secret;
    format!("{:x}", md5::compute(s.as_bytes()))
}

/// A signed POST to Last.fm (write calls: scrobbles, now playing, sessions).
pub async fn signed(st: &impl Ctx, method: &str, mut params: Vec<(&str, String)>) -> anyhow::Result<Value> {
    let (key, secret) = (st.cfg().str("lastfm_api_key"), st.cfg().str("lastfm_shared_secret"));
    if key.is_empty() || secret.is_empty() {
        anyhow::bail!("The admin needs to add the Last.fm API key and shared secret in Settings → Integrations");
    }
    params.push(("method", method.into()));
    params.push(("api_key", key));
    let api_sig = sig(&params, &secret);
    params.push(("api_sig", api_sig));
    params.push(("format", "json".into()));
    let d: Value = st.http().post(API).form(&params).send().await?.json().await?;
    if !d["error"].is_null() {
        anyhow::bail!("Last.fm: {}", d["message"].as_str().unwrap_or("error"));
    }
    Ok(d)
}

/// The request of a read call (no signature) with the API key `key`.
pub fn request(st: &impl Ctx, key: &str, method: &str, params: &[(&str, String)]) -> RequestBuilder {
    let mut q: Vec<(&str, String)> = vec![("method", method.into()), ("api_key", key.into()), ("format", "json".into())];
    q.extend(params.iter().cloned());
    st.http().get(API).query(&q)
}

/// A listener's profile (`user.getinfo`).
pub fn user_info(st: &impl Ctx, key: &str, user: &str) -> RequestBuilder {
    request(st, key, "user.getinfo", &[("user", user.into())])
}

/// One page (1000 songs) of the songs a listener loved (`user.getlovedtracks`).
pub fn loved_tracks(st: &impl Ctx, key: &str, user: &str, page: u32) -> RequestBuilder {
    request(st, key, "user.getlovedtracks", &[("user", user.into()), ("limit", "1000".into()), ("page", page.to_string())])
}

/// A cheap call that only works with a valid key (an artist's page), for the settings test.
pub fn check_key(st: &impl Ctx, key: &str) -> RequestBuilder {
    request(st, key, "artist.getinfo", &[("artist", "Cher".into())])
}

/// A read call (no signature) with the server's API key.
pub async fn get(st: &impl Ctx, method: &str, params: &[(&str, String)]) -> anyhow::Result<Value> {
    let key = st.cfg().str("lastfm_api_key");
    if key.is_empty() {
        anyhow::bail!("no Last.fm key");
    }
    let d: Value = request(st, &key, method, params).send().await?.json().await?;
    if !d["error"].is_null() {
        anyhow::bail!("{}", d["message"].as_str().map(String::from).unwrap_or(format!("Last.fm error {}", d["error"])));
    }
    Ok(d)
}

#[cfg(test)]
mod tests {
    #[test]
    fn signature_matches_lastfm_docs_shape() {
        // api_keyXmethodauth.getSessiontokenY + secret, md5
        let s = super::sig(&[("method", "auth.getSession".into()), ("api_key", "X".into()), ("token", "Y".into()), ("format", "json".into())], "S");
        assert_eq!(s, format!("{:x}", md5::compute(b"api_keyXmethodauth.getSessiontokenYS")));
    }
}
