//! Group listens through the real router (client-shapes §2.2, §2.3, §3.6–3.19): starting and joining by code,
//! the state's shape, who may do what, the host handing over, ending, the `rev` order on the event stream, the
//! anchor, the caps (the server's, the group's, guests counted), keeping groups through a restart (paused),
//! the list and its visibility (a link-only group answers 404 without leaking), the admin switch, guests (both
//! switches, the token's scope: its group's state, stream and queued songs only, 401 everywhere else; removal,
//! expiry), the 2-minute drop, idle groups, and the rate limits.
use super::*;
use crate::test_support::{send, Resp, TestServer};
use axum::Router;
use futures_util::StreamExt;
use serde_json::json;

struct Env {
    s: TestServer,
    app: Router,
    alex: String,
    sam: String,
    pat: String,
    boss: String,
    ids: Ids,
}

#[derive(Clone, Copy)]
struct Ids {
    alex: i64,
    sam: i64,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.s.dir);
    }
}

fn session(st: &AppState, uid: i64) -> String {
    let (t, _) = crate::auth::create_session(st, uid, "test", "1.2.3.4").unwrap();
    format!("encore_session={t}")
}

/// An account without a password (the tests sign in with sessions made here; bcrypt would cost a second each).
fn account(st: &AppState, name: &str, role: &str) -> Row {
    let id = st.db.execute("INSERT INTO users (username, password_hash, role, created_at) VALUES (?, '-', ?, ?)", p![name, role, now()]).unwrap();
    st.db.one("SELECT * FROM users WHERE id=?", p![id]).unwrap().unwrap()
}

/// A server with songs 1–5 (files on disk; 200, 180, 240, 100 and 90 s; albums al1 and al2) and the accounts
/// alex, sam, pat (users) and boss (admin).
fn setup(name: &str) -> Env {
    let s = TestServer::new(&format!("group-{name}"));
    let music = s.dir.join("music");
    std::fs::create_dir_all(&music).unwrap();
    for (id, dur, album) in [(1, 200.0, "al1"), (2, 180.0, "al1"), (3, 240.0, "al1"), (4, 100.0, "al2"), (5, 90.0, "al2")] {
        let path = music.join(format!("{id}.mp3"));
        std::fs::write(&path, format!("ID3 song {id}")).unwrap();
        s.st.db.execute("INSERT INTO tracks (id, path, title, artist, album, albumartist, album_id, track, disc, year, duration, format, bitrate) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
            p![id, path.to_string_lossy(), format!("Song {id}"), "Band", if album == "al1" { "Record" } else { "Other" }, "Band", album, id, 1, "2006", dur, "mp3", 320]).unwrap();
    }
    let st = s.st.clone();
    let (a, b, c, d) = (account(&st, "alex", "user"), account(&st, "sam", "user"), account(&st, "pat", "user"), account(&st, "boss", "admin"));
    let ids = Ids { alex: a.i("id"), sam: b.i("id") };
    Env { app: s.app(), alex: session(&st, a.i("id")), sam: session(&st, b.i("id")), pat: session(&st, c.i("id")), boss: session(&st, d.i("id")), ids, s }
}

impl Env {
    fn st(&self) -> &AppState {
        &self.s.st
    }
    async fn call(&self, method: &str, uri: &str, cookie: &str, body: Option<Value>) -> Resp {
        send(&self.app, method, uri, cookie, body, &[]).await
    }
    async fn ok(&self, method: &str, uri: &str, cookie: &str, body: Option<Value>) -> Value {
        let r = self.call(method, uri, cookie, body).await;
        assert!(r.status.is_success(), "{method} {uri}: {} {}", r.status, r.text());
        r.json()
    }
    /// alex starts a group on "pc" with songs 1–3, playing song 1 from 0.
    async fn start(&self, extra: Value) -> Value {
        let mut b = json!({ "device_id": "pc", "device_name": "Firefox on Linux", "tracks": [1, 2, 3], "index": 0, "position": 0.0, "playing": true });
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        self.ok("POST", "/api/group", &self.alex, Some(b)).await
    }
    async fn join(&self, cookie: &str, device: &str, code: &str) -> Resp {
        self.call("POST", "/api/group/join", cookie, Some(json!({ "device_id": device, "device_name": format!("{device} app"), "code": code }))).await
    }
    async fn command(&self, cookie: &str, device: &str, id: &str, op: &str, extra: Value) -> Resp {
        let mut b = json!({ "device_id": device, "device_name": "x", "op": op });
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        self.call("POST", &format!("/api/group/{id}/command"), cookie, Some(b)).await
    }
    /// A raw request with the given headers only (no cookie, no CSRF header unless given).
    async fn raw(&self, method: &str, uri: &str, headers: &[(&str, &str)], body: Option<Value>) -> Resp {
        let mut b = axum::http::Request::builder().method(method).uri(uri);
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        let body = match body {
            Some(v) => {
                b = b.header("content-type", "application/json");
                axum::body::Body::from(v.to_string())
            }
            None => axum::body::Body::empty(),
        };
        let resp = tower::ServiceExt::oneshot(self.app.clone(), b.body(body).unwrap()).await.unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let body = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap().to_vec();
        Resp { status, headers, body }
    }
    /// Opens an event stream.
    async fn events(&self, uri: &str, headers: &[(&str, &str)]) -> (StatusCode, Option<Events>) {
        let mut b = axum::http::Request::builder().method("GET").uri(uri).header("accept", "text/event-stream");
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        let resp = tower::ServiceExt::oneshot(self.app.clone(), b.body(axum::body::Body::empty()).unwrap()).await.unwrap();
        let status = resp.status();
        if !status.is_success() {
            return (status, None);
        }
        (status, Some(Events { s: resp.into_body().into_data_stream(), buf: String::new() }))
    }
}

struct Events {
    s: axum::body::BodyDataStream,
    buf: String,
}

impl Events {
    /// The next event (name, data); None when the stream closed (or nothing came for 5 s).
    async fn next(&mut self) -> Option<(String, Value)> {
        loop {
            if let Some(i) = self.buf.find("\n\n") {
                let block: String = self.buf.drain(..i + 2).collect();
                let (mut ev, mut data) = (String::new(), String::new());
                for line in block.lines() {
                    if let Some(x) = line.strip_prefix("event:") {
                        ev = x.trim().to_string();
                    } else if let Some(x) = line.strip_prefix("data:") {
                        data.push_str(x.strip_prefix(' ').unwrap_or(x));
                    }
                }
                if data.is_empty() {
                    continue;
                }
                return Some((ev, serde_json::from_str(&data).unwrap()));
            }
            let chunk = tokio::time::timeout(Duration::from_secs(5), self.s.next()).await.ok()??.ok()?;
            self.buf.push_str(&String::from_utf8_lossy(&chunk));
        }
    }
}

fn code_of(v: &Value) -> String {
    v["code"].as_str().unwrap().to_string()
}

fn id_of(v: &Value) -> String {
    v["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn start_and_join_by_code_answer_the_state() {
    let e = setup("start");
    let before = now_ms();
    let g = e.start(json!({ "tracks": [1, 2, 99, 3], "index": 1, "position": 12.5 })).await;
    let after = now_ms();
    let id = id_of(&g);
    assert!(id.starts_with("g_") && id.len() > 10, "{id}");
    let code = code_of(&g);
    assert_eq!(code.len(), 6);
    assert!(code.bytes().all(|b| gl::CODE_CHARS.contains(&b)), "{code}");
    let time = e.ok("GET", "/api/time", "", None).await;
    assert_eq!(g["boot"], time["boot"]);
    assert!(time["now_ms"].as_i64().unwrap() >= after);
    assert_eq!(g["host"], json!({ "user": e.ids.alex, "name": "alex", "avatar": "" }));
    assert_eq!((g["everyone_controls"].clone(), g["visibility"].clone(), g["guests_allowed"].clone(), g["guests_possible"].clone()), (json!(false), json!("anyone"), json!(false), json!(false)));
    assert_eq!((g["max_members"].clone(), g["server_max_members"].clone()), (json!(20), json!(20)));
    // 99 is not in the library: left out, and the index still names song 2
    let q = g["queue"].as_array().unwrap();
    assert_eq!(q.iter().map(|t| t["id"].as_i64().unwrap()).collect::<Vec<_>>(), vec![1, 2, 3]);
    assert_eq!((g["index"].clone(), g["track"]["id"].clone()), (json!(1), json!(2)));
    assert_eq!(g["track"], q[1], "track is queue[index]");
    assert_eq!(q[0]["added_by"], json!({ "user": e.ids.alex, "name": "alex", "guest": false }));
    assert!(q.iter().all(|t| t["qid"].as_str().is_some_and(|s| !s.is_empty()) && t["format"] == "mp3" && t["duration"].is_number()), "{q:?}");
    assert!(q[0].get("cover_v").is_some() || q[0].get("artists").is_some(), "the usual enrichment: {:?}", q[0]);
    assert_eq!((g["playing"].clone(), g["position"].clone()), (json!(true), json!(12.5)));
    let anchor = g["anchor_ms"].as_i64().unwrap();
    assert!(anchor >= before && anchor <= after);
    let m = &g["members"][0];
    assert_eq!((m["user"].clone(), m["name"].clone(), m["device_name"].clone(), m["host"].clone(), m["guest"].clone()), (json!(e.ids.alex), json!("alex"), json!("Firefox on Linux"), json!(true), json!(false)));
    assert_eq!(g["you"], json!({ "member": m["member"], "host": true, "can_control": true }));
    assert!(g["started_at"].as_i64().unwrap() > 1_700_000_000);
    // no host header: no invite link; the request's host, or the base_url setting, makes one
    assert_eq!(g["invite_url"], "");
    let r = send(&e.app, "GET", "/api/group/current?device_id=pc", &e.alex, None, &[("host", "music.local:8080")]).await;
    assert_eq!(r.json()["invite_url"], format!("http://music.local:8080/#/group/{code}"));
    e.st().cfg.set_many(&json!({ "base_url": "https://music.example.com/" }));
    let r = e.ok("GET", "/api/group/current?device_id=pc", &e.alex, None).await;
    assert_eq!(r["invite_url"], format!("https://music.example.com/#/group/{code}"));

    // sam joins with the code typed in lower case with a space
    let typed = format!("{} {}", code[..3].to_lowercase(), code[3..].to_lowercase());
    let r = e.join(&e.sam, "phone", &typed).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let s = r.json();
    assert_eq!(s["id"], json!(id));
    assert_eq!((s["you"]["host"].clone(), s["you"]["can_control"].clone()), (json!(false), json!(false)));
    assert_eq!(s["members"].as_array().unwrap().len(), 2);
    assert_eq!(s["members"][1]["device_name"], "phone app");
    assert!(s["rev"].as_u64() > g["rev"].as_u64());
    // again from the same device: the same membership, nothing changes
    let again = e.join(&e.sam, "phone", &code).await.json();
    assert_eq!((again["you"]["member"].clone(), again["rev"].clone()), (s["you"]["member"].clone(), s["rev"].clone()));
    // the same account on another device is another member
    let tablet = e.join(&e.sam, "tablet", &code).await.json();
    assert_eq!(tablet["members"].as_array().unwrap().len(), 3);
    assert_ne!(tablet["you"]["member"], s["you"]["member"]);
    // this device's group, or 204
    let cur = e.ok("GET", "/api/group/current?device_id=phone", &e.sam, None).await;
    assert_eq!((cur["id"].clone(), cur["you"]["member"].clone()), (json!(id), s["you"]["member"].clone()));
    assert_eq!(e.call("GET", "/api/group/current?device_id=desk", &e.sam, None).await.status, StatusCode::NO_CONTENT);
    // an unknown code
    let r = e.join(&e.pat, "web", "ZZZZZZ").await;
    assert_eq!((r.status, r.json()["detail"].clone()), (StatusCode::NOT_FOUND, json!("No group listen with that code")));
    // the lookup before "Join alex's group listen?"
    let l = e.ok("GET", &format!("/api/group/code/{}", code.to_lowercase()), &e.pat, None).await;
    assert_eq!((l["id"].clone(), l["host"]["name"].clone(), l["members_count"].clone(), l["mine"].clone(), l["guests"].clone()), (json!(id), json!("alex"), json!(3), json!(false), json!(false)));
    assert_eq!(l["track"]["title"], "Song 2");
    let r = e.call("GET", "/api/group/code/ZZZZZZ", &e.pat, None).await;
    assert_eq!((r.status, r.json()["detail"].clone()), (StatusCode::NOT_FOUND, json!("No group listen with that code")));
    // signed out: 401 everywhere but the lookup (which needs guests on) and the clock
    assert_eq!(e.call("GET", "/api/group", "", None).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(e.call("GET", &format!("/api/group/code/{code}"), "", None).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn who_may_do_what_and_every_command_bumps_rev_by_one() {
    let e = setup("perms");
    let g = e.start(json!({})).await;
    let (id, code) = (id_of(&g), code_of(&g));
    e.join(&e.sam, "phone", &code).await;
    // a member who may not control
    let r = e.command(&e.sam, "phone", &id, "pause", json!({})).await;
    assert_eq!((r.status, r.json()["detail"].clone()), (StatusCode::FORBIDDEN, json!("Only alex controls playback")));
    // every member adds songs
    let r = e.command(&e.sam, "phone", &id, "enqueue", json!({ "tracks": [4] })).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let s = r.json();
    let last = s["queue"].as_array().unwrap().last().unwrap().clone();
    assert_eq!((last["id"].clone(), last["added_by"].clone()), (json!(4), json!({ "user": e.ids.sam, "name": "sam", "guest": false })));
    let r = e.command(&e.sam, "phone", &id, "play_next", json!({ "tracks": [5] })).await.json();
    assert_eq!(r["queue"][1]["id"], 5);
    assert_eq!(r["rev"].as_u64().unwrap(), s["rev"].as_u64().unwrap() + 1);
    // the host's settings and ending are the host's
    let r = e.call("PATCH", &format!("/api/group/{id}"), &e.sam, Some(json!({ "device_id": "phone", "everyone_controls": true }))).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = e.call("POST", &format!("/api/group/{id}/end"), &e.sam, Some(json!({ "device_id": "phone" }))).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // not in it: 404, whatever it asks
    let r = e.command(&e.pat, "web", &id, "enqueue", json!({ "tracks": [1] })).await;
    assert_eq!((r.status, r.json()["detail"].clone()), (StatusCode::NOT_FOUND, json!("You aren't in this group listen")));
    assert_eq!(e.command(&e.sam, "tablet", &id, "enqueue", json!({ "tracks": [1] })).await.status, StatusCode::NOT_FOUND, "the account on a device that is not in it");
    // Everyone can control
    let p = e.ok("PATCH", &format!("/api/group/{id}"), &e.alex, Some(json!({ "device_id": "pc", "everyone_controls": true }))).await;
    assert_eq!(p["everyone_controls"], true);
    let r = e.command(&e.sam, "phone", &id, "pause", json!({})).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!((r.json()["playing"].clone(), r.json()["you"]["can_control"].clone()), (json!(false), json!(true)));
    // the ops, each one rev more than the last
    let mut rev = r.json()["rev"].as_u64().unwrap();
    let first_qid = r.json()["queue"][0]["qid"].as_str().unwrap().to_string();
    for (op, extra) in [("play", json!({})), ("seek", json!({ "position": 30.0 })), ("next", json!({})), ("prev", json!({})),
                        ("jump", json!({ "index": 2 })), ("move", json!({ "from": 0, "to": 3 })), ("remove", json!({ "qid": first_qid })),
                        ("set_queue", json!({ "tracks": [3, 4], "index": 1, "position": 5.0 }))] {
        let r = e.command(&e.alex, "pc", &id, op, extra).await;
        assert_eq!(r.status, StatusCode::OK, "{op}: {}", r.text());
        let now_rev = r.json()["rev"].as_u64().unwrap();
        assert_eq!(now_rev, rev + 1, "{op}");
        rev = now_rev;
    }
    let s = e.ok("GET", "/api/group/current?device_id=pc", &e.alex, None).await;
    assert_eq!(s["queue"].as_array().unwrap().iter().map(|t| t["id"].as_i64().unwrap()).collect::<Vec<_>>(), vec![3, 4]);
    assert_eq!((s["index"].clone(), s["playing"].clone()), (json!(1), json!(true)));
    // mistakes
    let r = e.command(&e.alex, "pc", &id, "jump", json!({ "qid": "q999", "index": 0 })).await;
    assert_eq!((r.status, r.json()["detail"].clone()), (StatusCode::CONFLICT, json!("That song is no longer in the queue")));
    assert_eq!(e.command(&e.alex, "pc", &id, "shuffle", json!({})).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(e.command(&e.alex, "pc", &id, "seek", json!({})).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(e.ok("GET", "/api/group/current?device_id=pc", &e.alex, None).await["rev"].as_u64().unwrap(), rev, "a refused command changes nothing");
    // the host ends it
    let r = e.call("POST", &format!("/api/group/{id}/end"), &e.alex, Some(json!({ "device_id": "pc" }))).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(e.call("GET", "/api/group/current?device_id=phone", &e.sam, None).await.status, StatusCode::NO_CONTENT);
    assert!(e.ok("GET", "/api/group", &e.sam, None).await.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn the_event_stream_sends_states_in_rev_order_and_ends() {
    let e = setup("events");
    let g = e.start(json!({})).await;
    let (id, code) = (id_of(&g), code_of(&g));
    e.join(&e.sam, "phone", &code).await;
    // not a member, or another group: 404
    let (s, _) = e.events(&format!("/api/group/{id}/events?device_id=web"), &[("cookie", &e.pat)]).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = e.events("/api/group/g_nope/events?device_id=phone", &[("cookie", &e.sam)]).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, ev) = e.events(&format!("/api/group/{id}/events?device_id=phone"), &[("cookie", &e.sam)]).await;
    assert_eq!(s, StatusCode::OK);
    let mut ev = ev.unwrap();
    let (name, first) = ev.next().await.unwrap();
    assert_eq!(name, "state");
    assert_eq!((first["id"].clone(), first["you"]["host"].clone()), (json!(id), json!(false)), "personalized for the device");
    let mut last = first["rev"].as_u64().unwrap();
    // many changes at once from two members: the stream never goes back
    let mut tasks = vec![];
    for i in 0..12 {
        let (app, cookie, dev, id) = (e.app.clone(), if i % 2 == 0 { e.alex.clone() } else { e.sam.clone() }, if i % 2 == 0 { "pc" } else { "phone" }, id.clone());
        tasks.push(tokio::spawn(async move {
            let r = send(&app, "POST", &format!("/api/group/{id}/command"), &cookie, Some(json!({ "device_id": dev, "op": "enqueue", "tracks": [4] })), &[]).await;
            assert_eq!(r.status, StatusCode::OK);
            r.json()["rev"].as_u64().unwrap()
        }));
    }
    for t in tasks {
        // an answer is the state after its command, which may already hold later ones
        assert!(t.await.unwrap() > last);
    }
    let top = e.ok("GET", "/api/group/current?device_id=phone", &e.sam, None).await["rev"].as_u64().unwrap();
    assert_eq!(top, last + 12, "every command one rev");
    while last < top {
        let (name, v) = ev.next().await.expect("a state for every change, up to the last");
        assert_eq!(name, "state");
        let rev = v["rev"].as_u64().unwrap();
        assert!(rev > last, "{rev} after {last}");
        last = rev;
    }
    // the stream keeps the member: not dropped while it is open
    tick_once(e.st(), false);
    {
        let inner = e.st().playback.groups.lock();
        let m = inner.groups[&id].members.iter().find(|m| m.device_id == "phone").unwrap();
        assert_eq!((m.streams, m.gone_since), (1, None));
    }
    // the host ends it: the stream says so and closes
    let r = e.call("POST", &format!("/api/group/{id}/end"), &e.alex, Some(json!({ "device_id": "pc" }))).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let (name, v) = ev.next().await.unwrap();
    assert_eq!((name.as_str(), v), ("ended", json!({ "reason": "ended" })));
    assert!(ev.next().await.is_none(), "closed after ended");
}

#[tokio::test]
async fn the_anchor_moves_with_play_pause_and_seek() {
    let e = setup("anchor");
    let g = e.start(json!({ "position": 10.0 })).await;
    let id = id_of(&g);
    let before = now_ms();
    let r = e.command(&e.alex, "pc", &id, "seek", json!({ "position": 50.0 })).await.json();
    let after = now_ms();
    assert_eq!(r["position"], 50.0);
    let a = r["anchor_ms"].as_i64().unwrap();
    assert!(a >= before && a <= after);
    tokio::time::sleep(Duration::from_millis(120)).await;
    let p = e.command(&e.alex, "pc", &id, "pause", json!({})).await.json();
    // paused where the music was: position + (now − anchor) / 1000
    let pos = p["position"].as_f64().unwrap();
    let expected = 50.0 + (p["anchor_ms"].as_i64().unwrap() - a) as f64 / 1000.0;
    assert!((pos - expected).abs() < 0.002, "{pos} vs {expected}");
    assert!(pos >= 50.12);
    assert_eq!(p["playing"], false);
}

#[tokio::test]
async fn the_host_hands_over_and_the_last_account_ends_it() {
    let e = setup("handover");
    let g = e.start(json!({})).await;
    let (id, code) = (id_of(&g), code_of(&g));
    e.join(&e.sam, "phone", &code).await;
    e.join(&e.pat, "web", &code).await;
    let r = e.call("POST", &format!("/api/group/{id}/leave"), &e.alex, Some(json!({ "device_id": "pc", "device_name": "x" }))).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let s = e.ok("GET", "/api/group/current?device_id=phone", &e.sam, None).await;
    assert_eq!((s["host"]["name"].clone(), s["you"]["host"].clone(), s["you"]["can_control"].clone()), (json!("sam"), json!(true), json!(true)), "the longest-present member hosts now");
    assert_eq!(s["members"].as_array().unwrap().iter().filter(|m| m["host"] == true).count(), 1);
    // leaving twice, or a group one is not in: still 204
    assert_eq!(e.call("POST", &format!("/api/group/{id}/leave"), &e.alex, Some(json!({ "device_id": "pc" }))).await.status, StatusCode::NO_CONTENT);
    e.call("POST", &format!("/api/group/{id}/leave"), &e.sam, Some(json!({ "device_id": "phone" }))).await;
    let p = e.ok("GET", "/api/group/current?device_id=web", &e.pat, None).await;
    assert_eq!(p["host"]["name"], "pat");
    e.call("POST", &format!("/api/group/{id}/leave"), &e.pat, Some(json!({ "device_id": "web" }))).await;
    assert!(e.st().playback.groups.lock().groups.is_empty(), "the last one out ends it");
    // starting or joining another group takes the device out of its group first
    let a = e.start(json!({})).await;
    let b = e.ok("POST", "/api/group", &e.sam, Some(json!({ "device_id": "phone", "tracks": [4] }))).await;
    let r = e.join(&e.alex, "pc", &code_of(&b)).await.json();
    assert_eq!(r["id"], b["id"]);
    assert!(!e.st().playback.groups.lock().groups.contains_key(&id_of(&a)), "alex's own group ended when alex (its only account) moved on");
}

#[tokio::test]
async fn the_cap_counts_everyone_and_the_server_cap_wins() {
    let e = setup("cap");
    e.st().cfg.set_many(&json!({ "group_listen_max": 3 }));
    let g = e.start(json!({})).await;
    let (id, code) = (id_of(&g), code_of(&g));
    assert_eq!((g["max_members"].clone(), g["server_max_members"].clone()), (json!(3), json!(3)));
    assert_eq!(e.join(&e.sam, "phone", &code).await.status, StatusCode::OK);
    assert_eq!(e.join(&e.pat, "web", &code).await.status, StatusCode::OK);
    let r = e.join(&e.boss, "desk", &code).await;
    assert_eq!((r.status, r.json()["detail"].clone()), (StatusCode::CONFLICT, json!("This group listen is full")));
    // the host's cap: never below the members there are, never above the server's
    let p = e.ok("PATCH", &format!("/api/group/{id}"), &e.alex, Some(json!({ "device_id": "pc", "max_members": 2 }))).await;
    assert_eq!(p["max_members"], 3);
    e.st().cfg.set_many(&json!({ "group_listen_max": 50 }));
    let p = e.ok("PATCH", &format!("/api/group/{id}"), &e.alex, Some(json!({ "device_id": "pc", "max_members": 500 }))).await;
    assert_eq!((p["max_members"].clone(), p["server_max_members"].clone()), (json!(50), json!(50)));
    let p = e.ok("PATCH", &format!("/api/group/{id}"), &e.alex, Some(json!({ "device_id": "pc", "max_members": 4 }))).await;
    assert_eq!(p["max_members"], 4);
    assert_eq!(e.join(&e.boss, "desk", &code).await.status, StatusCode::OK);
    assert_eq!(e.join(&e.boss, "laptop", &code).await.status, StatusCode::CONFLICT);
    // joins at the same moment never pass the cap
    let g2 = e.ok("POST", "/api/group", &e.pat, Some(json!({ "device_id": "tablet", "tracks": [1] }))).await;
    e.ok("PATCH", &format!("/api/group/{}", id_of(&g2)), &e.pat, Some(json!({ "device_id": "tablet", "max_members": 5 }))).await;
    let mut tasks = vec![];
    for i in 0..20 {
        let (app, cookie, code) = (e.app.clone(), e.sam.clone(), code_of(&g2));
        tasks.push(tokio::spawn(async move {
            send(&app, "POST", "/api/group/join", &cookie, Some(json!({ "device_id": format!("d{i}"), "code": code })), &[]).await.status
        }));
    }
    let mut ok = 0;
    for t in tasks {
        if t.await.unwrap() == StatusCode::OK {
            ok += 1;
        }
    }
    assert_eq!(ok, 4, "the host and four more");
    assert_eq!(e.st().playback.groups.lock().groups[&id_of(&g2)].members.len(), 5);
}

#[tokio::test]
async fn groups_are_kept_through_a_restart_and_come_back_paused() {
    let e = setup("restart");
    e.st().cfg.set_many(&json!({ "group_listen_guests": true }));
    let g = e.start(json!({ "position": 20.0, "visibility": "link" })).await;
    let (id, code) = (id_of(&g), code_of(&g));
    let sam = e.join(&e.sam, "phone", &code).await.json();
    e.ok("PATCH", &format!("/api/group/{id}"), &e.alex, Some(json!({ "device_id": "pc", "guests_allowed": true }))).await;
    let guest = e.ok("POST", "/api/group/guest-join", "", Some(json!({ "code": code, "name": "Kim" }))).await;
    let token = guest["token"].as_str().unwrap().to_string();
    let before = e.ok("GET", "/api/group/current?device_id=phone", &e.sam, None).await;
    tick_once(e.st(), true);
    let saved = e.st().db.query("SELECT * FROM group_listens", p![]).unwrap();
    assert_eq!(saved.len(), 1);
    // no token in the saved group or its members
    let members = e.st().db.query("SELECT * FROM group_listen_members", p![]).unwrap();
    assert_eq!(members.len(), 3);
    let all = format!("{saved:?}{members:?}");
    assert!(!all.contains(&token) && !all.contains(&crate::auth::app_token_key(&token)), "no token is saved");
    // the server starts again on the same folder
    let s2 = TestServer::in_dir(&e.s.dir);
    let app2 = s2.app();
    let r = send(&app2, "GET", "/api/group/current?device_id=phone", &e.sam, None, &[]).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let back = r.json();
    assert_eq!((back["id"].clone(), back["code"].clone(), back["visibility"].clone(), back["guests_allowed"].clone()), (json!(id), json!(code), json!("link"), json!(true)));
    assert_eq!(back["playing"], false, "restored paused");
    let pos = back["position"].as_f64().unwrap();
    assert!((20.0..25.0).contains(&pos), "at the saved position: {pos}");
    assert!(back["rev"].as_u64().unwrap() > before["rev"].as_u64().unwrap(), "a device that saw the old revs takes the new states");
    assert_eq!(back["you"]["member"], sam["you"]["member"], "the device's membership is the same");
    assert_eq!(back["members"].as_array().unwrap().len(), 3);
    assert_ne!(back["boot"], before["boot"], "the phone tells a restart by it");
    assert_eq!(back["queue"].as_array().unwrap().iter().map(|q| q["qid"].clone()).collect::<Vec<_>>(),
               before["queue"].as_array().unwrap().iter().map(|q| q["qid"].clone()).collect::<Vec<_>>());
    // the guest's token works again
    let r = send(&app2, "GET", "/api/group/current", "", None, &[("authorization", &format!("Bearer {token}"))]).await;
    assert_eq!((r.status, r.json()["you"]["member"].clone()), (StatusCode::OK, guest["you"]["member"].clone()));
    // the host presses play
    let r = send(&app2, "POST", &format!("/api/group/{id}/command"), &e.alex, Some(json!({ "device_id": "pc", "op": "play" })), &[]).await;
    assert_eq!(r.json()["playing"], true);

    // with group_listen_keep off: nothing is kept and a restart ends them
    s2.st.cfg.set_many(&json!({ "group_listen_keep": false }));
    tick_once(&s2.st, true);
    assert!(s2.st.db.query("SELECT * FROM group_listens", p![]).unwrap().is_empty());
    let s3 = TestServer::in_dir(&e.s.dir);
    let r = send(&s3.app(), "GET", "/api/group/current?device_id=phone", &e.sam, None, &[]).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert!(s3.st.db.query("SELECT * FROM sessions WHERE kind='guest'", p![]).unwrap().is_empty(), "the guests' tokens went with their group");
}

#[tokio::test]
async fn the_list_shows_what_the_account_may_see_and_link_only_groups_do_not_leak() {
    let e = setup("list");
    let open = e.start(json!({})).await;
    let hidden = e.ok("POST", "/api/group", &e.pat, Some(json!({ "device_id": "web", "tracks": [4, 5], "visibility": "link" }))).await;
    let list = e.ok("GET", "/api/group", &e.sam, None).await;
    let ids: Vec<Value> = list.as_array().unwrap().iter().map(|g| g["id"].clone()).collect();
    assert_eq!(ids, vec![open["id"].clone()], "only the open one");
    let s = &list[0];
    assert_eq!((s["host"]["name"].clone(), s["track"]["title"].clone(), s["track"]["artist"].clone(), s["track"]["album_id"].clone()), (json!("alex"), json!("Song 1"), json!("Band"), json!("al1")));
    assert_eq!((s["playing"].clone(), s["members_count"].clone(), s["visibility"].clone(), s["mine"].clone()), (json!(true), json!(1), json!("anyone"), json!(false)));
    assert!(s["track"].get("cover_v").is_some() || s["track"].get("artists").is_some(), "enriched: {s}");
    // the host sees their own link-only group too, newest first
    let pats: Vec<(Value, Value)> = e.ok("GET", "/api/group", &e.pat, None).await.as_array().unwrap().iter().map(|g| (g["id"].clone(), g["mine"].clone())).collect();
    assert_eq!(pats, vec![(hidden["id"].clone(), json!(true)), (open["id"].clone(), json!(false))]);
    // joining from the list: an open group yes; a link-only one answers like a group that does not exist
    let r = e.call("POST", &format!("/api/group/{}/join", id_of(&open)), &e.sam, Some(json!({ "device_id": "phone" }))).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(e.ok("GET", "/api/group", &e.sam, None).await[0]["mine"], true);
    let hidden_try = e.call("POST", &format!("/api/group/{}/join", id_of(&hidden)), &e.sam, Some(json!({ "device_id": "tablet" }))).await;
    let nothing = e.call("POST", "/api/group/g_0123456789abcdef0123/join", &e.sam, Some(json!({ "device_id": "tablet" }))).await;
    assert_eq!((hidden_try.status, hidden_try.json()), (StatusCode::NOT_FOUND, json!({ "detail": "No group listen with that code" })));
    assert_eq!((nothing.status, nothing.json()), (hidden_try.status, hidden_try.json()), "the same answer: nothing leaks");
    // its other routes say the same to someone not in it
    for (m, uri) in [("GET", format!("/api/group/{}/events?device_id=tablet", id_of(&hidden))), ("POST", format!("/api/group/{}/end", id_of(&hidden)))] {
        let a = e.call(m, &uri, &e.sam, Some(json!({ "device_id": "tablet" }))).await;
        let b = e.call(m, &uri.replace(&id_of(&hidden), "g_0123456789abcdef0123"), &e.sam, Some(json!({ "device_id": "tablet" }))).await;
        assert_eq!((a.status, a.json()), (b.status, b.json()), "{uri}");
    }
    // with the code it opens (the code is the secret), and then it is in sam's list
    let l = e.ok("GET", &format!("/api/group/code/{}", code_of(&hidden)), &e.sam, None).await;
    assert_eq!((l["visibility"].clone(), l["mine"].clone()), (json!("link"), json!(false)));
    assert_eq!(e.join(&e.sam, "tablet", &code_of(&hidden)).await.status, StatusCode::OK);
    assert_eq!(e.ok("GET", "/api/group", &e.sam, None).await.as_array().unwrap().len(), 2);
    // the host makes it open later: the others see it
    e.ok("PATCH", &format!("/api/group/{}", id_of(&hidden)), &e.pat, Some(json!({ "device_id": "web", "visibility": "anyone" }))).await;
    assert_eq!(e.ok("GET", "/api/group", &e.boss, None).await.as_array().unwrap().len(), 2);
    assert_eq!(e.call("PATCH", &format!("/api/group/{}", id_of(&hidden)), &e.pat, Some(json!({ "device_id": "web", "visibility": "secret" }))).await.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn the_admin_switch_turns_every_entry_off() {
    let e = setup("switch");
    let g = e.start(json!({})).await;
    let (id, code) = (id_of(&g), code_of(&g));
    e.join(&e.sam, "phone", &code).await;
    let (_, ev) = e.events(&format!("/api/group/{id}/events?device_id=phone"), &[("cookie", &e.sam)]).await;
    let mut ev = ev.unwrap();
    assert_eq!(ev.next().await.unwrap().0, "state");
    e.st().cfg.set_many(&json!({ "group_listen": false }));
    for (m, uri, body) in [("GET", "/api/group".to_string(), None), ("POST", "/api/group".to_string(), Some(json!({ "device_id": "pc", "tracks": [] }))),
                           ("POST", "/api/group/join".to_string(), Some(json!({ "device_id": "web", "code": code }))),
                           ("GET", format!("/api/group/code/{code}"), None), ("GET", "/api/group/current?device_id=phone".to_string(), None),
                           ("POST", format!("/api/group/{id}/command"), Some(json!({ "device_id": "pc", "op": "pause" })))] {
        let r = e.call(m, &uri, &e.alex, body).await;
        assert_eq!((r.status, r.json()["detail"].clone()), (StatusCode::FORBIDDEN, json!("Group listens are turned off on this server")), "{m} {uri}");
    }
    // the groups that were running ended ("off")
    let (name, v) = ev.next().await.unwrap();
    assert_eq!((name.as_str(), v), ("ended", json!({ "reason": "off" })));
    assert!(e.st().playback.groups.lock().groups.is_empty());
    // the clock answers anyway
    assert_eq!(e.call("GET", "/api/time", "", None).await.status, StatusCode::OK);
    e.st().cfg.set_many(&json!({ "group_listen": true }));
    assert!(e.ok("GET", "/api/group", &e.alex, None).await.as_array().unwrap().is_empty());
}

/// A request from the address `ip`, through a proxy on this machine (the connection comes from 127.0.0.1 and
/// the proxy names the address in X-Forwarded-For, after whatever the client wrote there).
async fn send_from(e: &Env, method: &str, uri: &str, cookie: &str, body: Option<Value>, ip: &str) -> Resp {
    let mut b = axum::http::Request::builder().method(method).uri(uri).header("x-requested-with", "encore").header("x-forwarded-for", format!("6.6.6.6, {ip}"));
    if !cookie.is_empty() {
        b = b.header("cookie", cookie);
    }
    let body = match body {
        Some(v) => {
            b = b.header("content-type", "application/json");
            axum::body::Body::from(v.to_string())
        }
        None => axum::body::Body::empty(),
    };
    let mut req = b.body(body).unwrap();
    req.extensions_mut().insert(axum::extract::ConnectInfo(std::net::SocketAddr::from(([127, 0, 0, 1], 40000))));
    let resp = tower::ServiceExt::oneshot(e.app.clone(), req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap().to_vec();
    Resp { status, headers, body }
}

/// A guest of a group with guests allowed: (state, token).
async fn guest_in(e: &Env, code: &str, name: &str, ip: &str) -> Resp {
    send_from(e, "POST", "/api/group/guest-join", "", Some(json!({ "code": code, "name": name })), ip).await
}

#[tokio::test]
async fn guests_need_both_switches_and_their_token_reaches_only_their_group() {
    let e = setup("guests");
    let g = e.start(json!({ "tracks": [1, 2] })).await;
    let (id, code) = (id_of(&g), code_of(&g));
    // the server's switch is off: no guest, and no lookup without an account
    let r = guest_in(&e, &code, "Kim", "5.5.5.5").await;
    assert_eq!((r.status, r.json()["detail"].clone()), (StatusCode::FORBIDDEN, json!("Only people with an account on this server can join this group listen")));
    assert_eq!(e.call("GET", &format!("/api/group/code/{code}"), "", None).await.status, StatusCode::NOT_FOUND);
    // the server's switch is off: 403 whatever the code, so nothing tells a live code from a wrong one
    let r = guest_in(&e, "ZZZZZZ", "Kim", "5.5.5.5").await;
    assert_eq!((r.status, r.json()["detail"].clone()), (StatusCode::FORBIDDEN, json!("Only people with an account on this server can join this group listen")));
    // on for the server, still off for the group: answered as a wrong code is (and counted as one)
    e.st().cfg.set_many(&json!({ "group_listen_guests": true }));
    assert_eq!(e.ok("GET", "/api/group/current?device_id=pc", &e.alex, None).await["guests_possible"], true);
    let r = guest_in(&e, &code, "Kim", "5.5.5.5").await;
    assert_eq!((r.status, r.json()["detail"].clone()), (StatusCode::NOT_FOUND, json!("No group listen with that code")));
    assert_eq!(guest_in(&e, "ZZZZZZ", "Kim", "5.5.5.5").await.status, StatusCode::NOT_FOUND);
    e.ok("PATCH", &format!("/api/group/{id}"), &e.alex, Some(json!({ "device_id": "pc", "guests_allowed": true }))).await;
    // both on: the lookup answers without an account, and the guest gets in
    let l = e.ok("GET", &format!("/api/group/code/{code}"), "", None).await;
    assert_eq!((l["host"]["name"].clone(), l["guests"].clone()), (json!("alex"), json!(true)));
    assert_eq!(guest_in(&e, &code, " K ", "5.5.5.5").await.status, StatusCode::BAD_REQUEST, "a name of 2 to 32 characters");
    assert_eq!(guest_in(&e, &code, &"x".repeat(33), "5.5.5.5").await.status, StatusCode::BAD_REQUEST);
    let r = guest_in(&e, &code, "Kim", "5.5.5.5").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let cookie = r.h("set-cookie");
    assert!(cookie.starts_with("encore_guest=") && cookie.contains("Path=/api/group") && cookie.contains("HttpOnly"), "{cookie}");
    let s = r.json();
    let token = s["token"].as_str().unwrap().to_string();
    let me = s["members"].as_array().unwrap().iter().find(|m| m["guest"] == true).unwrap().clone();
    assert_eq!((me["name"].clone(), me["user"].clone(), me["avatar"].clone(), me["device_name"].clone(), me["host"].clone()), (json!("Kim"), json!(0), json!(""), json!(""), json!(false)));
    assert_eq!(s["you"], json!({ "member": me["member"], "host": false, "can_control": false }));
    let bearer = format!("Bearer {token}");
    let auth = [("authorization", bearer.as_str())];
    // the group's state, by token or by cookie
    assert_eq!(e.raw("GET", "/api/group/current", &auth, None).await.json()["id"], json!(id));
    let c = format!("encore_guest={token}");
    assert_eq!(e.raw("GET", "/api/group/current", &[("cookie", &c)], None).await.json()["id"], json!(id));
    // its event stream
    let (st, ev) = e.events(&format!("/api/group/{id}/events"), &auth).await;
    assert_eq!(st, StatusCode::OK);
    let mut ev = ev.unwrap();
    assert_eq!(ev.next().await.unwrap().1["you"]["member"], me["member"]);
    // the queued songs and their covers; nothing else
    let r = e.raw("GET", &format!("/api/group/{id}/tracks/1/stream"), &auth, None).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.body, b"ID3 song 1");
    let r = e.raw("GET", &format!("/api/group/{id}/tracks/5/stream"), &auth, None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "a song not in the queue");
    assert_eq!(e.raw("GET", &format!("/api/group/{id}/albums/al2/cover"), &auth, None).await.status, StatusCode::UNAUTHORIZED, "an album not in the queue");
    assert_ne!(e.raw("GET", &format!("/api/group/{id}/albums/al1/cover"), &auth, None).await.status, StatusCode::UNAUTHORIZED, "an album of the queue (no cover file: 404)");
    for (m, uri) in [("GET", "/api/library/tracks/1/stream"), ("GET", "/api/library/tracks/5/stream"), ("GET", "/api/library/tracks"), ("GET", "/api/library/albums"),
                     ("GET", "/api/library/albums/al1/cover"), ("GET", "/api/library/search?q=song"), ("GET", "/api/library/tracks/1"), ("GET", "/api/group"),
                     ("GET", "/api/likes"), ("POST", "/api/likes/1"), ("POST", "/api/library/plays"), ("POST", "/api/library/now-playing"), ("GET", "/api/stats/leaderboard"),
                     ("GET", "/api/auth/me"), ("GET", "/api/admin/settings"), ("GET", "/api/remote/devices"), ("GET", "/api/users/1/avatar"),
                     ("GET", &format!("/api/group/code/{code}"))] {
        let r = e.raw(m, uri, &[("authorization", bearer.as_str()), ("x-requested-with", "encore")], Some(json!({ "track_id": 1 }))).await;
        // the lookup is open without an account while guests may join: it answers as to anyone signed out
        if uri.starts_with("/api/group/code/") {
            assert_eq!(r.status, StatusCode::OK);
            continue;
        }
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{m} {uri}");
    }
    for (m, uri, body) in [("POST", format!("/api/group/{id}/command"), json!({ "op": "enqueue", "tracks": [3] })), ("PATCH", format!("/api/group/{id}"), json!({ "everyone_controls": true })),
                           ("POST", format!("/api/group/{id}/end"), json!({})), ("POST", "/api/group".into(), json!({ "device_id": "x", "tracks": [3] })),
                           ("POST", "/api/group/join".into(), json!({ "device_id": "x", "code": code })), ("DELETE", format!("/api/group/{id}/members/m1"), json!({}))] {
        let r = e.raw(m, &uri, &[("authorization", bearer.as_str()), ("x-requested-with", "encore")], Some(body)).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{m} {uri}");
    }
    // a cookie write needs the CSRF header
    let r = e.raw("POST", &format!("/api/group/{id}/leave"), &[("cookie", &c)], None).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // a second guest; the host removes the first: its stream is told and its token stops working
    let lee = guest_in(&e, &code, "Lee", "6.6.6.6").await.json();
    let r = e.call("DELETE", &format!("/api/group/{id}/members/{}?device_id=phone", me["member"].as_str().unwrap()), &e.sam, None).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND, "sam is not in the group");
    e.join(&e.sam, "phone", &code).await;
    let r = e.call("DELETE", &format!("/api/group/{id}/members/{}?device_id=phone", me["member"].as_str().unwrap()), &e.sam, None).await;
    assert_eq!((r.status, r.json()["detail"].clone()), (StatusCode::FORBIDDEN, json!("Only the host and admins can remove someone from a group listen")));
    let r = e.call("DELETE", &format!("/api/group/{id}/members/m1?device_id=pc"), &e.alex, None).await;
    assert_eq!((r.status, r.json()["detail"].clone()), (StatusCode::FORBIDDEN, json!("Only guests can be removed from a group listen")));
    let r = e.call("DELETE", &format!("/api/group/{id}/members/{}?device_id=pc", me["member"].as_str().unwrap()), &e.alex, None).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert!(r.json()["members"].as_array().unwrap().iter().all(|m| m["member"] != me["member"]));
    loop {
        let (name, v) = ev.next().await.expect("the removed guest's stream is told");
        if name == "ended" {
            assert_eq!(v, json!({ "reason": "removed" }));
            break;
        }
    }
    assert_eq!(e.raw("GET", "/api/group/current", &auth, None).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(e.raw("GET", &format!("/api/group/{id}/tracks/1/stream"), &auth, None).await.status, StatusCode::UNAUTHORIZED);
    // an admin in the group may remove a guest too
    e.join(&e.boss, "desk", &code).await;
    let r = e.call("DELETE", &format!("/api/group/{id}/members/{}?device_id=desk", lee["you"]["member"].as_str().unwrap()), &e.boss, None).await;
    assert_eq!(r.status, StatusCode::OK);
    // a guest who leaves: the token goes, and the cookie is cleared
    let ann = guest_in(&e, &code, "Ann", "7.7.7.7").await.json();
    let ann_auth = format!("Bearer {}", ann["token"].as_str().unwrap());
    let r = e.raw("POST", &format!("/api/group/{id}/leave"), &[("authorization", ann_auth.as_str())], None).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(e.raw("GET", "/api/group/current", &[("authorization", ann_auth.as_str())], None).await.status, StatusCode::UNAUTHORIZED);
    // 12 hours later: the token stops working and the guest is out
    let bo = guest_in(&e, &code, "Bo", "8.8.8.8").await.json();
    let bo_auth = format!("Bearer {}", bo["token"].as_str().unwrap());
    e.st().db.execute("UPDATE sessions SET expires_at=? WHERE kind='guest'", p![now() - 1.0]).unwrap();
    assert_eq!(e.raw("GET", "/api/group/current", &[("authorization", bo_auth.as_str())], None).await.status, StatusCode::UNAUTHORIZED);
    {
        let mut inner = e.st().playback.groups.lock();
        for m in inner.groups.get_mut(&id).unwrap().members.iter_mut().filter(|m| m.is_guest()) {
            m.expires_at = Some(now() - 1.0);
        }
    }
    tick_once(e.st(), false);
    assert!(!e.st().playback.groups.lock().groups[&id].members.iter().any(Member::is_guest));
    // the group ends: every guest token goes with it
    let cy = guest_in(&e, &code, "Cy", "9.9.9.9").await.json();
    e.call("POST", &format!("/api/group/{id}/end"), &e.alex, Some(json!({ "device_id": "pc" }))).await;
    let cy_auth = format!("Bearer {}", cy["token"].as_str().unwrap());
    assert_eq!(e.raw("GET", "/api/group/current", &[("authorization", cy_auth.as_str())], None).await.status, StatusCode::UNAUTHORIZED);
    assert!(e.st().db.query("SELECT * FROM sessions WHERE kind='guest'", p![]).unwrap().is_empty());
}

#[tokio::test]
async fn turning_guests_off_takes_every_guest_out() {
    let e = setup("guestsoff");
    e.st().cfg.set_many(&json!({ "group_listen_guests": true }));
    let g = e.start(json!({ "tracks": [1, 2] })).await;
    let (id, code) = (id_of(&g), code_of(&g));
    e.ok("PATCH", &format!("/api/group/{id}"), &e.alex, Some(json!({ "device_id": "pc", "guests_allowed": true }))).await;
    let kim = guest_in(&e, &code, "Kim", "5.5.5.5").await.json();
    let bearer = format!("Bearer {}", kim["token"].as_str().unwrap());
    let auth = [("authorization", bearer.as_str())];
    let (_, ev) = e.events(&format!("/api/group/{id}/events"), &auth).await;
    let mut ev = ev.unwrap();
    ev.next().await.unwrap();
    assert_eq!(e.raw("GET", &format!("/api/group/{id}/tracks/1/stream"), &auth, None).await.status, StatusCode::OK);
    // an admin turns guests off: every guest route answers 401 at once …
    e.st().cfg.set_many(&json!({ "group_listen_guests": false }));
    assert_eq!(e.raw("GET", &format!("/api/group/{id}/tracks/1/stream"), &auth, None).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(e.raw("GET", "/api/group/current", &auth, None).await.status, StatusCode::UNAUTHORIZED);
    // … and the guests are taken out: the stream is told, the tokens are deleted
    tick_once(e.st(), false);
    loop {
        let (name, v) = ev.next().await.expect("the guest's stream is told");
        if name == "ended" {
            assert_eq!(v, json!({ "reason": "removed" }));
            break;
        }
    }
    assert!(!e.st().playback.groups.lock().groups[&id].members.iter().any(Member::is_guest));
    assert!(e.st().db.query("SELECT * FROM sessions WHERE kind='guest'", p![]).unwrap().is_empty());
    // turning guests on again does not bring the token back
    e.st().cfg.set_many(&json!({ "group_listen_guests": true }));
    assert_eq!(e.raw("GET", "/api/group/current", &auth, None).await.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn an_open_stream_checks_its_account_again() {
    let e = setup("recheck");
    let g = e.start(json!({})).await;
    let (id, code) = (id_of(&g), code_of(&g));
    e.join(&e.sam, "phone", &code).await;
    e.join(&e.pat, "web", &code).await;
    // the host signs out everywhere: its stream ends and the group is handed over
    let (_, ev) = e.events(&format!("/api/group/{id}/events?device_id=pc"), &[("cookie", &e.alex)]).await;
    let mut ev = ev.unwrap();
    ev.next().await.unwrap();
    e.st().db.execute("DELETE FROM sessions WHERE user_id=?", p![e.ids.alex]).unwrap();
    loop {
        let (name, v) = ev.next().await.expect("the stream is told");
        if name == "ended" {
            assert_eq!(v, json!({ "reason": "removed" }));
            break;
        }
    }
    let s = e.ok("GET", "/api/group/current?device_id=phone", &e.sam, None).await;
    assert_eq!(s["host"]["name"], "sam", "handed over");
    assert_eq!(s["members"].as_array().unwrap().len(), 2);
    // an account an admin disabled: out as well
    let (_, ev) = e.events(&format!("/api/group/{id}/events?device_id=web"), &[("cookie", &e.pat)]).await;
    let mut ev = ev.unwrap();
    ev.next().await.unwrap();
    e.st().db.execute("UPDATE users SET disabled=1 WHERE username='pat'", p![]).unwrap();
    loop {
        let (name, v) = ev.next().await.expect("the stream is told");
        if name == "ended" {
            assert_eq!(v, json!({ "reason": "removed" }));
            break;
        }
    }
    let s = e.ok("GET", "/api/group/current?device_id=phone", &e.sam, None).await;
    assert_eq!(s["members"].as_array().unwrap().iter().map(|m| m["name"].clone()).collect::<Vec<_>>(), vec![json!("sam")]);
    // a member still allowed keeps its stream
    let (_, ev) = e.events(&format!("/api/group/{id}/events?device_id=phone"), &[("cookie", &e.sam)]).await;
    let mut ev = ev.unwrap();
    ev.next().await.unwrap();
    tokio::time::sleep(RECHECK * 3).await;
    e.command(&e.sam, "phone", &id, "pause", json!({})).await;
    assert_eq!(ev.next().await.unwrap().0, "state");
}

#[tokio::test]
async fn guests_count_toward_the_cap_and_their_joins_are_rate_limited() {
    let e = setup("guestcap");
    e.st().cfg.set_many(&json!({ "group_listen_guests": true, "group_listen_max": 100 }));
    let g = e.start(json!({})).await;
    let (id, code) = (id_of(&g), code_of(&g));
    e.ok("PATCH", &format!("/api/group/{id}"), &e.alex, Some(json!({ "device_id": "pc", "guests_allowed": true, "max_members": 3 }))).await;
    assert_eq!(guest_in(&e, &code, "Kim", "1.1.1.1").await.status, StatusCode::OK);
    assert_eq!(guest_in(&e, &code, "Lee", "1.1.1.2").await.status, StatusCode::OK);
    let r = guest_in(&e, &code, "Ann", "1.1.1.3").await;
    assert_eq!((r.status, r.json()["detail"].clone()), (StatusCode::CONFLICT, json!("This group listen is full")));
    assert_eq!(e.join(&e.sam, "phone", &code).await.status, StatusCode::CONFLICT, "guests fill the cap for accounts too");
    // 20 guest joins per group in an hour
    e.ok("PATCH", &format!("/api/group/{id}"), &e.alex, Some(json!({ "device_id": "pc", "max_members": 100 }))).await;
    for i in 0..18 {
        let r = guest_in(&e, &code, &format!("Guest {i}"), &format!("2.2.2.{i}")).await;
        assert_eq!(r.status, StatusCode::OK, "{i}: {}", r.text());
    }
    let r = guest_in(&e, &code, "One more", "3.3.3.3").await;
    assert_eq!((r.status, r.json()["detail"].clone()), (StatusCode::TOO_MANY_REQUESTS, json!("Too many tries. Wait a moment and try again.")));
    // 60 per address in an hour, over several groups
    let mut codes = vec![];
    for (who, dev) in [(&e.sam, "a"), (&e.pat, "b"), (&e.boss, "c"), (&e.sam, "d")] {
        let g = e.ok("POST", "/api/group", who, Some(json!({ "device_id": dev, "tracks": [1] }))).await;
        e.ok("PATCH", &format!("/api/group/{}", id_of(&g)), who, Some(json!({ "device_id": dev, "guests_allowed": true, "max_members": 100 }))).await;
        codes.push(code_of(&g));
    }
    let mut ok = 0;
    for i in 0..61 {
        let r = guest_in(&e, &codes[i / 20], &format!("Guest {i}"), "4.4.4.4").await;
        if r.status == StatusCode::OK {
            ok += 1;
        } else {
            assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
        }
    }
    assert_eq!(ok, 60);
}

#[tokio::test]
async fn wrong_codes_and_starts_are_rate_limited() {
    let e = setup("limits");
    let g = e.start(json!({})).await;
    let code = code_of(&g);
    // 30 wrong codes per account: then every lookup and join answers 429, the right code too
    for i in 0..30 {
        let r = e.join(&e.sam, "phone", &format!("ZZZZ{:02}", i % 10 + 22)).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{i}");
    }
    let r = e.join(&e.sam, "phone", &code).await;
    assert_eq!((r.status, r.json()["detail"].clone()), (StatusCode::TOO_MANY_REQUESTS, json!("Too many tries. Wait a moment and try again.")));
    assert_eq!(e.call("GET", &format!("/api/group/code/{code}"), &e.sam, None).await.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(e.call("POST", &format!("/api/group/{}/join", id_of(&g)), &e.sam, Some(json!({ "device_id": "phone" }))).await.status, StatusCode::TOO_MANY_REQUESTS);
    // right answers do not count; another account has its own count
    for _ in 0..40 {
        assert_eq!(e.call("GET", &format!("/api/group/code/{code}"), &e.pat, None).await.status, StatusCode::OK);
    }
    assert_eq!(e.join(&e.pat, "web", &code).await.status, StatusCode::OK);
    // 60 per address: two accounts from one address
    for i in 0..60 {
        let who = if i < 30 { &e.boss } else { &e.alex };
        let r = send_from(&e, "GET", "/api/group/code/ZZZZZZ", who, None, "9.9.9.9").await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{i}");
    }
    let fresh = account(e.st(), "fresh", "user");
    let r = send_from(&e, "GET", &format!("/api/group/code/{code}"), &session(e.st(), fresh.i("id")), None, "9.9.9.9").await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS, "the address is past its 60");
    e.st().cfg.set_many(&json!({ "group_listen_guests": true }));
    let r = send_from(&e, "POST", "/api/group/guest-join", "", Some(json!({ "code": code, "name": "Kim" })), "9.9.9.9").await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS, "guests from that address too");
    // everyone without an account shares 300 wrong codes an hour, whatever address each try names
    for i in 0..(FAILED_SIGNED_OUT - 1) {
        let r = send_from(&e, "GET", "/api/group/code/ZZZZZZ", "", None, &format!("20.{}.{}.1", 1 + i / 200, i % 200)).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{i}");
    }
    // (the guest-join of a group without guests counts as a wrong code too: the 300th)
    let r = send_from(&e, "POST", "/api/group/guest-join", "", Some(json!({ "code": code, "name": "Kim" })), "11.0.0.1").await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = send_from(&e, "GET", &format!("/api/group/code/{code}"), "", None, "12.0.0.1").await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS, "a new address does not help");
    let r = send_from(&e, "POST", "/api/group/guest-join", "", Some(json!({ "code": code, "name": "Kim" })), "12.0.0.2").await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
    let r = send_from(&e, "GET", &format!("/api/group/code/{code}"), &e.pat, None, "12.0.0.3").await;
    assert_eq!(r.status, StatusCode::OK, "accounts have their own counts");
    // 10 starts per account in an hour
    for i in 0..9 {
        let r = e.call("POST", "/api/group", &e.alex, Some(json!({ "device_id": format!("d{i}"), "tracks": [] }))).await;
        assert_eq!(r.status, StatusCode::OK, "{i}: {}", r.text());
    }
    let r = e.call("POST", "/api/group", &e.alex, Some(json!({ "device_id": "d10", "tracks": [] }))).await;
    assert_eq!((r.status, r.json()["detail"].clone()), (StatusCode::TOO_MANY_REQUESTS, json!("You started 10 group listens in the last hour. Try again later.")));
    // starts sent at the same time: still 10 at most (checked and counted in one step)
    let st = e.st();
    let calls = (0..20).map(|i| send(&e.app, "POST", "/api/group", &e.sam, Some(json!({ "device_id": format!("p{i}"), "tracks": [] })), &[]));
    let answers = futures_util::future::join_all(calls).await;
    assert_eq!(answers.iter().filter(|r| r.status == StatusCode::OK).count(), 10);
    assert_eq!(st.playback.groups.lock().groups.values().filter(|g| g.host_user() == e.ids.sam).count(), 10);
}

#[tokio::test]
async fn members_gone_two_minutes_are_dropped_and_idle_groups_end() {
    let e = setup("drop");
    let g = e.start(json!({})).await;
    let (id, code) = (id_of(&g), code_of(&g));
    e.join(&e.sam, "phone", &code).await;
    e.join(&e.pat, "web", &code).await;
    // nobody has an event stream open: after 2 minutes the host and pat are out, sam (whose stream is open) stays
    let (_, ev) = e.events(&format!("/api/group/{id}/events?device_id=phone"), &[("cookie", &e.sam)]).await;
    let mut ev = ev.unwrap();
    ev.next().await.unwrap();
    tick_once(e.st(), false);
    assert_eq!(e.st().playback.groups.lock().groups[&id].members.len(), 3, "not yet");
    {
        let mut inner = e.st().playback.groups.lock();
        for m in inner.groups.get_mut(&id).unwrap().members.iter_mut() {
            if let Some(t) = m.gone_since.as_mut() {
                *t -= gl::DROP_SECS + 1.0;
            }
        }
    }
    tick_once(e.st(), false);
    let s = e.ok("GET", "/api/group/current?device_id=phone", &e.sam, None).await;
    assert_eq!(s["members"].as_array().unwrap().iter().map(|m| m["name"].clone()).collect::<Vec<_>>(), vec![json!("sam")]);
    assert_eq!(s["host"]["name"], "sam", "handed over");
    // the dropped device's stream answers 404; it joins again with the code
    let (st, _) = e.events(&format!("/api/group/{id}/events?device_id=web"), &[("cookie", &e.pat)]).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    // the stream that was open closes when its member goes; reopening counts again
    drop(ev);
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(e.st().playback.groups.lock().groups[&id].members[0].gone_since.is_some(), "its stream closed: the 2 minutes run");
    // 12 hours without a change: the group ends ("idle")
    let (_, ev) = e.events(&format!("/api/group/{id}/events?device_id=phone"), &[("cookie", &e.sam)]).await;
    let mut ev = ev.unwrap();
    ev.next().await.unwrap();
    e.command(&e.sam, "phone", &id, "pause", json!({})).await;
    ev.next().await.unwrap();
    e.st().playback.groups.lock().groups.get_mut(&id).unwrap().active_at -= gl::IDLE_SECS + 1.0;
    tick_once(e.st(), false);
    assert_eq!(ev.next().await.unwrap(), ("ended".to_string(), json!({ "reason": "idle" })));
    assert!(e.st().playback.groups.lock().groups.is_empty());
}

#[tokio::test]
async fn the_server_moves_on_by_itself_at_the_end_of_a_song() {
    let e = setup("advance");
    let g = e.start(json!({ "position": 199.9 })).await;
    let id = id_of(&g);
    tokio::time::sleep(Duration::from_millis(200)).await;
    tick_once(e.st(), false);
    let s = e.ok("GET", "/api/group/current?device_id=pc", &e.alex, None).await;
    assert_eq!((s["index"].clone(), s["track"]["id"].clone(), s["playing"].clone()), (json!(1), json!(2), json!(true)));
    assert!(s["rev"].as_u64() > g["rev"].as_u64());
    let pos = s["position"].as_f64().unwrap();
    assert_eq!(pos, 0.0);
    let anchor = s["anchor_ms"].as_i64().unwrap();
    assert!(anchor >= g["anchor_ms"].as_i64().unwrap() + 99 && anchor <= g["anchor_ms"].as_i64().unwrap() + 101, "anchored where song 1 ended");
    let _ = id;
}

#[tokio::test]
async fn the_answers_carry_the_usual_enrichment_once() {
    let e = setup("enrich");
    e.st().db.execute("UPDATE tracks SET rg_track_gain=-6.5, rg_track_peak=0.95, loudness_lufs=-11.5, loudness_source='tags', sample_rate=44100, bit_depth=16 WHERE id=1", p![]).unwrap();
    let g = e.start(json!({})).await;
    assert_eq!(g["queue"][0]["gain"]["track"], -6.5, "§2.1 fields on the queue rows: {}", g["queue"][0]);
    assert_eq!(g["queue"][0]["sample_rate"], 44100);
    let mut alex = e.st().db.one("SELECT * FROM users WHERE username='alex'", p![]).unwrap().unwrap();
    alex.remove("password_hash");
    for v in [g.clone(), e.ok("GET", "/api/group", &e.alex, None).await, e.ok("GET", &format!("/api/group/code/{}", code_of(&g)), &e.alex, None).await] {
        let (st, who, mut again) = (e.st().clone(), alex.clone(), v.clone());
        let again = tokio::task::spawn_blocking(move || {
            enrich_now(&st, Some(&who), &mut again);
            again
        }).await.unwrap();
        assert_eq!(again, v, "the answer was enriched already");
    }
}
