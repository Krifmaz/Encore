//! The Soulseek session and the search gate against a stand-in slskd: an axum app answering /server,
//! /application and /searches like slskd 0.26.1 does, called in-process (`Target::App`) so tokio's paused
//! time can run through hours of reconnects without HTTP timeouts firing.
//!
//! The stand-in plays slskd's reconnect loop (ConnectionWatchdog) as the real 0.26.1 was seen to behave
//! against a fake Soulseek server (Oct 19): it starts again at once after a session drops and after an
//! attempt fails, waits 0.5, 1.5, 3.5, 7.5 … s between its own attempts, a DELETE /server while it is
//! still connecting starts it again at once ("Operation cancelled"), a DELETE once connected ends the
//! session "by the user", PUT restarts it (205) when it runs. Every connection it makes goes through the
//! session's door, as through Encore's relay or SOCKS proxy: only those the door lets through reach the
//! "server" (`reached`), the others are reset at once. Searches run 10 s; slskd 0.26's DELETE of a search
//! forgets it without stopping it (only PUT stops it), and a session that ends cancels the searches.
//!
//! Covered: only Encore's attempts reach the server, on Encore's schedule (also when a DELETE lands while
//! slskd connects); a dropped session; an slskd restart while logged in; a rejected login and a fixed
//! password tried on the restarted slskd; Reconnect now; the schedule kept across a restart of Encore; no
//! search while logged out; one search at a time 3 s apart with people first; a cancelled search stopped
//! before the next; "too many searches"; "not connected" waking the supervisor; requests parked (session
//! down, dropped during the search, slskd not answering, waiting for the VPN) and going on after an hour;
//! a bulk retry of 200 requests.
use crate::state::AppState;
use crate::test_support::{call, TestServer};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use encore_db::{p, RowExt};
use encore_requests::slskd_managed::{Target, STAND_IN_DOWN};
use encore_requests::slskd_session::{self as session, Pass, Reason, Session, State as S};
use encore_requests::sources::slskd;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::Instant;

const PW: &str = "a-long-password-1";

/// How the Soulseek server answers a connection that reaches it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Login {
    /// the login is never answered: "The wait timed out after 5000 milliseconds"
    TimesOut,
    /// the connection itself is never answered (through the VPN's proxy: 30 s, then "Operation timed out")
    NoAnswer,
    /// "The server rejected login attempt: INVALIDPASS" (slskd does not try again)
    Rejected,
    Works,
}

/// slskd's client.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Client {
    Disconnected,
    Connecting,
    LoggingIn,
    LoggedIn,
}

struct Search {
    started: Instant,
    cancelled: bool,
    /// slskd still knows it (not deleted)
    known: bool,
    /// its searchText
    text: String,
}

struct Fake {
    login: Login,
    /// connections that reached the server (let through by the door)
    reached: Vec<Instant>,
    client: Client,
    /// the connection through the door, while it lasts
    pass: Option<Pass>,
    /// slskd's watchdog: on, its loop running, and a counter its cancels bump
    enabled: bool,
    looping: bool,
    epoch: u64,
    puts: Vec<Instant>,
    deletes: Vec<Instant>,
    /// searches slskd took: (when, text)
    posts: Vec<(Instant, String)>,
    /// searches turned away with 429
    refused: Vec<Instant>,
    /// turn the next N searches away with 429
    too_many: usize,
    searches: HashMap<String, Search>,
    /// searches stopped with PUT /searches/{id}: (when, id)
    stopped: Vec<(Instant, String)>,
    /// the most searches running at once
    most: usize,
    search_secs: u64,
    /// slskd's API doesn't answer (it is restarting)
    down: bool,
    /// where slskd's log lines go
    session: Session,
    /// downloads slskd knows, per user: (file name, state)
    transfers: HashMap<String, Vec<(String, String)>>,
    /// downloads slskd took (POST /transfers/downloads/{user}): (when, user)
    enqueues: Vec<(Instant, String)>,
    /// downloads turned away with 429
    enqueue_refused: Vec<Instant>,
    /// turn the next N downloads away with 429
    enqueue_too_many: usize,
    /// downloads being taken right now, and the most at once
    enqueuing: usize,
    most_enqueuing: usize,
    /// how long slskd takes to take a download (it asks the server, then the user's client)
    enqueue_secs: u64,
    /// the session is gone under slskd: a download is refused with Soulseek.NET's "must be connected"
    enqueue_not_connected: bool,
    /// what a search for a searchText finds (slskd's responses), handed out once it is over; none: nothing
    answers: HashMap<String, Value>,
    /// users gone offline: a download from them is refused as slskd refuses it ("User x appears to be offline")
    offline: Vec<String>,
}

type Shared = Arc<Mutex<Fake>>;

/// A line of slskd's log.
fn say(f: &Fake, line: &str) {
    f.session.on_line(line);
}

fn search_done(f: &Fake, s: &Search) -> bool {
    s.cancelled || s.started.elapsed() >= Duration::from_secs(f.search_secs)
}

fn running(f: &Fake) -> usize {
    f.searches.values().filter(|s| !search_done(f, s)).count()
}

/// The session ends: the connection through the door closes, and Soulseek.NET cancels the searches.
fn session_ends(f: &mut Fake) {
    f.client = Client::Disconnected;
    f.pass = None;
    for s in f.searches.values_mut() {
        s.cancelled = true;
    }
}

/// ConnectionWatchdog.Start(): on, and a loop unless one runs. The loop's first attempt begins before
/// Start() returns (slskd calls it from the Disconnected event, so it is under way before anything else
/// can happen); a loop that ends while the watchdog is on is followed by a new one from the watchdog's
/// 100 ms timer.
fn start(f: &mut Fake, sh: &Shared) {
    f.enabled = true;
    if !f.looping {
        f.looping = true;
        let first = (f.client == Client::Disconnected).then(|| (f.epoch, begin_attempt(f, "[INF] Attempting to connect to the Soulseek server...")));
        tokio::spawn(watchdog_task(sh.clone(), first, Duration::ZERO));
    }
}

/// The loop as a task of its own (boxed: the loop starts loops).
fn watchdog_task(sh: Shared, first: Option<(u64, Option<Pass>)>, after: Duration) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    Box::pin(watchdog(sh, first, after))
}

/// An attempt begins: connecting, through the door (Encore's relay) or turned away there.
fn begin_attempt(f: &mut Fake, line: &str) -> Option<Pass> {
    say(f, line);
    f.client = Client::Connecting;
    let p = f.session.door();
    if p.is_some() {
        f.reached.push(Instant::now());
    }
    p
}

/// ConnectionWatchdog.Stop(abortReconnect: true).
fn stop(f: &mut Fake) {
    f.enabled = false;
    f.epoch += 1;
}

/// Sleep unless cancelled (false: cancelled).
async fn sleep_unless_cancelled(sh: &Shared, epoch: u64, d: Duration) -> bool {
    let until = Instant::now() + d;
    while Instant::now() < until {
        tokio::time::sleep((until - Instant::now()).min(Duration::from_millis(100))).await;
        if sh.lock().unwrap().epoch != epoch {
            return false;
        }
    }
    true
}

/// ConnectionWatchdog.AttemptConnection's loop (`first`: its first attempt, already begun).
async fn watchdog(sh: Shared, mut first: Option<(u64, Option<Pass>)>, after: Duration) {
    tokio::time::sleep(after).await;
    let mut attempts = 0u32;
    loop {
        let (epoch, pass) = match first.take() {
            Some(begun) => begun,
            None => {
                let epoch = {
                    let f = sh.lock().unwrap();
                    if !f.enabled || f.client != Client::Disconnected {
                        break;
                    }
                    f.epoch
                };
                if attempts > 0 {
                    let secs = (((1u64 << attempts.min(20)) - 1) / 2).min(300);
                    let words = if secs == 0 { "a second".to_string() } else { format!("{} seconds", secs + 1) };
                    say(&sh.lock().unwrap(), &format!("[INF] Waiting about {words} before attempting to reconnect"));
                    if !sleep_unless_cancelled(&sh, epoch, Duration::from_millis(secs * 1000 + 500)).await {
                        say(&sh.lock().unwrap(), "[INF] Reconnection attempt cancelled");
                        break;
                    }
                }
                let mut f = sh.lock().unwrap();
                if !f.enabled || f.client != Client::Disconnected || f.epoch != epoch {
                    break;
                }
                let line = if attempts > 0 { format!("[INF] Attempting to connect to the Soulseek server (#{attempts})...") } else { "[INF] Attempting to connect to the Soulseek server...".into() };
                (epoch, begin_attempt(&mut f, &line))
            }
        };
        match connect(&sh, epoch, pass).await {
            Ok(()) => break,
            Err(None) => {
                say(&sh.lock().unwrap(), "[INF] Reconnection attempt cancelled");
                break;
            }
            Err(Some(msg)) => {
                attempts += 1;
                say(&sh.lock().unwrap(), &format!("[ERR] Failed to reconnect: {msg}"));
            }
        }
    }
    let mut f = sh.lock().unwrap();
    f.looping = false;
    if f.enabled && f.client == Client::Disconnected {
        f.looping = true;
        tokio::spawn(watchdog_task(sh.clone(), None, Duration::from_millis(100)));
    }
}

/// Client.ConnectAsync, once the door has answered. Err(None): cancelled; Err(Some(why)): failed.
async fn connect(sh: &Shared, epoch: u64, pass: Option<Pass>) -> Result<(), Option<String>> {
    let login = sh.lock().unwrap().login;
    let Some(pass) = pass else {
        // the door is shut: Encore's relay resets the connection, it fails in a moment on loopback
        tokio::time::sleep(Duration::from_millis(5)).await;
        let mut f = sh.lock().unwrap();
        f.client = Client::Disconnected;
        say(&f, "[ERR] Disconnected from the Soulseek server: Connection reset by peer");
        start(&mut f, sh); // an ordinary disconnect starts the watchdog
        return Err(Some("Failed to connect: Failed to connect to 127.0.0.1:40123: Connection reset by peer".into()));
    };
    sh.lock().unwrap().pass = Some(pass);
    // cancelled while still connecting: slskd counts it as an ordinary disconnect and starts again
    let cancelled_connecting = |sh: &Shared| {
        let mut f = sh.lock().unwrap();
        f.client = Client::Disconnected;
        f.pass = None;
        say(&f, "[ERR] Disconnected from the Soulseek server: Operation cancelled");
        start(&mut f, sh);
        Err(None)
    };
    if login == Login::NoAnswer {
        if !sleep_unless_cancelled(sh, epoch, Duration::from_secs(30)).await {
            return cancelled_connecting(sh);
        }
        let mut f = sh.lock().unwrap();
        f.client = Client::Disconnected;
        f.pass = None;
        say(&f, "[ERR] Disconnected from the Soulseek server: Operation timed out after 30000 milliseconds");
        start(&mut f, sh);
        return Err(Some("Operation timed out after 30000 milliseconds".into()));
    }
    tokio::time::sleep(Duration::from_millis(10)).await;
    {
        let mut f = sh.lock().unwrap();
        if f.epoch != epoch {
            drop(f);
            return cancelled_connecting(sh);
        }
        f.client = Client::LoggingIn;
        f.enabled = false; // Client_Connected stops the watchdog (its loop goes on with this attempt)
        say(&f, "[INF] Connected to the Soulseek server");
    }
    let wait = if login == Login::TimesOut { Duration::from_secs(5) } else { Duration::from_millis(200) };
    if !sleep_unless_cancelled(sh, epoch, wait).await {
        return Err(None); // a DELETE while logging in ends it "by the user" (the DELETE did that)
    }
    let mut f = sh.lock().unwrap();
    if f.client != Client::LoggingIn {
        return Err(None);
    }
    match login {
        Login::Works => {
            f.client = Client::LoggedIn;
            say(&f, "[INF] Logged in to the Soulseek server as alice");
            Ok(())
        }
        Login::Rejected => {
            session_ends(&mut f);
            say(&f, "[ERR] Disconnected from the Soulseek server: invalid username or password");
            Err(Some("The server rejected login attempt: INVALIDPASS".into()))
        }
        Login::TimesOut | Login::NoAnswer => {
            session_ends(&mut f);
            say(&f, "[ERR] Disconnected from the Soulseek server: The wait timed out after 5000 milliseconds");
            start(&mut f, sh);
            Err(Some("The wait timed out after 5000 milliseconds".into()))
        }
    }
}

/// The server ends a session that was logged in.
fn server_closes(sh: &Shared, why: &str) {
    let mut f = sh.lock().unwrap();
    assert_eq!(f.client, Client::LoggedIn);
    session_ends(&mut f);
    say(&f, &format!("[ERR] Disconnected from the Soulseek server: {why}"));
    if !why.contains("another client") {
        start(&mut f, sh);
    }
}

/// How slskd gets restarted in a test.
#[derive(Clone, Copy, Debug)]
enum Restart {
    /// it says it shuts down, and a new process runs before the supervisor looks
    Quick,
    /// the same, and the supervisor sees the new one starting first
    SeenStarting,
    /// killed without a word (a crash, kill -9), then started again
    Killed,
}

/// Encore restarts slskd (new settings, a new VPN port): the old process ends its session, then a new
/// process runs.
async fn restart_slskd(st: &AppState, sh: &Shared, how: Restart) {
    {
        let mut f = sh.lock().unwrap();
        if !matches!(how, Restart::Killed) {
            say(&f, "[WRN] Application is shutting down");
            if f.client != Client::Disconnected {
                say(&f, "[INF] Disconnected from the Soulseek server: the client is shutting down");
            }
        }
        session_ends(&mut f);
        stop(&mut f);
        f.down = true;
    }
    let m = &st.requests.slskd;
    tokio::time::sleep(Duration::from_secs(3)).await;
    m.restarted_for_tests();
    if matches!(how, Restart::SeenStarting) {
        m.set_state_for_tests("starting");
        tokio::time::sleep(Duration::from_secs(5)).await;
        m.set_state_for_tests("running");
    }
    sh.lock().unwrap().down = false;
}

fn server_json(f: &Fake) -> Value {
    let (state, connected) = match f.client {
        Client::Disconnected => ("Disconnected", false),
        Client::Connecting => ("Connecting", false),
        Client::LoggingIn => ("Connected, LoggingIn", true),
        Client::LoggedIn => ("Connected, LoggedIn", true),
    };
    json!({ "state": state, "isConnected": connected, "isLoggedIn": f.client == Client::LoggedIn, "isConnecting": f.client == Client::Connecting,
            "isLoggingIn": f.client == Client::LoggingIn, "isTransitioning": matches!(f.client, Client::Connecting | Client::LoggingIn) })
}

fn down() -> (StatusCode, Json<Value>) {
    (StatusCode::from_u16(STAND_IN_DOWN).unwrap(), Json(Value::Null))
}

async fn get_server(State(f): State<Shared>) -> (StatusCode, Json<Value>) {
    let f = f.lock().unwrap();
    if f.down {
        return down();
    }
    (StatusCode::OK, Json(server_json(&f)))
}

async fn application(State(f): State<Shared>) -> (StatusCode, Json<Value>) {
    let f = f.lock().unwrap();
    if f.down {
        return down();
    }
    (StatusCode::OK, Json(json!({ "server": server_json(&f), "user": { "username": "alice" },
        "connectionWatchdog": { "isEnabled": f.enabled, "isAttemptingConnection": f.looping, "isAwaitingVpn": false } })))
}

/// PUT /server: the watchdog restarts (205) when it is on, else it starts (200); either way it connects
/// at once.
async fn connect_put(State(sh): State<Shared>) -> StatusCode {
    let mut f = sh.lock().unwrap();
    if f.down {
        return StatusCode::from_u16(STAND_IN_DOWN).unwrap();
    }
    f.puts.push(Instant::now());
    if f.enabled {
        say(&f, "[INF] (Re)connection process restarted");
        stop(&mut f);
        start(&mut f, &sh);
        return StatusCode::RESET_CONTENT;
    }
    start(&mut f, &sh);
    StatusCode::OK
}

/// DELETE /server: the watchdog stops (a wait or a connection under way is cancelled); a client that is
/// connected is disconnected "by the user".
async fn disconnect(State(sh): State<Shared>, Json(_why): Json<String>) -> StatusCode {
    let mut f = sh.lock().unwrap();
    if f.down {
        return StatusCode::from_u16(STAND_IN_DOWN).unwrap();
    }
    f.deletes.push(Instant::now());
    stop(&mut f);
    if matches!(f.client, Client::LoggingIn | Client::LoggedIn) {
        session_ends(&mut f);
        say(&f, "[INF] Disconnected from the Soulseek server: disconnected by the user");
    }
    StatusCode::NO_CONTENT
}

async fn start_search(State(f): State<Shared>, Json(b): Json<Value>) -> (StatusCode, String) {
    let mut f = f.lock().unwrap();
    if f.down {
        return (StatusCode::from_u16(STAND_IN_DOWN).unwrap(), String::new());
    }
    if f.client != Client::LoggedIn {
        return (StatusCode::CONFLICT, "The server connection must be connected and logged in to perform a search (currently: Disconnected)".into());
    }
    if f.too_many > 0 {
        f.too_many -= 1;
        f.refused.push(Instant::now());
        return (StatusCode::TOO_MANY_REQUESTS, "Only one concurrent operation is permitted. Wait until the previous request completes".into());
    }
    let id = b["id"].as_str().unwrap_or("").to_string();
    f.posts.push((Instant::now(), b["searchText"].as_str().unwrap_or("").to_string()));
    f.most = f.most.max(running(&f) + 1);
    let text = b["searchText"].as_str().unwrap_or("").to_string();
    f.searches.insert(id.clone(), Search { started: Instant::now(), cancelled: false, known: true, text });
    (StatusCode::OK, json!({ "id": id }).to_string())
}

async fn search_state(State(f): State<Shared>, Path(id): Path<String>) -> (StatusCode, Json<Value>) {
    let f = f.lock().unwrap();
    if f.down {
        return down();
    }
    match f.searches.get(&id).filter(|s| s.known) {
        None => (StatusCode::NOT_FOUND, Json(Value::Null)),
        Some(s) => {
            let done = search_done(&f, s);
            let state = if s.cancelled { "Completed, Cancelled" } else if done { "Completed, TimedOut" } else { "InProgress" };
            (StatusCode::OK, Json(json!({ "id": id, "isComplete": done, "state": state })))
        }
    }
}

/// PUT /searches/{id}: stop it.
async fn stop_search(State(f): State<Shared>, Path(id): Path<String>) -> StatusCode {
    let mut f = f.lock().unwrap();
    let Some(s) = f.searches.get_mut(&id).filter(|s| s.known) else { return StatusCode::NOT_FOUND };
    s.cancelled = true;
    f.stopped.push((Instant::now(), id));
    StatusCode::OK
}

/// DELETE /searches/{id}: forgotten – but a search that still runs goes on running (slskd 0.26).
async fn end_search(State(f): State<Shared>, Path(id): Path<String>) -> StatusCode {
    let mut f = f.lock().unwrap();
    match f.searches.get_mut(&id).filter(|s| s.known) {
        Some(s) => {
            s.known = false;
            StatusCode::NO_CONTENT
        }
        None => StatusCode::NOT_FOUND,
    }
}

async fn responses(State(f): State<Shared>, Path(id): Path<String>) -> (StatusCode, Json<Value>) {
    let f = f.lock().unwrap();
    if f.down {
        return down();
    }
    // as slskd 0.26.1: the responses are there once the search is over, none before
    match f.searches.get(&id).filter(|s| s.known) {
        Some(s) if search_done(&f, s) && !s.cancelled => (StatusCode::OK, Json(f.answers.get(&s.text).cloned().unwrap_or(json!([])))),
        Some(_) => (StatusCode::OK, Json(json!([]))),
        None => (StatusCode::NOT_FOUND, Json(Value::Null)),
    }
}

/// POST /transfers/downloads/{user}, as slskd 0.26.1's TransfersController.EnqueueAsync: two at once (its
/// DownloadRequestLimiter), a third gets 429 "Only one concurrent operation is permitted"; the limiter is
/// held while slskd asks the server for the user's address and the user's client takes the request.
async fn enqueue(State(sh): State<Shared>, Path(user): Path<String>, Json(files): Json<Value>) -> (StatusCode, String) {
    let secs = {
        let mut f = sh.lock().unwrap();
        if f.down {
            return (StatusCode::from_u16(STAND_IN_DOWN).unwrap(), String::new());
        }
        if f.enqueue_too_many > 0 || f.enqueuing >= 2 {
            f.enqueue_too_many = f.enqueue_too_many.saturating_sub(1);
            f.enqueue_refused.push(Instant::now());
            return (StatusCode::TOO_MANY_REQUESTS, "Only one concurrent operation is permitted. Wait until the previous request completes".into());
        }
        if f.enqueue_not_connected || f.client != Client::LoggedIn {
            return (StatusCode::INTERNAL_SERVER_ERROR, "The server connection must be connected and logged in to perform the operation (currently: Disconnected)".into());
        }
        if f.offline.contains(&user) {
            // slskd asks the server for the user's address first; Soulseek.NET's answer for an offline user
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("User {user} appears to be offline"));
        }
        f.enqueuing += 1;
        f.most_enqueuing = f.most_enqueuing.max(f.enqueuing);
        f.enqueue_secs
    };
    tokio::time::sleep(Duration::from_secs(secs)).await;
    let mut f = sh.lock().unwrap();
    f.enqueuing -= 1;
    f.enqueues.push((Instant::now(), user.clone()));
    let names: Vec<String> = files.as_array().unwrap().iter().map(|x| x["filename"].as_str().unwrap().to_string()).collect();
    let list = f.transfers.entry(user).or_default();
    for n in &names {
        list.push((n.clone(), "Queued, Remotely".into()));
    }
    (StatusCode::CREATED, json!({ "enqueued": names, "failed": [] }).to_string())
}

/// GET /transfers/downloads/{user}: the user's downloads, one folder.
async fn user_transfers(State(sh): State<Shared>, Path(user): Path<String>) -> (StatusCode, Json<Value>) {
    let f = sh.lock().unwrap();
    if f.down {
        return down();
    }
    let files: Vec<Value> = f.transfers.get(&user).map(|l| l.iter().enumerate().map(|(i, (n, s))| json!({ "id": format!("{user}-{i}"), "filename": n,
        "state": s, "size": 1000, "bytesTransferred": 0 })).collect()).unwrap_or_default();
    (StatusCode::OK, Json(json!({ "username": user, "directories": [{ "directory": "Music\\Album", "files": files }] })))
}

async fn cancel_transfer(State(_): State<Shared>, Path((_user, _id)): Path<(String, String)>) -> StatusCode {
    StatusCode::NO_CONTENT
}

fn fake_slskd(f: Shared) -> Router {
    Router::new()
        .route("/api/v0/server", get(get_server).put(connect_put).delete(disconnect))
        .route("/api/v0/application", get(application))
        .route("/api/v0/searches", post(start_search))
        .route("/api/v0/searches/{id}", get(search_state).put(stop_search).delete(end_search))
        .route("/api/v0/searches/{id}/responses", get(responses))
        .route("/api/v0/transfers/downloads/{user}", get(user_transfers).post(enqueue))
        .route("/api/v0/transfers/downloads/{user}/{id}", axum::routing::delete(cancel_transfer))
        .with_state(f)
}

/// A server with Soulseek switched on, its engine "installed" and running, and slskd's API answered by
/// the stand-in (logged out, logins timing out, until a test says otherwise).
fn setup(name: &str) -> (TestServer, Shared) {
    let ts = TestServer::new(&format!("soulseek-{name}"))
        .with_sources()
        .with_urls(encore_http::Urls { ytmusic: "http://127.0.0.1:9".into(), ..Default::default() })
        .with(json!({ "slskd_enabled": true, "soulseek_username": "alice", "soulseek_password": "a-password", "slskd_search_timeout": 5,
                      "album_upgrade": "off", "quality_first": false, "source_order": "slskd", "ytdlp_enabled": false, "ytdlp_last_resort": false, "auto_approve": true }));
    let tools = ts.st.data_dir().join("tools").join("slskd");
    std::fs::create_dir_all(tools.join("0.26.1")).unwrap();
    std::fs::write(tools.join("0.26.1").join("slskd"), "#!/bin/sh\n").unwrap();
    std::fs::write(tools.join("current"), "0.26.1\n").unwrap();
    crate::test_support::user(&ts.st, "boss", "admin"); // user 1: the requests' owner and the admin
    let m = &ts.st.requests.slskd;
    m.set_state_for_tests("running");
    let f = Arc::new(Mutex::new(Fake {
        login: Login::TimesOut, reached: vec![], client: Client::Disconnected, pass: None, enabled: false, looping: false, epoch: 0,
        puts: vec![], deletes: vec![], posts: vec![], refused: vec![], too_many: 0, searches: HashMap::new(), stopped: vec![], most: 0,
        search_secs: 10, down: false, session: m.session().clone(), transfers: HashMap::new(), enqueues: vec![], enqueue_refused: vec![],
        enqueue_too_many: 0, enqueuing: 0, most_enqueuing: 0, enqueue_secs: 1, enqueue_not_connected: false, answers: HashMap::new(), offline: vec![],
    }));
    m.set_target(Target::App(fake_slskd(f.clone())));
    (ts, f)
}

/// The stand-in logged in already (an attempt of Encore's let through the door), without a supervisor.
fn logged_in_already(st: &AppState, f: &Shared) {
    let s = st.requests.slskd.session();
    s.open_door();
    let mut f = f.lock().unwrap();
    f.pass = s.door();
    f.client = Client::LoggedIn;
}

/// Wait (in the runtime's time) until `cond` holds; fails after `max`.
async fn until(what: &str, max: Duration, cond: impl Fn() -> bool) {
    let t = Instant::now();
    while !cond() {
        assert!(t.elapsed() < max, "waited {max:?} for: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn state(st: &AppState) -> S {
    st.requests.slskd.session().snapshot().state
}

fn gaps(times: &[Instant]) -> Vec<f64> {
    times.windows(2).map(|w| w[1].duration_since(w[0]).as_secs_f64()).collect()
}

async fn admin_cookie(app: &Router) -> String {
    let (code, body, cookie) = call(app, "POST", "/api/auth/login", "", Some(json!({ "username": "boss", "password": PW }))).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    cookie.split(';').next().unwrap().to_string()
}

fn request(st: &AppState, title: &str, status: &str, detail: Value) -> i64 {
    let t = encore_db::now();
    st.db.execute("INSERT INTO requests (user_id, kind, artist, title, status, message, detail, created_at, updated_at) VALUES (1,'track','Artist',?,?,'',?,?,?)",
        p![title, status, detail.to_string(), t, t]).unwrap()
}

fn row(st: &AppState, id: i64) -> (String, String, String, Value, f64) {
    let r = st.db.one("SELECT status, message, source, detail, updated_at FROM requests WHERE id=?", p![id]).unwrap().unwrap();
    (r.s("status"), r.s("message"), r.s("source"), serde_json::from_str(&r.s("detail")).unwrap(), r.f("updated_at"))
}

/// The gaps between attempts that keep Encore's schedule: 30 s, 1 m, 2 m, 5 m, 10 m, then 15 m after
/// each failure (±20 %), never two attempts within 30 s; `took` is how long a failing attempt lasts.
fn assert_schedule(starts: &[Instant], took: f64) {
    let want = [30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 900.0, 900.0, 900.0];
    for (i, (gap, w)) in gaps(starts).into_iter().zip(want).enumerate() {
        let lo = (took + w * 0.8).max(30.0) - 0.5;
        let hi = took + w * 1.2 + 1.5;
        assert!(gap >= 30.0 - 0.01, "attempts {} and {} only {gap} s apart", i + 1, i + 2);
        assert!(gap >= lo && gap <= hi, "gap {} is {gap} s, want {lo}–{hi} s", i + 1);
    }
}

/// A server that never answers the login, for an hour: the only connections that reach it are Encore's
/// attempts, on Encore's schedule – although slskd starts its own loop again after every failure – and
/// slskd's loop is quieted (a DELETE while it waits) within seconds of each failure. The card says why
/// and when; Reconnect now is refused within 30 s, then connects at once; a session that lasted 10
/// minutes starts the schedule over.
#[tokio::test(start_paused = true)]
async fn only_encores_attempts_reach_the_server_on_its_schedule() {
    let (ts, f) = setup("schedule");
    let st = ts.st.clone();
    let sup = tokio::spawn(session::supervisor(st.clone()));
    tokio::time::sleep(Duration::from_secs(3600)).await;
    until("between two attempts", Duration::from_secs(10), || state(&st) == S::Waiting).await;
    let (puts, reached) = { let f = f.lock().unwrap(); (f.puts.clone(), f.reached.clone()) };
    assert_eq!(reached.len(), puts.len(), "every connection that reached the server was one of Encore's attempts");
    for (p, r) in puts.iter().zip(&reached) {
        assert!(r.duration_since(*p) < Duration::from_secs(1));
    }
    assert!((7..=9).contains(&reached.len()), "{} attempts in an hour", reached.len());
    assert_schedule(&reached, 5.0);
    assert!(!f.lock().unwrap().enabled && !f.lock().unwrap().looping, "slskd's own loop is off");
    let s = session::status(&st);
    let n = puts.len() as u64;
    assert_eq!((s["state"].as_str(), s["reason"].as_str(), s["attempts"].as_u64()), (Some("waiting"), Some("timeout"), Some(n)), "{s}");
    assert_eq!(s["server_connections"].as_u64(), Some(n), "{s}");
    let text = s["text"].as_str().unwrap();
    assert!(text.starts_with("Disconnected – the login timed out (the Soulseek server did not answer in 5 s). Next attempt at "), "{text}");
    assert!(text.ends_with(&format!("(attempt {}).", n + 1)), "{text}");

    // slskd's own loop started behind Encore's back (anything that starts it): quieted at the next look
    {
        let mut fk = f.lock().unwrap();
        start(&mut fk, &f);
    }
    until("slskd's loop stopped", Duration::from_secs(15), || { let f = f.lock().unwrap(); !f.enabled && !f.looping }).await;
    assert_eq!(f.lock().unwrap().reached.len(), puts.len(), "its attempts were turned away at the door");
    assert!(st.requests.slskd.session().door_seen().refused >= 1);

    // Reconnect now: refused within 30 s of the last attempt, then the schedule starts over at once
    let app = ts.app();
    let cookie = admin_cookie(&app).await;
    let n = f.lock().unwrap().puts.len();
    until("the next attempt over", Duration::from_secs(1200), || f.lock().unwrap().puts.len() > n && state(&st) == S::Waiting).await;
    f.lock().unwrap().login = Login::Works;
    let (code, body, _) = call(&app, "POST", "/api/system/soulseek/reconnect", &cookie, None).await;
    assert_eq!(code, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert!(body["detail"].as_str().unwrap().starts_with("Wait "), "{body}");
    tokio::time::sleep(Duration::from_secs(30)).await; // the next planned attempt is 12 minutes away at least
    let puts = f.lock().unwrap().puts.clone();
    let (code, body, _) = call(&app, "POST", "/api/system/soulseek/reconnect", &cookie, None).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    let pressed = Instant::now();
    until("logged in", Duration::from_secs(3), || state(&st) == S::LoggedIn).await;
    assert_eq!(f.lock().unwrap().puts.len(), puts.len() + 1);
    assert!(f.lock().unwrap().puts.last().unwrap().duration_since(pressed) < Duration::from_secs(1), "Reconnect now connects at once");
    let s = session::status(&st);
    assert!(s["text"].as_str().unwrap().starts_with("Logged in as alice since "), "{s}");
    let (code, body, _) = call(&app, "POST", "/api/system/soulseek/reconnect", &cookie, None).await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "nothing to reconnect: {body}");

    // logged in for 11 minutes, then the server closes the connection: back to 30 s ± 20 %
    tokio::time::sleep(Duration::from_secs(660)).await;
    let n = f.lock().unwrap().reached.len();
    server_closes(&f, "Remote connection closed");
    let dropped = Instant::now();
    until("the next attempt", Duration::from_secs(60), || f.lock().unwrap().reached.len() > n).await;
    let gap = f.lock().unwrap().reached[n].duration_since(dropped).as_secs_f64();
    assert!((24.0..=36.5).contains(&gap), "after a 10-minute session the schedule starts over: {gap} s");
    sup.abort();
    ts.remove();
}

/// The review's case: a server that doesn't even answer the connection (30 s), while something sends
/// DELETE /server every 10 s – which, while slskd is still connecting, starts its loop again at once.
/// For an hour only Encore's attempts reach the server, on its schedule.
#[tokio::test(start_paused = true)]
async fn a_delete_while_slskd_connects_never_lets_its_loop_reach_the_server() {
    let (ts, f) = setup("chaos");
    let st = ts.st.clone();
    f.lock().unwrap().login = Login::NoAnswer;
    let sup = tokio::spawn(session::supervisor(st.clone()));
    let f2 = f.clone();
    let deleter = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            let mut f = f2.lock().unwrap();
            f.deletes.push(Instant::now());
            stop(&mut f); // slskd: ConnectionWatchdog.Stop(abortReconnect: true), nothing else while connecting
        }
    });
    tokio::time::sleep(Duration::from_secs(3600)).await;
    deleter.abort();
    let (puts, reached) = { let f = f.lock().unwrap(); (f.puts.clone(), f.reached.clone()) };
    assert_eq!(reached.len(), puts.len(), "only Encore's attempts reached the server");
    assert!((6..=9).contains(&reached.len()), "{} attempts in an hour", reached.len());
    for w in reached.windows(2) {
        assert!(w[1].duration_since(w[0]) >= session::MIN_GAP, "two attempts within 30 s");
    }
    assert!(st.requests.slskd.session().door_seen().refused >= 1, "slskd's own loop came back and was turned away");
    sup.abort();
    ts.remove();
}

/// A session that drops: slskd tries again at once by itself, and is turned away at the door; a
/// "logged in" line that doesn't hold isn't believed; the next attempt is Encore's, 30 s ± 20 % later.
#[tokio::test(start_paused = true)]
async fn a_dropped_session_reconnects_on_encores_schedule_not_slskds() {
    let (ts, f) = setup("drop");
    let st = ts.st.clone();
    f.lock().unwrap().login = Login::Works;
    let sup = tokio::spawn(session::supervisor(st.clone()));
    until("logged in", Duration::from_secs(5), || state(&st) == S::LoggedIn).await;
    tokio::time::sleep(Duration::from_secs(60)).await;
    server_closes(&f, "Remote connection closed");
    let dropped = Instant::now();
    tokio::time::sleep(Duration::from_secs(5)).await;
    let s = st.requests.slskd.session();
    assert!(s.door_seen().refused >= 1, "slskd's own retry came at once and was turned away");
    assert_eq!(f.lock().unwrap().reached.len(), 1);
    assert_eq!((state(&st), s.snapshot().reason), (S::Waiting, Some(Reason::Network)));
    until("slskd's loop quieted", Duration::from_secs(10), || { let f = f.lock().unwrap(); !f.enabled && !f.looping }).await;
    // a "logged in" line from an attempt Encore didn't make (as if slskd's own retry had got through):
    // slskd's state is read before it is believed
    s.on_line("[INF] Logged in to the Soulseek server as alice");
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(state(&st), S::Waiting, "not fooled");
    until("Encore's next attempt", Duration::from_secs(60), || f.lock().unwrap().reached.len() == 2).await;
    let gap = f.lock().unwrap().reached[1].duration_since(dropped).as_secs_f64();
    assert!((24.0..=36.5).contains(&gap), "{gap} s after the drop");
    until("logged in again", Duration::from_secs(2), || state(&st) == S::LoggedIn).await;
    assert_eq!(f.lock().unwrap().puts.len(), 2);
    sup.abort();
    ts.remove();
}

/// slskd restarted while logged in (new settings, a new VPN port, or killed), whether or not the
/// supervisor sees it starting: the gate closes at once, and the next attempt comes as soon as the 30 s gap
/// allows, without a backoff and without a reason of the server's – although the session followed four
/// failures.
#[tokio::test(start_paused = true)]
async fn an_slskd_restart_while_logged_in_reconnects_without_backoff() {
    let (ts, f) = setup("restart");
    let st = ts.st.clone();
    let sup = tokio::spawn(session::supervisor(st.clone()));
    until("four failed attempts", Duration::from_secs(3600), || f.lock().unwrap().puts.len() == 4 && state(&st) == S::Waiting).await;
    f.lock().unwrap().login = Login::Works;
    until("logged in at the fifth", Duration::from_secs(400), || state(&st) == S::LoggedIn).await;
    assert_eq!(st.requests.slskd.session().snapshot().attempts, 5);
    for how in [Restart::Quick, Restart::SeenStarting, Restart::Killed] {
        tokio::time::sleep(Duration::from_secs(120)).await; // a session of minutes, not 10
        let n = f.lock().unwrap().puts.len();
        let st2 = st.clone();
        let f2 = f.clone();
        let restart = tokio::spawn(async move { restart_slskd(&st2, &f2, how).await });
        let t = Instant::now();
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(slskd::search_now(&st, "x").await.unwrap_err().is::<slskd::NotConnected>(), "the gate closes at once ({how:?})");
        assert_eq!(st.requests.slskd.session().snapshot().reason, None, "not a failure");
        restart.await.unwrap();
        until("the next attempt", Duration::from_secs(40), || f.lock().unwrap().puts.len() == n + 1).await;
        let after = f.lock().unwrap().puts[n].duration_since(t).as_secs_f64();
        assert!(after <= 31.0, "{after} s after the restart ({how:?})");
        until("logged in again", Duration::from_secs(2), || state(&st) == S::LoggedIn).await;
        assert_eq!(st.requests.slskd.session().snapshot().reason, None);
    }
    sup.abort();
    ts.remove();
}

/// A rejected login stops the loop (no attempt for hours). The owner saves the right password: the
/// attempt waits for the slskd that runs with it (the old one would answer with the old password), and
/// logs in.
#[tokio::test(start_paused = true)]
async fn a_rejected_login_stops_the_loop_and_a_new_password_is_tried_on_the_new_slskd() {
    let (ts, f) = setup("rejected");
    let st = ts.st.clone();
    f.lock().unwrap().login = Login::Rejected;
    let sup = tokio::spawn(session::supervisor(st.clone()));
    until("stopped", Duration::from_secs(5), || state(&st) == S::Stopped).await;
    let s = session::status(&st);
    assert_eq!(s["reason"], "rejected", "{s}");
    assert_eq!(s["text"], "Rejected: invalid username or password – fix them below");
    assert_eq!(s["message"], "The server rejected login attempt: INVALIDPASS", "the server's words");
    // GET /api/soulseek/status (what the phone and older apps show): the session beside the engine, and its
    // sentence as the error, pointing at the settings instead of "below"
    let app = ts.app();
    let cookie = admin_cookie(&app).await;
    let (code, body, _) = call(&app, "GET", "/api/soulseek/status", &cookie, None).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!((body["state"].as_str(), body["logged_in"].as_bool()), (Some("running"), Some(false)), "{body}");
    assert_eq!(body["session"]["reason"], "rejected", "{body}");
    assert_eq!(body["error"], "Rejected: invalid username or password – fix them in Settings → Plugins → Soulseek", "{body}");
    // a request parked before the rejection goes on at the worker's next look: not "waiting"
    request(&st, "Parked", "queued", json!({ "soulseek_wait": { "since": encore_db::now() }, "sources": ["slskd"] }));
    assert_eq!(session::status(&st)["searches_waiting"], 0);
    tokio::time::sleep(Duration::from_secs(2 * 3600)).await;
    assert_eq!(f.lock().unwrap().puts.len(), 1, "not tried again by itself");
    // a person's search says why at once
    let e = slskd::search_now(&st, "x").await.unwrap_err();
    assert!(e.to_string().contains("rejected"), "{e}");
    // the right password is saved; the slskd that runs still has the old one until it is restarted
    st.cfg.set_many(&json!({ "soulseek_password": "the-right-one" }));
    st.requests.slskd.poke();
    tokio::time::sleep(Duration::from_secs(8)).await;
    assert_eq!(f.lock().unwrap().puts.len(), 1, "no attempt on the old slskd");
    assert_eq!(state(&st), S::Disconnected, "the old answer no longer counts");
    f.lock().unwrap().login = Login::Works;
    st.requests.slskd.restarted_for_tests();
    until("tried with the new password", Duration::from_secs(5), || f.lock().unwrap().puts.len() == 2).await;
    until("logged in", Duration::from_secs(2), || state(&st) == S::LoggedIn).await;
    sup.abort();
    ts.remove();
}

/// The schedule survives a restart of Encore: the state file the last run left (an attempt 10 s ago, the
/// next one planned in 100 s) is taken up, instead of logging in at once.
#[tokio::test(start_paused = true)]
async fn the_schedule_survives_a_restart_of_encore() {
    let (ts, f) = setup("persist");
    let st = ts.st.clone();
    let now = encore_core::time::now();
    let login = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        ("alice", "a-password", std::env::var("SOULSEEK_SERVER").unwrap_or_default()).hash(&mut h);
        h.finish()
    };
    let dir = st.data_dir().join("slskd");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("encore-session.json"), json!({ "attempts": 3, "last_attempt": now - 10.0, "next_at": now + 100.0, "reason": "timeout",
        "message": "The wait timed out after 5000 milliseconds", "stopped": false, "kicked_retry": false, "login": login.to_string() }).to_string()).unwrap();
    let sup = tokio::spawn(session::supervisor(st.clone()));
    tokio::time::sleep(Duration::from_secs(5)).await;
    let s = session::status(&st);
    assert_eq!((s["state"].as_str(), s["reason"].as_str(), s["attempts"].as_u64()), (Some("waiting"), Some("timeout"), Some(3)), "{s}");
    assert!(f.lock().unwrap().puts.is_empty(), "no login at once");
    let t = Instant::now();
    until("the planned attempt", Duration::from_secs(120), || !f.lock().unwrap().puts.is_empty()).await;
    let after = t.elapsed().as_secs_f64();
    assert!((93.0..=97.0).contains(&after), "{after} s");
    sup.abort();
    ts.remove();
}

/// No search reaches slskd while logged out; once logged in, searches run one at a time (each lasts
/// 10 s), each at least 3 s after the one before, and a person's search goes before the requests already
/// waiting.
#[tokio::test(start_paused = true)]
async fn searches_wait_for_the_login_and_run_one_at_a_time_3_s_apart() {
    let (ts, f) = setup("gate");
    let st = ts.st.clone();
    let e = slskd::search(&st, "worker").await.unwrap_err();
    assert!(e.is::<slskd::NotConnected>(), "{e}");
    let e = slskd::search_now(&st, "person").await.unwrap_err();
    assert!(e.is::<slskd::NotConnected>(), "{e}");
    assert_eq!(e.to_string(), "Soulseek isn't connected – connecting shortly");
    assert!(f.lock().unwrap().posts.is_empty(), "no search while logged out");
    // logged in now: the state known a moment ago counts for 10 s, then it is read again
    logged_in_already(&st, &f);
    assert!(slskd::search(&st, "worker").await.is_err());
    tokio::time::sleep(session::FRESH).await;
    let mut tasks = vec![];
    for i in 0..4 {
        let st = st.clone();
        tasks.push(tokio::spawn(async move { slskd::search(&st, &format!("worker {i}")).await }));
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    tokio::time::sleep(Duration::from_millis(10)).await;
    let st2 = st.clone();
    let person = tokio::spawn(async move { slskd::search_now(&st2, "person").await });
    for t in tasks {
        t.await.unwrap().unwrap();
    }
    person.await.unwrap().unwrap();
    let f = f.lock().unwrap();
    let texts: Vec<&str> = f.posts.iter().map(|(_, t)| t.as_str()).collect();
    assert_eq!(texts, ["worker 0", "person", "worker 1", "worker 2", "worker 3"], "a person goes before the waiting requests");
    assert_eq!(f.most, 1, "never two searches at once");
    let times: Vec<Instant> = f.posts.iter().map(|(t, _)| *t).collect();
    for g in gaps(&times) {
        assert!(g >= 9.99, "a search started {g} s after the one before, which ran 10 s");
    }
    assert!(session::status(&st)["last_search_at"].is_number());
    drop(f);
    ts.remove();
}

/// A search whose caller goes away (the request was cancelled, the picker closed) is stopped in slskd
/// before the turn passes on: the next search never runs beside it.
#[tokio::test(start_paused = true)]
async fn a_cancelled_search_is_stopped_before_the_next_one_starts() {
    let (ts, f) = setup("cancel");
    let st = ts.st.clone();
    logged_in_already(&st, &f);
    for who in [slskd::Who::Worker, slskd::Who::Person] {
        let n = f.lock().unwrap().posts.len();
        let st2 = st.clone();
        let first = tokio::spawn(async move { slskd::search_as(&st2, "first", who, |_| {}).await });
        until("posted", Duration::from_secs(20), || f.lock().unwrap().posts.len() == n + 1).await;
        tokio::time::sleep(Duration::from_secs(3)).await;
        first.abort();
        let _ = first.await;
        slskd::search(&st, "second").await.unwrap();
        let f = f.lock().unwrap();
        assert_eq!(f.posts.len(), n + 2);
        let (stopped_at, _) = f.stopped.last().expect("the first search was stopped");
        assert!(*stopped_at <= f.posts[n + 1].0, "stopped before the next one started");
        assert_eq!(f.most, 1, "never two searches at once ({who:?})");
    }
    ts.remove();
}

/// "Too many searches" (429): the request's search waits 60 s and goes on; a person is told when the next
/// search can go instead of waiting a minute; after five pauses the request's search gives up.
#[tokio::test(start_paused = true)]
async fn too_many_searches_waits_60_s_then_goes_on() {
    let (ts, f) = setup("rate");
    let st = ts.st.clone();
    logged_in_already(&st, &f);
    f.lock().unwrap().too_many = 1;
    let st2 = st.clone();
    let worker = tokio::spawn(async move { slskd::search(&st2, "a").await });
    tokio::time::sleep(Duration::from_secs(5)).await;
    let s = session::status(&st);
    assert_eq!((s["state"].as_str(), s["reason"].as_str()), (Some("logged_in"), Some("rate_limited")), "{s}");
    assert!(s["next_search_at"].is_number() && s["text"].as_str().unwrap().contains("Rate limited by the Soulseek server – next search at"), "{s}");
    let e = slskd::search_now(&st, "b").await.unwrap_err();
    assert!(e.is::<slskd::Busy>() && e.to_string().starts_with("Rate limited by the Soulseek server – next search at "), "{e}");
    worker.await.unwrap().unwrap();
    {
        let f = f.lock().unwrap();
        assert_eq!((f.refused.len(), f.posts.len()), (1, 1));
        assert!(f.posts[0].0.duration_since(f.refused[0]) >= Duration::from_secs(60), "one 60 s wait");
    }
    // turned away again and again: five pauses, then a normal failure
    f.lock().unwrap().too_many = 10;
    let e = slskd::search(&st, "c").await.unwrap_err();
    assert!(e.to_string().contains("kept turning searches away"), "{e}");
    let f = f.lock().unwrap();
    assert_eq!(f.refused.len(), 1 + 6);
    assert!(gaps(&f.refused[1..]).iter().all(|g| *g >= 60.0));
    drop(f);
    ts.remove();
}

/// A "not connected" answer to a search (the session ended without a word in slskd's log): the caller
/// gets NotConnected, and the supervisor looks at once instead of at its next look a minute away.
#[tokio::test(start_paused = true)]
async fn a_not_connected_answer_wakes_the_supervisor() {
    let (ts, f) = setup("conflict");
    let st = ts.st.clone();
    f.lock().unwrap().login = Login::Works;
    let sup = tokio::spawn(session::supervisor(st.clone()));
    until("logged in", Duration::from_secs(5), || state(&st) == S::LoggedIn).await;
    // slskd lost the session silently (the connection through the door still looks open)
    let keep = {
        let mut f = f.lock().unwrap();
        f.client = Client::Disconnected;
        f.login = Login::TimesOut;
        f.pass.take()
    };
    let e = slskd::search(&st, "a").await.unwrap_err();
    assert!(e.is::<slskd::NotConnected>(), "{e}");
    let t = Instant::now();
    until("the supervisor handled it", Duration::from_secs(2), || state(&st) == S::Waiting).await;
    assert!(t.elapsed() < Duration::from_secs(2));
    assert!(f.lock().unwrap().posts.is_empty());
    drop(keep);
    sup.abort();
    ts.remove();
}

/// A request whose Soulseek step finds the session down waits in the queue ("Waiting for Soulseek – next
/// attempt at …"), its attempts unchanged and nothing written while nothing changes; once the session is
/// back it searches where it stopped.
#[tokio::test(start_paused = true)]
async fn a_request_waits_for_soulseek_and_searches_once_it_is_back() {
    let (ts, f) = setup("park");
    let st = ts.st.clone();
    let sup = tokio::spawn(session::supervisor(st.clone()));
    until("waiting", Duration::from_secs(10), || state(&st) == S::Waiting).await;
    let id = request(&st, "Song", "queued", json!({ "attempts": 2 }));
    encore_requests::worker::step(st.clone(), id).await;
    let (status, message, source, d, at) = row(&st, id);
    assert_eq!((status.as_str(), source.as_str()), ("waiting", "slskd"));
    assert!(message.starts_with("Waiting for Soulseek – next attempt at "), "{message}");
    assert_eq!(d["attempts"], 2);
    assert_eq!(d["sources"], json!(["slskd"]));
    assert!(d["soulseek_wait"]["since"].is_number());
    assert_eq!(d["log"].as_array().unwrap().len(), 1, "{d}");
    tokio::time::sleep(Duration::from_millis(1)).await; // (the clock time in the message follows the wall clock, not paused time)
    encore_requests::worker::step(st.clone(), id).await;
    let again = row(&st, id);
    assert_eq!((again.0.as_str(), again.1.as_str(), again.3["attempts"].as_u64()), ("waiting", message.as_str(), Some(2)));
    assert_eq!(again.4, at, "nothing written while nothing changed");
    assert_eq!(session::status(&st)["searches_waiting"], 1);
    assert!(f.lock().unwrap().posts.is_empty());

    // the session comes back at the next attempt; the request goes on where it stopped
    f.lock().unwrap().login = Login::Works;
    until("logged in", Duration::from_secs(60), || state(&st) == S::LoggedIn).await;
    encore_requests::worker::step(st.clone(), id).await;
    let (status, message, _, d, _) = row(&st, id);
    assert_eq!(f.lock().unwrap().posts.len(), 1, "it searched");
    assert_eq!(status, "failed", "{message}");
    assert!(message.starts_with("All sources exhausted (slskd: waiting for Soulseek (next attempt at "), "{message}");
    assert!(message.ends_with("; slskd: no suitable results on Soulseek)"), "{message}");
    assert_eq!(d["attempts"], 2, "waiting counted no attempt");
    sup.abort();
    ts.remove();
}

/// The session drops while the request's search runs (slskd cancels the search): the request waits for
/// Soulseek instead of taking the cut-short search for "nothing found" and moving on.
#[tokio::test(start_paused = true)]
async fn a_session_that_drops_during_the_search_parks_the_request() {
    let (ts, f) = setup("dropsearch");
    let st = ts.st.clone();
    st.cfg.set_many(&json!({ "source_order": "slskd,ytdlp", "ytdlp_enabled": true }));
    f.lock().unwrap().login = Login::Works;
    let sup = tokio::spawn(session::supervisor(st.clone()));
    until("logged in", Duration::from_secs(5), || state(&st) == S::LoggedIn).await;
    let id = request(&st, "Song", "queued", json!({ "attempts": 1 }));
    let step = tokio::spawn(encore_requests::worker::step(st.clone(), id));
    until("searching", Duration::from_secs(20), || f.lock().unwrap().posts.len() == 1).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    server_closes(&f, "Remote connection closed");
    tokio::time::timeout(Duration::from_secs(30), step).await.unwrap().unwrap();
    let (status, message, source, d, _) = row(&st, id);
    assert_eq!((status.as_str(), source.as_str()), ("waiting", "slskd"), "{message}");
    assert!(message.starts_with("Waiting for Soulseek – "), "{message}");
    assert_eq!(d["sources"], json!(["slskd", "ytdlp"]), "yt-dlp not tried");
    assert_eq!(d["attempts"], 1);
    sup.abort();
    ts.remove();
}

/// slskd stops answering (it is being restarted) while the session was logged in: requests wait for
/// Soulseek instead of skipping it, a person is told it isn't connected; once slskd answers again and is
/// still logged in, the request searches.
#[tokio::test(start_paused = true)]
async fn requests_wait_while_slskd_does_not_answer() {
    let (ts, f) = setup("silent");
    let st = ts.st.clone();
    st.cfg.set_many(&json!({ "source_order": "slskd,ytdlp", "ytdlp_enabled": true }));
    f.lock().unwrap().login = Login::Works;
    let sup = tokio::spawn(session::supervisor(st.clone()));
    until("logged in", Duration::from_secs(5), || state(&st) == S::LoggedIn).await;
    f.lock().unwrap().down = true;
    tokio::time::sleep(session::FRESH).await;
    let id = request(&st, "Song", "queued", json!({}));
    encore_requests::worker::step(st.clone(), id).await;
    let (status, message, _, d, _) = row(&st, id);
    assert_eq!(status, "waiting", "{message}");
    assert!(message.starts_with("Waiting for Soulseek – "), "{message}");
    assert_eq!(d["sources"], json!(["slskd", "ytdlp"]));
    assert!(slskd::search_now(&st, "x").await.unwrap_err().is::<slskd::NotConnected>());
    f.lock().unwrap().down = false;
    until("confirmed logged in", Duration::from_secs(5), || { let c = st.requests.slskd.session().snapshot(); c.state == S::LoggedIn && !c.lost }).await;
    encore_requests::worker::step(st.clone(), id).await;
    assert_eq!(f.lock().unwrap().posts.len(), 1, "it searched");
    sup.abort();
    ts.remove();
}

/// While the engine waits for the VPN, or was stopped and is started again, requests wait for Soulseek;
/// an engine that can't start (blocked) is skipped, as before.
#[tokio::test(start_paused = true)]
async fn requests_wait_for_the_vpn_and_skip_a_blocked_engine() {
    let (ts, _f) = setup("vpn");
    let st = ts.st.clone();
    st.cfg.set_many(&json!({ "source_order": "slskd,ytdlp", "ytdlp_enabled": true }));
    st.requests.slskd.set_state_for_tests("waiting_vpn");
    let id = request(&st, "Song", "queued", json!({}));
    encore_requests::worker::step(st.clone(), id).await;
    let (status, message, ..) = row(&st, id);
    assert_eq!((status.as_str(), message.as_str()), ("waiting", "Waiting for Soulseek – waiting for the VPN"));
    st.requests.slskd.set_state_for_tests("crashed");
    encore_requests::worker::step(st.clone(), id).await;
    let (status, message, ..) = row(&st, id);
    assert_eq!((status.as_str(), message.as_str()), ("waiting", "Waiting for Soulseek – the Soulseek engine stopped and is started again shortly"));
    st.requests.slskd.set_state_for_tests("blocked");
    let task = tokio::spawn(encore_requests::worker::step(st.clone(), id));
    until("yt-dlp tried", Duration::from_secs(5), || row(&st, id).2 == "ytdlp").await;
    task.abort();
    ts.remove();
}

/// A request that has waited an hour for Soulseek goes on down its sources (yt-dlp here), as before.
#[tokio::test(start_paused = true)]
async fn a_request_that_waited_an_hour_goes_on_to_the_next_source() {
    let (ts, _f) = setup("hour");
    let st = ts.st.clone();
    st.cfg.set_many(&json!({ "source_order": "slskd,ytdlp", "ytdlp_enabled": true }));
    let sup = tokio::spawn(session::supervisor(st.clone()));
    until("waiting", Duration::from_secs(10), || state(&st) == S::Waiting).await;
    let id = request(&st, "Song", "queued", json!({ "attempts": 3 }));
    encore_requests::worker::step(st.clone(), id).await;
    assert_eq!(row(&st, id).3["sources"], json!(["slskd", "ytdlp"]));
    st.db.execute("UPDATE requests SET detail=json_set(detail, '$.soulseek_wait.since', ?) WHERE id=?", p![encore_db::now() - 3601.0, id]).unwrap();
    let task = tokio::spawn(encore_requests::worker::step(st.clone(), id));
    until("yt-dlp tried", Duration::from_secs(5), || row(&st, id).2 == "ytdlp").await;
    task.abort();
    let (_, _, _, d, _) = row(&st, id);
    let log: Vec<&str> = d["log"].as_array().unwrap().iter().filter_map(|l| l.as_str()).collect();
    assert!(log.contains(&"slskd: Soulseek was not connected for 60 minutes – trying the next source"), "{log:?}");
    assert_eq!(d["attempts"], 3);
    sup.abort();
    ts.remove();
}

/// A Soulseek candidate: one file from `user`.
fn cand(user: &str) -> Value {
    json!({ "username": user, "directory": "Music\\Album", "files": [{ "filename": format!("Music\\Album\\01 {user}.flac"), "size": 1000 }],
            "format": "flac", "bitrate": 0, "score": 50 })
}

/// A request downloading from the first of `users` (with `rest` sources after Soulseek), whose transfer
/// slskd lists as `state`.
fn downloading(st: &AppState, f: &Shared, users: &[&str], state: &str, rest: Value) -> i64 {
    let t = encore_db::now();
    let cands: Vec<Value> = users.iter().map(|u| cand(u)).collect();
    let id = request(st, "Song", "downloading", json!({ "attempts": 1, "sources": rest,
        "slskd": { "candidates": cands, "idx": 0, "started": t - 10.0, "last_change": t - 10.0, "last_bytes": 0 } }));
    st.db.execute("UPDATE requests SET source='slskd' WHERE id=?", p![id]).unwrap();
    f.lock().unwrap().transfers.insert(users[0].into(), vec![(format!("Music\\Album\\01 {}.flac", users[0]), state.into())]);
    id
}

fn tried(d: &Value) -> Vec<String> {
    d["slskd_tried"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

/// Downloads are handed to slskd one at a time (slskd 0.26.1 takes two at once and answers 429 to a
/// third, for as long as it waits on the server and the user): two requests that enqueue together never
/// overlap at slskd, and a 429 all the same is asked again 2 s later instead of failing.
#[tokio::test(start_paused = true)]
async fn downloads_are_handed_to_slskd_one_at_a_time_and_a_429_is_asked_again() {
    let (ts, f) = setup("enqueue");
    let st = ts.st.clone();
    logged_in_already(&st, &f);
    {
        let mut f = f.lock().unwrap();
        f.enqueue_secs = 3;
        f.enqueue_too_many = 1;
    }
    let (a, b) = (st.clone(), st.clone());
    let one = tokio::spawn(async move { slskd::enqueue(&a, "u1", &[cand("u1")["files"][0].clone()]).await });
    let two = tokio::spawn(async move { slskd::enqueue(&b, "u2", &[cand("u2")["files"][0].clone()]).await });
    one.await.unwrap().unwrap();
    two.await.unwrap().unwrap();
    let f = f.lock().unwrap();
    assert_eq!((f.enqueues.len(), f.enqueue_refused.len(), f.most_enqueuing), (2, 1, 1), "one at a time; the 429 asked again");
    assert!(f.enqueues[0].0.duration_since(f.enqueue_refused[0]) >= Duration::from_secs(2 + 3), "asked again 2 s after the 429");
    drop(f);
    ts.remove();
}

/// slskd keeps answering 429 to a download: the user is not counted as failed. The request goes on
/// downloading (it says it waits for Soulseek to take the download) and asks for the same user's files
/// at its next look; once slskd takes them, the download runs.
#[tokio::test(start_paused = true)]
async fn a_download_slskd_is_too_busy_to_take_is_asked_again_not_failed() {
    let (ts, f) = setup("enqueue-busy");
    let st = ts.st.clone();
    logged_in_already(&st, &f);
    let id = downloading(&st, &f, &["u1", "u2"], "Completed, Errored", json!([]));
    f.lock().unwrap().enqueue_too_many = 100;
    encore_requests::worker::step(st.clone(), id).await;
    let (status, message, source, d, _) = row(&st, id);
    assert_eq!((status.as_str(), source.as_str()), ("downloading", "slskd"), "{message}");
    assert!(message.starts_with("Waiting for Soulseek to take the download from u2"), "{message}");
    assert_eq!(tried(&d), vec!["u1|Music\\Album".to_string()], "u1's transfer failed; u2 was not refused by u2");
    assert_eq!((d["slskd"]["idx"].as_i64(), d["slskd"]["pending"].is_object()), (Some(1), true));
    assert_eq!(f.lock().unwrap().enqueue_refused.len(), 1 + slskd::ENQUEUE_TRIES as usize);
    // slskd takes it at the next look
    f.lock().unwrap().enqueue_too_many = 0;
    encore_requests::worker::step(st.clone(), id).await;
    let (status, message, _, d, _) = row(&st, id);
    assert_eq!(status, "downloading");
    assert!(message.starts_with("Downloading 1 file(s) from u2"), "{message}");
    assert_eq!(tried(&d), vec!["u1|Music\\Album".to_string()]);
    assert!(!d["slskd"]["pending"].is_object());
    assert_eq!(f.lock().unwrap().enqueues.iter().map(|(_, u)| u.as_str()).collect::<Vec<_>>(), vec!["u2"]);
    ts.remove();
}

/// The session drops while a Soulseek download runs (Soulseek.NET ends the transfers with it): the
/// request waits for Soulseek instead of going on to the next user or source, the user whose transfer
/// ended is not counted as failed, and nothing is logged as a failed transfer. Also when the worker only
/// learns it from slskd's answer to the next user's download.
#[tokio::test(start_paused = true)]
async fn a_transfer_that_fails_while_disconnected_waits_for_soulseek() {
    let (ts, f) = setup("dropdownload");
    let st = ts.st.clone();
    let sup = tokio::spawn(session::supervisor(st.clone()));
    until("waiting", Duration::from_secs(10), || state(&st) == S::Waiting).await;
    let id = downloading(&st, &f, &["u1", "u2"], "Completed, Errored", json!(["ytdlp"]));
    encore_requests::worker::step(st.clone(), id).await;
    let (status, message, source, d, _) = row(&st, id);
    assert_eq!((status.as_str(), source.as_str()), ("waiting", "slskd"), "{message}");
    assert!(message.starts_with("Waiting for Soulseek – next attempt at "), "{message}");
    assert_eq!(d["sources"], json!(["slskd", "ytdlp"]), "yt-dlp not tried");
    assert!(tried(&d).is_empty(), "u1 not counted as failed: {d}");
    assert_eq!(d["attempts"], 1);
    assert!(f.lock().unwrap().enqueues.is_empty());
    let warned = st.db.one("SELECT COUNT(*) n FROM activity WHERE message LIKE 'Soulseek transfer from%'", p![]).unwrap().unwrap().i("n");
    assert_eq!(warned, 0);
    sup.abort();
    ts.remove();

    // the worker still believes the session is up; slskd's answer to u2's download says it is gone
    let (ts, f) = setup("dropdownload2");
    let st = ts.st.clone();
    logged_in_already(&st, &f);
    f.lock().unwrap().enqueue_not_connected = true;
    let id = downloading(&st, &f, &["u1", "u2"], "Completed, Errored", json!(["ytdlp"]));
    encore_requests::worker::step(st.clone(), id).await;
    let (status, message, source, d, _) = row(&st, id);
    assert_eq!((status.as_str(), source.as_str()), ("waiting", "slskd"), "{message}");
    assert!(message.starts_with("Waiting for Soulseek – "), "{message}");
    assert!(tried(&d).is_empty(), "u1 not counted as failed: {d}");
    assert_eq!(d["sources"], json!(["slskd", "ytdlp"]));
    assert!(st.requests.slskd.session().snapshot().lost, "the supervisor looks at once");
    ts.remove();
}

/// A file picked by hand on the Soulseek page or in the picker waits for the session like any Soulseek
/// step (instead of failing at once with "All sources exhausted"), and is asked for once it is back.
#[tokio::test(start_paused = true)]
async fn a_picked_soulseek_file_waits_for_the_session() {
    let (ts, f) = setup("pickwait");
    let st = ts.st.clone();
    let sup = tokio::spawn(session::supervisor(st.clone()));
    until("waiting", Duration::from_secs(10), || state(&st) == S::Waiting).await;
    let id = request(&st, "Song", "queued", json!({ "pick": { "source": "slskd", "payload": cand("u1"), "title": "01 u1.flac" } }));
    encore_requests::worker::step(st.clone(), id).await;
    let (status, message, source, d, _) = row(&st, id);
    assert_eq!((status.as_str(), source.as_str()), ("waiting", "slskd"), "{message}");
    assert!(message.starts_with("Waiting for Soulseek – next attempt at "), "{message}");
    assert!(d["pick"].is_object() && d["soulseek_wait"]["since"].is_number(), "{d}");
    f.lock().unwrap().login = Login::Works;
    until("logged in", Duration::from_secs(60), || state(&st) == S::LoggedIn).await;
    encore_requests::worker::step(st.clone(), id).await;
    let (status, message, _, d, _) = row(&st, id);
    assert_eq!(status, "downloading", "{message}");
    assert!(message.starts_with("Downloading 1 file(s) from u1"), "{message}");
    assert!(!d["soulseek_wait"].is_object(), "{d}");
    assert!(d["log"].as_array().unwrap().iter().any(|l| l.as_str().unwrap_or("").starts_with("slskd: waiting for Soulseek")), "{d}");
    assert_eq!(f.lock().unwrap().enqueues.len(), 1);
    sup.abort();
    ts.remove();
}

/// A request's search that sits out the server's 60 s pause says so on the request ("Rate limited by the
/// Soulseek server – searching again at …") instead of "Searching Soulseek…" all along.
#[tokio::test(start_paused = true)]
async fn a_request_says_when_its_search_sits_out_a_rate_limit() {
    let (ts, f) = setup("ratenote");
    let st = ts.st.clone();
    logged_in_already(&st, &f);
    f.lock().unwrap().too_many = 1;
    let id = request(&st, "Song", "queued", json!({}));
    let step = tokio::spawn(encore_requests::worker::step(st.clone(), id));
    tokio::time::sleep(Duration::from_secs(5)).await;
    let (status, message, ..) = row(&st, id);
    assert_eq!(status, "searching");
    assert!(message.starts_with("Rate limited by the Soulseek server – searching again at "), "{message}");
    tokio::time::sleep(Duration::from_secs(57)).await;
    let (_, message, ..) = row(&st, id);
    assert_eq!(message, "Searching Soulseek…");
    tokio::time::timeout(Duration::from_secs(60), step).await.unwrap().unwrap();
    assert_eq!(f.lock().unwrap().posts.len(), 1);
    ts.remove();
}

/// A person's search never waits more than 30 s in all, and is told why: behind a request's search that
/// sits out the server's pause it is refused at once with the time of the next search; when the pause
/// begins while it waits, it gets that answer (not "busy") after 30 s.
#[tokio::test(start_paused = true)]
async fn a_persons_search_is_told_about_a_rate_limit_at_once() {
    let (ts, f) = setup("personrate");
    let st = ts.st.clone();
    logged_in_already(&st, &f);
    f.lock().unwrap().too_many = 1;
    let st2 = st.clone();
    let worker = tokio::spawn(async move { slskd::search(&st2, "a").await });
    tokio::time::sleep(Duration::from_secs(45)).await; // the pause ends in 15 s, the person would wait for it
    let t = Instant::now();
    let e = slskd::search_now(&st, "b").await.unwrap_err();
    assert!(e.is::<slskd::Busy>() && e.to_string().starts_with("Rate limited by the Soulseek server – next search at "), "{e}");
    assert!(t.elapsed() < Duration::from_secs(1), "at once");
    worker.await.unwrap().unwrap();

    // the pause begins while the person waits for the turn
    let (ts2, f2) = setup("personrate2");
    let st = ts2.st.clone();
    logged_in_already(&st, &f2);
    f2.lock().unwrap().search_secs = 1;
    let st2 = st.clone();
    let first = tokio::spawn(async move { slskd::search(&st2, "first").await });
    until("the first search", Duration::from_secs(5), || f2.lock().unwrap().posts.len() == 1).await;
    f2.lock().unwrap().too_many = 1;
    let st3 = st.clone();
    let second = tokio::spawn(async move { slskd::search(&st3, "second").await });
    until("the second search holds the turn", Duration::from_secs(5), || st.requests.slskd.session().searching() && first.is_finished()).await;
    let t = Instant::now();
    let e = slskd::search_now(&st, "person").await.unwrap_err();
    assert!(e.to_string().starts_with("Rate limited by the Soulseek server – next search at "), "{e}");
    assert!(t.elapsed() <= session::PERSON_WAIT, "{:?}", t.elapsed());
    second.await.unwrap().unwrap();
    ts.remove();
    ts2.remove();
}

/// Retry failed on 200 requests only queues them; the worker then searches Soulseek one at a time (each
/// search runs 10 s), at least 3 s apart – no storm of searches.
#[tokio::test(start_paused = true)]
async fn retry_failed_of_200_requests_searches_one_at_a_time() {
    let (ts, f) = setup("bulk");
    let st = ts.st.clone();
    logged_in_already(&st, &f);
    let app = ts.app();
    let cookie = admin_cookie(&app).await;
    for i in 0..200 {
        request(&st, &format!("Song {i}"), "failed", json!({}));
    }
    let (code, body, _) = call(&app, "POST", "/api/requests/retry-failed", &cookie, Some(json!({}))).await;
    assert_eq!((code, body["retried"].as_u64()), (StatusCode::OK, Some(200)), "{body}");
    assert!(f.lock().unwrap().posts.is_empty(), "retrying only queues them");
    let worker = tokio::spawn(encore_requests::worker::run(st.clone()));
    let active = || st.db.one("SELECT COUNT(*) n FROM requests WHERE status IN ('queued','searching','downloading','monitoring','importing','waiting')", p![]).unwrap().unwrap().i("n");
    until("all 200 searched", Duration::from_secs(8 * 3600), || active() == 0).await;
    worker.abort();
    let f = f.lock().unwrap();
    assert_eq!(f.posts.len(), 200);
    assert_eq!(f.most, 1, "never two searches at once");
    let times: Vec<Instant> = f.posts.iter().map(|(t, _)| *t).collect();
    assert!(gaps(&times).iter().all(|g| *g >= 3.0), "3 s between searches");
    let failed = st.db.one("SELECT COUNT(*) n FROM requests WHERE status='failed' AND message='All sources exhausted (slskd: no suitable results on Soulseek)'", p![]).unwrap().unwrap().i("n");
    assert_eq!(failed, 200);
    drop(f);
    ts.remove();
}

/// The same stand-in on a real port (slskd's API as Encore reaches it: HTTP on loopback, with its key),
/// in real time: a search while logged out never reaches it, one while logged in does, a "not connected"
/// answer comes back as NotConnected, and Encore's DELETE /server carries the JSON string slskd wants.
#[tokio::test]
async fn the_gate_works_over_http_on_a_port() {
    let (ts, f) = setup("http");
    let st = ts.st.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = fake_slskd(f.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    st.requests.slskd.set_target(Target::Url(format!("http://{addr}")));
    let e = slskd::search_now(&st, "a").await.unwrap_err();
    assert!(e.is::<slskd::NotConnected>(), "{e}");
    assert!(f.lock().unwrap().posts.is_empty());
    // a fresh state (the stand-in logged in before anything was read): the search goes through
    let (ts2, f2) = setup("http2");
    let st2 = ts2.st.clone();
    logged_in_already(&st2, &f2);
    f2.lock().unwrap().search_secs = 1;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr2 = listener.local_addr().unwrap();
    let app2 = fake_slskd(f2.clone());
    let server2 = tokio::spawn(async move { axum::serve(listener, app2).await.unwrap() });
    st2.requests.slskd.set_target(Target::Url(format!("http://{addr2}")));
    assert_eq!(slskd::search_now(&st2, "b").await.unwrap(), Vec::<Value>::new());
    assert_eq!(f2.lock().unwrap().posts.len(), 1);
    assert!(f2.lock().unwrap().searches.values().all(|s| !s.known), "the search was forgotten");
    // the session ends behind the gate's back: the next search finds out from slskd's 409
    f2.lock().unwrap().client = Client::Disconnected;
    let e = slskd::search_now(&st2, "c").await.unwrap_err();
    assert!(e.is::<slskd::NotConnected>(), "{e}");
    assert_eq!(f2.lock().unwrap().posts.len(), 1);
    assert!(st2.requests.slskd.session().snapshot().lost, "marked down");
    // the supervisor handles it; slskd's own loop waiting between two attempts is stopped with DELETE
    // /server and its JSON body
    let sup = tokio::spawn(session::supervisor(st2.clone()));
    until("the lost session handled", Duration::from_secs(10), || state(&st2) == S::Waiting).await;
    f2.lock().unwrap().enabled = true;
    st2.requests.slskd.session().on_line("[INF] Waiting about 4 seconds before attempting to reconnect");
    until("slskd's loop stopped", Duration::from_secs(10), || !f2.lock().unwrap().deletes.is_empty()).await;
    assert!(!f2.lock().unwrap().enabled);
    sup.abort();
    server.abort();
    server2.abort();
    ts.remove();
    ts2.remove();
}

/// The real thing, behind the door: SLSKD_BIN=<slskd 0.26.1's binary> cargo test -p encore-api --
/// --ignored real_slskd_behind_the_door --nocapture. Encore's own supervisors run the real slskd against
/// a fake Soulseek server on loopback (SOULSEEK_SERVER points at it, so slskd reaches it through Encore's
/// relay): logins that time out, then one that works and a session the server ends. Only Encore's
/// attempts may reach the server, on Encore's schedule; slskd's own (it starts again at once after each
/// failure and each dropped session) must not. Takes about four minutes.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn real_slskd_behind_the_door() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let bin = std::path::PathBuf::from(std::env::var("SLSKD_BIN").expect("SLSKD_BIN"));
    // the fake Soulseek server: logs each connection; answers the login only when `works` is set
    let server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = server.local_addr().unwrap().port();
    let conns: Arc<Mutex<Vec<Instant>>> = Default::default();
    let works = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let kick = Arc::new(tokio::sync::Notify::new());
    {
        let (conns, works, kick) = (conns.clone(), works.clone(), kick.clone());
        tokio::spawn(async move {
            loop {
                let (mut s, _) = server.accept().await.unwrap();
                conns.lock().unwrap().push(Instant::now());
                println!("server: connection #{}", conns.lock().unwrap().len());
                let (works, kick) = (works.load(std::sync::atomic::Ordering::SeqCst), kick.clone());
                tokio::spawn(async move {
                    let mut b = [0u8; 4096];
                    let _ = s.read(&mut b).await;
                    if works {
                        let str_ = |t: &[u8]| [&(t.len() as u32).to_le_bytes()[..], t].concat();
                        let body = [&1u32.to_le_bytes()[..], &[1], &str_(b"welcome"), &[1, 0, 0, 127], &str_(b"hash"), &[0]].concat();
                        let _ = s.write_all(&[&(body.len() as u32).to_le_bytes()[..], &body].concat()).await;
                        tokio::select! {
                            _ = kick.notified() => {}
                            _ = async { while s.read(&mut b).await.map(|n| n > 0).unwrap_or(false) {} } => {}
                        }
                    } else {
                        while s.read(&mut b).await.map(|n| n > 0).unwrap_or(false) {}
                    }
                });
            }
        });
    }
    // SAFETY: this test runs alone (ignored, started by name); nothing else reads the environment meanwhile
    unsafe { std::env::set_var("SOULSEEK_SERVER", format!("127.0.0.1:{port}")) };
    let ts = TestServer::new("soulseek-real").with_sources();
    let st = ts.st.clone();
    let dl = st.data_dir().join("dl");
    std::fs::create_dir_all(&dl).unwrap();
    st.cfg.set_many(&json!({ "slskd_enabled": true, "soulseek_username": "encoretest", "soulseek_password": "a-password",
        "slskd_download_path": dl.to_string_lossy(), "soulseek_port": 42347, "vpn_enabled": false }));
    let tools = st.data_dir().join("tools").join("slskd");
    std::fs::create_dir_all(tools.join("0.26.1")).unwrap();
    std::os::unix::fs::symlink(&bin, tools.join("0.26.1").join("slskd")).unwrap();
    // slskd looks for its (empty) web UI folder beside its real binary
    std::fs::create_dir_all(bin.parent().unwrap().join("no-web-ui")).unwrap();
    std::fs::write(tools.join("current"), "0.26.1\n").unwrap();
    let runner = tokio::spawn(encore_requests::slskd_managed::supervisor(st.clone()));
    let sup = tokio::spawn(session::supervisor(st.clone()));
    let t0 = Instant::now();
    for _ in 0..90 {
        if st.requests.slskd.state() == "running" {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert_eq!(st.requests.slskd.state(), "running", "{}\n{:#?}", st.requests.slskd.status(), st.requests.slskd.log_tail());
    println!("slskd running after {:?}", t0.elapsed());
    let s = st.requests.slskd.session().clone();
    let attempts = || s.snapshot().attempts as usize;
    // three attempts that time out: at once, ~35 s and ~100 s later
    until("three attempts", Duration::from_secs(150), || conns.lock().unwrap().len() >= 3 && attempts() >= 3).await;
    tokio::time::sleep(Duration::from_secs(10)).await;
    let c = conns.lock().unwrap().clone();
    println!("connections: {:?}; door: {:?}; session: {}", gaps(&c), s.door_seen(), session::status(&st)["text"]);
    assert_eq!(c.len(), attempts(), "every connection that reached the server was one of Encore's attempts");
    assert_eq!(s.door_seen().passed as usize, c.len(), "each carried slskd's login (none wasted while slskd was starting)");
    assert_schedule(&c, 5.0);
    // a login that works, then the server ends the session: slskd's own retry is turned away
    works.store(true, std::sync::atomic::Ordering::SeqCst);
    until("Reconnect now allowed", Duration::from_secs(40), || s.reconnect_now().is_ok()).await;
    until("logged in", Duration::from_secs(20), || state(&st) == S::LoggedIn).await;
    let n = conns.lock().unwrap().len();
    tokio::time::sleep(Duration::from_secs(5)).await;
    kick.notify_waiters();
    let dropped = Instant::now();
    until("the drop seen", Duration::from_secs(10), || state(&st) == S::Waiting).await;
    tokio::time::sleep(Duration::from_secs(15)).await;
    assert_eq!(conns.lock().unwrap().len(), n, "slskd's own retries didn't reach the server");
    println!("after the drop: door {:?}; log tail: {:#?}", s.door_seen(), st.requests.slskd.log_tail());
    until("Encore's next attempt", Duration::from_secs(30), || conns.lock().unwrap().len() > n).await;
    let gap = dropped.elapsed().as_secs_f64();
    println!("next attempt {gap:.1} s after the drop");
    assert!((24.0..=37.0).contains(&gap), "{gap}");
    until("logged in again", Duration::from_secs(20), || state(&st) == S::LoggedIn).await;
    sup.abort();
    runner.abort();
    encore_requests::slskd_managed::stop(&st).await;
    ts.remove();
}

/// Soulseek results kept for the picker, against this stand-in.
#[path = "soulseek_cache.rs"]
mod kept;
