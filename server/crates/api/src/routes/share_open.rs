//! Opening a share link where the listener already listens (0.42.4).
//!
//! A share link (`/s/<token>`) used to land everyone on the public share page, so a listener with the Encore app,
//! or one already signed in to this server in the browser, had to find the song again by hand. Now the page first
//! looks at who opened it:
//!
//! 1. **An Android phone's browser** is sent on to the Android app with an `intent://` link
//!    ([`android_intent`]). Chrome (and the other Android browsers that read intent links) opens the app when it is
//!    installed; when it is not, the browser goes on to the intent's fallback, the same link with `?app=0`, which
//!    skips this step.
//! 2. **A browser signed in to this server** (its session cookie) is sent to the thing in the web app's own pages
//!    ([`web_path`]): the album (a shared song's album, with that song picked out), the artist, or the playlist when
//!    it is the listener's own or one they added already.
//! 3. **Everyone else** – no app, not signed in, iPhones without a session, and every link-preview bot – gets the
//!    share page as before.
//!
//! The app asks [`open`] (`GET /api/share/{token}/open`) where to go once it has the link, with its own sign-in.

use super::{link_gone, share_row, shared_playlist, Shared};
use crate::auth::User;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Json;
use encore_core::error::ApiResult;
use encore_db::{p, Row, RowExt};
use serde_json::{json, Value};

/// The custom scheme the Android app answers share links on (`encore://<host>/…/s/<token>`, its manifest's
/// intent filter). The `intent://` link names it so the browser can hand the link to the app.
pub(crate) const APP_SCHEME: &str = "encore";

/// The query that turns the jump to the app off: the intent's fallback (the app isn't installed) and the app's own
/// "open in the browser" (it can't open the link) carry it, so neither goes round to the app again.
pub(crate) const NO_APP: &str = "app=0";

/// `GET /api/share/{token}/open`: where the signed-in listener's apps open what this link shares ([`target`]).
pub(super) async fn open(State(st): State<AppState>, User(u): User, Path(token): Path<String>) -> ApiResult<Json<Value>> {
    let uid = u.i("id");
    Ok(Json(st.blocking(move |st| target(st, uid, &token)).await?))
}

/// Where user `uid` finds what the link with `token` shares in the library, as the apps open it:
/// - a song: `{"kind": "track", "track_id", "album_id"}` (its album, playing or picking out the song);
/// - an album: `{"kind": "album", "album_id"}`;
/// - an artist: `{"kind": "artist", "name"}` (the name the library shows);
/// - a playlist: `{"kind": "playlist", "own", "added", "playlist_id", "name"}`: `playlist_id` is the listener's own
///   playlist (`own`) or their entry for it (`added`), and null while it is neither (the share page offers "Add to my
///   playlists" then).
///
/// A 404 when the link doesn't work any more (the song left the library, the playlist was deleted …).
pub(crate) fn target(st: &AppState, uid: i64, token: &str) -> ApiResult<Value> {
    let s = share_row(st, token)?;
    Ok(match s.kind.as_str() {
        "album" => {
            st.db.one("SELECT 1 FROM tracks WHERE album_id=? LIMIT 1", p![s.r])?.ok_or_else(link_gone)?;
            json!({ "kind": "album", "album_id": s.r })
        }
        "artist" => {
            // the artist index's display name, as the share page shows it; the stored name when the index has none
            let name = st.library.artists.index(&st.db).find(&s.r).map(|a| a.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| s.r.clone());
            json!({ "kind": "artist", "name": name })
        }
        "playlist" => playlist_target(st, uid, token, &s)?,
        _ => {
            let t: Row = st.db.one("SELECT id, album_id FROM tracks WHERE id=?", p![s.track_id])?.ok_or_else(link_gone)?;
            json!({ "kind": "track", "track_id": t.i("id"), "album_id": t.s("album_id") })
        }
    })
}

/// [`target`] of a playlist link: the listener's own playlist, their entry for someone else's
/// ([`super::super::playlist_links::entry_for`]), or neither.
fn playlist_target(st: &AppState, uid: i64, token: &str, s: &Shared) -> ApiResult<Value> {
    // the playlist as it is now: a deleted playlist, a disabled owner or Stop sharing end the link here too
    let pl = shared_playlist(st, s)?;
    if s.user_id == uid {
        return Ok(json!({ "kind": "playlist", "own": true, "added": false, "playlist_id": pl.id, "name": pl.name }));
    }
    let entry = super::super::playlist_links::entry_for(st, uid, token)?;
    Ok(json!({ "kind": "playlist", "own": false, "added": entry.is_some(), "playlist_id": entry, "name": pl.name }))
}

/// The web app's page (its hash route, without the `#`) for a [`target`]: `/library/album/<id>?track=<id>` for a
/// song (the album page picks the song out), `/library/album/<id>`, `/artist-name/<name>`, `/playlists/<id>`.
/// None for a playlist that is neither the listener's own nor added: the share page is where they can add it.
pub(crate) fn web_path(t: &Value) -> Option<String> {
    let album = || t["album_id"].as_str().filter(|a| !a.is_empty()).map(|a| urlencoding::encode(a).into_owned());
    match t["kind"].as_str()? {
        "track" => Some(format!("/library/album/{}?track={}", album()?, t["track_id"].as_i64()?)),
        "album" => Some(format!("/library/album/{}", album()?)),
        "artist" => t["name"].as_str().filter(|n| !n.is_empty()).map(|n| format!("/artist-name/{}", urlencoding::encode(n))),
        "playlist" => t["playlist_id"].as_i64().map(|id| format!("/playlists/{id}")),
        _ => None,
    }
}

/// Whether this user agent is a link-preview bot or crawler (Discord, Slack, X, Facebook, WhatsApp, Telegram,
/// Googlebot's phone crawler …). They must always get the share page with its preview tags, never a jump: some of
/// them say "Android" too.
pub(crate) fn is_preview_bot(ua: &str) -> bool {
    let ua = ua.to_ascii_lowercase();
    ["bot", "crawler", "spider", "preview", "facebookexternalhit", "whatsapp", "telegram", "embedly", "slack", "discord", "skype", "vkshare", "curl", "wget", "python"]
        .iter()
        .any(|w| ua.contains(w))
}

/// Whether this user agent is a browser on an Android phone that can hand an `intent://` link to an app: Android,
/// but not an app's built-in WebView (`; wv)`), which shows an error page for intent links instead.
pub(crate) fn is_android_browser(ua: &str) -> bool {
    ua.contains("Android") && !ua.contains("; wv)") && !is_preview_bot(ua)
}

/// Whether the request is a page opened in a browser tab (not a frame, a preview fetch or a script's request):
/// `Sec-Fetch-Dest: document` when the browser says, and only GET. X's player card loads the page in a frame,
/// which must keep the page.
fn is_top_level_page(method: &axum::http::Method, headers: &HeaderMap) -> bool {
    method == axum::http::Method::GET
        && headers.get("sec-fetch-dest").and_then(|v| v.to_str().ok()).is_none_or(|d| d.eq_ignore_ascii_case("document"))
}

/// Whether the query asks to skip the app ([`NO_APP`]).
fn skips_app(uri: &Uri) -> bool {
    uri.query().is_some_and(|q| q.split('&').any(|kv| kv == NO_APP))
}

/// The share link's address with `?app=0` ([`NO_APP`]): `base` (the server's public address) and the token.
pub(crate) fn no_app_link(base: &str, token: &str) -> String {
    format!("{}/s/{token}?{NO_APP}", base.trim_end_matches('/'))
}

/// The `intent://` link that opens the share link in the Android app: `encore://<host>[/<path>]/s/<token>` for the
/// app, the link itself as the `link` extra (so the app can send it back to the browser, http or https), and
/// [`no_app_link`] as the fallback the browser opens when the app isn't installed. No package is named, so both
/// the release and debug builds of the app answer it.
pub(crate) fn android_intent(base: &str, token: &str) -> String {
    let base = base.trim_end_matches('/');
    let rest = base.split_once("://").map(|(_, r)| r).unwrap_or(base);
    let link = format!("{base}/s/{token}");
    format!(
        "intent://{rest}/s/{token}#Intent;scheme={APP_SCHEME};action=android.intent.action.VIEW;category=android.intent.category.BROWSABLE;S.link={};S.browser_fallback_url={};end",
        urlencoding::encode(&link),
        urlencoding::encode(&no_app_link(base, token)),
    )
}

/// A redirect that no cache keeps or shares: what it says depends on the user agent and the session cookie.
fn jump_to(to: &str) -> Option<Response> {
    let loc = HeaderValue::from_str(to).ok()?;
    Some((
        StatusCode::FOUND,
        [
            (header::LOCATION, loc),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (header::VARY, HeaderValue::from_static("User-Agent, Cookie")),
        ],
    ).into_response())
}

/// What the share page does before it shows itself (see the module's top): the jump to the Android app, the jump
/// to the web app for `me` (the signed-in user, from the session cookie), or None for the share page. `base` is the
/// server's public address the page uses for its own links. A link that doesn't work never jumps: the page says so.
pub(super) async fn jump(st: &AppState, me: Option<&Row>, token: &str, method: &axum::http::Method, uri: &Uri, headers: &HeaderMap, base: &str) -> Option<Response> {
    let ua = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or("");
    if ua.is_empty() || is_preview_bot(ua) || !is_top_level_page(method, headers) {
        return None;
    }
    if !skips_app(uri) && is_android_browser(ua) {
        let tok = token.to_string();
        st.blocking(move |st| share_row(st, &tok).map(|_| ())).await.ok()?;
        return jump_to(&android_intent(base, token));
    }
    let uid = me?.i("id");
    let tok = token.to_string();
    let t = st.blocking(move |st| target(st, uid, &tok)).await.ok()?;
    jump_to(&format!("/#{}", web_path(&t)?))
}
