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
use std::sync::Mutex;
use std::time::Duration;

static CURRENT: Mutex<Option<(String, Client)>> = Mutex::new(None);

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

fn settings(st: &AppState) -> Settings {
    Settings {
        username: st.cfg.str("soulseek_username").trim().to_string(),
        password: st.cfg.str("soulseek_password"),
        server: std::env::var("SOULSEEK_SERVER").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| client::DEFAULT_SERVER.to_string()),
        listen_port: listen_port(st),
        download_dir: Path::new(&st.cfg.str("slskd_download_path")).to_path_buf(),
        upload_slots: st.cfg.int("soulseek_upload_slots").clamp(1, 20) as usize,
        share: st.cfg.bool("soulseek_share"),
    }
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
    st.cfg.int("soulseek_port").clamp(1, 65535) as u16
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
    let want = wanted(st) && configured(st);
    let key = if want { settings(st).key() } else { String::new() };
    let mut cur = CURRENT.lock().unwrap();
    if cur.as_ref().map(|(k, _)| k == &key).unwrap_or(!want) {
        return;
    }
    if let Some((_, old)) = cur.take() {
        old.stop();
    }
    if want {
        let s = settings(st);
        tracing::info!("soulseek: starting the built-in client as {} (port {})", s.username, s.listen_port);
        *cur = Some((key, Client::start(s, build_shares(st))));
    }
}

/// Keeps the client in step with the settings and the library.
pub async fn supervisor(st: AppState) {
    let mut tick: u64 = 0;
    loop {
        ensure(&st);
        if tick % 40 == 0 {
            if let Some(c) = client() {
                let st2 = st.clone();
                if let Ok(sh) = tokio::task::spawn_blocking(move || build_shares(&st2)).await {
                    c.set_shares(sh);
                }
            }
        }
        tick += 1;
        tokio::time::sleep(Duration::from_secs(15)).await;
    }
}

/// Status for diagnostics and the settings test button.
pub fn status(st: &AppState) -> Value {
    match client() {
        Some(c) => json!({ "mode": "builtin", "running": true, "client": c.status(), "downloads": c.downloads(), "uploads": c.uploads() }),
        None => json!({ "mode": mode(st), "running": false, "configured": configured(st) }),
    }
}
