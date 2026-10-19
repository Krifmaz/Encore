//! Downloaded songs (0.28): a song saved in this browser (crate::offline) plays from its copy here instead of
//! the server – always when the server can't be reached, and otherwise while "Play downloaded songs from this
//! device" is on (Settings → Playback, on by default). The copy plays through an object URL of its cached Blob,
//! which the <audio> element seeks in like in a file; the URL is made when the song is about to load or be
//! pre-buffered, and revoked once no element holds it any more.
//!
//! Plays and likes made while the server can't be reached wait in the outbox (offline/outbox.rs) and go out
//! as soon as it answers again. Covers and lyrics fall back to the saved ones.

use super::*;
use crate::offline::{self, mode, outbox};

/// An answer that means the server could not be reached: no connection, or a proxy in front saying it is down.
pub(super) use crate::offline::mode::unreachable;

/// localStorage: "0" when the switch is off (the server streams even a downloaded song while it can).
pub const PREFER_KEY: &str = "encore.offline.preferLocal";
/// localStorage: the liked track ids as this device last knew them, for the hearts while the server can't be
/// reached (forgotten at sign-out with the remembered user, offline/mod.rs `forget_me`).
use crate::offline::LIKES_KEY;

type Then = Box<dyn FnOnce()>;

thread_local! {
    /// track id → object URL of its saved audio (the song playing, the one pre-buffered next)
    static URLS: RefCell<HashMap<i64, String>> = RefCell::new(HashMap::new());
    /// object URLs being made, and what waits for each
    static MAKING: RefCell<HashMap<i64, Vec<Then>>> = RefCell::new(HashMap::new());
    /// saved copies that would not play this session: these songs stream from the server
    static BROKEN: RefCell<HashSet<i64>> = RefCell::new(HashSet::new());
    /// album id → a saved song of that album whose cover was saved (the cover fallback)
    static ALBUMS: RefCell<HashMap<String, i64>> = RefCell::new(HashMap::new());
    /// the outbox is being sent
    static SYNCING: Cell<bool> = const { Cell::new(false) };
    /// something was queued while it was being sent: send again once that is done
    static AGAIN: Cell<bool> = const { Cell::new(false) };
}

// ---- the switch and the state of the connection --------------------------------------------------

/// The stored switch: on unless it was turned off.
pub fn prefer_local_from(v: Option<&str>) -> bool { !matches!(v.map(str::trim), Some("0" | "false" | "off")) }
/// "Play downloaded songs from this device".
pub fn prefer_local() -> bool { prefer_local_from(dom::ls_get(PREFER_KEY).as_deref()) }
pub fn set_prefer_local(on: bool) { dom::ls_set(PREFER_KEY, if on { "1" } else { "0" }); }

/// There is no server to ask: offline mode (the app opened without it, offline/mode.rs), or the browser has no
/// network at all.
pub(super) fn no_server() -> bool { mode::active() || !dom::window().navigator().on_line() }

// ---- playing the saved copy ------------------------------------------------------------------------

/// Does this song play from its saved copy? Library songs saved here; in offline mode only while the grace
/// lasts (after 30 days without the server the downloads wait for a new sign-in).
pub(super) fn use_local(t: &Value) -> bool {
    if t.truthy("preview") || !t.g("id").is_number() || !offline::available() { return false; }
    let id = t.i("id");
    if BROKEN.with(|b| b.borrow().contains(&id)) || !offline::has(id) { return false; }
    if mode::active() { return mode::can_play(); }
    no_server() || prefer_local()
}

/// The object URL of a song's saved audio, once it has been made.
pub(super) fn url_for(id: i64) -> Option<String> { URLS.with(|u| u.borrow().get(&id).cloned()) }

/// `url` is the saved copy of song `t` (what the spare element pre-buffered).
pub(super) fn is_local_url(t: &Value, url: &str) -> bool { t.g("id").is_number() && url_for(t.i("id")).is_some_and(|u| u == url) }

/// Make the object URL of a saved song, then `then()`. When the copy can't be read the song is marked, so
/// `then()` streams it from the server instead.
pub(super) fn prepare(id: i64, then: impl FnOnce() + 'static) {
    if url_for(id).is_some() { then(); return; }
    let first = MAKING.with(|m| {
        let mut m = m.borrow_mut();
        let waiting = m.entry(id).or_default();
        waiting.push(Box::new(then));
        waiting.len() == 1
    });
    if !first { return; }
    dom::spawn(async move {
        match offline::audio_url(id).await {
            Some(u) => {
                let old = URLS.with(|m| m.borrow_mut().insert(id, u));
                if let Some(old) = old { let _ = web_sys::Url::revoke_object_url(&old); }
            }
            None => { BROKEN.with(|b| b.borrow_mut().insert(id)); }
        }
        let waiting = MAKING.with(|m| m.borrow_mut().remove(&id)).unwrap_or_default();
        for f in waiting { f(); }
    });
}

/// Revoke the object URLs no <audio> element holds any more (the song before, a pre-buffered song dropped).
/// A song fading out still holds its URL; it goes at the next sweep after the fade.
pub(super) fn sweep() {
    let fade_out = FADE.with(|f| f.borrow().as_ref().map(|f| f.out.clone()));
    let parked = PARKED.with(|p| p.borrow().clone());
    let held: Vec<String> = [audio(), spare(), plain(), parked, fade_out].into_iter().flatten()
        .filter_map(|a| a.get_attribute("src")).filter(|s| s.starts_with("blob:")).collect();
    URLS.with(|m| m.borrow_mut().retain(|_, u| {
        let keep = held.iter().any(|h| h == u);
        if !keep { let _ = web_sys::Url::revoke_object_url(u); }
        keep
    }));
}

/// The saved copy failed to play: this session the song streams from the server, from the same spot.
pub(super) fn copy_failed(t: &Value) {
    let id = t.i("id");
    BROKEN.with(|b| b.borrow_mut().insert(id));
    dom::console_error(&format!("offline copy {id} did not play"));
    toast("This download would not play – playing the song from the server", "");
    load_opts(LoadOpts { offset: spot_position(), hold: HELD.with(|h| h.get()), ..Default::default() });
}

/// What the player shows for a song playing from its saved copy: the record's quality ("FLAC 16/44.1",
/// "AAC 256k (converted)"), or the file format when the record has none.
pub(super) fn copy_quality(t: &Value) -> String {
    let q = offline::record(t.i("id")).map(|r| r.quality).unwrap_or_default();
    if q.trim().is_empty() { t.s("format").to_uppercase() } else { q }
}
/// The saved copy is a conversion, not the original file.
pub(super) fn copy_converted(t: &Value) -> bool { offline::record(t.i("id")).is_some_and(|r| r.converted()) }

// ---- plays and likes without the server --------------------------------------------------------------

/// A play the server didn't get: it waits in the outbox, stamped with now.
pub(super) fn play_later(id: i64, seconds: f64) { outbox::queue_play(id, seconds); }

/// Plays and likes waiting from a time without the server: send them now that it answers. Not in offline
/// mode – that sends them itself when the server is back, and says how many went.
pub(super) fn sync_outbox() {
    if mode::active() || outbox::pending() == 0 { return; }
    if SYNCING.with(|s| s.get()) { AGAIN.with(|a| a.set(true)); return; }
    SYNCING.with(|s| s.set(true));
    let (_, likes) = outbox::pending_counts();
    dom::spawn(async move {
        let r = outbox::flush().await;
        SYNCING.with(|s| s.set(false));
        if likes > 0 && matches!(r, Ok(n) if n > 0) { dom::dispatch("encore:likes", None); }
        if AGAIN.with(|a| a.replace(false)) && r.is_ok() { sync_outbox(); }
    });
}

/// The liked songs as the server listed them: kept on this device for the hearts in offline mode.
pub(super) fn keep_likes(s: &HashSet<i64>) {
    if !offline::available() { return; }
    let mut v: Vec<i64> = s.iter().copied().collect();
    v.sort_unstable();
    dom::ls_set(LIKES_KEY, &serde_json::to_string(&v).unwrap_or_default());
}
/// The liked songs as this device last knew them (empty when it never saw the list).
pub(super) fn kept_likes() -> HashSet<i64> { parse_likes(&dom::ls_get(LIKES_KEY).unwrap_or_default()) }
pub fn parse_likes(text: &str) -> HashSet<i64> { serde_json::from_str::<Vec<i64>>(text).map(|v| v.into_iter().collect()).unwrap_or_default() }

/// A like or an unlike the server couldn't take: it waits in the outbox, and the heart answers at once.
pub(super) fn like_later(id: i64, liked: bool) {
    outbox::queue_like(id, liked);
    LIKED.with(|l| {
        let mut l = l.borrow_mut();
        let s = l.get_or_insert_with(HashSet::new);
        if liked { s.insert(id); } else { s.remove(&id); }
    });
    if let Some(s) = LIKED.with(|l| l.borrow().clone()) { keep_likes(&s); }
}

// ---- covers and lyrics ---------------------------------------------------------------------------------

/// The album id of a library cover URL (`/api/library/albums/<id>/cover`, with or without a query).
pub fn album_of_cover(src: &str) -> Option<String> {
    const P: &str = "/api/library/albums/";
    let rest = &src[src.find(P)? + P.len()..];
    let (id, tail) = rest.split_once("/cover")?;
    if !(tail.is_empty() || tail.starts_with('?')) { return None; }
    let ok = !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    ok.then(|| id.to_string())
}

/// The albums whose cover is saved, from the index.
async fn note_albums() {
    let m: HashMap<String, i64> = offline::index().await.into_iter()
        .filter(|r| r.cover && !r.album_id.is_empty())
        .map(|r| (r.album_id, r.id))
        .collect();
    ALBUMS.with(|a| *a.borrow_mut() = m);
}

/// A library cover that can't load (no server) shows the saved one when a downloaded song has it. The listener
/// runs in the capture phase, before the <img>'s own `onerror` removes it.
fn cover_fallback() {
    let l = EventListener::new_with_options(&document(), "error", EventListenerOptions::run_in_capture_phase(), |e: &Event| {
        let Some(img) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlImageElement>().ok()) else { return };
        let Some(album) = img.get_attribute("src").and_then(|s| album_of_cover(&s)) else { return };
        let Some(id) = ALBUMS.with(|a| a.borrow().get(&album).copied()) else { return };
        e.stop_immediate_propagation();
        dom::spawn(async move {
            match offline::cover_url(id).await {
                Some(u) => { let _ = img.remove_attribute("srcset"); img.set_src(&u); }
                None => img.remove(),
            }
        });
    });
    dom::keep_forever(l);
}

/// The saved lyrics of a song (the server's `/lyrics` answer), when the request fails.
pub(super) async fn saved_lyrics(id: &Value) -> Option<Value> {
    let id = id.as_i64()?;
    if !offline::available() { return None; }
    serde_json::from_str(&offline::lyrics(id).await?).ok()
}

// ---- start-up and the Playback switch --------------------------------------------------------------------

/// From `mount()`: read the saved songs now (so the first song can play from its copy) and keep the cover
/// fallback ready. Plays and likes left from a time without the server go out once it answers again (an
/// earlier visit's are sent at sign-in, offline/mode.rs `signed_in`).
pub(super) fn start() {
    if !offline::available() { return; }
    dom::spawn(note_albums());
    dom::on_window(offline::CHANGED_EVENT, |_| dom::spawn(note_albums()));
    cover_fallback();
    let _ = outbox::pending();   // starts reading the outbox
    dom::on_window("online", |_| sync_outbox());
}

/// Settings → Playback: "Play downloaded songs from this device", after Buffering – only where this browser
/// keeps downloads.
pub fn offline_setting(root: &Element) {
    if !offline::available() || qs_in(root, "#pb-local").is_some() { return; }
    let Some(anchor) = qs_in(root, "#pb-preload").and_then(|c| dom::closest_of(&c, ".stack")) else { return };
    let html = format!(r#"<div class="stack" style="gap:6px"><h3>Downloads</h3>
        <label class="row" style="gap:8px;align-items:center"><input type="checkbox" id="pb-local" {}> Play downloaded songs from this device</label>
        <span class="small muted">Saves data when a song is already downloaded here.</span></div>"#,
        if prefer_local() { "checked" } else { "" });
    let _ = anchor.insert_adjacent_html("afterend", &html);
    if let Some(c) = qs_in(root, "#pb-local") {
        let c2 = c.clone();
        dom::on(&c, "change", move |_| set_prefer_local(dom::checked(&c2)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_switch_is_on_unless_turned_off() {
        assert!(prefer_local_from(None));
        assert!(prefer_local_from(Some("1")));
        assert!(prefer_local_from(Some("true")));
        assert!(prefer_local_from(Some("")));
        assert!(!prefer_local_from(Some("0")));
        assert!(!prefer_local_from(Some("false")));
        assert!(!prefer_local_from(Some(" off ")));
    }

    #[test]
    fn library_cover_urls_name_their_album() {
        assert_eq!(album_of_cover("/api/library/albums/0123456789abcdef/cover").as_deref(), Some("0123456789abcdef"));
        assert_eq!(album_of_cover("/api/library/albums/ab12/cover?s=400&v=17").as_deref(), Some("ab12"));
        assert_eq!(album_of_cover("https://music.example.org/api/library/albums/ab-12_x/cover?s=200").as_deref(), Some("ab-12_x"));
        assert_eq!(album_of_cover("/api/library/albums/ab12/coverart"), None);
        assert_eq!(album_of_cover("/api/library/albums//cover"), None);
        assert_eq!(album_of_cover("/api/library/albums/a%2Fb/cover"), None);
        assert_eq!(album_of_cover("/api/share/xyz/cover"), None);
        assert_eq!(album_of_cover("blob:https://music.example.org/5f0c"), None);
    }

    #[test]
    fn the_kept_likes_read_back() {
        let s = parse_likes("[3,1,2]");
        assert_eq!(s.len(), 3);
        assert!(s.contains(&1) && s.contains(&2) && s.contains(&3));
        assert!(parse_likes("").is_empty());
        assert!(parse_likes("not json").is_empty());
        assert!(parse_likes(r#"{"ids":[1]}"#).is_empty());
    }
}
