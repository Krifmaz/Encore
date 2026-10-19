//! ListenBrainz: playlists made for a listener, likes, listens, a listener's own playlists, and the Labs
//! API (similar artists). The functions hand out the requests, with the listener's token where the call
//! needs one; the caller sends them and reads the answer its way.
use crate::Ctx;
use reqwest::RequestBuilder;
use serde_json::Value;

pub const API: &str = "https://api.listenbrainz.org/1";
pub const LABS: &str = "https://labs.api.listenbrainz.org";

fn with_token(r: RequestBuilder, token: &str) -> RequestBuilder {
    r.header("Authorization", format!("Token {token}"))
}

/// Send listens (`body`: `{listen_type, payload}`) for the listener whose token this is.
pub fn submit_listens(st: &impl Ctx, token: &str, body: &Value) -> RequestBuilder {
    with_token(st.http().post(format!("{API}/submit-listens")), token).json(body)
}

/// The recordings `user` loved, with their names, 1000 at a time from `offset`.
pub fn feedback(st: &impl Ctx, user: &str, offset: usize) -> RequestBuilder {
    st.http().get(format!("{API}/feedback/user/{user}/get-feedback"))
        .query(&[("score", "1"), ("count", "1000"), ("offset", &offset.to_string()), ("metadata", "true")])
}

/// Love (`{recording_mbid, score: 1}`) or clear (score 0) a recording for the listener whose token this is.
pub fn recording_feedback(st: &impl Ctx, token: &str, body: &Value) -> RequestBuilder {
    with_token(st.http().post(format!("{API}/feedback/recording-feedback")), token).json(body)
}

/// Whether a token is valid (and whose it is): in the Authorization header (`header`), else as the older
/// `?token=` parameter.
pub fn validate_token(st: &impl Ctx, token: &str, header: bool) -> RequestBuilder {
    let r = st.http().get(format!("{API}/validate-token"));
    if header { with_token(r, token) } else { r.query(&[("token", token)]) }
}

/// The playlists ListenBrainz made for `user` (Weekly Jams, Daily Jams …), newest first.
pub fn created_for(st: &impl Ctx, user: &str, count: u32) -> RequestBuilder {
    st.http().get(format!("{API}/user/{user}/playlists/createdfor")).query(&[("count", count.to_string())])
}

/// One playlist with its tracks (a private one needs its owner's token).
pub fn playlist(st: &impl Ctx, id: &str, token: Option<&str>) -> RequestBuilder {
    let r = st.http().get(format!("{API}/playlist/{id}"));
    match token { Some(t) => with_token(r, t), None => r }
}

/// Make a playlist (`body`: `{playlist: {title, track, extension}}`) for the listener whose token this is.
pub fn playlist_create(st: &impl Ctx, token: &str, body: &Value) -> RequestBuilder {
    with_token(st.http().post(format!("{API}/playlist/create")), token).json(body)
}

/// Add tracks (`body`: `{playlist: {track}}`) to the end of a playlist.
pub fn playlist_add(st: &impl Ctx, token: &str, id: &str, body: &Value) -> RequestBuilder {
    with_token(st.http().post(format!("{API}/playlist/{id}/item/add")), token).json(body)
}

/// Take tracks (`body`: `{index, count}`) out of a playlist.
pub fn playlist_delete(st: &impl Ctx, token: &str, id: &str, body: &Value) -> RequestBuilder {
    with_token(st.http().post(format!("{API}/playlist/{id}/item/delete")), token).json(body)
}

/// Labs: the artists listened to in the same sessions as the artist `mbid`, by the named `algorithm`.
pub fn similar_artists(st: &impl Ctx, mbid: &str, algorithm: &str) -> RequestBuilder {
    st.http().get(format!("{LABS}/similar-artists/json")).query(&[("algorithm", algorithm), ("artist_mbids", mbid)])
}
