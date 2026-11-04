//! The Server group of Settings: General, Library, Downloads, Integrations, Server processing,
//! Email & notifications and Scheduled tasks are the admin settings form of web-rs/src/pages/settings.rs
//! (`show`), drawn from the field table in `fields.rs`: one card per section with its fields, secrets
//! with Show saved / Copy, Save and Test, the live blocks of `tools.rs` (Cadenza's on Server processing),
//! the API key card and About. Auto-download, Contribute and Blocked are pages of their own and are routed
//! to their modules; Plugins is plugins.rs. Since 0.32 Downloads holds the Download pipeline and Release
//! types only: the source cards are plugin cards on Plugins, and the Download tools card is gone. A server
//! without plugins (before 0.32) still gets its source cards here.

use super::fields::{self, Card, Field};
use super::{autodownload, blocked, contribute, tasks, tools, Section};
use super::{env_locked, help};
use crate::api::ApiError;
use crate::fmt;
use crate::pages::{Ctx, El};
use crate::theme::{self, Btn};
use crate::widgets::{self, btn, btn_icon, icon_btn, modal, t, Icon};
use iced::widget::{column, container, row};
use iced::{Alignment, Length, Subscription, Task};
use kit_iced::form::{self, Form};
use serde_json::{Map, Value};
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
pub enum Dialog {
    RegenKey,
}

#[derive(Debug, Default)]
pub struct State {
    section: Option<Section>,
    /// The settings form (the kit's): the values, what was typed, the open and busy cards.
    form: Form,
    api_key: Option<String>,
    key_shown: bool,
    dialog: Option<Dialog>,
    pub tools: tools::State,
    /// Downloads on a server without plugins (before 0.32): its source cards are drawn here as before.
    no_plugins: bool,
    pub tasks: tasks::State,
    pub autodownload: autodownload::State,
    pub contribute: contribute::State,
    pub blocked: blocked::State,
}

#[derive(Clone, Debug)]
pub enum Message {
    /// The settings form's own messages (kit_iced::form).
    Form(form::Msg),
    VpnSaved(Result<Value, ApiError>),
    ApiKey(Result<Value, ApiError>),
    ShowKey(bool),
    CopyKey,
    /// Copy a failed test's words.
    CopyText(String),
    AskRegen,
    Regen,
    Regenerated(Result<Value, ApiError>),
    OpenReleases,
    /// The line under How requests are fulfilled: Settings → Plugins.
    GoPlugins,
    Tools(tools::Message),
    /// GET /plugins on Downloads: whether this server has plugins (an older one answers with its web page).
    PluginsProbe(Result<Value, ApiError>),
    Tasks(tasks::Message),
    Autodownload(autodownload::Message),
    Contribute(contribute::Message),
    Blocked(blocked::Message),
    TidalTick,
    CloseDialog,
}

/// The field group a section shows (None for the pages of their own).
fn group_id(s: Section) -> Option<&'static str> {
    match s {
        Section::General => Some("general"),
        Section::Library => Some("library"),
        Section::Downloads => Some("downloads"),
        Section::Integrations => Some("integrations"),
        Section::Processing => Some("processing"),
        Section::Accounts => Some("accounts"),
        Section::Tasks => Some("tasks"),
        _ => None,
    }
}

fn load_values(ctx: &Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    Task::perform(async move { api.get("/admin/settings").await }, |r| Message::Form(form::Msg::Values(r.map_err(|e| e.message))))
}

pub fn open(state: &mut State, section: Section, ctx: &mut Ctx) -> Task<Message> {
    state.section = Some(section);
    // the WireGuard private key never comes back from the server: no Show saved / Copy
    state.form = std::mem::take(&mut state.form).never_shown(["vpn_wireguard_config"]);
    state.form.open(std::iter::empty::<String>(), None);
    state.dialog = None;
    match section {
        Section::Autodownload => return autodownload::open(&mut state.autodownload, ctx).map(Message::Autodownload),
        Section::Contribute => return contribute::open(&mut state.contribute, ctx).map(Message::Contribute),
        Section::Blocked => return blocked::open(&mut state.blocked, ctx).map(Message::Blocked),
        _ => {}
    }
    let group = group_id(section).and_then(fields::group);
    let focus = super::take_focus().filter(|f| group.is_some_and(|g| g.sections().any(|s| s == *f)));
    state.form.open(group.map(|g| g.sections().collect::<Vec<_>>()).unwrap_or_default(), focus.map(String::from));
    let mut tasks = vec![load_values(ctx)];
    match section {
        Section::Tasks => tasks.push(tasks::open(&mut state.tasks, ctx).map(Message::Tasks)),
        // the source cards are on Plugins, unless this server has none (then they stay here, with their blocks)
        Section::Downloads => {
            state.no_plugins = false;
            let api = ctx.api.clone();
            tasks.push(Task::perform(async move { api.get("/plugins").await }, Message::PluginsProbe));
        }
        // Cadenza's blocks: the cache and the engine's settings
        Section::Processing => tasks.push(tools::open_processing(&mut state.tools, ctx).map(Message::Tools)),
        Section::General if ctx.session.is_owner() => {
            let api = ctx.api.clone();
            tasks.push(Task::perform(async move { api.get("/system/api-key").await }, Message::ApiKey));
        }
        _ => {}
    }
    Task::batch(tasks)
}

/// A card's Save body with what the form does not hold: Cadenza's engine settings (drawn by `tools.rs`).
fn with_engine(state: &State, card: &str, body: &mut Map<String, Value>) {
    if card == "cadenza" && !state.form.values().is_some_and(|v| env_locked(v, "cadenza_engine_settings")) {
        if let Some(e) = tools::engine_settings_value(&state.tools) {
            body.insert("cadenza_engine_settings".into(), Value::String(e));
        }
    }
}

/// Runs what the form asks for: the requests, the clipboard and the toasts.
fn run(state: &State, out: form::Out, ctx: &mut Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    let put = |body: Map<String, Value>| {
        let api = api.clone();
        async move { api.put("/admin/settings", Value::Object(body)).await }
    };
    match out {
        form::Out::Save { card, mut body } => {
            with_engine(state, &card, &mut body);
            Task::perform(put(body), move |r| Message::Form(form::Msg::Saved(card.clone(), r.map_err(|e| e.message))))
        }
        form::Out::Test { card, test, mut body } => {
            with_engine(state, &card, &mut body);
            Task::perform(
                async move {
                    let saved = api.put("/admin/settings", Value::Object(body)).await?;
                    let tested = api.post_empty(&format!("/system/test/{test}")).await;
                    Ok((saved, tested.map_err(|e| e.message)))
                },
                move |r: Result<(Value, Result<Value, String>), ApiError>| Message::Form(form::Msg::Tested(card.clone(), r.map_err(|e| e.message))),
            )
        }
        form::Out::Clear { body, .. } => Task::perform(put(body), |r| Message::Form(form::Msg::Cleared(r.map_err(|e| e.message)))),
        form::Out::Reveal { key } => {
            let k = key.clone();
            Task::perform(
                async move { api.get(&format!("/admin/settings/secret/{}", crate::pages::urlencode(&k))).await },
                move |r| Message::Form(form::Msg::Revealed(key.clone(), r.map_err(|e| e.message))),
            )
        }
        form::Out::CopySecret { key } => Task::perform(
            async move { api.get(&format!("/admin/settings/secret/{}", crate::pages::urlencode(&key))).await },
            |r| Message::Form(form::Msg::Copied(r.map_err(|e| e.message))),
        ),
        form::Out::Copy(text) => {
            ctx.copy(text);
            Task::none()
        }
        form::Out::CopyText(text) => {
            ctx.copy_text(text);
            Task::none()
        }
        form::Out::ToastOk(text) => {
            ctx.toast_ok(text);
            Task::none()
        }
        form::Out::ToastErr(text) => {
            ctx.toast_err(text);
            Task::none()
        }
    }
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Form(m) => {
            let outs = state.form.update(m, fields::SECTIONS);
            let tasks: Vec<Task<Message>> = outs.into_iter().map(|o| run(state, o, ctx)).collect();
            return Task::batch(tasks);
        }
        Message::VpnSaved(Ok(v)) => {
            state.form.take_values(v, fields::SECTIONS);
            if let Some(card) = fields::card("slskd") {
                state.form.settle(card);
            }
            return tools::start_vpn_test(&mut state.tools, ctx).map(Message::Tools);
        }
        Message::VpnSaved(Err(e)) => tools::vpn_test_failed(&mut state.tools, e.message),
        Message::ApiKey(Ok(r)) => state.api_key = Some(fmt::s(&r, "api_key")),
        Message::ApiKey(Err(_)) => state.api_key = None,
        Message::ShowKey(on) => state.key_shown = on,
        Message::CopyText(text) => ctx.copy_text(text),
        Message::CopyKey => {
            if let Some(k) = &state.api_key {
                ctx.copy(k.clone());
                ctx.toast_ok("Copied to clipboard");
            }
        }
        Message::AskRegen => state.dialog = Some(Dialog::RegenKey),
        Message::Regen => {
            state.dialog = None;
            let api = ctx.api.clone();
            return Task::perform(async move { api.post_empty("/admin/settings/api-key").await }, Message::Regenerated);
        }
        Message::Regenerated(Ok(r)) => {
            state.api_key = Some(fmt::s(&r, "api_key"));
            ctx.toast_ok("New API key generated");
        }
        Message::Regenerated(Err(e)) => ctx.toast_err(e.message),
        Message::OpenReleases => ctx.open_url("https://github.com/Krifmaz/Encore/releases"),
        Message::GoPlugins => ctx.navigate(crate::pages::Page::Settings(Section::Plugins)),
        Message::Tools(tools::Message::VpnTest) => {
            // save the Soulseek card first, then run the test
            let (Some(values), Some(card)) = (state.form.values(), fields::card("slskd")) else { return Task::none() };
            let body = state.form.save_body(values, card);
            state.tools.vpn_testing_started();
            let api = ctx.api.clone();
            return Task::perform(async move { api.put("/admin/settings", Value::Object(body)).await }, Message::VpnSaved);
        }
        Message::Tools(m) => return tools::update(&mut state.tools, m, ctx).map(Message::Tools),
        Message::PluginsProbe(r) => {
            state.no_plugins = no_plugins(&r);
            if state.no_plugins && state.section == Some(Section::Downloads) {
                return tools::open_sources(&mut state.tools, ctx).map(Message::Tools);
            }
        }
        Message::Tasks(m) => return tasks::update(&mut state.tasks, m, ctx).map(Message::Tasks),
        Message::Autodownload(m) => return autodownload::update(&mut state.autodownload, m, ctx).map(Message::Autodownload),
        Message::Contribute(m) => return contribute::update(&mut state.contribute, m, ctx).map(Message::Contribute),
        Message::Blocked(m) => return blocked::update(&mut state.blocked, m, ctx).map(Message::Blocked),
        Message::TidalTick => return tools::update(&mut state.tools, tools::Message::TidalTick, ctx).map(Message::Tools),
        Message::CloseDialog => state.dialog = None,
    }
    Task::none()
}

// ----- views -------------------------------------------------------------------------------

/// Whether GET /plugins says this server has no plugins: an older server answers the address with its web page
/// (or not at all). A refusal for another reason (signed out, not an admin) says nothing either way.
pub fn no_plugins(r: &Result<Value, ApiError>) -> bool {
    match r {
        Ok(v) => !v["plugins"].is_array(),
        Err(e) => e.status == 404,
    }
}

/// The cards of a page, under their part headings: the group's own, and on Downloads of a server without plugins
/// the source cards that were there before 0.32.
fn page_parts(group: &fields::Group, section: Section, no_plugins: bool) -> Vec<(&'static str, &'static [&'static str])> {
    let mut parts: Vec<(&'static str, &'static [&'static str])> = group.parts.to_vec();
    if section == Section::Downloads && no_plugins {
        parts.push(("", fields::LEGACY_SOURCES));
    }
    parts
}

/// A card of the form, with the blocks of its own the Soulseek, YouTube, streamrip and Cadenza cards carry.
fn card_view<'a>(state: &'a State, values: &'a Value, card: &'static Card, ctx: &Ctx<'a>) -> El<'a, Message> {
    let reveal = ctx.has_feature("reveal") && ctx.session.is_owner();
    let f = &state.form;
    let mut slots = form::Slots::default();
    match &*card.id {
        "youtube" => slots.top.push(tools::youtube_block(&state.tools).map(Message::Tools)),
        "slskd" => {
            let (vpn, rest): (Vec<&'static Field>, Vec<&'static Field>) = f.visible_fields(card).into_iter().partition(|f| f.key.starts_with("vpn_"));
            let mut c = column![f.grid(&rest, reveal).map(Message::Form)].spacing(14).width(Length::Fill);
            if !vpn.is_empty() {
                c = c.push(tools::vpn_block(
                    &state.tools,
                    f.grid(&vpn, reveal).map(Message::Form),
                    (!state.tools.vpn_testing()).then_some(Message::Tools(tools::Message::VpnTest)),
                    Message::CopyText,
                ));
            }
            c = c.push(tools::soulseek_block(&state.tools).map(Message::Tools));
            slots.body = Some(c.into());
        }
        "streamrip" => {
            slots.bottom.push(tools::tidal_block(&state.tools).map(Message::Tools));
            slots.bottom.push(tools::streamrip_block(&state.tools).map(Message::Tools));
        }
        "cadenza" => slots.bottom.push(tools::cadenza_block(&state.tools, values).map(Message::Tools)),
        _ => {}
    }
    f.card_view(card, reveal, Message::Form, slots)
}

fn api_key_card<'a>(state: &'a State) -> Option<El<'a, Message>> {
    let key = state.api_key.as_ref()?;
    let shown = if state.key_shown { key.clone() } else { "•".repeat(key.chars().count().min(32)) };
    Some(super::card(
        "Encore API key",
        vec![
            help("Send it in an X-Api-Key header to automate Encore (scripts, Home Assistant…). It acts as the owner account – keep it private.").into(),
            row![
                container(t(shown).size(13).font(theme::mono())).padding(8).width(Length::Fill).style(theme::panel2),
                icon_btn(if state.key_shown { Icon::X } else { Icon::Search }, if state.key_shown { "Hide" } else { "Show" }, Some(Message::ShowKey(!state.key_shown))),
                icon_btn(Icon::Copy, "Copy", Some(Message::CopyKey)),
                btn_icon(Icon::Refresh, "Regenerate", Btn::Danger, Some(Message::AskRegen)),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into(),
        ],
    ))
}

/// How requests are fulfilled (Downloads); since 0.32 a line under it says where the sources are installed.
fn pipeline_help<'a>(plugins: bool) -> El<'a, Message> {
    let steps = [
        ("Library check", "if the album/track is already in your library (by MusicBrainz id or name) it's marked available instantly."),
        ("streamrip", "lossless from Qobuz, Tidal or Deezer with your own accounts, matched by ISRC."),
        ("Torrents & Usenet", "Prowlarr searches your indexers; the best album release goes to your torrent or Usenet client and is imported when it finishes. If it stalls past the timeout, the next release or source is tried."),
        ("Soulseek", "Encore searches the network, results are ranked by track count, format, bitrate, folder name and the uploader's free slots/speed. Stalled or failed users are skipped automatically."),
        ("yt-dlp", "each track from the MusicBrainz tracklist is matched on YouTube (preferring official \"Topic\" uploads with matching length)."),
        ("Import", "files are matched to the MusicBrainz release, tagged (incl. MBIDs, ISRCs, cover art), renamed with your template, and your media servers are refreshed."),
    ];
    let mut lines: Vec<El<'a, Message>> = Vec::new();
    for (i, (name, what)) in steps.iter().enumerate() {
        lines.push(
            row![
                widgets::muted(format!("{}.", i + 1)).size(13).width(22),
                t(*name).size(13).font(theme::semibold()),
                widgets::muted(format!("– {what}")).size(13).width(Length::Fill),
            ]
            .spacing(6)
            .align_y(Alignment::Start)
            .into(),
        );
    }
    let mut c = column![widgets::h3("How requests are fulfilled")].spacing(8);
    for l in lines {
        c = c.push(l);
    }
    if plugins {
        c = c.push(
            row![help("Each source is a plugin: install, update or disable it under"), super::link("Settings → Plugins", Message::GoPlugins)]
                .spacing(2)
                .align_y(Alignment::Center)
                .wrap(),
        );
    }
    widgets::card(c.width(Length::Fill)).width(Length::Fill).into()
}

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let section = state.section.unwrap_or(Section::General);
    match section {
        Section::Autodownload => return autodownload::view(&state.autodownload, ctx).map(Message::Autodownload),
        Section::Contribute => return contribute::view(&state.contribute, ctx).map(Message::Contribute),
        Section::Blocked => return blocked::view(&state.blocked, ctx).map(Message::Blocked),
        _ => {}
    }
    if !state.form.error().is_empty() {
        return widgets::error_box(state.form.error());
    }
    let Some(values) = state.form.values() else {
        return widgets::skeleton_rows(6);
    };
    let Some(group) = group_id(section).and_then(fields::group) else {
        return widgets::skeleton_rows(2);
    };
    let mut page = column![].spacing(16).width(Length::Fill);
    if section == Section::Tasks {
        page = page.push(tasks::view(&state.tasks, ctx).map(Message::Tasks));
    }
    for (label, ids) in page_parts(group, section, state.no_plugins) {
        let cards: Vec<&'static Card> = ids
            .iter()
            .filter_map(|id| fields::card(id))
            .filter(|c| c.shown(values))
            .collect();
        if cards.is_empty() {
            continue;
        }
        if !label.is_empty() {
            page = page.push(form::part_heading(label));
        }
        for c in cards {
            page = page.push(card_view(state, values, c, ctx));
        }
    }
    if section == Section::Downloads {
        page = page.push(pipeline_help(!state.no_plugins));
    }
    if section == Section::General {
        if let Some(k) = api_key_card(state) {
            page = page.push(k);
        }
        page = page.push(super::account::about_card(ctx, Message::OpenReleases));
    }
    page.into()
}

pub fn overlay<'a>(state: &'a State, ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    match state.section.unwrap_or(Section::General) {
        Section::Autodownload => return None,
        Section::Contribute => return None,
        Section::Blocked => return blocked::overlay(&state.blocked, ctx).map(|e| e.map(Message::Blocked)),
        _ => {}
    }
    if let Some(d) = tools::overlay(&state.tools) {
        return Some(d.map(Message::Tools));
    }
    match state.dialog? {
        Dialog::RegenKey => Some(modal(
            "Encore API key",
            widgets::muted("Make a new API key? Scripts using the old one stop working.").size(14),
            Some(
                row![
                    btn("Cancel", Btn::Default, Some(Message::CloseDialog)),
                    btn("Regenerate", Btn::Danger, Some(Message::Regen)),
                ]
                .spacing(10)
                .into(),
            ),
            false,
            Message::CloseDialog,
        )),
    }
}

pub fn subscription(state: &State) -> Subscription<Message> {
    // the Soulseek and VPN states and the Tidal sign-in are on Downloads of a server without plugins
    if state.section != Some(Section::Downloads) || !state.no_plugins {
        return Subscription::none();
    }
    let mut subs = vec![iced::time::every(Duration::from_secs(5)).map(|_| Message::Tools(tools::Message::Tick))];
    if tools::tidal_waiting(&state.tools) {
        subs.push(iced::time::every(Duration::from_secs(3)).map(|_| Message::TidalTick));
    }
    Subscription::batch(subs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The form after `GET /admin/settings` answered `values`.
    fn loaded(values: &Value) -> Form {
        let mut f = Form::new();
        f.update(form::Msg::Values(Ok(values.clone())), fields::SECTIONS);
        f
    }

    /// Replace what a text area holds, as typing does.
    fn type_into(f: &mut Form, key: &str, text: &str) {
        use iced::widget::text_editor::{Action, Edit};
        f.update(form::Msg::Area(key.into(), Action::SelectAll), fields::SECTIONS);
        f.update(form::Msg::Area(key.into(), Action::Edit(Edit::Paste(std::sync::Arc::new(text.into())))), fields::SECTIONS);
    }

    // The rules themselves are tested in kit-iced (form.rs); these run them on this app's table.
    #[test]
    fn a_plain_text_area_keeps_what_is_saved() {
        let values = json!({ "library_path": "/music", "library_extra_roots": "/mnt/a\n/mnt/b", "staging_path": "/s" });
        let card = fields::card("library").unwrap();
        let mut st = loaded(&values);
        assert_eq!(st.save_body(&values, card)["library_extra_roots"], "/mnt/a\n/mnt/b", "an untouched Save keeps the folders");
        // a value changed on the server (an in-place import) shows when the values come again
        let newer = json!({ "library_path": "/music", "library_extra_roots": "/mnt/a\n/mnt/b\n/mnt/c", "staging_path": "/s" });
        st.take_values(newer.clone(), fields::SECTIONS);
        assert_eq!(st.save_body(&newer, card)["library_extra_roots"], "/mnt/a\n/mnt/b\n/mnt/c");
        // what was typed stays until it is saved
        type_into(&mut st, "library_extra_roots", "/x");
        st.take_values(newer.clone(), fields::SECTIONS);
        assert_eq!(st.save_body(&newer, card)["library_extra_roots"], "/x");
        st.settle(card);
        assert_eq!(st.save_body(&newer, card)["library_extra_roots"], "/mnt/a\n/mnt/b\n/mnt/c");
        // a secret one still starts empty and is left out until something is pasted
        let wg = json!({ "vpn_wireguard_config": "•••• (saved)" });
        let st = loaded(&wg);
        assert!(st.save_body(&wg, fields::card("slskd").unwrap()).get("vpn_wireguard_config").is_none());
    }

    #[test]
    fn the_save_body_follows_the_web_rules() {
        let values = json!({
            "app_name": "Encore", "base_url": "http://x", "auto_approve": 1, "github_token": "•••• (saved)", "signup_mode": "invite",
            "_meta": { "base_url": { "source": "env", "env": "ENCORE_BASE_URL" } }
        });
        let mut st = loaded(&values);
        st.update(form::Msg::Edit("app_name".into(), "Mine".into()), fields::SECTIONS);
        st.update(form::Msg::Bool("auto_approve".into(), false), fields::SECTIONS);
        let card = fields::card("general").unwrap();
        let b = st.save_body(&values, card);
        assert_eq!(b["app_name"], "Mine");
        assert_eq!(b["auto_approve"], false);
        assert_eq!(b["signup_mode"], "invite");
        assert!(b.get("base_url").is_none(), "locked by the environment");
        assert!(b.get("github_token").is_none(), "nothing typed keeps the saved secret");
        assert!(b.get("session_days").is_none(), "the server does not have it");
        st.update(form::Msg::Edit("github_token".into(), " tok ".into()), fields::SECTIONS);
        assert_eq!(st.save_body(&values, card)["github_token"], "tok");
        // checks join the ticked options in the table's spelling
        let values = json!({ "release_types": "album,ep", "release_skip": "" });
        let mut st = loaded(&values);
        st.update(form::Msg::Check("release_skip".into(), "Live".into(), true), fields::SECTIONS);
        st.update(form::Msg::Check("release_skip".into(), "DJ-mix".into(), true), fields::SECTIONS);
        let b = st.save_body(&values, fields::card("releases").unwrap());
        assert_eq!(b["release_types"], "Album,EP");
        assert_eq!(b["release_skip"], "Live,DJ-mix");
    }

    #[test]
    fn when_rules_follow_the_select() {
        let values = json!({ "torrent_client": "qbittorrent", "qbit_url": "", "transmission_url": "" });
        let mut st = loaded(&values);
        let qbit = fields::card("torznab").unwrap().fields.iter().find(|f| f.key == "qbit_url").unwrap();
        let tr = fields::card("torznab").unwrap().fields.iter().find(|f| f.key == "transmission_url").unwrap();
        assert!(st.visible(&values, qbit) && !st.visible(&values, tr));
        st.update(form::Msg::Edit("torrent_client".into(), "transmission".into()), fields::SECTIONS);
        assert!(!st.visible(&values, qbit) && st.visible(&values, tr));
        assert_eq!(group_id(Section::Tasks), Some("tasks"));
        assert_eq!(group_id(Section::Blocked), None);
        assert_eq!(group_id(Section::Processing), Some("processing"));
        // the first card the server has starts open, or the one a link asked for
        let downloads = || fields::group("downloads").unwrap().sections();
        let mut st = Form::new();
        st.open(downloads(), None);
        st.update(form::Msg::Values(Ok(json!({ "release_types": "Album" }))), fields::SECTIONS);
        assert!(st.is_open("releases") && !st.is_open("pipeline"));
        st.open(downloads(), Some("pipeline".into()));
        st.update(form::Msg::Values(Ok(json!({ "release_types": "Album" }))), fields::SECTIONS);
        assert!(st.is_open("pipeline"));
    }

    #[test]
    fn downloads_keeps_the_pipeline_and_the_release_types() {
        let g = fields::group("downloads").unwrap();
        let ids = |parts: Vec<(&'static str, &'static [&'static str])>| parts.iter().flat_map(|(_, ids)| ids.iter().copied()).collect::<Vec<_>>();
        assert_eq!(ids(page_parts(g, Section::Downloads, false)), ["pipeline", "releases"]);
        // a server without plugins (before 0.32) keeps its source cards here
        assert_eq!(ids(page_parts(g, Section::Downloads, true)), ["pipeline", "releases", "streamrip", "torznab", "slskd", "ytdlp", "youtube"]);
        assert!(no_plugins(&Ok(Value::String("<!doctype html>".into()))));
        assert!(no_plugins(&Err(ApiError::new(404, "Not Found"))));
        assert!(!no_plugins(&Ok(json!({ "plugins": [] }))));
        assert!(!no_plugins(&Err(ApiError::new(403, "Admins only"))));
    }
}
