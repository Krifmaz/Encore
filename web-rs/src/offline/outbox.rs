//! Plays and likes made while the server could not be reached, kept in `/offline/outbox.json` (store.rs)
//! until [`flush`] sends them: the plays in one `POST /library/plays` (the list form, each with the time it
//! was played, 500 at most per call), the likes one by one. A song's likes and unlikes collapse into the last
//! one (the server's like and unlike change nothing when they are already so, so the last tap is what it
//! gets, even when the hearts this device showed were out of date).
//!
//! A device that is full may refuse the document: it is then kept in localStorage until the cache takes it
//! again. One flush at a time runs, in this page and across the site's tabs (Web Locks, store.rs).
//!
//! `queue_play` and `queue_like` answer at once: the change waits in memory and is written to the cache
//! under the store's lock right after, so [`pending`] counts it straight away.
//!
//! The outbox belongs to the user who made the changes (`user_id`). [`flush`] asks the server whose session
//! the browser holds now and sends only that user's outbox; another user's is dropped, and so is the outbox
//! when a different user queues something. One user's plays and likes never go out under another's session.

use super::store::{self, Lock};
use crate::api;
use crate::json::J;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;

/// The server takes at most this many plays per call.
pub const PLAYS_PER_CALL: usize = 500;
/// The outbox keeps at most this many plays (the oldest go first).
pub const MAX_PLAYS: usize = 5000;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Play {
    pub track_id: i64,
    /// how long it played
    pub seconds: f64,
    /// when (unix seconds)
    pub played_at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Like {
    pub track_id: i64,
    pub liked: bool,
    /// when it was pressed (unix seconds)
    pub at: i64,
}

/// The outbox document.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Outbox {
    /// the user whose plays and likes these are (0: nobody known – never sent)
    pub user_id: i64,
    pub plays: Vec<Play>,
    pub likes: Vec<Like>,
}

impl Outbox {
    pub fn len(&self) -> usize { self.plays.len() + self.likes.len() }
    pub fn is_empty(&self) -> bool { self.len() == 0 }

    /// A change by `user_id` is coming: the outbox of another user is dropped first.
    pub fn own(&mut self, user_id: i64) {
        if self.user_id != user_id { *self = Outbox { user_id, ..Outbox::default() }; }
    }

    /// These may go out under the session of `user_id` (an outbox without an owner never goes out).
    pub fn belongs_to(&self, user_id: i64) -> bool { self.user_id > 0 && self.user_id == user_id }

    /// A play, once (the same song at the same second is the same play).
    pub fn add_play(&mut self, p: Play) {
        if p.track_id <= 0 || self.plays.iter().any(|x| x.track_id == p.track_id && x.played_at == p.played_at) { return; }
        self.plays.push(p);
        if self.plays.len() > MAX_PLAYS {
            let extra = self.plays.len() - MAX_PLAYS;
            self.plays.drain(..extra);
        }
    }

    /// A like or an unlike: it replaces the one still waiting for the same song (the last tap wins; one being
    /// sent right now is forgotten once it went only when nothing changed meanwhile, see [`Outbox::forget`]).
    pub fn add_like(&mut self, l: Like) {
        if l.track_id <= 0 { return; }
        match self.likes.iter().position(|x| x.track_id == l.track_id) {
            None => self.likes.push(l),
            Some(i) if self.likes[i].liked == l.liked => {}
            Some(i) => self.likes[i] = l,
        }
    }

    /// What went: the plays sent, and the likes sent (an entry that changed meanwhile stays).
    pub fn forget(&mut self, plays: &[Play], likes: &[Like]) {
        let sent: HashSet<(i64, i64)> = plays.iter().map(|p| (p.track_id, p.played_at)).collect();
        self.plays.retain(|p| !sent.contains(&(p.track_id, p.played_at)));
        self.likes.retain(|l| !likes.iter().any(|s| s.track_id == l.track_id && s.liked == l.liked));
    }

    /// The saved document (an unreadable one counts as empty).
    pub fn parse(text: &str) -> Outbox { serde_json::from_str(text).unwrap_or_default() }
}

/// The body of `POST /library/plays` for these plays (the list form).
pub fn plays_body(plays: &[Play]) -> Value {
    json!({ "plays": plays.iter().map(|p| json!({
        "track_id": p.track_id,
        "seconds": (p.seconds.max(0.0) * 10.0).round() / 10.0,
        "played_at": p.played_at,
    })).collect::<Vec<_>>() })
}

enum Change { Play(Play), Like(Like) }

/// A change and the user who made it.
struct Queued { user_id: i64, change: Change }

thread_local! {
    /// The outbox as last read or written.
    static MIRROR: RefCell<Outbox> = RefCell::new(Outbox::default());
    static LOADED: Cell<bool> = const { Cell::new(false) };
    /// Changes not written yet.
    static UNSAVED: RefCell<Vec<Queued>> = const { RefCell::new(Vec::new()) };
    /// One flush at a time.
    static FLUSH_LOCK: Lock = const { Lock::new() };
    /// "No room for the plays and likes" was said in this page.
    static TOLD_FULL: Cell<bool> = const { Cell::new(false) };
}

/// The Web Locks lock a flush holds in every tab, from reading the outbox to forgetting what went.
const FLUSH_WEB_LOCK: &str = "encore-offline-flush";
/// Waiting longer than this for another tab's flush goes on without it (the server keeps a play once anyway).
const FLUSH_WAIT_MS: u32 = 30_000;
/// localStorage: the outbox when the cache refused it (a full device). Newer than the cache's copy while it is
/// there; removed once the cache takes the document again. 5000 plays are about 250 KB.
const SPARE_KEY: &str = "encore.offline.outbox";
/// Said once when neither the cache nor localStorage takes the outbox.
pub const NO_ROOM_FOR_OUTBOX: &str = "There is no room left on this device – plays and likes are kept only until the page closes";

fn apply(o: &mut Outbox, q: &Queued) {
    o.own(q.user_id);
    match &q.change {
        Change::Play(p) => o.add_play(p.clone()),
        Change::Like(l) => o.add_like(l.clone()),
    }
}

/// The signed-in user's id (in offline mode the remembered user), 0 when there is none.
fn me() -> i64 { crate::app::user().map(|u| u.i("id")).unwrap_or(0) }

/// The document with the unsaved changes in it, written back when there were some. Call it holding the
/// store's lock. Without the Cache API the outbox lives in memory only.
async fn load_merged() -> Outbox {
    let cache = store::open().await.ok();
    let mut doc = match &cache {
        Some(c) => match spare().or(store::read_text(c, store::OUTBOX_KEY).await) { Some(t) => Outbox::parse(&t), None => Outbox::default() },
        None => MIRROR.with(|m| m.borrow().clone()),
    };
    let changes: Vec<Queued> = UNSAVED.with(|u| std::mem::take(&mut *u.borrow_mut()));
    let any = !changes.is_empty();
    for q in &changes { apply(&mut doc, q); }
    if any {
        if let Some(c) = &cache {
            if let Err(e) = write(c, &doc).await {
                crate::dom::console_error(&format!("outbox: {e}"));
                if !TOLD_FULL.with(|t| t.replace(true)) { crate::ui::toast(NO_ROOM_FOR_OUTBOX, "warn"); }
            }
        }
    }
    MIRROR.with(|m| *m.borrow_mut() = doc.clone());
    LOADED.with(|l| l.set(true));
    doc
}

/// The copy localStorage keeps while the cache refuses the document.
fn spare() -> Option<String> { crate::dom::ls_get(SPARE_KEY) }

async fn write(cache: &web_sys::Cache, doc: &Outbox) -> Result<(), String> {
    let text = serde_json::to_string(doc).map_err(|e| e.to_string())?;
    match store::write_text(cache, store::OUTBOX_KEY, &text, "application/json").await {
        Ok(()) => {
            crate::dom::ls_remove(SPARE_KEY);
            Ok(())
        }
        // a full device: localStorage keeps it until the cache takes it again
        Err(e) => match crate::dom::local_storage().map(|ls| ls.set_item(SPARE_KEY, &text)) {
            Some(Ok(())) => Ok(()),
            _ => Err(e),
        },
    }
}

fn queue(change: Change) {
    // a change nobody can be named for is not kept: it could only go out under the wrong session
    let user_id = me();
    if user_id <= 0 { return; }
    UNSAVED.with(|u| u.borrow_mut().push(Queued { user_id, change }));
    store::changed();
    crate::dom::spawn(async {
        let _g = store::lock().await;
        load_merged().await;
    });
}

/// A play to send later (it played `seconds`, stamped with now).
pub fn queue_play(track_id: i64, seconds: f64) {
    let seconds = if seconds.is_finite() { seconds.max(0.0) } else { 0.0 };
    queue(Change::Play(Play { track_id, seconds, played_at: crate::dom::now_s() as i64 }));
}

/// A like (`liked`) or an unlike to send later.
pub fn queue_like(track_id: i64, liked: bool) {
    queue(Change::Like(Like { track_id, liked, at: crate::dom::now_s() as i64 }));
}

/// The outbox now: what was read or written last, with the changes still on their way to the cache. The
/// first call starts reading the saved document.
fn current() -> Outbox {
    if !LOADED.with(|l| l.replace(true)) && store::available() {
        crate::dom::spawn(async {
            { let _g = store::lock().await; load_merged().await; }
            store::changed();
        });
    }
    let mut o = MIRROR.with(|m| m.borrow().clone());
    UNSAVED.with(|u| for q in u.borrow().iter() { apply(&mut o, q); });
    o
}

/// A like or an unlike of this song waits in the signed-in user's outbox: a new tap must go after it, through
/// the outbox, so the server sees them in order (player/bar.rs).
pub fn has_like(track_id: i64) -> bool { mine().likes.iter().any(|l| l.track_id == track_id) }

/// The signed-in user's outbox (another user's counts as empty: it never goes out).
fn mine() -> Outbox {
    let o = current();
    if o.belongs_to(me()) { o } else { Outbox::default() }
}

/// Plays and likes waiting to be sent.
pub fn pending() -> usize { mine().len() }
/// The same, as (plays, likes): "3 plays and 1 like will be sent when the server is back".
pub fn pending_counts() -> (usize, usize) { let o = mine(); (o.plays.len(), o.likes.len()) }

/// Send what waits, when it belongs to the user whose session the browser holds now (`/auth/me`); the outbox
/// of another user is dropped. Ok(n): n plays and likes went (when something failed on the way, the rest
/// stays and [`pending`] says so); Err: nothing could be sent.
pub async fn flush() -> Result<usize, String> {
    let _f = store::acquire(&FLUSH_LOCK).await;
    // another tab sending the same document would send its plays twice
    let _tabs = store::web_lock(FLUSH_WEB_LOCK, FLUSH_WAIT_MS).await;
    let doc = {
        let _g = store::lock().await;
        load_merged().await
    };
    if doc.is_empty() { return Ok(0); }
    let r = send(doc).await;
    store::changed();
    r
}

/// The other user's outbox goes (changes `user_id` queued meanwhile stay).
async fn drop_foreign(user_id: i64) {
    let _g = store::lock().await;
    let mut now = load_merged().await;
    if now.belongs_to(user_id) { return; }
    now.own(user_id);
    if let Ok(c) = store::open().await {
        if let Err(e) = write(&c, &now).await { crate::dom::console_error(&format!("outbox: {e}")); }
    }
    MIRROR.with(|m| *m.borrow_mut() = now);
}

async fn send(doc: Outbox) -> Result<usize, String> {
    // the session the browser holds now decides: never one user's plays and likes under another's
    let user_id = api::get("/auth/me").await.map_err(|e| e.message)?.i("id");
    if !doc.belongs_to(user_id) {
        drop_foreign(user_id).await;
        return Ok(0);
    }
    let mut sent_plays: Vec<Play> = vec![];
    let mut sent_likes: Vec<Like> = vec![];
    let mut error: Option<String> = None;
    for chunk in doc.plays.chunks(PLAYS_PER_CALL) {
        match api::post("/library/plays", plays_body(chunk)).await {
            Ok(_) => sent_plays.extend_from_slice(chunk),
            Err(e) => { error = Some(e.message); break; }
        }
    }
    if error.is_none() {
        for l in &doc.likes {
            let path = format!("/likes/{}", l.track_id);
            let r = if l.liked { api::post_empty(&path).await } else { api::del(&path).await };
            match r {
                // 404: the song is gone from the library – nothing left to like
                Ok(_) => sent_likes.push(l.clone()),
                Err(e) if e.status == 404 => sent_likes.push(l.clone()),
                Err(e) => { error = Some(e.message); break; }
            }
        }
    }
    let n = sent_plays.len() + sent_likes.len();
    if n > 0 {
        let _g = store::lock().await;
        let mut now = load_merged().await;
        now.forget(&sent_plays, &sent_likes);
        if let Ok(c) = store::open().await {
            if let Err(e) = write(&c, &now).await { crate::dom::console_error(&format!("outbox: {e}")); }
        }
        MIRROR.with(|m| *m.borrow_mut() = now);
    }
    match error {
        Some(e) if n == 0 => Err(e),
        _ => Ok(n),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn like(id: i64, liked: bool) -> Like { Like { track_id: id, liked, at: 100 } }
    fn play(id: i64, at: i64) -> Play { Play { track_id: id, seconds: 120.0, played_at: at } }

    #[test]
    fn a_like_then_an_unlike_collapses_into_the_unlike() {
        let mut o = Outbox::default();
        o.add_like(like(1, true));
        o.add_like(like(1, false));
        assert_eq!(o.likes, vec![like(1, false)], "the last tap is what the server gets");
        // the hearts were out of date (unliked on another device): unlike, like – the server gets the like
        o.add_like(like(2, false));
        o.add_like(like(2, true));
        assert_eq!(o.likes, vec![like(1, false), like(2, true)]);
        o.add_like(like(2, false));
        o.add_like(like(2, true));
        assert_eq!(o.likes, vec![like(1, false), like(2, true)], "four presses: one like");
        o.add_like(like(2, true));
        assert_eq!(o.likes.len(), 2, "the same again changes nothing");
        o.add_like(like(0, true));
        assert_eq!(o.likes.len(), 2, "no song, no like");
    }

    #[test]
    fn a_like_on_its_way_is_replaced_not_cancelled() {
        let mut o = Outbox::default();
        o.add_like(like(1, true));
        // flush is sending the like: an unlike now must go after it
        o.add_like(like(1, false));
        assert_eq!(o.likes, vec![like(1, false)]);
        // the like went: only it is forgotten, the unlike stays
        o.forget(&[], &[like(1, true)]);
        assert_eq!(o.likes, vec![like(1, false)]);
        o.forget(&[], &[like(1, false)]);
        assert!(o.is_empty());
    }

    #[test]
    fn plays_are_kept_once_and_forgotten_once_sent() {
        let mut o = Outbox::default();
        o.add_play(play(1, 1000));
        o.add_play(play(1, 1000));
        o.add_play(play(1, 2000));
        o.add_play(play(2, 1000));
        o.add_play(play(-3, 1000));
        assert_eq!(o.plays.len(), 3);
        let sent = o.plays[..2].to_vec();
        o.add_play(play(4, 3000));
        o.forget(&sent, &[]);
        assert_eq!(o.plays, vec![play(2, 1000), play(4, 3000)], "a play queued while sending stays");
        let mut big = Outbox::default();
        for i in 0..(MAX_PLAYS as i64 + 3) { big.add_play(play(7, i)); }
        assert_eq!(big.plays.len(), MAX_PLAYS);
        assert_eq!(big.plays[0].played_at, 3, "the oldest went");
    }

    #[test]
    fn the_plays_go_in_the_list_form() {
        let body = plays_body(&[Play { track_id: 5, seconds: 61.26, played_at: 1_760_000_000 }, Play { track_id: 6, seconds: -2.0, played_at: 1_760_000_100 }]);
        assert_eq!(body, json!({ "plays": [
            { "track_id": 5, "seconds": 61.3, "played_at": 1_760_000_000 },
            { "track_id": 6, "seconds": 0.0, "played_at": 1_760_000_100 },
        ] }));
        assert_eq!(plays_body(&[]), json!({ "plays": [] }));
    }

    #[test]
    fn one_users_outbox_never_goes_out_for_another() {
        let mut o = Outbox::default();
        o.own(1);
        o.add_play(play(5, 1000));
        o.add_like(like(6, false));
        assert!(o.belongs_to(1));
        assert!(!o.belongs_to(2), "user 2's session must not send user 1's plays and unlikes");
        assert!(!o.belongs_to(0));
        let text = serde_json::to_string(&o).unwrap();
        assert_eq!(Outbox::parse(&text).user_id, 1, "the owner is saved with the document");
        // a document from before the owner was kept: nobody's, never sent
        let old = Outbox::parse(r#"{"plays":[{"track_id":3,"seconds":30,"played_at":10}],"likes":[]}"#);
        assert_eq!(old.user_id, 0);
        assert!(!old.belongs_to(0) && !old.belongs_to(3));
    }

    #[test]
    fn a_change_by_another_user_empties_the_outbox_first() {
        let q = |user_id: i64, change: Change| Queued { user_id, change };
        let mut o = Outbox::default();
        apply(&mut o, &q(1, Change::Play(play(5, 1000))));
        apply(&mut o, &q(1, Change::Like(like(6, true))));
        assert_eq!((o.user_id, o.len()), (1, 2));
        apply(&mut o, &q(1, Change::Play(play(7, 2000))));
        assert_eq!((o.user_id, o.len()), (1, 3), "the same user adds to it");
        apply(&mut o, &q(2, Change::Like(like(8, false))));
        assert_eq!(o, Outbox { user_id: 2, plays: vec![], likes: vec![like(8, false)] }, "user 1's entries are gone");
        let mut empty = Outbox { user_id: 2, ..Outbox::default() };
        empty.own(2);
        assert_eq!(empty.user_id, 2);
    }

    #[test]
    fn the_document_round_trips() {
        let mut o = Outbox::default();
        o.add_play(play(1, 10));
        o.add_like(like(2, true));
        let text = serde_json::to_string(&o).unwrap();
        assert_eq!(Outbox::parse(&text), o);
        assert_eq!(Outbox::parse("garbage"), Outbox::default());
        assert_eq!(Outbox::parse(r#"{"plays":[{"track_id":3}]}"#).plays, vec![Play { track_id: 3, seconds: 0.0, played_at: 0 }]);
        assert_eq!(o.len(), 2);
    }
}
