//! Sign-ins that come back through a browser (Spotify, Tidal, Last.fm, MusicBrainz): the single-use state,
//! its 10 minutes, the account linked to the user who started it and never to the browser's, and the
//! confirmation page against account-linking CSRF. The services are stood in for by `TEST_CODE` codes (no
//! network).
use super::*;
use crate::test_support::{send, user, Resp, TestServer};
use axum::body::Body;
use axum::http::Request;
use serde_json::json;
use sha2::Digest;
use tower::ServiceExt;

const PHONE_UA: &str = "EncoreAndroid/0.31.0 (Android 15; Pixel 9 Pro)";
const FIREFOX_UA: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:140.0) Gecko/20100101 Firefox/140.0";

fn server(name: &str) -> TestServer {
    TestServer::new(&format!("signin-{name}")).with(json!({
        "base_url": "https://encore.example", "spotify_client_id": "spotify-cid", "tidal_client_id": "tidal-cid",
        "lastfm_api_key": "lfm-key", "lastfm_shared_secret": "lfm-secret", "mb_client_id": "mb-cid", "mb_client_secret": "mb-secret",
    }))
}

/// A browser signed in as `u` (Firefox on Linux): its `cookie` header.
fn browser(st: &AppState, u: &Row) -> String {
    let (token, _) = auth::create_session(st, u.i("id"), FIREFOX_UA, "127.0.0.1").unwrap();
    format!("{}={token}", auth::COOKIE)
}

/// The phone app signed in as `u` on a "Pixel 9 Pro": its `authorization` header.
fn phone(st: &AppState, u: &Row) -> String {
    let token = auth::create_app_session(st, u.i("id"), PHONE_UA, "127.0.0.1", &format!("dev-{}", u.i("id")), "Pixel 9 Pro").unwrap();
    format!("Bearer {token}")
}

/// A GET as a browser (`cookie` "" for none).
async fn get(app: &Router, uri: &str, cookie: &str) -> Resp {
    send(app, "GET", uri, cookie, None, &[("user-agent", FIREFOX_UA)]).await
}

/// The sign-in start of the phone app: the service's sign-in address.
async fn start_on_phone(app: &Router, bearer: &str, path: &str) -> String {
    let r = send(app, "POST", path, "", Some(json!({})), &[("authorization", bearer), ("user-agent", PHONE_UA)]).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    r.json()["url"].as_str().unwrap().to_string()
}

/// The `state` in a sign-in address (Last.fm's is inside the callback address `cb`).
fn state_in(url: &str) -> String {
    let decoded = urlencoding::decode(url).unwrap().into_owned();
    let decoded = urlencoding::decode(&decoded).unwrap().into_owned();
    let at = decoded.find("state=").expect("a state in the address") + 6;
    decoded[at..].chars().take_while(|c| c.is_ascii_hexdigit()).collect()
}

/// The confirmation page's secret.
fn confirm_key(page: &str) -> String {
    let at = page.find(r#"name="confirm" value=""#).expect("a confirmation form") + r#"name="confirm" value=""#.len();
    page[at..].chars().take_while(|c| *c != '"').collect()
}

/// The confirmation page's answer, posted as its form does (no CSRF header: a plain HTML form).
async fn answer(app: &Router, key: &str, answer: &str) -> Resp {
    let req = Request::builder().method("POST").uri("/api/sign-in/confirm")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(format!("confirm={}&answer={answer}", urlencoding::encode(key)))).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap().to_vec();
    Resp { status, headers, body }
}

/// `user`'s connection with `service`: its account name, or None.
fn connected(st: &AppState, u: &Row, service: &str) -> Option<String> {
    connections::get(st, u.i("id"), service).map(|c| c.s("account"))
}

fn code(account: &str) -> String {
    urlencoding::encode(&format!("{TEST_CODE}{account}")).into_owned()
}

// ---- the store ----------------------------------------------------------------------------------------

#[test]
fn a_state_is_single_use_and_lasts_ten_minutes() {
    let s = SignIns::default();
    let a = s.start(7, "spotify", "Pixel 9 Pro", "v");
    assert_eq!(a.len(), 64, "32 random bytes as hex");
    assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    let p = s.take_state(&a).expect("the state");
    assert_eq!((p.user_id, p.service.as_str(), p.device.as_str(), p.verifier.as_str()), (7, "spotify", "Pixel 9 Pro", "v"));
    assert!(s.take_state(&a).is_none(), "deleted on first use");

    let b = s.start(7, "tidal", "", "");
    s.age(&b, STATE_SECS - 30);
    assert!(s.take_state(&b).is_some(), "still good just under 10 minutes");
    let c = s.start(7, "tidal", "", "");
    s.age(&c, STATE_SECS + 1);
    assert!(s.take_state(&c).is_none(), "gone after 10 minutes");
    assert!(s.take_state("").is_none());
    assert_ne!(s.start(7, "tidal", "", ""), s.start(7, "tidal", "", ""), "random");
}

#[test]
fn states_and_confirmation_secrets_do_not_stand_in_for_each_other() {
    let s = SignIns::default();
    let state = s.start(3, "lastfm", "", "");
    let p = s.take_state(&state).unwrap();
    let key = s.hold(p, Linked { account: "sam".into(), data: json!({}) });
    assert!(s.take_state(&key).is_none(), "a confirmation secret is no state");
    let state2 = s.start(3, "lastfm", "", "");
    assert!(s.take_confirm(&state2).is_none(), "a state confirms nothing");
    assert!(s.take_confirm("").is_none());
    let p = s.take_confirm(&key).expect("the held sign-in");
    assert_eq!(p.linked.unwrap().account, "sam");
    assert!(s.take_confirm(&key).is_none(), "single use");
    // the confirmation page is good for what is left of the 10 minutes
    let state3 = s.start(3, "lastfm", "", "");
    let key = s.hold(s.take_state(&state3).unwrap(), Linked { account: "sam".into(), data: json!({}) });
    s.age(&key, STATE_SECS + 1);
    assert!(s.take_confirm(&key).is_none());
}

#[test]
fn one_user_keeps_at_most_ten_sign_ins() {
    let s = SignIns::default();
    let first = s.start(5, "spotify", "", "");
    s.age(&first, 5);
    let rest: Vec<String> = (0..PER_USER).map(|_| s.start(5, "spotify", "", "")).collect();
    assert_eq!(s.count_for(5), PER_USER);
    assert!(s.take_state(&first).is_none(), "the oldest went");
    assert!(rest.iter().all(|k| s.take_state(k).is_some()));
    let other = s.start(6, "spotify", "", "");
    assert!(s.take_state(&other).is_some(), "another user's are their own");
}

// ---- Spotify / Tidal ------------------------------------------------------------------------------

#[tokio::test]
async fn the_phone_app_signs_in_without_a_browser_session() {
    let ts = server("phone");
    let (st, app) = (ts.st.clone(), ts.app());
    let sam = user(&st, "sam", "user");
    let url = start_on_phone(&app, &phone(&st, &sam), "/api/connections/spotify").await;
    assert!(url.starts_with("https://accounts.spotify.com/authorize?"), "{url}");
    let state = state_in(&url);
    assert_eq!(state.len(), 64);
    let p = store(&st).peek(&state).expect("kept");
    assert_eq!((p.user_id, p.service.as_str(), p.device.as_str()), (sam.i("id"), "spotify", "Encore app on Android"), "the user agent's words, never the device name the app sent");
    // the PKCE challenge in the address is the verifier the state keeps
    let challenge = base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, sha2::Sha256::digest(p.verifier.as_bytes()));
    assert!(url.contains(&format!("code_challenge={challenge}")), "{url}");

    // the phone's browser is signed in to nobody: asked first, nothing linked yet
    let r = get(&app, &format!("/api/connections/spotify/callback?code={}&state={state}", code("alex-sp")), "").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let page = r.text();
    assert!(page.contains("Link the Spotify account “alex-sp” to the Encore user “sam”?"), "{page}");
    assert!(page.contains("started this sign-in on “Encore app on Android”."), "{page}");
    assert!(!page.contains("Pixel 9 Pro"), "{page}");
    assert!(!page.contains("This browser is signed in"), "{page}");
    assert_eq!(connected(&st, &sam, "spotify"), None);
    // the state went with the first use
    let again = get(&app, &format!("/api/connections/spotify/callback?code={}&state={state}", code("alex-sp")), "").await;
    assert_eq!(again.status, StatusCode::BAD_REQUEST);
    assert!(again.text().contains(EXPIRED));

    let key = confirm_key(&page);
    let r = answer(&app, &key, "link").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert!(r.text().contains("The Spotify account “alex-sp” is now linked to the Encore user “sam”."), "{}", r.text());
    assert_eq!(connected(&st, &sam, "spotify").as_deref(), Some("alex-sp"));
    // the page's secret is single use too
    let r = answer(&app, &key, "link").await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    ts.remove();
}

#[tokio::test]
async fn the_same_users_browser_is_linked_at_once() {
    let ts = server("same");
    let (st, app) = (ts.st.clone(), ts.app());
    let sam = user(&st, "sam", "user");
    let cookie = browser(&st, &sam);
    let r = send(&app, "POST", "/api/connections/tidal", &cookie, Some(json!({})), &[("user-agent", FIREFOX_UA)]).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let state = state_in(r.json()["url"].as_str().unwrap());
    assert_eq!(store(&st).peek(&state).unwrap().device, "Firefox on Linux");
    let r = get(&app, &format!("/api/connections/tidal/callback?code={}&state={state}", code("sam-tidal")), &cookie).await;
    assert!(r.status.is_redirection(), "{}", r.status);
    assert_eq!(r.h("location"), "/#/connections?connected=tidal");
    assert_eq!(connected(&st, &sam, "tidal").as_deref(), Some("sam-tidal"));
    // used: the web app says so
    let r = get(&app, &format!("/api/connections/tidal/callback?code={}&state={state}", code("sam-tidal")), &cookie).await;
    assert!(r.h("location").starts_with("/#/connections?error="), "{}", r.h("location"));
    ts.remove();
}

/// Account-linking CSRF: mallory starts a sign-in on her own Encore account and gets bob to open Spotify's
/// sign-in page. Bob's browser comes back with mallory's state: it is asked first, naming mallory, and Cancel
/// links nothing to anyone.
#[tokio::test]
async fn another_users_browser_is_asked_first_and_cancel_links_nothing() {
    let ts = server("csrf");
    let (st, app) = (ts.st.clone(), ts.app());
    let mallory = user(&st, "mallory", "user");
    let bob = user(&st, "bob", "user");
    let url = start_on_phone(&app, &phone(&st, &mallory), "/api/connections/spotify").await;
    let state = state_in(&url);
    let bobs = browser(&st, &bob);
    let r = get(&app, &format!("/api/connections/spotify/callback?code={}&state={state}", code("bob-on-spotify")), &bobs).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let page = r.text();
    assert!(page.contains("Link the Spotify account “bob-on-spotify” to the Encore user “mallory”?"), "{page}");
    assert!(page.contains("This browser is signed in to Encore as “bob”, not as “mallory”."), "{page}");
    assert!(page.contains("If someone else sent you the sign-in link, choose Cancel."), "{page}");
    assert_eq!(connected(&st, &mallory, "spotify"), None);
    assert_eq!(connected(&st, &bob, "spotify"), None, "never the browser's user");
    // the page cannot be framed, kept or passed on as a referrer
    assert_eq!(r.h("x-frame-options"), "DENY");
    assert!(r.h("content-security-policy").contains("frame-ancestors 'none'"));
    assert_eq!(r.h("cache-control"), "no-store");
    assert_eq!(r.h("referrer-policy"), "no-referrer");

    let key = confirm_key(&page);
    let r = answer(&app, &key, "cancel").await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.text().contains("Nothing was linked"));
    assert_eq!(connected(&st, &mallory, "spotify"), None);
    assert_eq!(connected(&st, &bob, "spotify"), None);
    // and the secret is gone: no Link after a Cancel
    assert_eq!(answer(&app, &key, "link").await.status, StatusCode::BAD_REQUEST);
    assert_eq!(connected(&st, &mallory, "spotify"), None);
    ts.remove();
}

#[tokio::test]
async fn link_from_another_users_browser_goes_to_the_user_who_started_it() {
    let ts = server("other");
    let (st, app) = (ts.st.clone(), ts.app());
    let sam = user(&st, "sam", "user");
    let kim = user(&st, "kim", "user");
    // sam starts on the phone; the phone's browser is still signed in as kim
    let state = state_in(&start_on_phone(&app, &phone(&st, &sam), "/api/connections/spotify").await);
    let r = get(&app, &format!("/api/connections/spotify/callback?code={}&state={state}", code("sam-sp")), &browser(&st, &kim)).await;
    let key = confirm_key(&r.text());
    assert_eq!(answer(&app, &key, "link").await.status, StatusCode::OK);
    assert_eq!(connected(&st, &sam, "spotify").as_deref(), Some("sam-sp"));
    assert_eq!(connected(&st, &kim, "spotify"), None);
    ts.remove();
}

#[tokio::test]
async fn unknown_forged_expired_and_wrong_service_states_link_nothing() {
    let ts = server("forged");
    let (st, app) = (ts.st.clone(), ts.app());
    let sam = user(&st, "sam", "user");
    let bearer = phone(&st, &sam);
    // no state, an unknown one
    for q in [format!("code={}", code("x")), format!("code={}&state={}", code("x"), new_state())] {
        let r = get(&app, &format!("/api/connections/spotify/callback?{q}"), "").await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST);
        assert!(r.text().contains(EXPIRED), "{}", r.text());
    }
    // expired
    let state = state_in(&start_on_phone(&app, &bearer, "/api/connections/spotify").await);
    store(&st).age(&state, STATE_SECS + 1);
    let r = get(&app, &format!("/api/connections/spotify/callback?code={}&state={state}", code("x")), "").await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    // Tidal's state at Spotify's callback: refused, and used up
    let state = state_in(&start_on_phone(&app, &bearer, "/api/connections/tidal").await);
    let r = get(&app, &format!("/api/connections/spotify/callback?code={}&state={state}", code("x")), "").await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = get(&app, &format!("/api/connections/tidal/callback?code={}&state={state}", code("x")), "").await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    // a made-up confirmation secret, and a state posted as one
    let state = state_in(&start_on_phone(&app, &bearer, "/api/connections/spotify").await);
    assert_eq!(answer(&app, &new_state(), "link").await.status, StatusCode::BAD_REQUEST);
    assert_eq!(answer(&app, &state, "link").await.status, StatusCode::BAD_REQUEST);
    // a confirmation page left open past the 10 minutes
    let r = get(&app, &format!("/api/connections/spotify/callback?code={}&state={state}", code("late")), "").await;
    let key = confirm_key(&r.text());
    store(&st).age(&key, STATE_SECS + 1);
    assert_eq!(answer(&app, &key, "link").await.status, StatusCode::BAD_REQUEST);
    assert_eq!(connected(&st, &sam, "spotify"), None);
    assert_eq!(connected(&st, &sam, "tidal"), None);
    ts.remove();
}

#[tokio::test]
async fn a_refused_sign_in_uses_up_its_state() {
    let ts = server("denied");
    let (st, app) = (ts.st.clone(), ts.app());
    let sam = user(&st, "sam", "user");
    let state = state_in(&start_on_phone(&app, &phone(&st, &sam), "/api/connections/spotify").await);
    let r = get(&app, &format!("/api/connections/spotify/callback?error=access_denied&state={state}"), "").await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.text().contains("You did not allow Encore"), "{}", r.text());
    let r = get(&app, &format!("/api/connections/spotify/callback?code={}&state={state}", code("x")), "").await;
    assert!(r.text().contains(EXPIRED));
    assert_eq!(connected(&st, &sam, "spotify"), None);
    ts.remove();
}

#[tokio::test]
async fn a_start_that_fails_keeps_no_state() {
    let ts = server("nostate");
    let (st, app) = (ts.st.clone(), ts.app());
    st.cfg.set_many(&json!({ "spotify_client_id": "" }));
    let sam = user(&st, "sam", "user");
    let bearer = phone(&st, &sam);
    let r = send(&app, "POST", "/api/connections/spotify", "", Some(json!({})), &[("authorization", bearer.as_str())]).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.text().contains("Client ID"), "{}", r.text());
    assert_eq!(store(&st).count_for(sam.i("id")), 0);
    ts.remove();
}

#[tokio::test]
async fn a_disabled_user_gets_nothing_linked() {
    let ts = server("disabled");
    let (st, app) = (ts.st.clone(), ts.app());
    let sam = user(&st, "sam", "user");
    let state = state_in(&start_on_phone(&app, &phone(&st, &sam), "/api/connections/spotify").await);
    let r = get(&app, &format!("/api/connections/spotify/callback?code={}&state={state}", code("sam-sp")), "").await;
    let key = confirm_key(&r.text());
    st.db.execute("UPDATE users SET disabled=1 WHERE id=?", p![sam.i("id")]).unwrap();
    let r = answer(&app, &key, "link").await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(connected(&st, &sam, "spotify"), None);
    ts.remove();
}

// ---- Last.fm ------------------------------------------------------------------------------------------

/// Last.fm used to link to whichever Encore user the browser was signed in as; now to the state's user.
#[tokio::test]
async fn lastfm_links_to_the_user_who_started_it_not_the_browsers() {
    let ts = server("lastfm");
    let (st, app) = (ts.st.clone(), ts.app());
    let sam = user(&st, "sam", "user");
    let kim = user(&st, "kim", "user");
    let url = start_on_phone(&app, &phone(&st, &sam), "/api/connections/lastfm").await;
    assert!(url.starts_with("https://www.last.fm/api/auth/?api_key=lfm-key&cb="), "{url}");
    let cb = urlencoding::decode(url.split("cb=").nth(1).unwrap()).unwrap().into_owned();
    let state = state_in(&url);
    assert_eq!(cb, format!("https://encore.example/api/connections/lastfm/callback?state={state}"));
    let kims = browser(&st, &kim);

    // the old way, a token and no state, links nothing to the browser's user
    let r = get(&app, &format!("/api/connections/lastfm/callback?token={}", code("kim-lfm")), &kims).await;
    assert!(r.h("location").starts_with("/#/connections?error="), "{}", r.h("location"));
    assert_eq!(connected(&st, &kim, "lastfm"), None);

    // Last.fm adds &token= to the callback address
    let r = get(&app, &format!("/api/connections/lastfm/callback?state={state}&token={}", code("sam-lfm")), &kims).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert!(r.text().contains("Link the Last.fm account “sam-lfm” to the Encore user “sam”?"), "{}", r.text());
    assert_eq!(answer(&app, &confirm_key(&r.text()), "link").await.status, StatusCode::OK);
    assert_eq!(connected(&st, &sam, "lastfm").as_deref(), Some("sam-lfm"));
    assert_eq!(connected(&st, &kim, "lastfm"), None);
    ts.remove();
}

#[tokio::test]
async fn lastfm_in_the_same_users_browser_is_linked_at_once() {
    let ts = server("lastfm-same");
    let (st, app) = (ts.st.clone(), ts.app());
    let sam = user(&st, "sam", "user");
    let cookie = browser(&st, &sam);
    let r = send(&app, "POST", "/api/connections/lastfm", &cookie, Some(json!({})), &[]).await;
    let state = state_in(r.json()["url"].as_str().unwrap());
    let r = get(&app, &format!("/api/connections/lastfm/callback?state={state}&token={}", code("sam-lfm")), &cookie).await;
    assert_eq!(r.h("location"), "/#/connections?connected=lastfm");
    assert_eq!(connected(&st, &sam, "lastfm").as_deref(), Some("sam-lfm"));
    ts.remove();
}

// ---- MusicBrainz ----------------------------------------------------------------------------------

#[tokio::test]
async fn musicbrainz_links_for_the_admin_who_started_it() {
    let ts = server("mb");
    let (st, app) = (ts.st.clone(), ts.app());
    let ann = user(&st, "ann", "admin");
    let joe = user(&st, "joe", "user");
    // only admins start it
    let joes = phone(&st, &joe);
    let r = send(&app, "POST", "/api/contribute/mb/connect", "", None, &[("authorization", joes.as_str())]).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let url = start_on_phone(&app, &phone(&st, &ann), "/api/contribute/mb/connect").await;
    assert!(url.starts_with("https://musicbrainz.org/oauth2/authorize?"), "{url}");
    let state = state_in(&url);

    // joe's browser comes back with ann's state: asked, and linked for the server only on Link
    let r = get(&app, &format!("/api/contribute/mb/callback?code={}&state={state}", code("ann-mb")), &browser(&st, &joe)).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert!(r.text().contains("Link the MusicBrainz account “ann-mb” to this Encore server?"), "{}", r.text());
    assert!(r.text().contains("The Encore user “ann” started this sign-in on “Encore app on Android”."), "{}", r.text());
    assert_eq!(st.cfg.str("mb_oauth_account"), "");
    let r = answer(&app, &confirm_key(&r.text()), "link").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert!(r.text().contains("is now linked to this Encore server"), "{}", r.text());
    assert_eq!(st.cfg.str("mb_oauth_account"), "ann-mb");
    assert_eq!(st.cfg.str("mb_oauth_refresh_token"), "test-refresh");
    ts.remove();
}

#[tokio::test]
async fn musicbrainz_needs_the_starter_to_still_be_an_admin() {
    let ts = server("mb-demoted");
    let (st, app) = (ts.st.clone(), ts.app());
    let ann = user(&st, "ann", "admin");
    let state = state_in(&start_on_phone(&app, &phone(&st, &ann), "/api/contribute/mb/connect").await);
    let r = get(&app, &format!("/api/contribute/mb/callback?code={}&state={state}", code("ann-mb")), "").await;
    let key = confirm_key(&r.text());
    st.db.execute("UPDATE users SET role='user' WHERE id=?", p![ann.i("id")]).unwrap();
    let r = answer(&app, &key, "link").await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.text().contains("Only an admin can connect"), "{}", r.text());
    assert_eq!(st.cfg.str("mb_oauth_account"), "");
    ts.remove();
}

#[tokio::test]
async fn musicbrainz_in_the_admins_own_browser_goes_back_to_contribute() {
    let ts = server("mb-same");
    let (st, app) = (ts.st.clone(), ts.app());
    let ann = user(&st, "ann", "admin");
    let cookie = browser(&st, &ann);
    let r = send(&app, "POST", "/api/contribute/mb/connect", &cookie, None, &[]).await;
    let state = state_in(r.json()["url"].as_str().unwrap());
    let r = get(&app, &format!("/api/contribute/mb/callback?code={}&state={state}", code("ann-mb")), &cookie).await;
    assert_eq!(r.h("location"), "/#/settings?s=contribute&mb=connected");
    assert_eq!(st.cfg.str("mb_oauth_account"), "ann-mb");
    ts.remove();
}
