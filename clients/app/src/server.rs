//! Talking to the Encore server: is it one, can we reach it, which pages may the window show,
//! and the watchdog that notices when the server page never comes up.

use crate::{config, State, VERSION};
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Manager, Runtime, Url};

/// Our own pages live here (tauri://localhost on Linux/macOS, http://tauri.localhost on Windows/Android).
pub fn app_url(page: &str) -> Url {
    let base = if cfg!(any(windows, target_os = "android")) { "http://tauri.localhost/" } else { "tauri://localhost/" };
    Url::parse(&format!("{base}{page}")).expect("app url")
}

fn is_app_page(u: &Url) -> bool {
    u.scheme() == "tauri" || u.host_str() == Some("tauri.localhost") || u.scheme() == "about" || u.scheme() == "blob" || u.scheme() == "data"
}

/// `u` belongs to the Encore server at `base` (same scheme, host and port, under its path).
pub fn on_server(base: &str, u: &Url) -> bool {
    let Ok(b) = Url::parse(base) else { return false };
    u.scheme() == b.scheme() && u.host_str() == b.host_str() && u.port_or_known_default() == b.port_or_known_default() && u.path().starts_with(b.path().trim_end_matches('/'))
}

/// Navigation policy: our pages and the server stay in the window, everything else opens in the browser.
pub fn allow_navigation<R: Runtime>(app: &AppHandle<R>, u: &Url) -> bool {
    if is_app_page(u) {
        return true;
    }
    let server = app.state::<State>().settings.lock().map(|s| s.server.clone()).unwrap_or_default();
    if !server.is_empty() && on_server(&server, u) {
        return true;
    }
    open_external(app, u.as_str());
    false
}

pub fn open_external<R: Runtime>(app: &AppHandle<R>, url: &str) {
    use tauri_plugin_opener::OpenerExt;
    if url.starts_with("http://") || url.starts_with("https://") || url.starts_with("mailto:") {
        let _ = app.opener().open_url(url, None::<&str>);
    }
}

/// GET /api/health: Ok(()) when an Encore server answers, otherwise the message to show.
pub async fn probe(http: &reqwest::Client, base: &str) -> Result<(), String> {
    let r = http.get(format!("{base}/api/health")).timeout(std::time::Duration::from_secs(6)).send().await
        .map_err(|_| format!("No Encore server answered at {base}"))?;
    if !r.status().is_success() {
        return Err(format!("{base} answered {} – is that your Encore address?", r.status().as_u16()));
    }
    let ok = r.json::<serde_json::Value>().await.ok().and_then(|v| v.get("ok").and_then(|o| o.as_bool())).unwrap_or(false);
    if ok { Ok(()) } else { Err(format!("{base} answered, but it isn't an Encore server")) }
}

pub fn main_window<R: Runtime>(app: &AppHandle<R>) -> Option<tauri::WebviewWindow<R>> {
    app.get_webview_window("main")
}

/// Show the server picker (optionally with what went wrong).
pub fn show_picker<R: Runtime>(app: &AppHandle<R>, problem: Option<&str>) {
    disarm(app);
    let mut u = app_url("index.html");
    if let Some(p) = problem {
        u.query_pairs_mut().append_pair("err", p);
    }
    if let Some(w) = main_window(app) {
        let _ = w.navigate(u);
    }
}

/// No server: the offline player when songs are saved, otherwise the picker.
pub fn unreachable<R: Runtime>(app: &AppHandle<R>, problem: &str) {
    if !crate::offline::list(app).is_empty() {
        disarm(app);
        let mut u = app_url("offline.html");
        u.query_pairs_mut().append_pair("err", problem);
        if let Some(w) = main_window(app) {
            let _ = w.navigate(u);
        }
    } else {
        show_picker(app, Some(problem));
    }
}

/// Load the saved server in the window and start the watchdog.
pub fn open_server<R: Runtime>(app: &AppHandle<R>) {
    let server = app.state::<State>().settings.lock().map(|s| s.server.clone()).unwrap_or_default();
    if server.is_empty() {
        return show_picker(app, None);
    }
    if let (Some(w), Ok(u)) = (main_window(app), Url::parse(&format!("{server}/"))) {
        arm(app);
        let _ = w.navigate(u);
    }
}

/// Watchdog: we expect the Encore page to report in (page_alive) soon after we load the server. When it
/// doesn't – the webview is showing its own error page, a proxy answered with HTML, the address moved –
/// we check the server ourselves and show the picker or the offline player with the reason.
pub fn arm<R: Runtime>(app: &AppHandle<R>) {
    let st = app.state::<State>();
    let gen = st.generation.fetch_add(1, Ordering::SeqCst) + 1;
    st.armed.store(true, Ordering::SeqCst);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        let st = app.state::<State>();
        if !st.armed.load(Ordering::SeqCst) || st.generation.load(Ordering::SeqCst) != gen {
            return;
        }
        let server = st.settings.lock().map(|s| s.server.clone()).unwrap_or_default();
        match probe(&st.http, &server).await {
            Err(problem) => unreachable(&app, &problem),
            Ok(()) => {
                // the server is fine, the page is just slow – give it a while longer
                tokio::time::sleep(std::time::Duration::from_secs(20)).await;
                let st = app.state::<State>();
                if st.armed.load(Ordering::SeqCst) && st.generation.load(Ordering::SeqCst) == gen {
                    unreachable(&app, &format!("Can't open {server}"));
                }
            }
        }
    });
}

pub fn disarm<R: Runtime>(app: &AppHandle<R>) {
    #[cfg(debug_assertions)]
    eprintln!("watchdog off");
    app.state::<State>().armed.store(false, Ordering::SeqCst);
}

/// A page in the window has loaded and ran our bridge script. `encore` says whether it is the Encore web app.
pub fn page_alive<R: Runtime>(app: &AppHandle<R>, url: &str, encore: bool) {
    #[cfg(debug_assertions)]
    eprintln!("page_alive {url} encore={encore} armed={}", app.state::<State>().armed.load(Ordering::SeqCst));
    let st = app.state::<State>();
    if !st.armed.load(Ordering::SeqCst) {
        return;
    }
    let server = st.settings.lock().map(|s| s.server.clone()).unwrap_or_default();
    let Ok(u) = Url::parse(url) else { return };
    if !on_server(&server, &u) {
        return;
    }
    if encore {
        disarm(app);
    } else {
        // the address answers (a proxy, another service) but it isn't Encore – the probe tells which
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let st = app.state::<State>();
            let problem = probe(&st.http, &server).await.err().unwrap_or_else(|| format!("{server} answered, but it isn't an Encore server"));
            if st.armed.load(Ordering::SeqCst) {
                unreachable(&app, &problem);
            }
        });
    }
}

/// Check `raw`, remember it and open it. Returns the problem when it isn't an Encore server.
pub async fn connect<R: Runtime>(app: &AppHandle<R>, raw: &str) -> Result<(), String> {
    let base = config::normalize(raw);
    if base.len() <= "http://".len() {
        return Err("Enter your server address".into());
    }
    let http = app.state::<State>().http.clone();
    probe(&http, &base).await?;
    {
        let st = app.state::<State>();
        let mut s = st.settings.lock().map_err(|_| "settings busy")?;
        s.remember(&base);
        config::save(app, &s);
    }
    #[cfg(mobile)]
    crate::mobile::remember_server(app, &base);
    open_server(app);
    Ok(())
}

/// Browser-like user agent with our own tag, so the server lists the device as "Encore desktop on Windows"
/// (web/js/remote.js and server/src/routes/auth.rs look for EncoreDesktop / EncoreAndroid).
pub fn user_agent() -> String {
    let os = if cfg!(target_os = "windows") { "Windows NT 10.0; Win64; x64" }
        else if cfg!(target_os = "macos") { "Macintosh; Intel Mac OS X 10_15_7" }
        else if cfg!(target_os = "android") { "Linux; Android 14; Mobile" }
        else { "X11; Linux x86_64" };
    let tag = if cfg!(target_os = "android") { "EncoreAndroid" } else { "EncoreDesktop" };
    format!("Mozilla/5.0 ({os}) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36 {tag}/{VERSION}")
}
