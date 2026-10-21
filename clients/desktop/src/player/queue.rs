//! The queue: what plays next and in what order. The engine holds the queue itself; this
//! module keeps the order from before shuffling (to put it back), decides what happens when
//! the queue ends (repeat, the library shuffle), builds the slim rows other devices and the
//! saved spot use, and remembers where you left off on this computer (`resume.json`).

use crate::engine::{self, Command, Status, Track};
use serde_json::{json, Value};
use std::sync::Mutex;

/// The queue's order from before shuffling (None when shuffle is off or nothing was shuffled).
static ORDER: Mutex<Option<Vec<Track>>> = Mutex::new(None);

/// Repeat mode: `off`, `all` (the queue) or `one` (this song).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

impl Repeat {
    pub fn id(self) -> &'static str {
        match self {
            Repeat::Off => "off",
            Repeat::All => "all",
            Repeat::One => "one",
        }
    }

    pub fn parse(s: &str) -> Repeat {
        match s {
            "all" => Repeat::All,
            "one" => Repeat::One,
            _ => Repeat::Off,
        }
    }

    /// The Repeat button cycles off → all → one → off.
    pub fn next(self) -> Repeat {
        match self {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        }
    }

    /// The button's tooltip.
    pub fn title(self) -> &'static str {
        match self {
            Repeat::One => "Repeat: this song",
            Repeat::All => "Repeat: the queue",
            Repeat::Off => "Repeat: off",
        }
    }
}

// ----- random --------------------------------------------------------------------------

/// A small xorshift generator seeded from the clock: good enough to shuffle a queue.
pub struct Rng(u64);

impl Rng {
    pub fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9e37_79b9_7f4a_7c15);
        Self(nanos | 1)
    }

    #[cfg(test)]
    pub fn seeded(seed: u64) -> Self {
        Self(seed | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// 0 ≤ n < `bound`.
    pub fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            0
        } else {
            (self.next_u64() % bound as u64) as usize
        }
    }
}

impl Default for Rng {
    fn default() -> Self {
        Self::new()
    }
}

/// Fisher–Yates over `v`.
pub fn shuffle_in_place(v: &mut [Track], rng: &mut Rng) {
    for i in (1..v.len()).rev() {
        let j = rng.below(i + 1);
        v.swap(i, j);
    }
}

// ----- shuffle: the engine's queue order, the original kept here ----------------------

/// `shuffleRest()`: the songs after `index` in random order; the rest untouched.
pub fn shuffled_rest(mut queue: Vec<Track>, index: usize, rng: &mut Rng) -> Vec<Track> {
    let start = (index + 1).min(queue.len());
    shuffle_in_place(&mut queue[start..], rng);
    queue
}

/// A new queue with shuffle on: remember its order, shuffle what follows `index`.
pub fn shuffle_new(tracks: Vec<Track>, index: usize) -> Vec<Track> {
    *ORDER.lock().unwrap_or_else(|e| e.into_inner()) = Some(tracks.clone());
    shuffled_rest(tracks, index, &mut Rng::new())
}

/// Shuffle went off (or a new queue plays with it off): no order to put back.
pub fn forget_order() {
    *ORDER.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The order kept from before shuffling, if any.
pub fn order() -> Option<Vec<Track>> {
    ORDER.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

pub fn set_order(o: Option<Vec<Track>>) {
    *ORDER.lock().unwrap_or_else(|e| e.into_inner()) = o;
}

/// Songs added to the end also go to the end of the remembered order.
pub fn order_push(tracks: &[Track]) {
    if let Some(o) = ORDER.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        o.extend(tracks.iter().cloned());
    }
}

/// Songs put after the current one go after it in the remembered order too.
pub fn order_insert_after(st: &Status, tracks: &[Track]) {
    let mut g = ORDER.lock().unwrap_or_else(|e| e.into_inner());
    let Some(o) = g.as_mut() else {
        return;
    };
    let at = st
        .track
        .as_ref()
        .and_then(|c| o.iter().position(|x| x == c))
        .map(|i| i + 1)
        .unwrap_or(o.len());
    let tail = o.split_off(at);
    o.extend(tracks.iter().cloned());
    o.extend(tail);
}

/// `unshuffled()`: the queue in its order from before shuffling. Songs removed since stay out,
/// songs added since go at the end; a song that is in the queue twice keeps both copies.
pub fn unshuffled(order: &[Track], queue: &[Track]) -> Vec<Track> {
    let mut left: Vec<(&Track, usize)> = Vec::new();
    for t in queue {
        match left.iter_mut().find(|(x, _)| *x == t) {
            Some(e) => e.1 += 1,
            None => left.push((t, 1)),
        }
    }
    let mut out = Vec::with_capacity(queue.len());
    let mut take = |t: &Track, out: &mut Vec<Track>| {
        if let Some(e) = left.iter_mut().find(|(x, n)| *n > 0 && *x == t) {
            e.1 -= 1;
            out.push(t.clone());
        }
    };
    for t in order {
        take(t, &mut out);
    }
    for t in queue {
        take(t, &mut out);
    }
    out
}

/// Replace the songs after the current one with `rest`, without touching the one playing:
/// the engine has no reorder command, so the tail is removed from the end and appended again.
pub fn replace_rest(st: &Status, rest: Vec<Track>) {
    let Some(i) = st.index else {
        return;
    };
    for k in ((i + 1)..st.queue.len()).rev() {
        engine::send(Command::Remove(k));
    }
    if !rest.is_empty() {
        engine::send(Command::Enqueue(rest));
    }
}

/// `setShuffle(on)`: on shuffles the rest of the queue, off puts the original order back (the
/// song playing stays where it is).
pub fn set_shuffle(on: bool, st: &Status) {
    engine::set_shuffle(on);
    let Some(i) = st.index else {
        if !on {
            forget_order();
        }
        return;
    };
    if on {
        if order().is_none() {
            set_order(Some(st.queue.clone()));
        }
        let rest = shuffled_rest(st.queue[i..].to_vec(), 0, &mut Rng::new());
        replace_rest(st, rest[1..].to_vec());
    } else if let Some(o) = order() {
        let q = unshuffled(&o, &st.queue);
        if let Some(cur) = st.track.as_ref() {
            if let Some(pos) = q.iter().position(|x| x == cur) {
                // the songs before the current one in the original order play again only
                // with repeat; what follows it is what the engine needs now
                let mut rest: Vec<Track> = q[pos + 1..].to_vec();
                rest.extend(q[..pos].iter().cloned());
                replace_rest(st, rest);
            }
        }
        forget_order();
    }
}

/// Move the song at `from` one place up (`to` = from − 1); nothing for the current song or
/// a target at or before it. Done as remove + play-next when the target is right after the
/// current song, else by rebuilding the tail.
pub fn move_up(st: &Status, from: usize) {
    let Some(i) = st.index else {
        return;
    };
    if from == 0 || from <= i + 1 || from >= st.queue.len() {
        return;
    }
    let to = from - 1;
    let mut rest: Vec<Track> = st.queue[i + 1..].to_vec();
    let t = rest.remove(from - (i + 1));
    rest.insert(to - (i + 1), t);
    replace_rest(st, rest);
}

/// Remove the song at `i` (not the one playing).
pub fn remove_at(st: &Status, i: usize) {
    if Some(i) == st.index || i >= st.queue.len() {
        return;
    }
    let removed = st.queue[i].clone();
    if let Some(o) = ORDER.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        if let Some(p) = o.iter().position(|x| *x == removed) {
            o.remove(p);
        }
    }
    engine::send(Command::Remove(i));
}

/// Up next's Clear: every song after the current one goes (the order kept for shuffle forgets
/// them too). Returns what went, for Undo.
pub fn clear_rest(st: &Status) -> Vec<Track> {
    let Some(i) = st.index else {
        return Vec::new();
    };
    let gone: Vec<Track> = st.queue.get(i + 1..).map(<[Track]>::to_vec).unwrap_or_default();
    // from the end, so the indices before stay
    for k in ((i + 1)..st.queue.len()).rev() {
        engine::send(Command::Remove(k));
    }
    if let Some(o) = ORDER.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        for t in &gone {
            if let Some(p) = o.iter().position(|x| x == t) {
                o.remove(p);
            }
        }
    }
    gone
}

/// Play the song at `i`.
pub fn jump(i: usize) {
    engine::send(Command::PlayIndex(i));
}

// ----- the end of the queue --------------------------------------------------------------

/// What to do when the queue has played to its end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AtEnd {
    Nothing,
    /// Repeat the queue from the top.
    Restart,
    /// Repeat the last song.
    Again(usize),
    /// Keep going with a shuffle of the library.
    Library,
}

/// `next(auto)` at the end of the queue, as the web decides it.
pub fn at_end(repeat: Repeat, autoplay: bool, last: usize) -> AtEnd {
    match repeat {
        Repeat::One => AtEnd::Again(last),
        Repeat::All => AtEnd::Restart,
        Repeat::Off if autoplay => AtEnd::Library,
        Repeat::Off => AtEnd::Nothing,
    }
}

/// Whether a status change is the queue reaching its end on its own: playing a moment ago,
/// stopped now, no error, the last song was the one playing.
pub fn reached_end(prev: &Status, now: &Status) -> bool {
    use crate::engine::PlayState;
    prev.state != PlayState::Stopped
        && now.state == PlayState::Stopped
        && now.error.is_none()
        && !now.queue.is_empty()
        && now.runs_finished > prev.runs_finished
        && now.index.is_some_and(|i| i + 1 >= now.queue.len())
}

// ----- rows for other devices and the saved spot ----------------------------------------

/// The Encore id of a queue entry, when it is a library song.
pub fn lib_id(t: &Track) -> Option<i64> {
    engine::tracks::encore_id(t)
}

/// `{id, title, artist, album, album_id, format, duration}` – what other devices and the saved
/// spot get for a library song (None for files and the test sound).
pub fn slim(t: &Track) -> Option<Value> {
    let id = lib_id(t)?;
    Some(json!({
        "id": id,
        "title": t.title,
        "artist": t.artist,
        "album": t.album,
        "album_id": t.album_id,
        "format": t.format,
        "duration": t.duration,
    }))
}

/// The library songs of the queue, at most 300 around the current one, and where the current
/// song is in them (the web's `lib_window`).
pub fn lib_window(st: &Status) -> (Vec<Value>, i64) {
    let mut queue: Vec<Value> = st.queue.iter().filter_map(slim).collect();
    let mut index = st
        .index
        .map(|i| st.queue.iter().take(i).filter(|t| lib_id(t).is_some()).count() as i64)
        .unwrap_or(0);
    if queue.len() > 300 {
        let from = (index - 50).min(queue.len() as i64 - 300).max(0) as usize;
        queue = queue[from..from + 300].to_vec();
        index -= from as i64;
    }
    (queue, index)
}

/// What this device sends another one with "Send my music there": the library part of the
/// queue, the current song's place in it and the position.
pub fn send_payload(st: &Status) -> Option<Value> {
    let (queue, index) = lib_window(st);
    if queue.is_empty() {
        return None;
    }
    let cur = st.track.as_ref().and_then(lib_id)?;
    let index = queue
        .iter()
        .position(|t| t["id"].as_i64() == Some(cur))
        .map(|i| i as i64)
        .unwrap_or(index);
    Some(json!({ "action": "play_queue", "tracks": queue, "index": index, "position": st.position }))
}

// ----- where you left off on this computer ---------------------------------------------

/// `<config dir>/encore-desktop/resume.json`.
pub fn resume_path() -> std::path::PathBuf {
    crate::config::dir().join("resume.json")
}

/// The spot this computer saved: `{queue, index, position, shuffle, repeat, saved_at}`.
pub fn saved_spot() -> Option<Value> {
    let v: Value = serde_json::from_slice(&std::fs::read(resume_path()).ok()?).ok()?;
    v["queue"]
        .as_array()
        .is_some_and(|q| q.iter().any(|t| t["id"].is_number()))
        .then_some(v)
}

/// The spot to save for `st` (None when nothing from the library plays).
pub fn spot_json(st: &Status, shuffle: bool, repeat: Repeat, saved_at: f64) -> Option<Value> {
    let cur = st.track.as_ref()?;
    lib_id(cur)?;
    let (queue, index) = lib_window(st);
    Some(json!({
        "queue": queue,
        "index": index,
        "position": st.position,
        "shuffle": shuffle,
        "repeat": repeat.id(),
        "saved_at": saved_at,
    }))
}

/// Write the spot (errors are logged, not raised).
pub fn save_spot(v: &Value) {
    let p = resume_path();
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&p, v.to_string()) {
        log::warn!("could not save {}: {e}", p.display());
    }
}

pub fn clear_spot() {
    let _ = std::fs::remove_file(resume_path());
}

/// Seconds since the epoch.
pub fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::PlayState;

    fn t(id: i64) -> Track {
        Track {
            id: Some(id.to_string()),
            source: cadenza::decode::Source::Url {
                url: format!("http://s/api/library/tracks/{id}/stream"),
                headers: vec![],
            },
            title: format!("Song {id}"),
            ..Default::default()
        }
    }

    #[test]
    fn repeat_cycles_and_names() {
        assert_eq!(Repeat::Off.next(), Repeat::All);
        assert_eq!(Repeat::All.next(), Repeat::One);
        assert_eq!(Repeat::One.next(), Repeat::Off);
        assert_eq!(Repeat::parse("one").title(), "Repeat: this song");
        assert_eq!(Repeat::parse("nonsense"), Repeat::Off);
        assert_eq!(at_end(Repeat::One, true, 3), AtEnd::Again(3));
        assert_eq!(at_end(Repeat::All, true, 3), AtEnd::Restart);
        assert_eq!(at_end(Repeat::Off, true, 3), AtEnd::Library);
        assert_eq!(at_end(Repeat::Off, false, 3), AtEnd::Nothing);
    }

    #[test]
    fn shuffling_keeps_the_current_song_and_unshuffle_restores_the_order() {
        let q: Vec<Track> = (1..=8).map(t).collect();
        let mut rng = Rng::seeded(42);
        let s = shuffled_rest(q.clone(), 2, &mut rng);
        assert_eq!(&s[..3], &q[..3], "the songs up to the current one stay");
        let mut rest: Vec<i64> = s[3..].iter().map(|x| lib_id(x).unwrap()).collect();
        rest.sort();
        assert_eq!(rest, [4, 5, 6, 7, 8]);
        // Put the order back: a removed song stays out, an added one goes last.
        let mut shuffled = s.clone();
        shuffled.remove(5);
        shuffled.push(t(9));
        let back = unshuffled(&q, &shuffled);
        let ids: Vec<i64> = back.iter().map(|x| lib_id(x).unwrap()).collect();
        let removed = lib_id(&s[5]).unwrap();
        let mut want: Vec<i64> = (1..=8).filter(|i| *i != removed).collect();
        want.push(9);
        assert_eq!(ids, want);
        // A song twice in the queue keeps both copies.
        let twice = vec![t(1), t(2), t(1)];
        assert_eq!(unshuffled(&twice, &[t(1), t(1), t(2)]).len(), 3);
    }

    #[test]
    fn the_end_of_the_queue_is_recognised_and_rows_are_slim() {
        let q: Vec<Track> = (1..=3).map(t).collect();
        let prev = Status {
            state: PlayState::Playing,
            queue: q.clone(),
            index: Some(2),
            track: Some(q[2].clone()),
            runs_finished: 1,
            ..Default::default()
        };
        let now = Status {
            state: PlayState::Stopped,
            runs_finished: 2,
            ..prev.clone()
        };
        assert!(reached_end(&prev, &now));
        let failed = Status {
            error: Some("cannot play".into()),
            ..now.clone()
        };
        assert!(!reached_end(&prev, &failed), "an error is not the end of the queue");
        let mid = Status {
            index: Some(1),
            ..now.clone()
        };
        assert!(!reached_end(&prev, &mid), "stopping in the middle is a Stop");
        let s = slim(&q[0]).unwrap();
        assert_eq!(s["id"], 1);
        assert_eq!(s["title"], "Song 1");
        assert!(slim(&Track::default()).is_none());
        let (win, idx) = lib_window(&prev);
        assert_eq!((win.len(), idx), (3, 2));
        let p = send_payload(&prev).unwrap();
        assert_eq!(p["index"], 2);
        assert_eq!(p["action"], "play_queue");
        let spot = spot_json(&prev, false, Repeat::All, 5.0).unwrap();
        assert_eq!(spot["repeat"], "all");
        assert_eq!(spot["index"], 2);
    }
}
