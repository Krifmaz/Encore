//! Discogs: releases, masters and artists (a personal token raises the rate limit and is needed to search).
use crate::Ctx;
use reqwest::RequestBuilder;

/// The Discogs API.
pub const API: &str = "https://api.discogs.com";

/// A GET on `url` with the `discogs_token` setting, when there is one.
fn req(st: &impl Ctx, url: &str) -> RequestBuilder {
    let mut r = st.http().get(url);
    let tok = st.cfg().str("discogs_token");
    if !tok.is_empty() {
        r = r.header("Authorization", format!("Discogs token={tok}"));
    }
    r
}

/// The database search with these parameters (`q`, `artist`, `release_title`, `type`, `per_page` …).
pub fn search(st: &impl Ctx, params: &[(&str, &str)]) -> RequestBuilder {
    req(st, &format!("{API}/database/search")).query(params)
}

/// One path of the API (`releases/123`, `masters/45`, `artists/6/releases`).
pub fn get(st: &impl Ctx, path: &str) -> RequestBuilder {
    req(st, &format!("{API}/{path}"))
}

/// Who the token `token` belongs to: the settings test of a token before it is saved.
pub fn identity(st: &impl Ctx, token: &str) -> RequestBuilder {
    st.http().get(format!("{API}/oauth/identity")).header("Authorization", format!("Discogs token={token}"))
        .header("User-Agent", "Encore/2 (+https://github.com/Krifmaz/Encore)")
}
