//! Your devices: every other signed-in device (and Roon zone) with what it plays and its
//! controls, Link as remote / Unlink, Play here and Send my music there – as web-rs/src/remote
//! does. This computer is a device through the engine link (`link.rs`); this file is the
//! picker and the "linked" mode, where everything you play or press here goes to that device.

use super::{Message, State};
use crate::api::{Api, ApiError};
use crate::engine::Track;
use crate::fmt;
use crate::pages::{Ctx, El};
use crate::theme::{self, Badge, Btn};
use crate::widgets::{self, badge, icon, icon_btn, t, Icon};
use iced::widget::{column, container, image, row, slider, space, text_input};
use iced::{Alignment, ContentFit, Length, Task};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// This device locked as a remote for another one.
#[derive(Clone, Debug, Default)]
pub struct Linked {
    pub id: String,
    pub name: String,
    /// That device's last check-in state (`title`, `playing`, `position`, `queue`…).
    pub state: Value,
    pub online: bool,
    /// Device listings in a row without it.
    pub missed: u32,
}

/// The picker's state.
#[derive(Debug, Default)]
pub struct Devices {
    pub linked: Option<Linked>,
    /// The other devices as the server last listed them (this computer left out).
    pub list: Vec<Value>,
    pub zones: Vec<Value>,
    pub loaded: bool,
    /// The "Play in Roon" search box, per zone id.
    pub zone_query: String,
    pub zone_for: String,
}

/// Whether this computer is linked (read by the engine's play functions, any thread).
static LINKED: AtomicBool = AtomicBool::new(false);
/// Commands for the linked device that were asked for outside the player's update (a page's
/// Play while linked); the player sends them on its next tick.
static PENDING: Mutex<Vec<Value>> = Mutex::new(Vec::new());

pub fn is_linked() -> bool {
    LINKED.load(Ordering::SeqCst)
}

fn set_linked(on: bool) {
    LINKED.store(on, Ordering::SeqCst);
}

fn sendable(tracks: &[Track]) -> Vec<Value> {
    tracks.iter().filter_map(super::queue::slim).collect()
}

/// A page's Play while linked: queue a `play_queue` for the linked device. False when this
/// computer plays itself.
pub fn forward_play(tracks: &[Track], index: usize) -> bool {
    if !is_linked() {
        return false;
    }
    let first = tracks.get(index).and_then(super::queue::lib_id);
    let out = sendable(tracks);
    let mut g = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    if out.is_empty() {
        g.push(json!({ "error": "Only songs from your library can be played on another device" }));
    } else {
        let index = first
            .and_then(|f| out.iter().position(|t| t["id"].as_i64() == Some(f)))
            .unwrap_or(0);
        g.push(json!({ "action": "play_queue", "tracks": out, "index": index }));
    }
    true
}

/// Add to queue / Play next while linked.
pub fn forward_queue(tracks: &[Track], next: bool) -> bool {
    if !is_linked() {
        return false;
    }
    let out = sendable(tracks);
    if !out.is_empty() {
        PENDING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(json!({ "action": if next { "play_next" } else { "enqueue" }, "tracks": out }));
    }
    true
}

/// The commands queued since the last call.
pub fn take_pending() -> Vec<Value> {
    std::mem::take(&mut *PENDING.lock().unwrap_or_else(|e| e.into_inner()))
}

// ----- the saved link ------------------------------------------------------------------

fn link_path() -> std::path::PathBuf {
    crate::config::dir().join("link.json")
}

/// `{id, name}` of the device this computer was linked to when it last ran.
pub fn saved_link() -> Option<(String, String)> {
    let v: Value = serde_json::from_slice(&std::fs::read(link_path()).ok()?).ok()?;
    let id = fmt::s(&v, "id");
    (!id.is_empty()).then(|| (id, fmt::s(&v, "name")))
}

fn save_link(l: Option<&Linked>) {
    let p = link_path();
    match l {
        Some(l) => {
            if let Some(parent) = p.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&p, json!({ "id": l.id, "name": l.name }).to_string());
        }
        None => {
            let _ = std::fs::remove_file(&p);
        }
    }
}

impl Devices {
    /// Lock this computer as a remote for `d` (a device row, or `{id, name}`).
    pub fn link_to(&mut self, d: &Value) {
        let l = Linked {
            id: fmt::s(d, "id"),
            name: fmt::s(d, "name"),
            state: d["state"].clone(),
            online: true,
            missed: 0,
        };
        save_link(Some(&l));
        set_linked(true);
        self.linked = Some(l);
    }

    pub fn unlink(&mut self) {
        self.linked = None;
        save_link(None);
        set_linked(false);
    }

    /// A new device list from the server: remember it, refresh the linked device's state (or
    /// mark it offline after two listings without it).
    pub fn got(&mut self, list: Vec<Value>, me: &str) {
        self.list = list.into_iter().filter(|d| fmt::s(d, "id") != me).collect();
        self.loaded = true;
        if let Some(l) = self.linked.as_mut() {
            match self.list.iter().find(|d| fmt::s(d, "id") == l.id) {
                Some(d) => {
                    l.state = d["state"].clone();
                    l.online = true;
                    l.missed = 0;
                }
                None => {
                    l.missed += 1;
                    if l.missed > 1 {
                        l.online = false;
                    }
                }
            }
        }
    }

    /// Between check-ins the linked device's seek bar moves along on its own.
    pub fn tick(&mut self) {
        if let Some(l) = self.linked.as_mut() {
            if fmt::truthy(&l.state, "playing") {
                let p = fmt::f(&l.state, "position") + 1.0;
                l.state["position"] = json!(p);
            }
        }
    }

    /// Note a change we asked the linked device for, so the bar shows it at once.
    pub fn assume(&mut self, key: &str, v: Value) {
        if let Some(l) = self.linked.as_mut() {
            if l.state.is_object() {
                l.state[key] = v;
            } else {
                l.state = json!({ key: v });
            }
        }
    }
}

// ----- requests ------------------------------------------------------------------------

/// `GET /api/remote/devices`.
pub fn fetch(api: Arc<Api>) -> Task<Message> {
    Task::perform(async move { api.get("/remote/devices").await }, Message::DevicesList)
}

/// `GET /api/roon/zones` (when the server has Roon).
pub fn fetch_zones(api: Arc<Api>) -> Task<Message> {
    Task::perform(async move { api.get("/roon/zones").await }, Message::Zones)
}

/// `POST /api/remote/{id}/command` with `from` = this computer's name.
pub fn send(api: Arc<Api>, id: String, mut body: Value) -> Task<Message> {
    if !body.is_object() {
        body = json!({});
    }
    body["from"] = Value::String(crate::engine::device_name());
    Task::perform(
        async move {
            api.post(&format!("/remote/{}/command", crate::pages::urlencode(&id)), body)
                .await
        },
        Message::Sent,
    )
}

/// A Roon zone command or search.
pub fn zone(api: Arc<Api>, id: String, what: &str, body: Value) -> Task<Message> {
    let path = format!("/roon/zones/{}/{what}", crate::pages::urlencode(&id));
    Task::perform(async move { api.post(&path, body).await }, Message::ZoneDone)
}


// ----- the window ------------------------------------------------------------------------

fn cover_el<'a>(ctx: &Ctx<'a>, cover: &str, size: f32) -> El<'a, Message> {
    let p = ctx.look;
    let h = if cover.is_empty() {
        None
    } else {
        ctx.covers.get(&crate::widgets::cover::absolute(&ctx.base(), &with_size(cover, 80)))
    };
    match h {
        Some(h) => image(h)
            .width(size)
            .height(size)
            .content_fit(ContentFit::Cover)
            .border_radius(6.0)
            .into(),
        None => container(icon(Icon::Music, size * 0.4, p.faint))
            .width(size)
            .height(size)
            .center(size)
            .style(theme::art)
            .into(),
    }
}

/// A library cover path with a thumbnail size (other covers as they are).
pub fn with_size(cover: &str, size: u32) -> String {
    if !cover.contains("/api/library/albums/") {
        return cover.to_string();
    }
    let (path, q) = cover.split_once('?').unwrap_or((cover, ""));
    let mut parts: Vec<String> = q
        .split('&')
        .filter(|kv| !kv.is_empty() && !kv.starts_with("s="))
        .map(str::to_string)
        .collect();
    parts.insert(0, format!("s={size}"));
    format!("{path}?{}", parts.join("&"))
}

fn device_card<'a>(d: &'a Value, state: &'a State, ctx: &Ctx<'a>, me: &str, playing_here: bool) -> El<'a, Message> {
    let p = ctx.look;
    let id = fmt::s(d, "id");
    let name = fmt::s(d, "name");
    let s = &d["state"];
    let linked = state.devices.linked.as_ref().is_some_and(|l| l.id == id);
    let remote_of = fmt::s(s, "remote_of");
    let is_remote = !remote_of.is_empty();

    let now: El<'a, Message> = if is_remote {
        let who = if remote_of == me {
            "this device".to_string()
        } else {
            state
                .devices
                .list
                .iter()
                .find(|x| fmt::s(x, "id") == remote_of)
                .map(|x| fmt::s(x, "name"))
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "another device".into())
        };
        row![icon(Icon::Link, 14.0, p.muted), widgets::small_muted(format!("Remote for {who}"))]
            .spacing(6)
            .align_y(Alignment::Center)
            .into()
    } else if fmt::truthy(s, "title") {
        let label = fmt::s(&s["signal"], "label");
        let album = fmt::s(s, "album");
        let line = format!(
            "{}{}",
            fmt::s(s, "artist"),
            if album.is_empty() { String::new() } else { format!(" · {album}") }
        );
        let state_line = format!(
            "{} · {} / {}{}",
            if fmt::truthy(s, "playing") { "Playing" } else { "Paused" },
            fmt::mmss(fmt::f(s, "position")),
            fmt::mmss(fmt::f(s, "duration")),
            if label.is_empty() { String::new() } else { format!(" · {label}") }
        );
        row![
            cover_el(ctx, &fmt::s(s, "cover"), 52.0),
            column![
                t(fmt::s(s, "title")).size(14).font(theme::semibold()),
                widgets::small_muted(line),
                widgets::faint(state_line).size(12),
            ]
            .spacing(2)
            .width(Length::Fill)
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .into()
    } else {
        widgets::small_muted("Nothing playing").into()
    };

    let mut head = row![
        icon(Icon::Devices, 16.0, p.text),
        t(name.clone()).size(14).font(theme::bold()),
        space().width(Length::Fill)
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    if !is_remote {
        if linked {
            head = head.push(widgets::btn_icon(Icon::Link, "Linked · Unlink", Btn::Primary, Some(Message::Unlink)));
        } else {
            head = head.push(widgets::btn_icon(
                Icon::Link,
                "Link as remote",
                Btn::Default,
                Some(Message::LinkTo(d.clone())),
            ));
        }
    }

    let mut card = column![head, now].spacing(10);
    if !is_remote {
        let mut controls = row![].spacing(8).align_y(Alignment::Center);
        if fmt::truthy(s, "title") {
            controls = controls.push(icon_btn(
                Icon::Prev,
                "Previous",
                Some(Message::Command(id.clone(), json!({ "action": "prev" }))),
            ));
            controls = controls.push(widgets::btn_icon(
                if fmt::truthy(s, "playing") { Icon::Pause } else { Icon::Play },
                if fmt::truthy(s, "playing") { "Pause" } else { "Play" },
                Btn::Default,
                Some(Message::Command(id.clone(), json!({ "action": "toggle" }))),
            ));
            controls = controls.push(icon_btn(
                Icon::Next,
                "Next",
                Some(Message::Command(id.clone(), json!({ "action": "next" }))),
            ));
            let vol = s["volume"].as_f64().unwrap_or(0.9).clamp(0.0, 1.0);
            let vid = id.clone();
            controls = controls.push(
                slider(0.0..=1.0, vol, move |v| {
                    Message::Command(vid.clone(), json!({ "action": "volume", "value": (v * 20.0).round() / 20.0 }))
                })
                .step(0.05)
                .width(110)
                .style(theme::slider_style),
            );
            if !linked {
                controls = controls.push(iced::widget::tooltip(widgets::btn_sm("Play here", Btn::Default, Some(Message::PlayHere(d.clone()))), crate::widgets::tip_box("Continue this song on this device"), iced::widget::tooltip::Position::Bottom).gap(4));
            }
        }
        if playing_here && state.devices.linked.is_none() {
            controls = controls.push(widgets::btn_sm(
                "Send my music there",
                Btn::Primary,
                Some(Message::SendThere(id.clone())),
            ));
        }
        card = card.push(controls.wrap());
    }
    container(card)
        .padding(14)
        .width(Length::Fill)
        .style(theme::card)
        .into()
}

fn zone_card<'a>(z: &'a Value, state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let p = ctx.look;
    let id = fmt::s(z, "zone_id");
    let name = fmt::s(z, "name");
    let np = &z["now_playing"];
    let st = fmt::s(z, "state");
    let now: El<'a, Message> = if np.is_object() {
        let length = fmt::f(np, "length");
        let state_line = format!(
            "{}{}",
            match st.as_str() {
                "playing" => "Playing",
                "paused" => "Paused",
                "loading" => "Loading",
                _ => "Stopped",
            },
            if length > 0.0 {
                format!(" · {} / {}", fmt::mmss(fmt::f(np, "seek_position")), fmt::mmss(length))
            } else {
                String::new()
            }
        );
        let line3 = fmt::s(np, "line3");
        column![
            t(fmt::s(np, "line1")).size(14).font(theme::semibold()),
            widgets::small_muted(format!(
                "{}{}",
                fmt::s(np, "line2"),
                if line3.is_empty() { String::new() } else { format!(" · {line3}") }
            )),
            widgets::faint(state_line).size(12),
        ]
        .spacing(2)
        .into()
    } else {
        widgets::small_muted("Nothing playing").into()
    };
    let head = row![
        icon(Icon::Devices, 16.0, p.text),
        t(name).size(14).font(theme::bold()),
        badge("Roon", Badge::Plain),
        space().width(Length::Fill),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let mut controls = row![
        icon_btn(
            Icon::Prev,
            "Previous",
            fmt::truthy(z, "is_previous_allowed").then(|| Message::ZoneCommand(id.clone(), json!({ "action": "prev" })))
        ),
        widgets::btn_icon(
            if st == "playing" { Icon::Pause } else { Icon::Play },
            if st == "playing" { "Pause" } else { "Play" },
            Btn::Default,
            Some(Message::ZoneCommand(id.clone(), json!({ "action": "toggle" })))
        ),
        icon_btn(
            Icon::Next,
            "Next",
            fmt::truthy(z, "is_next_allowed").then(|| Message::ZoneCommand(id.clone(), json!({ "action": "next" })))
        ),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let vol = fmt::items(&z["outputs"])
        .into_iter()
        .find(|o| o["volume"].is_object())
        .map(|o| {
            let v = &o["volume"];
            let (min, max) = (fmt::f(v, "min"), fmt::f(v, "max"));
            let span = if max - min == 0.0 { 1.0 } else { max - min };
            ((fmt::f(v, "value") - min) / span).clamp(0.0, 1.0)
        });
    if let Some(v) = vol {
        let zid = id.clone();
        controls = controls.push(
            slider(0.0..=1.0, v, move |x| {
                Message::ZoneCommand(zid.clone(), json!({ "action": "volume", "value": (x * 50.0).round() / 50.0 }))
            })
            .step(0.02)
            .width(110)
            .style(theme::slider_style),
        );
    }
    let qid = id.clone();
    let search = row![
        text_input("Play in Roon: artist, album or song", if state.devices.zone_for == id { &state.devices.zone_query } else { "" })
            .on_input(move |q| Message::ZoneQuery(qid.clone(), q))
            .on_submit(Message::ZonePlay(id.clone()))
            .padding([7, 10])
            .size(13)
            .style(theme::input),
        iced::widget::tooltip(widgets::btn_sm("Play in Roon…", Btn::Default, Some(Message::ZonePlay(id.clone()))), crate::widgets::tip_box("Search Roon's library and play the first match here"), iced::widget::tooltip::Position::Bottom).gap(4),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    container(column![head, now, controls.wrap(), search].spacing(10))
        .padding(14)
        .width(Length::Fill)
        .style(theme::card)
        .into()
}

/// The body of "Your devices".
pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let p = ctx.look;
    let me = crate::link::device_id(&ctx.config.server);
    let mut col = column![].spacing(12).width(Length::Fill);
    let mut me_row = row![
        widgets::small_muted("This device: "),
        t(crate::engine::device_name()).size(13).font(theme::semibold()),
        space().width(Length::Fill),
    ]
    .spacing(4)
    .align_y(Alignment::Center);
    if let Some(l) = state.devices.linked.as_ref() {
        me_row = me_row.push(widgets::btn_icon(
            Icon::Link,
            format!("Unlink from {}", l.name),
            Btn::Default,
            Some(Message::Unlink),
        ));
    }
    col = col.push(me_row);
    if let Some(l) = state.devices.linked.as_ref() {
        col = col.push(
            row![
                icon(Icon::Link, 14.0, p.muted),
                widgets::small_muted(format!(
                    "Linked: everything you play here goes to {}. Unlink to listen on this device.",
                    l.name
                ))
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        );
    }
    let roon = ctx.has_feature("roon");
    if !state.devices.loaded {
        col = col.push(widgets::skeleton_rows(2));
    } else if state.devices.list.is_empty() && state.devices.zones.is_empty() {
        col = col.push(
            widgets::muted(format!(
                "No other devices right now. Open Encore on your phone, computer or the apps (signed in as you) and they show up here.{}",
                if roon {
                    " Roon zones appear once Encore is connected to your Roon Core (Settings → Apps & sources → Roon)."
                } else {
                    ""
                }
            ))
            .size(14),
        );
    } else {
        for z in &state.devices.zones {
            col = col.push(zone_card(z, state, ctx));
        }
        let playing_here = state.status.track.is_some();
        for d in &state.devices.list {
            col = col.push(device_card(d, state, ctx, &me, playing_here));
        }
    }
    col.into()
}

/// The server's answer to a command, as a toast when it failed.
pub fn sent(r: Result<Value, ApiError>, ctx: &mut Ctx) {
    if let Err(e) = r {
        ctx.toast_err(e.message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_get_their_size_and_forwarding_follows_the_link() {
        assert_eq!(with_size("/api/library/albums/a/cover", 80), "/api/library/albums/a/cover?s=80");
        assert_eq!(
            with_size("http://s/api/library/albums/a/cover?s=600&v=3", 80),
            "http://s/api/library/albums/a/cover?s=80&v=3"
        );
        assert_eq!(with_size("https://x/y.jpg", 80), "https://x/y.jpg");
        set_linked(false);
        let t = Track {
            id: Some("5".into()),
            source: cadenza::decode::Source::Url {
                url: "http://s/api/library/tracks/5/stream".into(),
                headers: vec![],
            },
            ..Default::default()
        };
        assert!(!forward_play(std::slice::from_ref(&t), 0));
        set_linked(true);
        assert!(forward_play(&[t.clone(), Track::default()], 0));
        assert!(forward_queue(&[Track::default()], true), "a file-only list goes nowhere but counts as handled");
        let pending = take_pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0]["action"], "play_queue");
        assert_eq!(pending[0]["tracks"].as_array().unwrap().len(), 1);
        assert!(forward_play(&[Track::default()], 0));
        assert!(take_pending()[0]["error"].is_string());
        set_linked(false);
    }
}
