//! Encore server: builds the state, starts the background work and serves the API and the web app.
//! Everything else lives in the crates under crates/: this file only wires them together.
use encore_api::state::AppState;
use encore_api::{app, VERSION};
use encore_db::{backup, now};
use encore_jobs::supervisor::{self, looping, with_stop};
use encore_jobs::Supervisor;
use std::net::SocketAddr;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // `encore --version`: the version and nothing else (no database, no web server)
    if matches!(std::env::args().nth(1).as_deref(), Some("--version" | "-V")) {
        println!("encore {VERSION}");
        return Ok(());
    }
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into())).init();
    // `encore self-update-helper …`: the container swap of "Update now" (no database, no web server)
    if let Some(code) = encore_system::selfupdate::main_if_helper().await {
        std::process::exit(code);
    }
    let st = match encore_api::build_state() {
        Ok(st) => st,
        Err(e) => {
            if let Some(n) = e.downcast_ref::<encore_db::NewerDatabase>() {
                // an older image was started against a newer database: say so plainly and leave it alone
                tracing::error!("{n}");
                eprintln!("\n{n}\n");
                std::process::exit(encore_db::EXIT_NEWER_DB);
            }
            return Err(e);
        }
    };
    // only the newest backups are kept: an older server's pile goes at the first start
    backup::prune(st.data_dir());
    // a copy of the database at least once a day, even when the server is restarted all the time
    if backup::newest_copy(st.data_dir()).map(|b| now() - b.time > 86400.0).unwrap_or(true) {
        match st.db.backup(backup::Kind::Scheduled) {
            Ok(b) => { st.db.log("info", &format!("Database backed up to backups/{}", b.name)); backup::prune(st.data_dir()); }
            Err(e) => tracing::warn!("startup backup failed: {e}"),
        }
    }
    let port: u16 = std::env::var("ENCORE_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8080);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("Encore {VERSION} listening on {addr}");
    // the builtin plugins' adapters, and on the first start after an upgrade what was in use adopted (its
    // missing programs installed in the background); kept for as long as the server runs
    let _plugins = encore_api::start_plugins(&st);
    start_library(&st);
    // the artist index, the tools' versions and Home's new releases, made before anyone asks
    encore_api::warm_up(&st);
    let loops = background(&st);
    supervisor::start(&loops);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    // until "Restart server": then a graceful stop (the loops' helper programs too) and exit code 75
    encore_system::update::serve(listener, app(st.clone()), st, loops).await?;
    Ok(())
}

/// The library at start: cover thumbnails and colours for albums that have none yet (a new install, a
/// cleared cache, an update that added sizes) at once, then a scan of every library folder. Later scans come
/// from the scheduler.
fn start_library(st: &AppState) {
    st.library.warm_covers(st.db.clone());
    let st = st.clone();
    tokio::spawn(async move { st.library.scan_library(st.db.clone(), &st.cfg, false).await });
}

/// The loops that run beside the web server, each from the crate it belongs to. Before a restart the
/// helper programs they run (slskd, the Translator's model server) are stopped in this order.
fn background(st: &AppState) -> Vec<Arc<dyn Supervisor>> {
    let s = st.clone();
    vec![
        looping("request worker", { let st = s.clone(); move || encore_requests::worker::run(st.clone()) }),
        looping("scheduler", { let st = s.clone(); move || encore_jobs::scheduler::run(st.clone()) }),
        looping("artist aliases", { let st = s.clone(); move || encore_library::artists::alias_worker(st.clone()) }),
        st.vpn.supervisor(st.cfg.clone()),
        with_stop("slskd", { let st = s.clone(); move || encore_requests::slskd_managed::supervisor(st.clone()) }, {
            let st = s.clone();
            move || { let st = st.clone(); async move { encore_requests::slskd_managed::stop(&st).await } }
        }),
        // the Soulseek server session: Encore's reconnect schedule (slskd's own loop is kept off)
        looping("soulseek session", { let st = s.clone(); move || encore_requests::slskd_session::supervisor(st.clone()) }),
        looping("roon", { let st = s.clone(); move || encore_playback::roon::supervisor(st.clone()) }),
        // group listens: the end of a song, members gone 2 minutes, idle groups; saved before a restart
        with_stop("group listens", { let st = s.clone(); move || encore_api::routes::group::run(st.clone()) }, {
            let st = s.clone();
            move || encore_api::routes::group::flush(st.clone())
        }),
        looping("contributions", { let st = s.clone(); move || encore_library::contribute::worker(st.clone()) }),
        looping("youtube sign-in", { let st = s.clone(); move || encore_requests::ytauth::supervisor(st.clone()) }),
        with_stop("translator", { let st = s.clone(); move || encore_library::translate::worker(st.clone()) }, {
            let st = s.clone();
            move || { let st = st.clone(); async move { encore_library::translate::llama::stop(&st).await } }
        }),
        // plugin programs run only while a command runs: before a restart the running ones are stopped
        // (SIGTERM to their processes, SIGKILL to their groups 10 seconds later)
        with_stop("plugins", std::future::pending::<()>, {
            let st = s;
            move || { let st = st.clone(); async move { st.plugins.stop_all().await } }
        }),
    ]
}
