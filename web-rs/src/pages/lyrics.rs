//! Lyrics for one song (shown, fetched and saved as .lrc/.txt next to the file) or a whole album. The dialog's body
//! is a Leptos view ([`LyricsBody`]) over the last answer.

use crate::api;
use crate::dom;
use crate::json::J;
use crate::ui::{esc, modal, toast, ModalOpts};
use leptos::prelude::*;
use serde_json::Value;

/// Opens the Lyrics dialog of song `t` and looks its lyrics up; Search again asks LRCLIB again.
pub fn lyrics_modal(t: &Value) {
    let m = modal(ModalOpts::new(&format!("Lyrics · {}", esc(&t.s("title"))), "")
        .footer(r#"<button class="btn" id="ly-refetch">Search again</button><button class="btn primary" data-close>Close</button>"#));
    let id = t.s("id");
    // the body is the view over the last answer ("Looking for lyrics…" until one has come)
    let owner = Owner::new();
    let answer: RwSignal<Option<Result<Value, String>>> = owner.with(|| RwSignal::new(None));
    if let Some(body) = m.qs(".modal-body") {
        dom::set_html(&body, "");
        let shown = owner.with(|| {
            kit_leptos::rt::mount(&body, move || {
                move || match answer.get() {
                    None => view! { <div class="muted"><span class="spinner"></span>" Looking for lyrics…"</div> }.into_any(),
                    Some(a) => view! { <LyricsBody answer=a/> }.into_any(),
                }
            })
        });
        // kept while the page is on screen (the dialog closes before the page changes, or with it)
        if let Some(v) = shown {
            dom::keep(v);
        }
    }
    dom::keep(owner);
    if let Some(b) = m.qs("#ly-refetch") {
        let id2 = id.clone();
        dom::on(&b, "click", move |_| dom::spawn(look_up(answer, id2.clone(), true)));
    }
    dom::spawn(look_up(answer, id, false));
}

/// Fetches the lyrics of track `id` (`force`: from LRCLIB again) into `answer`.
async fn look_up(answer: RwSignal<Option<Result<Value, String>>>, id: String, force: bool) {
    let r = api::get(&format!("/library/tracks/{id}/lyrics{}", if force { "?force=true" } else { "" })).await.map_err(|e| e.message);
    answer.set(Some(r));
}

/// The dialog's body for one answer: the lyrics with where they come from, that the song is instrumental, that
/// none were found, or the error.
#[component]
fn LyricsBody(
    /// The answer of GET /api/library/tracks/{id}/lyrics, or its error.
    answer: Result<Value, String>,
) -> impl IntoView {
    let r = match answer {
        Ok(r) => r,
        Err(e) => return view! { <div class="error-box">{e}</div> }.into_any(),
    };
    let synced = r.truthy("synced");
    let text = if synced { plain_text(&r.s("lyrics")) } else { r.s("lyrics") };
    if r.truthy("instrumental") {
        return view! { <div class="muted">"Instrumental – no lyrics."</div> }.into_any();
    }
    if text.is_empty() {
        return view! { <div class="muted">"No lyrics found on LRCLIB."</div> }.into_any();
    }
    let kind = if synced { "Time-synced" } else { "Plain" };
    let from = if r.g("source").as_str() == Some("file") { "saved next to the song" } else { "from LRCLIB, saved next to the song" };
    view! {
        <div class="small faint" style="margin-bottom:10px">{format!("{kind} lyrics · {from}")}</div><div style="white-space:pre-wrap;line-height:1.6">{text}</div>
    }
    .into_any()
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
