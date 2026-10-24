//! "Get the rest of the album" for a library album MusicBrainz doesn't list (the album page's ⋯ menu; the web's
//! web-rs/src/pages/rest_of_album.rs). `GET /api/discover/album-lookup` found the album either on another
//! catalogue (Deezer, Apple Music, Discogs) – its track list is shown with Request on each missing song and
//! "Request the N missing songs" – or nowhere: then its songs can be typed in (each missing one is requested)
//! or the download sources searched for the album by name.

use crate::api::ApiError;
use crate::fmt;
use crate::pages::{Ctx, El};
use crate::theme::{self, Btn};
use crate::widgets::text_area::text_area;
use crate::widgets::{self, btn_icon, catalog, modal, small_muted, t, Icon};
use iced::widget::{column, container, row, text_editor, toggler};
use iced::{Alignment, Length, Task};
use serde_json::{json, Map, Value};
use std::collections::HashSet;

/// The dialog's state.
#[derive(Debug)]
pub struct State {
    /// the library album's artist, as the lookup was asked
    pub artist: String,
    /// the library album's title, as the lookup was asked
    pub album: String,
    /// the lookup's answer: `found` "elsewhere" (with `tracks`, `source`, `ext` …) or "none" (with `message`)
    pub found: Value,
    /// the typed track list (when no catalogue has the album)
    pub lines: text_editor::Content,
    /// lines of the typed list are "Artist - Title"
    pub artist_lines: bool,
    /// the songs (by [`catalog::song_key`]) whose request is on its way or made
    pub sent: HashSet<String>,
    /// a request to the server is on its way (the buttons wait)
    pub busy: bool,
}

impl State {
    /// The dialog for the lookup answer `found` of `album` by `artist`.
    pub fn new(artist: String, album: String, found: Value) -> Self {
        Self { artist, album, found, lines: text_editor::Content::new(), artist_lines: false, sent: HashSet::new(), busy: false }
    }

    /// Whether a catalogue had the album (its track list is shown), else the typed list is.
    pub fn is_found(&self) -> bool {
        fmt::s(&self.found, "found") == "elsewhere"
    }

    /// The album's artist for its songs: the catalogue's, else the library album's.
    fn album_artist(&self) -> String {
        Some(fmt::s(&self.found, "artist")).filter(|a| !a.is_empty()).unwrap_or_else(|| self.artist.clone())
    }

    /// The catalogue's songs that are neither in the library nor requested (nor sent from this dialog).
    pub fn missing(&self) -> Vec<Value> {
        self.found["tracks"].as_array().into_iter().flatten()
            .filter(|tr| !fmt::truthy(tr, "in_library") && !fmt::truthy(tr, "request_status") && !self.sent.contains(&catalog::song_key(tr)))
            .cloned().collect()
    }
}

/// What the dialog does.
#[derive(Clone, Debug)]
pub enum Message {
    /// Request one song of the catalogue's list (its index).
    RequestOne(usize),
    /// Request every missing song of the catalogue's list.
    RequestAll,
    /// An edit of the typed track list.
    Lines(text_editor::Action),
    /// "Lines are Artist - Title".
    ArtistLines(bool),
    /// Request the typed list's missing songs.
    RequestTyped,
    /// Request the whole album by name (the download sources search for it).
    RequestByName,
    /// A song request's answer (its key).
    OneDone(String, Result<Value, ApiError>),
    /// A many-songs request's answer.
    ManyDone(Result<Value, ApiError>),
    /// The by-name album request's answer.
    ByNameDone(Result<Value, ApiError>),
}

/// What the album page does after a message: keep the dialog or close it.
pub enum After {
    /// keep it open
    Keep,
    /// close it
    Close,
}

/// Handle `msg`; `wrap` turns the dialog's messages into the page's.
pub fn update<M: Send + 'static>(state: &mut State, msg: Message, ctx: &mut Ctx, wrap: fn(Message) -> M) -> (Task<M>, After) {
    let api = ctx.api.clone();
    match msg {
        Message::RequestOne(i) => {
            let Some(tr) = state.found["tracks"].as_array().and_then(|a| a.get(i)).cloned() else { return (Task::none(), After::Keep) };
            let key = catalog::song_key(&tr);
            state.sent.insert(key.clone());
            let mut j = song_body(&tr, &state.album_artist(), &state.found);
            j.insert("kind".into(), json!("track"));
            let body = Value::Object(j);
            (Task::perform(async move { api.post("/requests", body).await }, move |r| wrap(Message::OneDone(key.clone(), r))), After::Keep)
        }
        Message::RequestAll => {
            let missing = state.missing();
            for tr in &missing {
                state.sent.insert(catalog::song_key(tr));
            }
            state.busy = true;
            let body = json!({ "artist": state.album_artist(), "album": state.found["title"], "year": state.found["year"], "cover": state.found["cover"], "tracks": missing });
            (Task::perform(async move { api.post("/discover/album-rest/request", body).await }, move |r| wrap(Message::ManyDone(r))), After::Keep)
        }
        Message::Lines(a) => {
            state.lines.perform(a);
            (Task::none(), After::Keep)
        }
        Message::ArtistLines(on) => {
            state.artist_lines = on;
            (Task::none(), After::Keep)
        }
        Message::RequestTyped => {
            let text = state.lines.text();
            if text.trim().is_empty() {
                ctx.toast_warn("Type the album's songs, one per line");
                return (Task::none(), After::Keep);
            }
            state.busy = true;
            let body = json!({ "artist": state.artist, "album": state.album, "text": text, "artist_lines": state.artist_lines });
            (Task::perform(async move { api.post("/discover/album-rest/request", body).await }, move |r| wrap(Message::ManyDone(r))), After::Keep)
        }
        Message::RequestByName => {
            state.busy = true;
            // the whole album by name: each download source searches for it, and the songs already here don't stop it
            let body = json!({ "kind": "album", "artist": state.artist, "title": state.album, "album": state.album, "fill_in": true });
            (Task::perform(async move { api.post("/requests", body).await }, move |r| wrap(Message::ByNameDone(r))), After::Keep)
        }
        Message::OneDone(key, r) => {
            match r {
                Ok(r) => ctx.toast_ok(if fmt::s(&r, "status") == "pending" { "Request sent – waiting for approval" } else { "Request queued" }),
                Err(e) => {
                    state.sent.remove(&key);
                    ctx.toast_err(e.message);
                }
            }
            (Task::none(), After::Keep)
        }
        Message::ManyDone(r) => {
            state.busy = false;
            match r {
                Ok(r) => {
                    ctx.toast_ok(many_text(&r));
                    (Task::none(), After::Close)
                }
                Err(e) => {
                    ctx.toast_err(e.message);
                    // the songs can be asked for again
                    state.sent.clear();
                    (Task::none(), After::Keep)
                }
            }
        }
        Message::ByNameDone(r) => {
            state.busy = false;
            match r {
                Ok(r) => {
                    ctx.toast_ok(if fmt::s(&r, "status") == "pending" { "Album requested – waiting for approval" } else { "Album requested – the download sources search for it by name" });
                    (Task::none(), After::Close)
                }
                Err(e) => {
                    ctx.toast_err(e.message);
                    (Task::none(), After::Keep)
                }
            }
        }
    }
}

/// The toast for `POST /api/discover/album-rest/request`'s answer `r`.
pub fn many_text(r: &Value) -> String {
    let n = r["requested"].as_i64().unwrap_or(0);
    let already = r["already_requested"].as_i64().unwrap_or(0);
    if n == 0 {
        return "Every missing song is already requested".into();
    }
    let mut s = format!("Requested {n} song{}", if n == 1 { "" } else { "s" });
    if already > 0 {
        s.push_str(&format!(" ({already} already requested)"));
    }
    if fmt::s(r, "status") == "pending" {
        s.push_str(" – waiting for approval");
    }
    s
}

/// A song request's fields for the catalogue's song `tr` of the album `found` (artist, title, album, cover,
/// year, length, ISRC; only this song).
fn song_body(tr: &Value, artist: &str, found: &Value) -> Map<String, Value> {
    let mut j = Map::new();
    j.insert("artist".into(), json!(if fmt::truthy(tr, "artist") { fmt::s(tr, "artist") } else { artist.to_string() }));
    catalog::copy(&mut j, "title", tr, "title");
    catalog::copy(&mut j, "album", found, "title");
    catalog::copy(&mut j, "cover_url", found, "cover");
    catalog::copy(&mut j, "year", found, "year");
    if fmt::truthy(tr, "length") {
        catalog::copy(&mut j, "length", tr, "length");
    }
    if fmt::truthy(tr, "isrc") {
        catalog::copy(&mut j, "isrc", tr, "isrc");
    }
    j.insert("album_mode".into(), json!("song"));
    j
}

/// The dialog; `wrap` turns its messages into the page's, `close` closes it.
pub fn view<'a, M: Clone + 'a>(state: &'a State, wrap: fn(Message) -> M, close: M) -> El<'a, M> {
    if state.is_found() { found_view(state, wrap, close) } else { typed_view(state, wrap, close) }
}

/// The catalogue's track list with Request on each missing song.
fn found_view<'a, M: Clone + 'a>(state: &'a State, wrap: fn(Message) -> M, close: M) -> El<'a, M> {
    let tracks = state.found["tracks"].as_array().map(|a| a.as_slice()).unwrap_or(&[]);
    let missing = state.missing().len();
    let mut col = column![small_muted(format!("Not on MusicBrainz – track list from {}.", fmt::s(&state.found, "source")))].spacing(12);
    if tracks.is_empty() {
        col = col.push(widgets::muted("No track list available."));
    }
    let mut list = column![];
    for (i, tr) in tracks.iter().enumerate() {
        let mut lines = column![t(fmt::s(tr, "title")).size(14).font(theme::semibold())].spacing(2);
        if fmt::truthy(tr, "artist") {
            lines = lines.push(small_muted(fmt::s(tr, "artist")).size(12.5));
        }
        let num = container(small_muted(fmt::s(tr, "track"))).width(32).align_x(Alignment::Center);
        let mut r = row![num, lines.width(Length::Fill)].spacing(12).align_y(Alignment::Center);
        if fmt::f(tr, "length") > 0.0 {
            r = r.push(small_muted(fmt::mmss(fmt::f(tr, "length"))));
        }
        if fmt::truthy(tr, "in_library") {
            r = r.push(catalog::in_library());
        } else if fmt::truthy(tr, "request_status") {
            r = r.push(catalog::live_badge(&fmt::s(tr, "request_status"), &fmt::s(tr, "request_label")));
        } else if state.sent.contains(&catalog::song_key(tr)) {
            r = r.push(catalog::live_badge("queued", ""));
        } else {
            r = r.push(btn_icon(Icon::Plus, "Track", Btn::Default, Some(wrap(Message::RequestOne(i)))));
        }
        list = list.push(column![container(r).padding([8, 10]).width(Length::Fill), widgets::divider()]);
    }
    if !tracks.is_empty() {
        col = col.push(widgets::card_flat(list));
    }
    let footer = if missing > 0 {
        let label = format!("Request the {missing} missing song{}", if missing == 1 { "" } else { "s" });
        Some(row![btn_icon(Icon::Plus, label, Btn::Primary, (!state.busy).then(|| wrap(Message::RequestAll)))].into())
    } else {
        None
    };
    modal(fmt::s(&state.found, "title"), col, footer, false, close)
}

/// No catalogue has the album: type its songs in, or search the download sources for it by name.
fn typed_view<'a, M: Clone + 'a>(state: &'a State, wrap: fn(Message) -> M, close: M) -> El<'a, M> {
    let message = Some(fmt::s(&state.found, "message")).filter(|m| !m.is_empty()).unwrap_or_else(|| format!("No catalogue lists “{}”.", state.album));
    let body = column![
        widgets::muted(message).size(14),
        small_muted("The album's songs, one per line (numbers and lengths are fine: “1. Song 3:45”)"),
        text_area(&state.lines, "1. First song\n2. Second song", 10, move |a| wrap(Message::Lines(a))),
        toggler(state.artist_lines).label("Lines are “Artist - Title”").on_toggle(move |on| wrap(Message::ArtistLines(on))),
    ]
    .spacing(10);
    let footer = row![
        btn_icon(Icon::Download, "Search downloads for the album", Btn::Default, (!state.busy).then(|| wrap(Message::RequestByName))),
        btn_icon(Icon::Plus, "Request the missing songs", Btn::Primary, (!state.busy).then(|| wrap(Message::RequestTyped))),
    ]
    .spacing(10);
    modal(format!("Get the rest of {}", state.album), body, Some(footer.into()), false, close)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_toast_counts_what_was_asked_for() {
        assert_eq!(many_text(&json!({ "requested": 0, "already_requested": 2 })), "Every missing song is already requested");
        assert_eq!(many_text(&json!({ "requested": 1, "already_requested": 0, "status": "queued" })), "Requested 1 song");
        assert_eq!(many_text(&json!({ "requested": 3, "already_requested": 1, "status": "pending" })), "Requested 3 songs (1 already requested) – waiting for approval");
    }

    #[test]
    fn missing_songs_leave_out_owned_requested_and_sent_ones() {
        let found = json!({ "found": "elsewhere", "title": "Lost Tapes", "tracks": [
            { "title": "Owned", "in_library": true },
            { "title": "Asked", "request_status": "queued" },
            { "title": "Missing", "artist": "Band" },
            { "title": "Sent", "artist": "Band" },
        ] });
        let mut st = State::new("Band".into(), "Lost Tapes".into(), found);
        assert!(st.is_found());
        let sent = st.found["tracks"][3].clone();
        st.sent.insert(catalog::song_key(&sent));
        let titles: Vec<String> = st.missing().iter().map(|t| fmt::s(t, "title")).collect();
        assert_eq!(titles, vec!["Missing"]);
        assert!(!State::new("Band".into(), "X".into(), json!({ "found": "none" })).is_found());
    }
}
