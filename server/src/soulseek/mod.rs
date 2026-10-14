//! Built-in Soulseek client: the protocol, what we share, and the connection runtime. Encore talks
//! to it through `sources::slskd`, which picks this or an external slskd by the `soulseek_client`
//! setting, so the worker, picker and diagnostics don't care which one is behind them.
pub mod client;
pub mod proto;
pub mod shares;
#[cfg(test)]
mod tests;

use crate::db::RowExt;
use crate::p;
use crate::state::AppState;
use client::{Client, Settings};
use serde_json::{json, Value};
use shares::{Item, Shares};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

static CURRENT: Mutex<Option<(String, Client)>> = Mutex::new(None);
/// The VPN is on with its kill switch but not connected, so the client is held off.
static WAITING: AtomicBool = AtomicBool::new(false);

pub const WAITING_FOR_VPN: &str = "Waiting for the VPN – Soulseek stays off until it is connected";

/// Which client the settings ask for: "builtin" or "slskd". "auto" means built-in as soon as a
/// Soulseek username is set, slskd otherwise (so existing slskd setups keep working untouched).
pub fn mode(st: &AppState) -> &'static str {
    match st.cfg.str("soulseek_client").as_str() {
        "builtin" => "builtin",
        "slskd" => "slskd",
        _ => if st.cfg.str("soulseek_username").trim().is_empty() { "slskd" } else { "builtin" },
    }
}

pub fn wanted(st: &AppState) -> bool {
    mode(st) == "builtin"
}

/// Enough settings to log in.
pub fn configured(st: &AppState) -> bool {
    st.cfg.bool("slskd_enabled") && !st.cfg.str("soulseek_username").trim().is_empty() && !st.cfg.str("soulseek_password").is_empty()
}

/// How the client reaches the network right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Route {
    /// The normal connection.
    Direct,
    /// Only through the built-in VPN, listening on its forwarded port when it has one.
    Vpn(Option<u16>),
    /// The VPN is on but not connected and the kill switch is set: no client at all.
    Wait,
}

/// `vpn_on` is the setting; `usable` means the config parses; `active` a recent handshake.
fn route_for(vpn_on: bool, usable: bool, active: bool, kill_switch: bool, forwarded: Option<u16>) -> Route {
    if !vpn_on {
        Route::Direct
    } else if usable && active {
        Route::Vpn(forwarded)
    } else if kill_switch {
        Route::Wait
    } else {
        Route::Direct
    }
}

fn route(st: &AppState) -> Route {
    let on = st.cfg.bool("vpn_enabled");
    if !on {
        return Route::Direct;
    }
    let usable = crate::vpn::enabled(st);
    route_for(on, usable, usable && crate::vpn::active(), st.cfg.bool("vpn_kill_switch"), crate::vpn::forwarded_port())
}

fn settings(st: &AppState, route: Route) -> Settings {
    let base = Settings {
        username: st.cfg.str("soulseek_username").trim().to_string(),
        password: st.cfg.str("soulseek_password"),
        server: std::env::var("SOULSEEK_SERVER").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| client::DEFAULT_SERVER.to_string()),
        listen_port: listen_port(st),
        download_dir: Path::new(&st.cfg.str("slskd_download_path")).to_path_buf(),
        upload_slots: st.cfg.int("soulseek_upload_slots").clamp(1, 20) as usize,
        share: st.cfg.bool("soulseek_share"),
        vpn: false,
    };
    with_route(base, route, configured_port(st))
}

/// On the VPN the listen port is its forwarded one (the port file is a gluetun thing and ignored),
/// else the configured port. Both are in the settings key, so a new forwarded port restarts the client.
fn with_route(mut s: Settings, route: Route, configured: u16) -> Settings {
    if let Route::Vpn(fwd) = route {
        s.vpn = true;
        s.listen_port = fwd.filter(|p| *p > 0).unwrap_or(configured);
    }
    s
}

fn configured_port(st: &AppState) -> u16 {
    st.cfg.int("soulseek_port").clamp(1, 65535) as u16
}

/// The port we listen on: the configured one, or the one a VPN wrote to a file (gluetun's
/// forwarded_port) – that file is re-read every supervisor tick, so a changed port restarts the client.
fn listen_port(st: &AppState) -> u16 {
    let file = st.cfg.str("soulseek_port_file");
    if !file.trim().is_empty() {
        if let Some(p) = std::fs::read_to_string(file.trim()).ok().and_then(|s| s.trim().parse::<u16>().ok()).filter(|p| *p > 0) {
            return p;
        }
    }
    configured_port(st)
}

/// The library as a share list, straight from the tracks table.
pub fn build_shares(st: &AppState) -> Shares {
    let rows = st.db.query("SELECT path, size, bitrate, duration FROM tracks", p![]).unwrap_or_default();
    let items = rows.into_iter().map(|r| Item { path: r.s("path"), size: r.i("size").max(0) as u64, bitrate: r.i("bitrate").max(0) as u32, duration: r.f("duration").max(0.0) as u32 });
    Shares::build(Path::new(&st.cfg.str("library_path")), items)
}

pub fn client() -> Option<Client> {
    CURRENT.lock().unwrap().as_ref().map(|(_, c)| c.clone())
}

/// Start, restart or stop the client so it matches the settings. Cheap when nothing changed.
pub fn ensure(st: &AppState) {
    let mut want = wanted(st) && configured(st);
    let route = if want { route(st) } else { Route::Direct };
    let was_waiting = WAITING.swap(want && route == Route::Wait, Ordering::Relaxed);
    if want && route == Route::Wait {
        if !was_waiting {
            tracing::info!("soulseek: {WAITING_FOR_VPN} (the VPN kill switch is on)");
        }
        want = false;
    }
    let s = if want { Some(settings(st, route)) } else { None };
    let key = s.as_ref().map(|s| s.key()).unwrap_or_default();
    let mut cur = CURRENT.lock().unwrap();
    if cur.as_ref().map(|(k, _)| k == &key).unwrap_or(!want) {
        return;
    }
    if let Some((_, old)) = cur.take() {
        old.stop();
    }
    if let Some(s) = s {
        tracing::info!("soulseek: starting the built-in client as {} (port {}{})", s.username, s.listen_port, if s.vpn { ", through the VPN" } else { "" });
        *cur = Some((key, Client::start(s, build_shares(st))));
    }
}

/// Keeps the client in step with the settings and the library.
pub async fn supervisor(st: AppState) {
    let mut shared: Option<Instant> = None;
    loop {
        ensure(&st);
        if shared.map(|t| t.elapsed() >= Duration::from_secs(600)).unwrap_or(true) {
            shared = Some(Instant::now());
            if let Some(c) = client() {
                let st2 = st.clone();
                if let Ok(sh) = tokio::task::spawn_blocking(move || build_shares(&st2)).await {
                    c.set_shares(sh);
                }
            }
        }
        // on the VPN follow its state closely: it coming up, going down or a new forwarded port
        let wait = if st.cfg.bool("vpn_enabled") { 5 } else { 15 };
        tokio::time::sleep(Duration::from_secs(wait)).await;
    }
}

/// Status for diagnostics and the settings test button.
pub fn status(st: &AppState) -> Value {
    match client() {
        Some(c) => json!({ "mode": "builtin", "running": true, "client": c.status(), "downloads": c.downloads(), "uploads": c.uploads() }),
        None if WAITING.load(Ordering::Relaxed) && wanted(st) => json!({ "mode": "builtin", "running": false, "configured": configured(st), "waiting_for_vpn": true, "error": WAITING_FOR_VPN }),
        None => json!({ "mode": mode(st), "running": false, "configured": configured(st) }),
    }
}
