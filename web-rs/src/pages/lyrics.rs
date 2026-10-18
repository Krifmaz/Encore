//! Port of web/js/pages/lyrics.js – see docs/archive/ui-rewrite/CONTRACT.md
//! Lyrics for one song (shown, fetched and saved as .lrc/.txt next to the file) or a whole album.

use crate::api;
use crate::dom;
use crate::json::J;
use crate::ui::{esc, modal, toast, Modal, ModalOpts};
use serde_json::Value;

/// `lyricsModal(t)`: opens the modal and looks the lyrics up (spawns).
pub fn lyrics_modal(t: &Value) {
    let m = modal(ModalOpts::new(&format!("Lyrics · {}", esc(&t.s("title"))), r#"<div class="muted"><span class="spinner"></span> Looking for lyrics…</div>"#)
        .footer(r#"<button class="btn" id="ly-refetch">Search again</button><button class="btn primary" data-close>Close</button>"#));
    let id = t.s("id");
    if let Some(b) = m.qs("#ly-refetch") {
        let (m2, id2) = (m.clone(), id.clone());
        dom::on(&b, "click", move |_| dom::spawn(show(m2.clone(), id2.clone(), true)));
    }
    dom::spawn(show(m, id, false));
}

/// The JS `show(force)`: fetch the lyrics of track `id` and render them into the modal body.
async fn show(m: Modal, id: String, force: bool) {
    let Some(body) = m.qs(".modal-body") else { return };
    match api::get(&format!("/library/tracks/{id}/lyrics{}", if force { "?force=true" } else { "" })).await {
        Ok(r) => {
            let synced = r.truthy("synced");
            let text = if synced { plain_text(&r.s("lyrics")) } else { r.s("lyrics") };
            let html = if r.truthy("instrumental") {
                r#"<div class="muted">Instrumental – no lyrics.</div>"#.to_string()
            } else if !text.is_empty() {
                format!(r#"<div class="small faint" style="margin-bottom:10px">{} lyrics · {}</div><div style="white-space:pre-wrap;line-height:1.6">{}</div>"#,
                    if synced { "Time-synced" } else { "Plain" },
                    if r.g("source").as_str() == Some("file") { "saved next to the song" } else { "from LRCLIB, saved next to the song" },
                    esc(&text))
            } else {
                r#"<div class="muted">No lyrics found on LRCLIB.</div>"#.to_string()
            };
            dom::set_html(&body, &html);
        }
        Err(e) => dom::set_html(&body, &format!(r#"<div class="error-box">{}</div>"#, esc(&e.message))),
    }
}

/// `lyrics.replace(/\[[^\]]*\]/g, '').replace(/\n{3,}/g, '\n\n').trim()`: synced lyrics without their
/// `[mm:ss.xx]` tags, runs of blank lines collapsed, surrounding whitespace removed.
fn plain_text(lyrics: &str) -> String {
    // /\[[^\]]*\]/g → '' : every "[" up to the next "]" (a "[" with no "]" after it stays)
    let mut untagged = String::with_capacity(lyrics.len());
    let mut rest = lyrics;
    while let Some(i) = rest.find('[') {
        match rest[i + 1..].find(']') {
            Some(j) => { untagged.push_str(&rest[..i]); rest = &rest[i + 1 + j + 1..]; }
            None => break,
        }
    }
    untagged.push_str(rest);
    // /\n{3,}/g → '\n\n'
    let mut out = String::with_capacity(untagged.len());
    let mut run = 0;
    for c in untagged.chars() {
        if c == '\n' { run += 1; if run > 2 { continue; } } else { run = 0; }
        out.push(c);
    }
    // String.prototype.trim(): WhiteSpace + LineTerminator (includes U+FEFF, excludes U+0085 unlike Rust's trim)
    out.trim_matches(|c: char| matches!(c, '\u{9}'..='\u{D}' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}')).to_string()
}

/// `fetchAlbumLyrics(albumId)`: look up lyrics for every song of the album (spawns).
pub fn fetch_album_lyrics(album_id: &str) {
    toast("Looking for lyrics…", "");
    let path = format!("/library/albums/{album_id}/lyrics");
    dom::spawn(async move {
        match api::post_empty(&path).await {
            Ok(r) => toast(&if r.truthy("checked") { format!("Lyrics found for {} of {} song(s)", r.s("found"), r.s("checked")) } else { "Every song already has lyrics".to_string() }, "ok"),
            Err(e) => toast(&e.message, "err"),
        }
    });
}
