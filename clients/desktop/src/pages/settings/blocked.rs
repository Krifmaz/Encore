//! Settings → Blocked. Behaviour reference: web-rs/src/pages/report.rs: the reports people made
//! (⋯ → Report a problem), the blocked sources with Undo, the songs the quality checks flagged,
//! and the switch that checks every download before it enters the library.

use super::{env_locked, help, input, switch_maybe};
use crate::api::ApiError;
use crate::fmt;
use crate::pages::{Ctx, El, Page};
use crate::theme::{self, Badge, Btn};
use crate::widgets::{self, badge, btn, btn_icon, btn_sm, modal, t, Icon};
use iced::widget::{column, container, radio, row};
use iced::{Alignment, Length, Task};
use serde_json::{json, Value};

pub const REASONS: [(&str, &str); 7] = [
    ("wrong_song", "Wrong song"),
    ("wrong_version", "Wrong version (live, remix, edit, missing parts)"),
    ("bad_audio", "Bad audio quality"),
    ("loudness", "Too quiet or too loud"),
    ("fake_lossless", "Fake lossless"),
    ("glitches", "Skips or glitches"),
    ("other", "Other"),
];

/// The reason a quality-check flag suggests.
pub fn reason_for(codes: &[String]) -> &'static str {
    let has = |c: &str| codes.iter().any(|x| x == c);
    if has("fake_lossless") || has("upsampled") || has("low_bitrate") {
        return "fake_lossless";
    }
    if has("quiet") {
        return "loudness";
    }
    if has("wrong_length") {
        return "wrong_version";
    }
    "bad_audio"
}

/// `(label, badge)` of a report's status.
pub fn status_words(s: &str) -> (&str, Badge) {
    match s {
        "replacing" => ("Looking for a replacement", Badge::Warn),
        "replaced" => ("Replaced", Badge::Ok),
        "no_replacement" => ("No replacement yet", Badge::Err),
        // lyrics reports (the song stays; only its lyrics are looked up again)
        "lyrics_searching" => ("Looking for other lyrics", Badge::Warn),
        "lyrics_found" => ("New lyrics", Badge::Ok),
        "lyrics_none" => ("No other lyrics yet", Badge::Err),
        "dismissed" => ("Dismissed", Badge::Plain),
        other => (other, Badge::Plain),
    }
}

/// The Report a problem dialog for a flagged song (Replace).
#[derive(Debug)]
pub struct Report {
    track: Value,
    reason: String,
    note: String,
    busy: bool,
}

#[derive(Debug, Default)]
pub struct State {
    values: Value,
    reports: Option<Vec<Value>>,
    blocks: Vec<Value>,
    suspect: Vec<Value>,
    busy: Option<String>,
    dialog: Option<Report>,
    error: String,
}

#[derive(Clone, Debug)]
pub enum Message {
    Values(Result<Value, ApiError>),
    Reports(Result<Value, ApiError>),
    Blocks(Result<Value, ApiError>),
    Suspect(Result<Value, ApiError>),
    QualityImport(bool),
    Saved(Result<Value, ApiError>),
    Retry(i64),
    Dismiss(i64),
    Undo(i64),
    Check(usize),
    Checked(Result<Value, ApiError>),
    Done(Result<Value, ApiError>),
    Replace(usize),
    Reason(String),
    Note(String),
    Send,
    Reported(Result<Value, ApiError>),
    GoAlbum(String),
    CloseDialog,
}

fn reload(ctx: &Ctx) -> Task<Message> {
    let api = ctx.api.clone();
    let mut tasks = vec![Task::perform(
        {
            let api = api.clone();
            async move { api.get("/reports").await }
        },
        Message::Reports,
    )];
    if ctx.is_admin() {
        let a1 = api.clone();
        let a2 = api.clone();
        tasks.push(Task::perform(async move { a1.get("/admin/settings").await }, Message::Values));
        tasks.push(Task::perform(async move { a2.get("/blocks").await }, Message::Blocks));
        tasks.push(Task::perform(async move { api.get("/quality/suspect").await }, Message::Suspect));
    }
    Task::batch(tasks)
}

pub fn open(state: &mut State, ctx: &mut Ctx) -> Task<Message> {
    state.reports = None;
    state.blocks.clear();
    state.suspect.clear();
    state.busy = None;
    state.dialog = None;
    state.error.clear();
    reload(ctx)
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Values(Ok(v)) => state.values = v,
        Message::Values(Err(_)) => {}
        Message::Reports(Ok(r)) => state.reports = Some(fmt::items(&r["reports"])),
        Message::Reports(Err(e)) => state.error = e.message,
        Message::Blocks(Ok(b)) => state.blocks = fmt::items(&b["blocks"]),
        Message::Blocks(Err(_)) => {}
        Message::Suspect(Ok(s)) => state.suspect = fmt::items(&s["tracks"]),
        Message::Suspect(Err(_)) => {}
        Message::QualityImport(on) => {
            state.values["quality_check_on_import"] = json!(on);
            let api = ctx.api.clone();
            return Task::perform(async move { api.put("/admin/settings", json!({ "quality_check_on_import": on })).await }, Message::Saved);
        }
        Message::Saved(Ok(_)) => ctx.toast_ok("Saved"),
        Message::Saved(Err(e)) => ctx.toast_err(e.message),
        Message::Retry(id) => {
            state.busy = Some(format!("retry-{id}"));
            let api = ctx.api.clone();
            return Task::perform(async move { api.post_empty(&format!("/reports/{id}/retry")).await }, |r| Message::Done(r.map(|_| json!("Trying again"))));
        }
        Message::Dismiss(id) => {
            state.busy = Some(format!("dismiss-{id}"));
            let api = ctx.api.clone();
            return Task::perform(async move { api.post_empty(&format!("/reports/{id}/dismiss")).await }, |r| Message::Done(r.map(|_| json!("Dismissed"))));
        }
        Message::Undo(id) => {
            state.busy = Some(format!("undo-{id}"));
            let api = ctx.api.clone();
            return Task::perform(async move { api.del(&format!("/blocks/{id}")).await }, |r| Message::Done(r.map(|_| json!("Unblocked – that source may be used again"))));
        }
        Message::Check(i) => {
            let Some(t) = state.suspect.get(i) else { return Task::none() };
            state.busy = Some(format!("check-{i}"));
            let id = fmt::i(t, "id");
            let api = ctx.api.clone();
            return Task::perform(async move { api.post_empty(&format!("/quality/tracks/{id}/check")).await }, Message::Checked);
        }
        Message::Checked(r) => {
            state.busy = None;
            match r {
                Ok(q) => {
                    if fmt::s(&q, "status") == "suspect" {
                        ctx.toast_err(fmt::s(&q, "summary"))
                    } else {
                        ctx.toast_ok("Looks fine now")
                    }
                    return reload(ctx);
                }
                Err(e) => ctx.toast_err(e.message),
            }
        }
        Message::Done(r) => {
            state.busy = None;
            match r {
                Ok(word) => {
                    ctx.toast_ok(super::vs(&word));
                    return reload(ctx);
                }
                Err(e) => ctx.toast_err(e.message),
            }
        }
        Message::Replace(i) => {
            let Some(t) = state.suspect.get(i).cloned() else { return Task::none() };
            let codes: Vec<String> = fmt::items(&t["issues"]).iter().map(|x| fmt::s(x, "code")).collect();
            state.dialog = Some(Report {
                reason: reason_for(&codes).into(),
                track: t,
                note: String::new(),
                busy: false,
            });
        }
        Message::Reason(r) => {
            if let Some(d) = state.dialog.as_mut() {
                d.reason = r;
            }
        }
        Message::Note(n) => {
            if let Some(d) = state.dialog.as_mut() {
                d.note = n;
            }
        }
        Message::Send => {
            if let Some(d) = state.dialog.as_mut() {
                if d.reason == "other" && d.note.trim().is_empty() {
                    ctx.toast_err("Say what is wrong");
                    return Task::none();
                }
                d.busy = true;
                let api = ctx.api.clone();
                let body = json!({ "kind": "track", "reason": d.reason, "note": d.note.trim(), "track_id": d.track["id"] });
                return Task::perform(async move { api.post("/report", body).await }, Message::Reported);
            }
        }
        Message::Reported(Ok(res)) => {
            state.dialog = None;
            ctx.toast_ok(crate::pages::library::report_toast(&res));
            return reload(ctx);
        }
        Message::Reported(Err(e)) => {
            if let Some(d) = state.dialog.as_mut() {
                d.busy = false;
            }
            ctx.toast_err(e.message);
        }
        Message::GoAlbum(id) => ctx.navigate(Page::LibraryAlbum(id)),
        Message::CloseDialog => state.dialog = None,
    }
    Task::none()
}

fn report_row<'a>(state: &'a State, r: &'a Value, admin: bool) -> El<'a, Message> {
    let p = theme::look();
    let st = fmt::s(r, "status");
    let (label, kind) = status_words(&st);
    let id = fmt::i(r, "id");
    let name = format!("{} – {}", fmt::s(r, "artist"), fmt::s(r, "title"));
    let title: El<'a, Message> = if fmt::s(r, "kind") == "album" && fmt::truthy(r, "album_id") {
        super::link(name, Message::GoAlbum(fmt::s(r, "album_id")))
    } else {
        t(name).size(14).font(theme::semibold()).into()
    };
    let mut who = Vec::new();
    if admin && fmt::truthy(r, "username") {
        who.push(fmt::s(r, "username"));
    }
    who.push(fmt::ago(fmt::f(r, "created_at")));
    if fmt::truthy(r, "source") {
        who.push(format!("from {}", fmt::s(r, "source")));
    }
    if fmt::truthy(r, "message") {
        who.push(fmt::s(r, "message"));
    }
    let reason = format!(
        "{}{}",
        if fmt::truthy(r, "reason_label") { fmt::s(r, "reason_label") } else { fmt::s(r, "reason") },
        if fmt::truthy(r, "note") { format!(" · “{}”", fmt::s(r, "note")) } else { String::new() }
    );
    let left = column![
        row![title, help(if fmt::s(r, "kind") == "album" { "album" } else { "song" })].spacing(6).align_y(Alignment::Center),
        t(reason).size(12.5),
        widgets::faint(who.join(" · ")).size(12),
    ]
    .spacing(2)
    .width(Length::Fill);
    let mut actions = row![badge(label, kind)].spacing(6).align_y(Alignment::Center);
    // Try again: an audio report with no replacement, a lyrics report with no lyrics
    if st == "no_replacement" || st == "lyrics_none" {
        actions = actions.push(btn_icon(Icon::Refresh, "Try again", Btn::Default, state.busy.is_none().then_some(Message::Retry(id))));
    }
    // Dismiss (admins): also a settled lyrics report, so the lyrics it rejected may come back
    if admin && ["replacing", "no_replacement", "lyrics_found", "lyrics_none"].contains(&st.as_str()) {
        actions = actions.push(btn_sm("Dismiss", Btn::Ghost, state.busy.is_none().then_some(Message::Dismiss(id))));
    }
    let _ = p;
    column![container(row![left, actions].spacing(10).align_y(Alignment::Center)).padding([10, 14]), widgets::divider()].into()
}

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    if !state.error.is_empty() {
        return widgets::error_box(&state.error);
    }
    let admin = ctx.is_admin();
    let mut intro: Vec<El<'a, Message>> = vec![help("When someone reports a song or album (⋯ → Report a problem), the exact source it came from is blocked – that Soulseek file or folder, torrent, Usenet post, streaming item or YouTube video – and the music is fetched again from somewhere else. Undo a block to allow that source again.").into()];
    if admin && !state.values.is_null() {
        let locked = env_locked(&state.values, "quality_check_on_import");
        intro.push(switch_maybe(
            fmt::truthy(&state.values, "quality_check_on_import"),
            "Check every download before it enters the library",
            (!locked).then_some(Message::QualityImport),
        ));
        intro.push(help("Fake lossless (a lossy file saved as FLAC), upsampled hi-res, very quiet audio and the wrong length are turned away and the next copy is tried. The Quality scan under Scheduled tasks checks the songs already in the library.").into());
    }
    let reports: El<'a, Message> = match &state.reports {
        None => widgets::skeleton_rows(2),
        Some(l) if l.is_empty() => help("Nothing reported yet.").into(),
        Some(l) => {
            let mut c = column![];
            for r in l {
                c = c.push(report_row(state, r, admin));
            }
            widgets::card_flat(c).width(Length::Fill).into()
        }
    };
    let mut page = column![super::card("Blocked", intro), super::card("Reports", vec![reports])].spacing(16).width(Length::Fill);
    if admin {
        let blocks: El<'a, Message> = if state.blocks.is_empty() {
            help("No blocked sources.").into()
        } else {
            let mut c = column![];
            for b in &state.blocks {
                let id = fmt::i(b, "id");
                let mut meta = vec![fmt::s(b, "kind")];
                if fmt::truthy(b, "reason_label") {
                    meta.push(fmt::s(b, "reason_label"));
                }
                if fmt::truthy(b, "title") {
                    meta.push(format!("{} – {}", fmt::s(b, "artist"), fmt::s(b, "title")));
                }
                if fmt::truthy(b, "username") {
                    meta.push(fmt::s(b, "username"));
                }
                meta.push(fmt::ago(fmt::f(b, "created_at")));
                c = c.push(widgets::list_row(
                    None,
                    if fmt::truthy(b, "label") { fmt::s(b, "label") } else { fmt::s(b, "key") },
                    meta.join(" · "),
                    vec![btn_sm("Undo", Btn::Default, state.busy.is_none().then_some(Message::Undo(id))).into()],
                    None::<Message>,
                ));
            }
            widgets::card_flat(c).width(Length::Fill).into()
        };
        page = page.push(super::card("Blocked sources", vec![blocks]));
        let suspect: El<'a, Message> = if state.suspect.is_empty() {
            help("Nothing flagged.").into()
        } else {
            let mut c = column![];
            for (i, tr) in state.suspect.iter().enumerate() {
                let mut sub = vec![fmt::s(tr, "format").to_uppercase()];
                if fmt::truthy(tr, "bitrate") {
                    sub.push(format!("{}k", fmt::s(tr, "bitrate")));
                }
                if fmt::truthy(tr, "album") {
                    sub.push(fmt::s(tr, "album"));
                }
                let left = column![
                    super::link(format!("{} – {}", fmt::s(tr, "artist"), fmt::s(tr, "title")), Message::GoAlbum(fmt::s(tr, "album_id"))),
                    super::warn_text(fmt::s(tr, "summary")),
                    widgets::faint(sub.join(" · ")).size(12),
                ]
                .spacing(2)
                .width(Length::Fill);
                let actions = row![
                    btn_icon(Icon::Refresh, "Check again", Btn::Ghost, state.busy.is_none().then_some(Message::Check(i))),
                    btn_sm("Replace", Btn::Primary, state.busy.is_none().then_some(Message::Replace(i))),
                ]
                .spacing(6);
                c = c.push(column![container(row![left, actions].spacing(10).align_y(Alignment::Center)).padding([10, 14]), widgets::divider()]);
            }
            widgets::card_flat(c).width(Length::Fill).into()
        };
        page = page.push(super::card(
            "Suspect quality",
            vec![help("Songs the quality checks flagged. Replace one to block where it came from and get it again.").into(), suspect],
        ));
    }
    page.into()
}

pub fn overlay<'a>(state: &'a State, _ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    let d = state.dialog.as_ref()?;
    let title = fmt::s(&d.track, "title");
    let mut c = column![help(format!("What is wrong with “{title}”? Encore stops using the source this song came from, moves the file to the trash for 7 days and gets the song again from somewhere else. Plays, likes and playlists stay with the song."))].spacing(8);
    for (k, l) in REASONS {
        c = c.push(radio(l, k, Some(d.reason.as_str()), |v: &str| Message::Reason(v.to_string())).size(16).text_size(14).spacing(8));
    }
    c = c.push(widgets::field("Note", input("Optional – e.g. the intro is missing", &d.note, Message::Note).width(Length::Fill)));
    Some(modal(
        "Report a problem",
        c,
        Some(
            row![
                btn("Cancel", Btn::Default, Some(Message::CloseDialog)),
                btn(if d.busy { "Reporting…" } else { "Report" }, Btn::Primary, (!d.busy).then_some(Message::Send)),
            ]
            .spacing(10)
            .into(),
        ),
        false,
        Message::CloseDialog,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasons_and_status_words() {
        assert_eq!(reason_for(&["upsampled".to_string()]), "fake_lossless");
        assert_eq!(reason_for(&["quiet".to_string()]), "loudness");
        assert_eq!(reason_for(&["wrong_length".to_string()]), "wrong_version");
        assert_eq!(reason_for(&[]), "bad_audio");
        assert_eq!(status_words("replaced").0, "Replaced");
        assert_eq!(status_words("odd").0, "odd");
        assert_eq!(REASONS.len(), 7);
    }
}
