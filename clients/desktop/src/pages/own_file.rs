//! Bring your own music: ⋯ → *Replace with my own file* on a song, and Library → *Upload your own files* (a
//! batch matched to the songs by their tags, the sure matches chosen, replaced once you confirm). Behaviour
//! reference: web-rs/src/pages/own_file.rs; server: routes::own_file. The song keeps its id, so plays, likes,
//! playlists and lyrics stay; the old file waits in the server's trash for 7 days. Admins, and accounts with
//! the "own_files" permission.
//!
//! The dialogs live with the song menu's ([`super::library::Dialog`]); this file holds their state, their
//! views and what they send.

use super::library::Act;
use crate::api::{Api, ApiError};
use crate::fmt;
use crate::pages::{Ctx, El};
use crate::theme::{self, Btn};
use crate::widgets::drop_zone::{self, Accept, DropZone};
use crate::widgets::{btn, btn_icon, modal, small_muted, t, Icon};
use iced::widget::{column, container, pick_list, row, scrollable};
use iced::{Alignment, Length};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

/// The extensions the file chooser offers (the server checks the bytes too).
pub const EXTS: [&str; 14] = ["flac", "mp3", "m4a", "mp4", "aac", "ogg", "opus", "wav", "aiff", "aif", "wma", "ape", "wv", "alac"];

/// What Replace with my own file takes, picked or dropped: one audio file.
pub const ONE: Accept = Accept::one(&EXTS);
/// What Upload your own files takes, picked or dropped: any number of audio files.
pub const MANY: Accept = Accept::many(&EXTS);

/// May this account (`user`, as `/api/auth/me` gives it) replace songs' files? Admins, and accounts with the
/// "own_files" permission.
pub fn allowed_for(user: &Value) -> bool {
    matches!(fmt::s(user, "role").as_str(), "admin" | "owner") || fmt::truthy(&user["perms"], "own_files")
}

/// "FLAC · 24-bit / 96 kHz" of a file's facts (`{format, bit_depth, sample_rate, bitrate}`).
pub fn quality_text(f: &Value) -> String {
    let mut parts = vec![fmt::s(f, "format").to_uppercase()];
    let (depth, rate) = (fmt::i(f, "bit_depth"), fmt::i(f, "sample_rate"));
    if depth > 0 && rate > 0 {
        parts.push(format!("{depth}-bit / {} kHz", khz(rate)));
    } else if fmt::i(f, "bitrate") > 0 {
        parts.push(format!("{} kbps", fmt::i(f, "bitrate")));
    }
    parts.retain(|p| !p.is_empty());
    parts.join(" · ")
}

/// 44100 → "44.1", 96000 → "96".
fn khz(hz: i64) -> String {
    if hz % 1000 == 0 {
        (hz / 1000).to_string()
    } else {
        format!("{:.1}", hz as f64 / 1000.0)
    }
}

/// The line under a replaced song: "MP3 · 320 kbps → FLAC · 24-bit / 96 kHz", then any warnings.
pub fn result_text(v: &Value) -> String {
    let mut s = format!("{} → {}", quality_text(&v["before"]), quality_text(&v["after"]));
    for w in v["warnings"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        s.push_str(&format!(". {w}"));
    }
    if fmt::s(&v["quality"], "status") == "suspect" {
        s.push_str(&format!(". Quality check: {}", fmt::s(&v["quality"], "summary")));
    }
    s
}

/// "Artist – Title" of a song summary.
fn song_name(t: &Value) -> String {
    format!("{} – {}", fmt::s(t, "artist"), fmt::s(t, "title"))
}

/// "Your own file – uploaded by alex on 24 Oct 2026, replacing an MP3" when someone replaced the song's file
/// with their own (`/info`'s `own_file`; the Signal path's Source note), else "".
pub fn own_file_note(info: Option<&Value>) -> String {
    let Some(o) = info.map(|i| &i["own_file"]).filter(|o| o.is_object()) else { return String::new() };
    let mut s = "Your own file".to_string();
    if fmt::truthy(o, "by") {
        s.push_str(&format!(" – uploaded by {}", fmt::s(o, "by")));
    }
    if fmt::f(o, "at") > 0.0 {
        s.push_str(&format!(" on {}", fmt::date(fmt::f(o, "at"))));
    }
    if fmt::truthy(o, "old_format") {
        s.push_str(&format!(", replacing {}", article(&fmt::s(o, "old_format").to_uppercase())));
    }
    s
}

/// "an MP3" / "a FLAC": the article a format's name takes when read out.
pub fn article(name: &str) -> String {
    // letters said with a vowel sound first: A, E, F, H, I, L, M, N, O, R, S, X
    let vowel = name.chars().next().is_some_and(|c| "AEFHILMNORSX".contains(c)) && name != "FLAC";
    format!("{} {name}", if vowel { "an" } else { "a" })
}

// ---- one song ----------------------------------------------------------------------------------------

/// ⋯ → Replace with my own file: the song, and how the upload went.
#[derive(Debug)]
pub struct ReplaceState {
    /// the song whose file is replaced
    pub track: Value,
    /// choosing or uploading a file
    pub busy: bool,
    /// "Replaced: MP3 → FLAC …" once it worked
    pub done: String,
    /// why it didn't
    pub error: String,
    /// a file dragged onto the window takes the place of the chooser
    pub drop: DropZone,
}

impl ReplaceState {
    /// The dialog for song `t`, nothing chosen yet.
    pub fn new(t: Value) -> ReplaceState {
        ReplaceState { track: t, busy: false, done: String::new(), error: String::new(), drop: DropZone::default() }
    }

    /// Takes a file now (nothing is uploading and nothing was replaced yet)?
    pub fn takes_files(&self) -> bool {
        !self.busy && self.done.is_empty()
    }
}

/// Choose an audio file for Replace with my own file. → None when nothing was chosen.
pub async fn pick_file() -> Option<PathBuf> {
    let f = rfd::AsyncFileDialog::new().set_title("Replace with my own file").add_filter("Audio", ONE.exts).pick_file().await?;
    Some(f.path().to_path_buf())
}

/// Uploads `path` (picked or dropped) in place of song `id`'s file.
pub async fn replace(api: Arc<Api>, id: i64, path: PathBuf) -> Result<Value, ApiError> {
    upload(&api, &format!("/library/tracks/{id}/own-file"), &path).await
}

/// One file as the multipart `file` part to `path` (no time limit but the slow one: a hi-res file is big).
async fn upload(api: &Api, path: &str, file: &std::path::Path) -> Result<Value, ApiError> {
    let part = Api::file_part(file).await?;
    api.post_form_slow(path, reqwest::multipart::Form::new().part("file", part)).await
}

/// The Replace with my own file dialog.
pub fn replace_view<'a>(s: &'a ReplaceState) -> El<'a, Act> {
    let mut body = column![small_muted(format!(
        "Your file takes the place of “{}” for everyone. It is tagged as this song, and the song keeps its plays, likes, playlists and lyrics. The file it replaces stays in the trash for 7 days.",
        fmt::s(&s.track, "title")
    ))
    .size(13)]
    .spacing(14);
    let label = if s.busy { "Uploading and checking…" } else { "Choose a file…" };
    let pick = btn_icon(Icon::Disc, label, Btn::Default, s.takes_files().then_some(Act::ReplaceOwnPick));
    body = body.push(drop_zone::view(pick, &s.drop, "Drop the audio file here to use it"));
    if !s.done.is_empty() {
        body = body.push(t(format!("Replaced: {}", s.done)).size(13).color(theme::look().ok));
    }
    if !s.error.is_empty() {
        body = body.push(t(s.error.clone()).size(13).color(theme::look().err));
    }
    let footer = row![btn("Close", Btn::Primary, Some(Act::CloseDialog))].spacing(10);
    modal("Replace with my own file", body, Some(footer.into()), false, Act::CloseDialog)
}

// ---- a batch -------------------------------------------------------------------------------------------

/// One choice of a staged upload's list: a song (`id`) or "Leave out" (None).
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    /// the song, or None to leave the file out
    pub id: Option<i64>,
    /// what the list shows
    pub label: String,
}

impl std::fmt::Display for Choice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

/// The choices of a staged upload: "Leave out", then its candidates ("Artist – Title (Album)").
pub fn choices(staged: &Value) -> Vec<Choice> {
    let mut out = vec![Choice { id: None, label: "Leave out".into() }];
    for c in staged["candidates"].as_array().into_iter().flatten() {
        let t = &c["track"];
        let album = if fmt::truthy(t, "album") { format!(" ({})", fmt::s(t, "album")) } else { String::new() };
        out.push(Choice { id: Some(fmt::i(t, "id")), label: format!("{}{album}", song_name(t)) });
    }
    out
}

/// The song a staged upload starts with: its sure match, else none.
pub fn default_pick(staged: &Value) -> Option<i64> {
    staged["match"]["track"]["id"].as_i64()
}

/// One file of the batch.
#[derive(Debug, Clone)]
pub struct BulkRow {
    /// where it is on this computer
    pub path: PathBuf,
    /// its name
    pub name: String,
    /// the server's answer once it is staged (`{token, tags, match, candidates}`)
    pub staged: Option<Value>,
    /// the song it replaces (None: left out)
    pub pick: Option<i64>,
    /// why it can't be used, or how the replacement went
    pub note: String,
    /// the note is a failure
    pub failed: bool,
}

/// Library → Upload your own files.
#[derive(Debug, Default)]
pub struct BulkState {
    /// every file chosen, in order
    pub rows: Vec<BulkRow>,
    /// the file being uploaded now (its index), None when none is
    pub uploading: Option<usize>,
    /// Replace was pressed and the server is at it
    pub applying: bool,
    /// the replacements are done (the list shows how each went)
    pub applied: bool,
    /// files dragged onto the window join the chosen ones
    pub drop: DropZone,
}

impl BulkState {
    /// Takes more files now (Replace wasn't pressed)?
    pub fn takes_files(&self) -> bool {
        !self.applying && !self.applied
    }

    /// How many songs Replace would replace.
    pub fn chosen(&self) -> usize {
        self.rows.iter().filter(|r| r.staged.is_some() && r.pick.is_some()).count()
    }

    /// The next file still to upload.
    pub fn next_pending(&self) -> Option<usize> {
        self.rows.iter().position(|r| r.staged.is_none() && r.note.is_empty())
    }

    /// Adds the files chosen (a file chosen twice is there once).
    pub fn add(&mut self, paths: Vec<PathBuf>) {
        for p in paths {
            if !self.rows.iter().any(|r| r.path == p) {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.rows.push(BulkRow { path: p, name, staged: None, pick: None, note: String::new(), failed: false });
            }
        }
    }

    /// The server staged row `i` (or refused it).
    pub fn staged(&mut self, i: usize, r: Result<Value, ApiError>) {
        let Some(row) = self.rows.get_mut(i) else { return };
        match r {
            Ok(v) => {
                if let Some(s) = v["files"].as_array().and_then(|f| f.first()) {
                    row.pick = default_pick(s);
                    row.staged = Some(s.clone());
                } else {
                    let why = v["failed"][0]["reason"].as_str().unwrap_or("The server didn't take this file");
                    (row.note, row.failed) = (why.to_string(), true);
                }
            }
            Err(e) => (row.note, row.failed) = (e.message, true),
        }
    }

    /// The `{items: [{token, track_id}]}` Replace sends.
    pub fn apply_body(&self) -> Value {
        let items: Vec<Value> = self.rows.iter().filter_map(|r| Some(json!({ "token": fmt::s(r.staged.as_ref()?, "token"), "track_id": r.pick? }))).collect();
        json!({ "items": items })
    }

    /// The server replaced (or didn't) the songs: each row says how it went.
    pub fn applied(&mut self, v: &Value) {
        self.applying = false;
        self.applied = true;
        for r in &mut self.rows {
            let Some(token) = r.staged.as_ref().map(|s| fmt::s(s, "token")) else { continue };
            if let Some(ok) = v["replaced"].as_array().into_iter().flatten().find(|x| fmt::s(x, "token") == token) {
                let warn: String = ok["warnings"].as_array().into_iter().flatten().filter_map(Value::as_str).map(|w| format!(" · {w}")).collect();
                r.note = format!("Replaced {} – now {}{warn}", song_name(ok), fmt::s(ok, "format").to_uppercase());
            } else if let Some(bad) = v["failed"].as_array().into_iter().flatten().find(|x| fmt::s(x, "token") == token) {
                (r.note, r.failed) = (fmt::s(bad, "reason"), true);
            }
        }
    }
}

/// Choose audio files for the batch.
pub async fn pick_files() -> Vec<PathBuf> {
    let picked = rfd::AsyncFileDialog::new().set_title("Upload your own files").add_filter("Audio", MANY.exts).pick_files().await;
    picked.unwrap_or_default().iter().map(|f| f.path().to_path_buf()).collect()
}

/// Uploads one file of the batch: staged and matched by the server.
pub async fn stage(api: Arc<Api>, path: PathBuf) -> Result<Value, ApiError> {
    upload(&api, "/library/own-files", &path).await
}

/// The Upload your own files dialog.
pub fn bulk_view<'a>(s: &'a BulkState, ctx: &Ctx<'a>) -> El<'a, Act> {
    let p = ctx.look;
    let choose = btn_icon(Icon::Plus, "Choose files…", Btn::Default, s.takes_files().then_some(Act::OwnBulkPick));
    let mut body = column![
        small_muted("Choose (or drop) your own copies of songs that are in the library. Each file is matched to its song by its tags (artist, title, album, length) and replaces that song's file for everyone, keeping its plays, likes, playlists and lyrics. Replaced files stay in the trash for 7 days.").size(13),
        drop_zone::view(choose, &s.drop, "Drop audio files here to match them"),
    ]
    .spacing(14);
    let mut list = column![].spacing(8);
    for (i, r) in s.rows.iter().enumerate() {
        let mut line = column![t(r.name.clone()).size(13.5).font(theme::semibold())].spacing(4);
        if let Some(st) = &r.staged {
            let tags = &st["tags"];
            line = line.push(small_muted(format!("{} – {} · {}", fmt::s(tags, "artist"), fmt::s(tags, "title"), fmt::s(st, "format").to_uppercase())));
            if !s.applied {
                let options = choices(st);
                let selected = options.iter().find(|c| c.id == r.pick).cloned();
                line = line.push(
                    pick_list(options, selected, move |c: Choice| Act::OwnBulkChoose(i, c.id))
                        .text_size(13)
                        .padding([6, 10])
                        .width(Length::Fill)
                        .style(theme::pick)
                        .menu_style(theme::pick_menu),
                );
                if default_pick(st).is_none() {
                    line = line.push(t("Not sure – choose the song or leave it out").size(12).color(p.warn));
                }
            }
        } else if r.note.is_empty() {
            line = line.push(small_muted(if s.uploading == Some(i) { "Uploading and matching…" } else { "Waiting…" }));
        }
        if !r.note.is_empty() {
            line = line.push(t(r.note.clone()).size(12.5).color(if r.failed { p.err } else { p.ok }));
        }
        list = list.push(container(line).padding([8, 10]).width(Length::Fill).style(theme::card));
    }
    body = body.push(scrollable(list).height(Length::Shrink));
    let n = s.chosen();
    let label = if n == 1 { "Replace 1 song".to_string() } else { format!("Replace {n} songs") };
    let footer = row![
        btn("Close", Btn::Default, Some(Act::CloseDialog)),
        btn(label, Btn::Primary, (n > 0 && s.uploading.is_none() && !s.applying && !s.applied).then_some(Act::OwnBulkApply)),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    modal("Upload your own files", body, Some(footer.into()), true, Act::CloseDialog)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn who_may_replace() {
        assert!(allowed_for(&json!({ "role": "admin" })));
        assert!(allowed_for(&json!({ "role": "user", "perms": { "own_files": true } })));
        assert!(!allowed_for(&json!({ "role": "user", "perms": { "own_files": false } })));
        assert!(!allowed_for(&json!({ "role": "user" })));
    }

    #[test]
    fn whose_file_it_is() {
        assert_eq!(own_file_note(None), "");
        assert_eq!(own_file_note(Some(&json!({ "own_file": null }))), "");
        let note = own_file_note(Some(&json!({ "own_file": { "by": "alex", "old_format": "mp3" } })));
        assert_eq!(note, "Your own file – uploaded by alex, replacing an MP3");
        assert_eq!(article("FLAC"), "a FLAC");
        assert_eq!(article("WAV"), "a WAV");
        assert_eq!(article("OPUS"), "an OPUS");
    }

    #[test]
    fn what_changed_in_words() {
        let v = json!({ "before": { "format": "mp3", "bitrate": 320 }, "after": { "format": "flac", "bit_depth": 24, "sample_rate": 96000 }, "warnings": [] });
        assert_eq!(result_text(&v), "MP3 · 320 kbps → FLAC · 24-bit / 96 kHz");
    }

    #[test]
    fn a_batch_chooses_the_sure_matches_and_reports_each_file() {
        let mut b = BulkState::default();
        b.add(vec![PathBuf::from("/m/a.flac"), PathBuf::from("/m/b.flac"), PathBuf::from("/m/a.flac")]);
        assert_eq!(b.rows.len(), 2);
        assert_eq!(b.next_pending(), Some(0));
        let sure = json!({ "files": [{ "token": "t1", "match": { "track": { "id": 7 } }, "candidates": [{ "track": { "id": 7, "artist": "A", "title": "B" } }] }] });
        b.staged(0, Ok(sure));
        b.staged(1, Ok(json!({ "files": [], "failed": [{ "name": "b.flac", "reason": "this isn't audio" }] })));
        assert_eq!((b.rows[0].pick, b.rows[1].failed, b.chosen()), (Some(7), true, 1));
        assert_eq!(b.next_pending(), None);
        assert_eq!(choices(b.rows[0].staged.as_ref().unwrap())[1].label, "A – B");
        assert_eq!(b.apply_body(), json!({ "items": [{ "token": "t1", "track_id": 7 }] }));
        b.applied(&json!({ "replaced": [{ "token": "t1", "artist": "A", "title": "B", "format": "flac", "warnings": [] }], "failed": [] }));
        assert_eq!(b.rows[0].note, "Replaced A – B – now FLAC");
    }
}
