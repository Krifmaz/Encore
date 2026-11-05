//! Settings → Contribute: send AcoustID fingerprints and MusicBrainz ISRCs for songs the audio confirmed,
//! and how albums missing from MusicBrainz get added.
//!
//! A Leptos view ([`Contribute`]) over one signal: the last answers of GET /api/admin/settings and
//! GET /api/contribute. Saving, Send now and the MusicBrainz account load them again, which draws the cards anew
//! (as the old page did, with the typed values cleared and without the note the MusicBrainz sign-in came back
//! with). The page's first load fails the page; a later load that fails keeps what is shown.

use crate::api;
use crate::app::{Ctx, PageFuture};
use crate::dom;
use crate::json::{js_string, truthy, J};
use crate::ui::{fmt, icon, toast};
use kit_leptos::html::html;
use leptos::prelude::*;
use serde_json::{Map, Value};
use web_sys::Element;

/// What a saved secret's box shows when the server sends it back masked; such a value is not sent again.
const MASK: &str = "••••";

/// What the page draws from: the settings, the contribution status, and the note on top (`Some((ok, text))`,
/// from the MusicBrainz sign-in coming back to the page; only on the first load).
#[derive(Clone)]
pub struct Loaded {
    /// The answer of GET /api/admin/settings (`_meta` says which values the environment sets).
    pub values: Value,
    /// The answer of GET /api/contribute: `acoustid` and `isrc`, each with its counts, last run and state.
    pub status: Value,
    /// The note on top: whether it is good news, and its text.
    pub flash: Option<(bool, String)>,
}

/// Whether the environment sets setting `key` (its box is then locked).
fn locked(meta: &Value, key: &str) -> bool {
    meta.o(key).s("source") == "env"
}

/// A setting's help line under its box, as HTML (it may hold links); nothing when there is none.
fn help_view(help: &str) -> Option<impl IntoView> {
    (!help.is_empty()).then(|| view! { <small {..html(help.to_string())}>{""}</small> })
}

/// The box of a secret setting: always empty, its placeholder saying whether one is saved.
fn secret_input(key: &str, label: &str, v: &Value, meta: &Value, help: &str) -> impl IntoView {
    let l = locked(meta, key);
    let placeholder = if l { "set by environment" } else if truthy(v) { "saved – paste a new value to replace" } else { "not set" };
    view! {
        <label class="field"><span>{label.to_string()}</span><input class="input" type="password" data-k=key.to_string() data-secret="1" value="" disabled=l autocomplete="new-password" placeholder=placeholder/>{help_view(help)}</label>
    }
}

/// The box of a text setting, filled with its value.
fn text_input(key: &str, label: &str, v: &Value, meta: &Value, help: &str) -> impl IntoView {
    let val = if v.is_null() { String::new() } else { js_string(v) };
    view! {
        <label class="field"><span>{label.to_string()}</span><input class="input" type="text" data-k=key.to_string() value=val autocomplete="off" disabled=locked(meta, key)/>{help_view(help)}</label>
    }
}

/// The switch of an on/off setting.
fn toggle(key: &str, label: &str, v: &Value, meta: &Value) -> impl IntoView {
    view! {
        <div class="field" style="justify-content:center"><label class="switch"><input type="checkbox" data-k=key.to_string() checked=truthy(v) disabled=locked(meta, key)/><i></i>{label.to_string()}</label></div>
    }
}

/// One count: the number over what it counts.
fn stat(n: f64, label: &'static str) -> impl IntoView {
    view! { <div class="stat"><b>{fmt::num(n)}</b><span>{label}</span></div> }
}

/// "Last run 5 minutes ago: …" (red when it failed); nothing before the first run.
fn last_line(l: &Value) -> Option<impl IntoView> {
    l.truthy("at").then(|| {
        let ok = l.truthy("ok");
        view! { <div class=if ok { "small muted" } else { "small " } style=if ok { "" } else { "color:var(--err)" }>{format!("Last run {}: {}", fmt::ago(l.f("at")), l.s("message"))}</div> }
    })
}

/// Saves the settings of card `part` (`acoustid` or `isrc`) as they are in the boxes under `view`: a locked box
/// is left out, and so is a secret box that is empty or still masked.
async fn save(view: &Element, part: &str) -> Result<(), api::ApiError> {
    let mut body = Map::new();
    if let Some(root) = dom::qs_in(view, &format!(r#"[data-part="{part}"]"#)) {
        for el in dom::qsa_in(&root, "[data-k]") {
            if el.has_attribute("disabled") {
                continue;
            }
            let k = dom::data(&el, "k");
            if el.get_attribute("type").as_deref() == Some("checkbox") {
                body.insert(k, Value::Bool(dom::checked(&el)));
                continue;
            }
            let v = dom::value(&el);
            if !dom::data(&el, "secret").is_empty() && (v.trim().is_empty() || v.starts_with(MASK)) {
                continue;
            }
            body.insert(k, Value::String(v.trim().to_string()));
        }
    }
    api::put("/admin/settings", Value::Object(body)).await.map(|_| ())
}

/// Both answers at once (the settings and the contribution status).
async fn fetch() -> Result<(Value, Value), String> {
    futures::future::try_join(api::get("/admin/settings"), api::get("/contribute")).await.map_err(|e| e.message)
}

/// Loads both answers into `state` again (without the note on top); a failure keeps what is shown, as before.
async fn reload(state: RwSignal<Option<Loaded>>) {
    match fetch().await {
        Ok((values, status)) => state.set(Some(Loaded { values, status, flash: None })),
        Err(e) => dom::console_error(&e),
    }
}

/// The Contribute page: the view, mounted once the first answers have come (their failure is the page's error).
pub fn render(view: Element, ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let (values, status) = fetch().await?;
        let flash = if !ctx.q("error").is_empty() {
            Some((false, ctx.q("error")))
        } else if !ctx.q("mb").is_empty() {
            Some((true, "MusicBrainz account connected".to_string()))
        } else {
            None
        };
        dom::set_html(&view, "");
        // the signal lives as long as the page: the owner is dropped with it
        let owner = Owner::new();
        let state = owner.with(|| RwSignal::new(Some(Loaded { values, status, flash })));
        let root = view.clone();
        if let Some(m) = owner.with(|| kit_leptos::rt::mount(&view, move || view! { <Contribute state root/> })) {
            dom::keep(m);
        }
        dom::keep(owner);
        Ok(())
    })
}

/// The four cards: the introduction, AcoustID, MusicBrainz ISRCs and missing albums.
#[component]
pub fn Contribute(
    /// What the page draws from (always `Some` once mounted).
    state: RwSignal<Option<Loaded>>,
    /// The page's element: Save reads its card's boxes from it.
    root: Element,
) -> impl IntoView {
    let root = StoredValue::new_local(root);
    // Save: the card's settings, then everything again
    let on_save = move |part: &'static str| {
        move |_| {
            dom::spawn(async move {
                match save(&root.get_value(), part).await {
                    Ok(()) => {
                        toast("Settings saved", "ok");
                        reload(state).await;
                    }
                    Err(e) => toast(&e.message, "err"),
                }
            });
        }
    };
    // Send now: that contribution's run, its answer as a toast
    let on_run = move |what: &'static str| {
        move |e: web_sys::MouseEvent| {
            if let Some(b) = dom::target_el(&e).and_then(|t| t.closest("button").ok().flatten()) {
                dom::set_disabled(&b, true);
            }
            dom::spawn(async move {
                match api::post_empty(&format!("/contribute/{what}/run")).await {
                    Ok(r) => toast(&if r.truthy("message") { r.s("message") } else { "Done".to_string() }, "ok"),
                    Err(e) => toast(&e.message, "err"),
                }
                reload(state).await;
            });
        }
    };
    // Connect (save the app's ID and secret, then MusicBrainz's sign-in) and Disconnect
    let on_mb = move |what: &'static str| {
        move |e: web_sys::MouseEvent| {
            if let Some(b) = dom::target_el(&e).and_then(|t| t.closest("button").ok().flatten()) {
                dom::set_disabled(&b, true);
            }
            dom::spawn(async move {
                let r: Result<(), api::ApiError> = async {
                    if what == "connect" {
                        save(&root.get_value(), "isrc").await?;
                        let r = api::post_empty("/contribute/mb/connect").await?;
                        let _ = dom::location().set_href(&r.s("url"));
                        return Ok(());
                    }
                    api::post_empty("/contribute/mb/disconnect").await?;
                    toast("Disconnected", "ok");
                    Ok(())
                }
                .await;
                if what == "connect" && r.is_ok() {
                    return;
                }
                if let Err(e) = r {
                    toast(&e.message, "err");
                }
                reload(state).await;
            });
        }
    };
    move || {
        let Loaded { values, status, flash } = state.get()?;
        let meta = values.o("_meta");
        let (a, m) = (status.o("acoustid"), status.o("isrc"));
        let flash = flash.map(|(ok, text)| {
            let (cls, ic) = if ok { ("test-result ok", "check") } else { ("test-result err", "x") };
            view! { <div class={cls} {..html(format!("{}<span>{}</span>", icon(ic), crate::ui::esc(&text)))}>{""}</div> }
        });
        let why = (a.truthy("why_not") && a.truthy("enabled")).then(|| {
            view! { <div class="small muted">{format!("{}{}", a.s("why_not"), if a.truthy("can_fingerprint") { "" } else { ". fpcalc is not installed on this server" })}</div> }
        });
        let mb_buttons = if m.truthy("connected") {
            view! {
                <button class="btn sm" data-run="isrc" on:click=on_run("isrc") {..html(format!("{}Send now", icon("refresh")))}>{""}</button>
                <button class="btn sm" data-mb="disconnect" on:click=on_mb("disconnect")>"Disconnect"</button>
            }
            .into_any()
        } else {
            view! { <button class="btn sm" data-mb="connect" disabled=!m.truthy("has_client") on:click=on_mb("connect") {..html(format!("{}Connect MusicBrainz", icon("link")))}>{""}</button> }.into_any()
        };
        let callback = if m.truthy("redirect_uri") {
            format!(r#"<span class="mono">{}</span>"#, crate::ui::esc(&m.s("redirect_uri")))
        } else {
            "&lt;Public URL&gt;/api/contribute/mb/callback (set the Public URL under General first)".to_string()
        };
        let account = if m.truthy("connected") {
            let who = if m.truthy("account") { m.s("account") } else { "your account".to_string() };
            view! { "Connected as "<b>{who}</b> }.into_any()
        } else {
            "Not connected".into_any()
        };
        let save_label = format!("{}Save", icon("check"));
        Some(view! {
            <div class="card stack" style="margin-bottom:16px">
                <h2>"Contribute"</h2>
                <p class="muted small" style="margin:0">"Encore leans on AcoustID and MusicBrainz to know what a file is. Here it can give back: fingerprints of songs whose recording the audio confirmed, ISRCs MusicBrainz is missing, and albums that are only on Deezer or Apple Music. Only songs Encore is sure about are sent; nothing is sent until you add your own keys."</p>
                {flash}
            </div>
            <div class="card stack" data-part="acoustid" style="margin-bottom:16px">
                <div class="row between wrap" style="gap:8px"><h2>"AcoustID fingerprints"</h2><button class="btn sm" data-run="acoustid" disabled=!a.truthy("ready") on:click=on_run("acoustid") {..html(format!("{}Send now", icon("refresh")))}>{""}</button></div>
                <p class="muted small" style="margin:0">"When a download's audio confirms the MusicBrainz recording it was asked for, its fingerprint is sent to AcoustID in batches, once per song. Your user key says who sent it; the API key under Settings → Library → Verification & duplicates is Encore's application key."</p>
                <div class="stats">{stat(a.f("queued"), "Waiting to send")}{stat(a.f("submitted"), "Sent, being processed")}{stat(a.f("imported"), "In AcoustID")}{stat(a.f("failed"), "Failed")}</div>
                {why}
                {last_line(a.g("last"))}
                <div class="form-grid">
                    {toggle("contribute_acoustid", "Send fingerprints of confirmed songs to AcoustID", values.g("contribute_acoustid"), &meta)}
                    {secret_input("acoustid_user_key", "AcoustID user key", values.g("acoustid_user_key"), &meta, r#"Sign in at <a href="https://acoustid.org/new-application" target="_blank" rel="noopener">acoustid.org</a> and copy the user API key shown on your profile. It is not the application key: that one identifies Encore, this one identifies you."#)}
                </div>
                <div class="row" style="justify-content:flex-end"><button class="btn primary" data-save="acoustid" on:click=on_save("acoustid") {..html(save_label.clone())}>{""}</button></div>
            </div>
            <div class="card stack" data-part="isrc" style="margin-bottom:16px">
                <div class="row between wrap" style="gap:8px"><h2>"MusicBrainz ISRCs"</h2>
                    <span class="row" style="gap:8px">{mb_buttons}</span></div>
                <p class="muted small" style="margin:0">"Songs from Deezer, Tidal and Qobuz carry the ISRC of the recording. When the audio confirmed the MusicBrainz recording and MusicBrainz does not list that ISRC yet, Encore adds it under your MusicBrainz account. ISRCs that MusicBrainz has on another recording are left for a person to look at."</p>
                <div class="small">{account}</div>
                <div class="stats">{stat(m.f("queued"), "Waiting to send")}{stat(m.f("submitted"), "Added to MusicBrainz")}{stat(m.f("skipped"), "Skipped")}{stat(m.f("failed"), "Failed")}</div>
                {last_line(m.g("last"))}
                <div class="form-grid">
                    {toggle("contribute_isrc", "Add missing ISRCs to MusicBrainz", values.g("contribute_isrc"), &meta)}
                    {text_input("mb_client_id", "MusicBrainz application client ID", values.g("mb_client_id"), &meta, &format!(r#"Register an application at <a href="https://musicbrainz.org/account/applications" target="_blank" rel="noopener">musicbrainz.org/account/applications</a> (type: web application) with the callback URL {callback}."#))}
                    {secret_input("mb_client_secret", "MusicBrainz application client secret", values.g("mb_client_secret"), &meta, "")}
                </div>
                <div class="row" style="justify-content:flex-end"><button class="btn primary" data-save="isrc" on:click=on_save("isrc") {..html(save_label)}>{""}</button></div>
            </div>
            <div class="card stack">
                <h2>"Albums missing from MusicBrainz"</h2>
                <p class="muted small" style="margin:0">"Artist pages list albums found on Deezer and Apple Music that MusicBrainz does not have. Their Add to MusicBrainz link opens the MusicBrainz release editor with the title, artists, date, label, barcode, track list and store links filled in. Check it, then enter the edit under your own MusicBrainz account. Once the album is in MusicBrainz, Encore re-tags your copy on its own."</p>
            </div>
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A setting the environment sets is locked; one saved in Settings is not.
    #[test]
    fn a_setting_from_the_environment_is_locked() {
        let meta = json!({ "acoustid_user_key": { "source": "env" }, "mb_client_id": { "source": "db" } });
        assert!(locked(&meta, "acoustid_user_key"));
        assert!(!locked(&meta, "mb_client_id"));
        assert!(!locked(&meta, "contribute_isrc"));
    }
}
