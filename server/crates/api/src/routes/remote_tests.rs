//! The playback spot end to end through the real router: check-ins write it (throttled), `/api/remote/spot`
//! returns it with its song and whether the device is online, and a remote's check-ins leave it alone.
use encore_db::RowExt;
use encore_db::p;
use crate::state::AppState;
use axum::http::StatusCode;
use axum::Router;
use serde_json::{json, Value};
use crate::test_support::call;

const PW: &str = "listener-pass-1";

struct Env {
    st: AppState,
    app: Router,
    dir: std::path::PathBuf,
    cookie: String,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A fresh server with one user whose id is `uid` (the hubs and the throttle are per user id and shared by
/// every test in the process, so each test gets its own) and three songs, ids 1 to 3.
async fn setup(name: &str, uid: i64) -> Env {
    let dir = std::env::temp_dir().join(format!("encore-spot-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let st = crate::test_support::state_in(&dir);
    let id = crate::auth::create_user(&st, "listener", PW, "user", None, None).unwrap();
    st.db.execute("UPDATE users SET id=? WHERE id=?", p![uid, id]).unwrap();
    for (i, title) in ["First", "Second", "Third"].iter().enumerate() {
        st.db.execute("INSERT INTO tracks (id, path, title, artist, album, album_id, duration) VALUES (?,?,?,?,?,?,?)",
            p![i as i64 + 1, format!("/m/{i}.flac"), title, "Band", "Record", "alb1", 200.0 + i as f64]).unwrap();
    }
    let app = crate::app(st.clone());
    let r = call(&app, "POST", "/api/auth/login", "", Some(json!({ "username": "listener", "password": PW }))).await;
    assert_eq!(r.0, StatusCode::OK, "{:?}", r.1);
    let cookie = r.2.split(';').next().unwrap().to_string();
    Env { st, app, dir, cookie }
}


impl Env {
    async fn beat(&self, device: &str, name: &str, state: Value) {
        let r = call(&self.app, "POST", "/api/remote/heartbeat", &self.cookie, Some(json!({ "id": device, "name": name, "state": state }))).await;
        assert_eq!(r.0, StatusCode::OK, "{:?}", r.1);
    }
    async fn spot(&self) -> Value {
        let r = call(&self.app, "GET", "/api/remote/spot", &self.cookie, None).await;
        assert_eq!(r.0, StatusCode::OK, "{:?}", r.1);
        r.1
    }
    fn row(&self) -> Option<encore_db::Row> {
        self.st.db.one("SELECT * FROM playback_spot", p![]).unwrap()
    }
}

fn playing(id: i64, position: f64) -> Value {
    json!({ "id": id, "title": "x", "playing": true, "position": position, "index": id - 1,
            "queue": [{ "id": 1 }, { "id": 2 }, { "id": 3 }] })
}

#[tokio::test]
async fn heartbeat_writes_the_spot_throttled_and_get_returns_it() {
    let e = setup("write", 9101).await;
    assert_eq!(e.spot().await, Value::Null);
    // nothing playing, or a stream that isn't a library song: nothing written
    e.beat("phone", "Pixel", json!({ "playing": false })).await;
    e.beat("phone", "Pixel", json!({ "title": "radio", "playing": true, "position": 3.0 })).await;
    assert!(e.row().is_none());

    e.beat("phone", "Pixel", playing(1, 5.0)).await;
    let r = e.row().expect("first check-in while playing writes");
    assert_eq!((r.i("track_id"), r.f("position"), r.i("idx"), r.s("queue")), (1, 5.0, 0, "[1,2,3]".to_string()));
    assert_eq!((r.s("device_id"), r.s("device_name"), r.b("playing")), ("phone".into(), "Pixel".into(), true));
    let written = r.f("updated_at");

    // the next check-in a couple of seconds later: throttled
    e.beat("phone", "Pixel", playing(1, 7.0)).await;
    let r = e.row().unwrap();
    assert_eq!((r.f("position"), r.f("updated_at")), (5.0, written));

    // a song change is written at once
    e.beat("phone", "Pixel", playing(2, 0.5)).await;
    let r = e.row().unwrap();
    assert_eq!((r.i("track_id"), r.f("position"), r.i("idx")), (2, 0.5, 1));

    // and so is a pause
    let mut paused = playing(2, 42.0);
    paused["playing"] = json!(false);
    e.beat("phone", "Pixel", paused).await;
    let r = e.row().unwrap();
    assert_eq!((r.f("position"), r.b("playing")), (42.0, false));

    // another app opening paused on an old song doesn't take the spot over
    let mut other = playing(3, 10.0);
    other["playing"] = json!(false);
    e.beat("laptop", "Laptop", other).await;
    assert_eq!(e.row().unwrap().s("device_id"), "phone");

    let s = e.spot().await;
    assert_eq!(s["track_id"], 2);
    assert_eq!(s["position"], 42.0);
    assert_eq!(s["queue"], json!([1, 2, 3]));
    assert_eq!(s["index"], 1);
    assert_eq!(s["device_id"], "phone");
    assert_eq!(s["device_name"], "Pixel");
    assert_eq!(s["playing"], false);
    assert_eq!(s["title"], "Second");
    assert_eq!(s["artist"], "Band");
    assert_eq!(s["album"], "Record");
    assert_eq!(s["album_id"], "alb1");
    assert_eq!(s["duration"], 201.0);
    assert_eq!(s["online"], true);
    assert_eq!(s["device_playing"], false);
    assert!(s["updated_at"].as_f64().unwrap() > 0.0);
    assert!(s.get("idx").is_none() && s.get("user_id").is_none());

    // a device that isn't in the hub (or not seen lately) is offline
    e.st.db.execute("UPDATE playback_spot SET device_id='gone'", p![]).unwrap();
    assert_eq!(e.spot().await["online"], false);

    // a song that left the library: nothing to pick up
    e.st.db.execute("UPDATE playback_spot SET track_id=99", p![]).unwrap();
    assert_eq!(e.spot().await, Value::Null);

    // DELETE clears it, and the next check-in while playing writes again
    let r = call(&e.app, "DELETE", "/api/remote/spot", &e.cookie, None).await;
    assert_eq!(r.0, StatusCode::OK);
    assert!(e.row().is_none());
    e.beat("laptop", "Laptop", playing(3, 1.0)).await;
    let s = e.spot().await;
    assert_eq!((s["track_id"].as_i64(), s["device_name"].as_str(), s["online"].as_bool(), s["device_playing"].as_bool()),
        (Some(3), Some("Laptop"), Some(true), Some(true)));
}

#[tokio::test]
async fn a_remote_check_in_does_not_write_the_spot() {
    let e = setup("remote-of", 9102).await;
    e.beat("phone", "Pixel", json!({ "playing": false, "remote_of": "laptop" })).await;
    // even if a remote sends a song, it's the other device's
    let mut s = playing(1, 3.0);
    s["remote_of"] = json!("laptop");
    e.beat("phone", "Pixel", s).await;
    assert!(e.row().is_none());
    assert_eq!(e.spot().await, Value::Null);
}

#[tokio::test]
async fn spot_needs_sign_in() {
    let e = setup("signin", 9103).await;
    let r = call(&e.app, "GET", "/api/remote/spot", "", None).await;
    assert_eq!(r.0, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn two_devices_playing_at_once_stay_throttled() {
    let e = setup("two-playing", 9104).await;
    e.beat("phone", "Pixel", playing(1, 5.0)).await;
    let first = e.row().unwrap().f("updated_at");
    // the laptop plays too: the phone wrote moments ago and still plays, so the laptop waits its turn
    e.beat("laptop", "Laptop", playing(2, 1.0)).await;
    e.beat("phone", "Pixel", playing(1, 7.0)).await;
    e.beat("laptop", "Laptop", playing(2, 3.0)).await;
    let r = e.row().unwrap();
    assert_eq!((r.s("device_id"), r.i("track_id"), r.f("position"), r.f("updated_at")), ("phone".into(), 1, 5.0, first));
    // once the phone pauses, the laptop takes over at once
    let mut paused = playing(1, 9.0);
    paused["playing"] = json!(false);
    e.beat("phone", "Pixel", paused).await;
    assert_eq!(e.row().unwrap().f("position"), 9.0);
    e.beat("laptop", "Laptop", playing(2, 4.0)).await;
    let r = e.row().unwrap();
    assert_eq!((r.s("device_id"), r.i("track_id"), r.f("position")), ("laptop".into(), 2, 4.0));
}

#[tokio::test]
async fn shuffle_and_repeat_commands_reach_the_device_and_its_state_shows_them() {
    let e = setup("shuffle-repeat", 9105).await;
    let mut s = playing(1, 5.0);
    s["shuffle"] = json!(true);
    s["repeat"] = json!("one");
    e.beat("phone", "Pixel", s).await;
    let d = call(&e.app, "GET", "/api/remote/devices", &e.cookie, None).await.1;
    assert_eq!((d[0]["state"]["shuffle"].clone(), d[0]["state"]["repeat"].clone()), (json!(true), json!("one")), "{d}");
    for c in [json!({ "action": "shuffle", "value": false }), json!({ "action": "repeat", "value": "all" })] {
        let r = call(&e.app, "POST", "/api/remote/phone/command", &e.cookie, Some(c)).await;
        assert_eq!(r.0, StatusCode::OK, "{:?}", r.1);
    }
    let r = call(&e.app, "POST", "/api/remote/heartbeat", &e.cookie, Some(json!({ "id": "phone", "name": "Pixel", "state": playing(1, 6.0) }))).await;
    let cmds = r.1["commands"].as_array().unwrap().clone();
    assert_eq!(cmds.iter().map(|c| (c["action"].clone(), c["value"].clone())).collect::<Vec<_>>(),
        vec![(json!("shuffle"), json!(false)), (json!("repeat"), json!("all"))]);
    let r = call(&e.app, "POST", "/api/remote/phone/command", &e.cookie, Some(json!({ "action": "dance" }))).await;
    assert_eq!(r.0, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_system_volume_command_reaches_the_device_and_its_state_shows_the_level() {
    let e = setup("system-volume", 9107).await;
    let mut s = playing(1, 5.0);
    s["system_volume"] = json!(0.4);
    e.beat("phone", "Pixel", s).await;
    let d = call(&e.app, "GET", "/api/remote/devices", &e.cookie, None).await.1;
    assert_eq!(d[0]["state"]["system_volume"], json!(0.4), "{d}");
    let r = call(&e.app, "POST", "/api/remote/phone/command", &e.cookie, Some(json!({ "action": "system_volume", "value": 0.75 }))).await;
    assert_eq!(r.0, StatusCode::OK, "{:?}", r.1);
    let r = call(&e.app, "POST", "/api/remote/heartbeat", &e.cookie, Some(json!({ "id": "phone", "name": "Pixel", "state": playing(1, 6.0) }))).await;
    let cmds = r.1["commands"].as_array().unwrap().clone();
    assert_eq!(cmds.iter().map(|c| (c["action"].clone(), c["value"].clone())).collect::<Vec<_>>(), vec![(json!("system_volume"), json!(0.75))]);
}

#[tokio::test]
async fn a_prefs_command_reaches_the_device_and_its_state_shows_its_normalization() {
    let e = setup("prefs", 9106).await;
    let mut s = playing(1, 5.0);
    s["norm"] = json!({ "mode": "off", "target": -14, "clip": true });
    e.beat("tab", "Chrome on Linux", s).await;
    let d = call(&e.app, "GET", "/api/remote/devices", &e.cookie, None).await.1;
    assert_eq!(d[0]["state"]["norm"]["mode"], json!("off"), "{d}");
    let c = json!({ "action": "prefs", "norm": { "mode": "album", "target": -12 } });
    let r = call(&e.app, "POST", "/api/remote/tab/command", &e.cookie, Some(c)).await;
    assert_eq!(r.0, StatusCode::OK, "{:?}", r.1);
    let r = call(&e.app, "POST", "/api/remote/heartbeat", &e.cookie, Some(json!({ "id": "tab", "name": "Chrome on Linux", "state": playing(1, 6.0) }))).await;
    let cmds = r.1["commands"].as_array().unwrap().clone();
    assert_eq!(cmds.len(), 1, "{cmds:?}");
    assert_eq!((cmds[0]["action"].clone(), cmds[0]["norm"].clone()), (json!("prefs"), json!({ "mode": "album", "target": -12 })));
}

#[tokio::test]
async fn songs_carry_each_artist_for_links() {
    let e = setup("credits", 9106).await;
    e.st.db.execute("INSERT INTO tracks (id, path, title, artist, albumartist, album, album_id, duration) VALUES (?,?,?,?,?,?,?,?)",
        p![10, "/m/10.flac", "Duet", "Band feat. Guest Singer", "Band", "Record", "alb1", 180.0]).unwrap();
    e.st.library.artists.invalidate();
    let r = call(&e.app, "GET", "/api/library/tracks/10", &e.cookie, None).await;
    assert_eq!(r.0, StatusCode::OK, "{:?}", r.1);
    // the featured singer is an artist of the library too (0.41.1: featured artists have pages of their own)
    assert_eq!(r.1["artists"], json!([{ "name": "Band", "in_library": true }, { "name": "Guest Singer", "in_library": true }]), "{}", r.1);
}
