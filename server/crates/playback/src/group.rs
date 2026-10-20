//! Group listens (0.31): several people on this server hear the same queue at the same time, each on their
//! own device. This module keeps the groups (in memory, saved to the database while `group_listen_keep` is
//! on) and their rules: who may do what, where the music is now (`position` at `anchor_ms`, the server's
//! clock), moving on at the end of a song, the host handing over, members dropped after 2 minutes without
//! an event stream. The routes are the server's (crates/api routes/group.rs); the shapes they answer are in
//! docs/contracts/client-shapes-0.31-0.33.md §2.2, §2.3 and §3.6–3.18.
//!
//! A member is one (account, device) or one guest. Every change of anything in a group's state bumps its
//! `rev` and is announced on its channel ([`Event::State`]); the event streams then send each device the
//! state as it is at that moment, so a device never sees an older `rev` after a newer one.
use encore_core::error::{err, ApiResult};
use encore_db::{p, Db, Row, RowExt};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::broadcast;

/// The letters of an invite code: upper case without 0/O and 1/I/L, so a code read out loud or typed from a
/// screen can't be mistaken.
pub const CODE_CHARS: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
pub const CODE_LEN: usize = 6;
/// A member whose event stream has been gone this long (seconds) is dropped.
pub const DROP_SECS: f64 = 120.0;
/// A group with no change for this long (seconds) ends ("idle").
pub const IDLE_SECS: f64 = 12.0 * 3600.0;
/// A guest's token works this long (seconds) at most.
pub const GUEST_SECS: f64 = 12.0 * 3600.0;
/// Saving waits for this much quiet after a change (seconds) …
pub const SAVE_QUIET_SECS: f64 = 2.0;
/// … but a group that keeps changing is saved at least this often …
pub const SAVE_LATEST_SECS: f64 = 10.0;
/// … and a playing group is saved this often, so its position after a restart is near where it was (a
/// restart from Settings saves every group first; a stop of the container does not).
pub const SAVE_PLAYING_SECS: f64 = 10.0;
/// The most `group_listen_max` allows: a new group's own limit until its host sets a lower one, so the
/// server's cap alone counts (also after an admin raises it).
pub const MAX_MEMBERS: i64 = 100;
/// Songs a start or a `set_queue` may carry.
pub const START_MAX: usize = 1000;
/// Songs a queue may hold.
pub const QUEUE_MAX: usize = 5000;
/// A restored group's `rev` jumps by this much: a change in the last 2 s before a crash may not have been
/// saved, and a device ignores states with a lower `rev` than the one it has.
pub const REV_AFTER_RESTORE: u64 = 1000;
/// "prev" past this many seconds into a song goes back to its start.
pub const PREV_RESTARTS_AFTER: f64 = 3.0;
/// The columns of a queue entry's song (the library's usual row; the server's enrichment adds the rest).
pub const ROW_COLS: &str = "id, title, artist, album, albumartist, album_id, track, disc, year, duration, format, bitrate";

pub const ANYONE: &str = "anyone";
pub const LINK: &str = "link";

/// Who added a queue entry (the songs of a start or a `set_queue`: whoever sent them).
#[derive(Clone, Debug, PartialEq)]
pub struct AddedBy {
    pub user: i64,
    pub name: String,
    pub guest: bool,
}

impl AddedBy {
    pub fn json(&self) -> Value {
        json!({ "user": self.user, "name": self.name, "guest": self.guest })
    }
}

/// One entry of a group's queue.
#[derive(Clone, Debug)]
pub struct Entry {
    /// unique within the group and stable while the entry exists (the same song queued twice has two)
    pub qid: String,
    pub track: i64,
    pub added_by: AddedBy,
    /// the song's row as the library lists it ([`ROW_COLS`]), not enriched
    pub row: Arc<Row>,
    /// queued with "play next" and not reached yet: the next "play next" goes after it
    pub next: bool,
}

impl Entry {
    /// Seconds; 0 when unknown (such a song never ends by itself).
    pub fn duration(&self) -> f64 {
        self.row.f("duration").max(0.0)
    }
}

/// One member: an account on one device, or a guest.
#[derive(Clone, Debug)]
pub struct Member {
    /// the membership's id (what "Remove from the group" names)
    pub member: String,
    /// the account; 0 for a guest
    pub user: i64,
    /// a guest's own id; None for an account
    pub guest: Option<String>,
    pub name: String,
    /// a server path of the account's picture, or ""
    pub avatar: String,
    /// the device the account joined from ("" for a guest)
    pub device_id: String,
    pub device_name: String,
    pub joined_at: f64,
    pub host: bool,
    /// the account that added this member, when someone did (never so far: everyone joins by themselves)
    pub added_by: Option<i64>,
    /// event streams open now
    pub streams: u32,
    /// since when it has had no event stream open (it joined, or its last stream closed); None while one is
    pub gone_since: Option<f64>,
    /// a guest's token stops working then
    pub expires_at: Option<f64>,
}

impl Member {
    pub fn is_guest(&self) -> bool {
        self.guest.is_some()
    }

    pub fn json(&self) -> Value {
        json!({
            "member": self.member, "user": self.user, "name": self.name, "avatar": self.avatar,
            "device_name": self.device_name, "joined_at": self.joined_at.floor() as i64, "host": self.host,
            "guest": self.is_guest(),
        })
    }
}

/// What a group's channel says.
#[derive(Clone, Debug)]
pub enum Event {
    /// something changed: the streams send the state as it is now
    State,
    /// the group ended for everyone (`member` None), or for one member (`member`: removed, expired)
    Ended { reason: &'static str, member: Option<String> },
}

/// Who is asking: an account on a device, or a guest (by the membership its token names).
#[derive(Clone, Debug)]
pub enum Caller {
    Account { user: i64, device: String },
    Guest { member: String },
}

/// What the caller is in a group, for `you` and the permission checks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct You {
    /// the membership of this device ("" when the device is not a member)
    pub member: String,
    /// the caller's account hosts the group
    pub host: bool,
    /// may play, pause, seek and change the queue: the host, or everyone when "Everyone can control" (never a guest)
    pub can_control: bool,
    /// the caller is a guest
    pub guest: bool,
}

impl You {
    pub fn json(&self) -> Value {
        json!({ "member": self.member, "host": self.host, "can_control": self.can_control })
    }
}

/// One group listen.
pub struct Group {
    pub id: String,
    pub code: String,
    /// [`ANYONE`] or [`LINK`]
    pub visibility: String,
    pub everyone_controls: bool,
    /// the host's switch "Guests can join with the link"
    pub guests_allowed: bool,
    /// the host's cap ("Limit to N people"); the server's `group_listen_max` caps it too
    pub max_members: i64,
    pub queue: Vec<Entry>,
    pub index: usize,
    pub playing: bool,
    /// seconds into the current song at `anchor_ms`
    pub position: f64,
    /// the server's clock (Unix ms) when `position` was true
    pub anchor_ms: i64,
    pub rev: u64,
    pub started_at: f64,
    /// the last change (Unix seconds): idle groups end after [`IDLE_SECS`]
    pub active_at: f64,
    /// ordered by `joined_at`
    pub members: Vec<Member>,
    next_qid: u64,
    next_member: u64,
    pub tx: broadcast::Sender<Event>,
    /// when the oldest change not saved yet happened (None: nothing to save)
    pub unsaved: Option<f64>,
    /// when it was saved last
    pub saved_at: f64,
    /// the state everyone sees at `.0` = rev, made (and enriched) by the API on first use
    pub cache: Option<(u64, Arc<Value>)>,
}

/// Where a new queue starts: the entry, seconds into it, playing or not.
#[derive(Clone, Copy, Debug)]
pub struct At {
    pub index: usize,
    pub position: f64,
    pub playing: bool,
}

/// A new group's settings.
pub struct NewGroup {
    pub id: String,
    pub code: String,
    pub visibility: String,
    pub everyone_controls: bool,
    pub guests_allowed: bool,
    pub max_members: i64,
}

impl Group {
    pub fn new(n: NewGroup, now_ms: i64) -> Group {
        let t = now_ms as f64 / 1000.0;
        Group {
            id: n.id, code: n.code, visibility: n.visibility, everyone_controls: n.everyone_controls, guests_allowed: n.guests_allowed,
            max_members: n.max_members, queue: vec![], index: 0, playing: false, position: 0.0, anchor_ms: now_ms, rev: 1,
            started_at: t, active_at: t, members: vec![], next_qid: 1, next_member: 1, tx: broadcast::channel(64).0,
            unsaved: Some(t), saved_at: 0.0, cache: None,
        }
    }

    // ---- who is who -----------------------------------------------------------------------------------

    pub fn host(&self) -> Option<&Member> {
        self.members.iter().find(|m| m.host)
    }

    /// The host's account (0 when there is none, which a live group never is).
    pub fn host_user(&self) -> i64 {
        self.host().map(|m| m.user).unwrap_or(0)
    }

    pub fn host_name(&self) -> String {
        self.host().map(|m| m.name.clone()).unwrap_or_default()
    }

    pub fn member(&self, id: &str) -> Option<&Member> {
        self.members.iter().find(|m| m.member == id)
    }

    /// The caller's membership in this group.
    pub fn find(&self, c: &Caller) -> Option<&Member> {
        match c {
            Caller::Account { user, device } => self.members.iter().find(|m| m.guest.is_none() && m.user == *user && m.device_id == *device),
            Caller::Guest { member } => self.members.iter().find(|m| m.member == *member && m.is_guest()),
        }
    }

    /// The account is in the group on any device.
    pub fn has_account(&self, user: i64) -> bool {
        user > 0 && self.members.iter().any(|m| m.guest.is_none() && m.user == user)
    }

    pub fn you(&self, c: &Caller) -> You {
        let member = self.find(c).map(|m| m.member.clone()).unwrap_or_default();
        match c {
            Caller::Account { user, .. } => {
                let host = *user > 0 && *user == self.host_user();
                // controlling takes being in the group (on any device): the host's account, or everyone when allowed
                let can_control = self.has_account(*user) && (host || self.everyone_controls);
                You { member, host, can_control, guest: false }
            }
            Caller::Guest { .. } => You { member, host: false, can_control: false, guest: true },
        }
    }

    /// The cap: the host's, never above the server's.
    pub fn cap(&self, server_max: i64) -> i64 {
        self.max_members.min(server_max).max(2)
    }

    pub fn full(&self, server_max: i64) -> bool {
        self.members.len() as i64 >= self.cap(server_max)
    }

    pub fn anyone(&self) -> bool {
        self.visibility != LINK
    }

    // ---- members ------------------------------------------------------------------------------------

    /// Adds a member and gives it its id. The caller checked the cap.
    pub fn add(&mut self, mut m: Member) -> String {
        m.member = format!("m{}", self.next_member);
        self.next_member += 1;
        if self.members.iter().all(|x| !x.host) && m.guest.is_none() {
            m.host = true;
        }
        let id = m.member.clone();
        self.members.push(m);
        self.members.sort_by(|a, b| a.joined_at.total_cmp(&b.joined_at));
        id
    }

    /// Takes a member out. The host's leaving hands the group to the host's account on another device when it
    /// is still in the group, else to the account member who has been in it longest (a guest never hosts).
    /// True when no account member is left: the group ends.
    pub fn remove(&mut self, member: &str) -> bool {
        let Some(at) = self.members.iter().position(|m| m.member == member) else { return !self.members.iter().any(|m| m.guest.is_none()) };
        let gone = self.members.remove(at);
        if gone.host {
            // host rights are the account's: while it is still in the group on another device, that device hosts
            let longest = |same: bool| {
                self.members.iter().enumerate()
                    .filter(|(_, m)| m.guest.is_none() && (!same || m.user == gone.user))
                    .min_by(|a, b| a.1.joined_at.total_cmp(&b.1.joined_at))
                    .map(|(i, _)| i)
            };
            if let Some(i) = longest(true).or_else(|| longest(false)) {
                self.members[i].host = true;
            }
        }
        !self.members.iter().any(|m| m.guest.is_none())
    }

    // ---- the clock ----------------------------------------------------------------------------------

    /// Seconds into the current song at `now_ms`.
    pub fn position_at(&self, now_ms: i64) -> f64 {
        let p = if self.playing { self.position + (now_ms - self.anchor_ms).max(0) as f64 / 1000.0 } else { self.position };
        match self.current() {
            Some(e) if e.duration() > 0.0 => p.min(e.duration()),
            _ => p,
        }
    }

    pub fn current(&self) -> Option<&Entry> {
        self.queue.get(self.index)
    }

    /// The music is at `position` now.
    fn set_at(&mut self, position: f64, now_ms: i64) {
        self.position = position.max(0.0);
        self.anchor_ms = now_ms;
    }

    /// Moves on to the next entry at the end of a song while playing (several when the server was busy):
    /// `anchor_ms` is the moment the song ended. At the end of the last entry it stops there. True when
    /// anything changed (the caller announces it).
    pub fn advance(&mut self, now_ms: i64) -> bool {
        let mut changed = false;
        while self.playing {
            let Some(d) = self.current().map(Entry::duration) else {
                self.playing = false;
                self.set_at(0.0, now_ms);
                return true;
            };
            if d <= 0.0 {
                break;
            }
            let left_ms = ((d - self.position) * 1000.0).round() as i64;
            let end_ms = self.anchor_ms + left_ms.max(0);
            if end_ms > now_ms {
                break;
            }
            changed = true;
            if self.index + 1 < self.queue.len() {
                self.index += 1;
                self.queue[self.index].next = false;
                self.position = 0.0;
                self.anchor_ms = end_ms;
            } else {
                self.playing = false;
                self.position = d;
                self.anchor_ms = end_ms;
            }
        }
        changed
    }

    /// Something changed: `rev` grows by one, the change is to be saved and the streams are told.
    pub fn changed(&mut self, now: f64) {
        self.rev += 1;
        self.active_at = now;
        self.unsaved.get_or_insert(now);
        self.cache = None;
        let _ = self.tx.send(Event::State);
    }

    /// Whether the saver should write this group now (see the SAVE_* times).
    pub fn save_due(&self, now: f64) -> bool {
        match self.unsaved {
            Some(first) => now - self.active_at >= SAVE_QUIET_SECS || now - first >= SAVE_LATEST_SECS,
            None => self.playing && now - self.saved_at >= SAVE_PLAYING_SECS,
        }
    }

    // ---- the queue ----------------------------------------------------------------------------------

    fn entry(&mut self, track: i64, row: Arc<Row>, by: &AddedBy) -> Entry {
        let qid = format!("q{}", self.next_qid);
        self.next_qid += 1;
        Entry { qid, track, added_by: by.clone(), row, next: false }
    }

    /// The queue becomes `tracks` (each with its row; songs without one are left out, and `at.index` follows
    /// the song it named, or the next one that is kept).
    pub fn set_queue(&mut self, tracks: &[i64], rows: &HashMap<i64, Arc<Row>>, at: At, by: &AddedBy, now_ms: i64) {
        let At { index, position, playing } = at;
        let mut queue = Vec::with_capacity(tracks.len());
        let mut found = None;
        for (i, t) in tracks.iter().enumerate() {
            if i == index {
                found = Some(queue.len());
            }
            if let Some(r) = rows.get(t) {
                queue.push(self.entry(*t, r.clone(), by));
            }
        }
        // the song asked for was left out: the next kept one plays from its start
        let named_kept = index < tracks.len() && rows.contains_key(&tracks[index]);
        let at = found.unwrap_or(0).min(queue.len().saturating_sub(1));
        self.queue = queue;
        self.index = at;
        self.playing = playing && !self.queue.is_empty();
        let pos = if named_kept { position } else { 0.0 };
        let pos = match self.current() {
            Some(e) if e.duration() > 0.0 => pos.clamp(0.0, e.duration()),
            Some(_) => pos.max(0.0),
            None => 0.0,
        };
        self.set_at(pos, now_ms);
    }

    /// The index of the entry a command names: `qid` wins over `index` when both are given and disagree.
    fn pick(&self, qid: Option<&str>, index: Option<i64>) -> ApiResult<usize> {
        if let Some(q) = qid.filter(|q| !q.is_empty()) {
            return self.queue.iter().position(|e| e.qid == q).ok_or_else(|| err(409, "That song is no longer in the queue"));
        }
        match index {
            Some(i) if i >= 0 && (i as usize) < self.queue.len() => Ok(i as usize),
            Some(_) => Err(err(400, "There is no such entry in the queue")),
            None => Err(err(400, "The command doesn't say which song in the queue")),
        }
    }

    fn room(&self, more: usize) -> ApiResult<()> {
        if self.queue.len() + more > QUEUE_MAX {
            return Err(err(400, format!("A group listen's queue holds at most {QUEUE_MAX} songs")));
        }
        Ok(())
    }

    /// One command (client-shapes §3.15). `rows`: the songs its `tracks` name (unknown ids left out).
    /// The caller checked the permission ([`needs_control`]) and announces the change.
    pub fn command(&mut self, op: &str, body: &Value, rows: &HashMap<i64, Arc<Row>>, by: &AddedBy, now_ms: i64) -> ApiResult<()> {
        self.advance(now_ms);
        let now_pos = self.position_at(now_ms);
        let qid = body["qid"].as_str();
        let index = body["index"].as_i64();
        match op {
            "play" => {
                if !self.queue.is_empty() {
                    let d = self.current().map(Entry::duration).unwrap_or(0.0);
                    let at_end = !self.playing && d > 0.0 && now_pos >= d;
                    let pos = if at_end && self.index + 1 < self.queue.len() {
                        // the queue ran out and songs were added since: the first of them, from its start
                        self.index += 1;
                        self.queue[self.index].next = false;
                        0.0
                    } else if at_end {
                        // at the very end of the last song: that song again from its start
                        0.0
                    } else {
                        now_pos
                    };
                    self.playing = true;
                    self.set_at(pos, now_ms);
                }
            }
            "pause" => {
                self.playing = false;
                self.set_at(now_pos, now_ms);
            }
            "seek" => {
                let p = body["position"].as_f64().ok_or_else(|| err(400, "The seek doesn't say where to"))?;
                let p = match self.current() {
                    Some(e) if e.duration() > 0.0 => p.clamp(0.0, e.duration()),
                    _ => p.max(0.0),
                };
                self.set_at(p, now_ms);
            }
            "next" => {
                if self.index + 1 < self.queue.len() {
                    self.index += 1;
                    self.queue[self.index].next = false;
                    self.set_at(0.0, now_ms);
                } else if let Some(d) = self.current().map(Entry::duration) {
                    self.playing = false;
                    self.set_at(d, now_ms);
                }
            }
            "prev" => {
                if now_pos > PREV_RESTARTS_AFTER || self.index == 0 {
                    self.set_at(0.0, now_ms);
                } else {
                    self.index -= 1;
                    self.set_at(0.0, now_ms);
                }
            }
            "jump" => {
                let at = self.pick(qid, index)?;
                self.index = at;
                self.queue[at].next = false;
                self.playing = true;
                self.set_at(0.0, now_ms);
            }
            "set_queue" => {
                let tracks = ids(body)?;
                if tracks.len() > START_MAX {
                    return Err(err(400, format!("A group listen's queue can be set to at most {START_MAX} songs at once")));
                }
                let at = index.unwrap_or(0).max(0) as usize;
                let pos = body["position"].as_f64().unwrap_or(0.0);
                self.set_queue(&tracks, rows, At { index: at, position: pos, playing: true }, by, now_ms);
            }
            "enqueue" | "play_next" => {
                let tracks: Vec<i64> = ids(body)?.into_iter().filter(|t| rows.contains_key(t)).collect();
                if tracks.is_empty() {
                    return Err(err(400, "Only songs from your library can be played in a group listen"));
                }
                self.room(tracks.len())?;
                let was_empty = self.queue.is_empty();
                let mut new: Vec<Entry> = tracks.iter().map(|t| self.entry(*t, rows[t].clone(), by)).collect();
                if op == "enqueue" || was_empty {
                    self.queue.append(&mut new);
                } else {
                    // after the current entry and after the "play next" entries that follow it, not reached yet
                    let mut at = self.index + 1;
                    while at < self.queue.len() && self.queue[at].next {
                        at += 1;
                    }
                    for e in new.iter_mut() {
                        e.next = true;
                    }
                    self.queue.splice(at..at, new);
                }
                if was_empty {
                    // the first songs of an empty queue: the group is at the first one, not playing yet
                    self.index = 0;
                    self.playing = false;
                    self.set_at(0.0, now_ms);
                }
            }
            "remove" => {
                let at = self.pick(qid, index)?;
                self.queue.remove(at);
                if self.queue.is_empty() {
                    self.index = 0;
                    self.playing = false;
                    self.set_at(0.0, now_ms);
                } else if at < self.index {
                    self.index -= 1;
                } else if at == self.index {
                    // the current one: the next plays (from its start); there is none after the last
                    if self.index >= self.queue.len() {
                        self.index = self.queue.len() - 1;
                        self.playing = false;
                        let d = self.current().map(Entry::duration).unwrap_or(0.0);
                        self.set_at(d, now_ms);
                    } else {
                        self.queue[self.index].next = false;
                        self.set_at(0.0, now_ms);
                    }
                }
            }
            "move" => {
                let from = self.pick(qid, body["from"].as_i64().or(index))?;
                let to = body["to"].as_i64().ok_or_else(|| err(400, "The move doesn't say where to"))?;
                if to < 0 || to as usize >= self.queue.len() {
                    return Err(err(400, "There is no such entry in the queue"));
                }
                let to = to as usize;
                let current = self.queue[self.index].qid.clone();
                let e = self.queue.remove(from);
                self.queue.insert(to, e);
                self.index = self.queue.iter().position(|e| e.qid == current).unwrap_or(0);
            }
            _ => return Err(err(400, "Unknown command")),
        }
        Ok(())
    }

    // ---- the state ----------------------------------------------------------------------------------

    /// The state everyone in the group sees (client-shapes §2.2), before the server's enrichment and without
    /// what differs per device or per setting (`invite_url`, `you`, the caps, `guests_possible`).
    pub fn base_json(&self, boot: &str) -> Value {
        let queue: Vec<Value> = self.queue.iter().map(entry_json).collect();
        let track = queue.get(self.index).cloned().unwrap_or(Value::Null);
        let host = self.host();
        json!({
            "id": self.id, "code": self.code, "rev": self.rev, "boot": boot,
            "host": { "user": host.map(|m| m.user).unwrap_or(0), "name": host.map(|m| m.name.clone()).unwrap_or_default(), "avatar": host.map(|m| m.avatar.clone()).unwrap_or_default() },
            "everyone_controls": self.everyone_controls, "visibility": self.visibility, "guests_allowed": self.guests_allowed,
            "queue": queue, "index": self.index, "track": track, "playing": self.playing,
            "position": (self.position * 1000.0).round() / 1000.0, "anchor_ms": self.anchor_ms,
            "members": self.members.iter().map(Member::json).collect::<Vec<_>>(), "started_at": self.started_at.floor() as i64,
        })
    }

    /// The group in the server's list (client-shapes §2.3) for an account (`mine`); `track` before enrichment.
    pub fn summary_json(&self, user: i64) -> Value {
        let track = self.current().map(|e| {
            let r = &e.row;
            json!({ "id": e.track, "title": r.s("title"), "artist": r.s("artist"), "album_id": r.s("album_id") })
        });
        let host = self.host();
        json!({
            "id": self.id,
            "host": { "user": host.map(|m| m.user).unwrap_or(0), "name": host.map(|m| m.name.clone()).unwrap_or_default(), "avatar": host.map(|m| m.avatar.clone()).unwrap_or_default() },
            "track": track, "playing": self.playing, "members_count": self.members.len(), "started_at": self.started_at.floor() as i64,
            "visibility": self.visibility, "mine": self.has_account(user),
        })
    }
}

fn entry_json(e: &Entry) -> Value {
    let mut o = (*e.row).clone();
    o.insert("qid".into(), json!(e.qid));
    o.insert("added_by".into(), e.added_by.json());
    Value::Object(o)
}

/// The song ids of a command's `tracks`.
pub fn ids(body: &Value) -> ApiResult<Vec<i64>> {
    let a = body["tracks"].as_array().ok_or_else(|| err(400, "The command doesn't list any songs"))?;
    Ok(a.iter().filter_map(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).collect())
}

/// Whether an op needs control (the host, or everyone when "Everyone can control"); None for an unknown op.
/// Adding songs (enqueue, play_next) is for every member with an account.
pub fn needs_control(op: &str) -> Option<bool> {
    match op {
        "enqueue" | "play_next" => Some(false),
        "play" | "pause" | "seek" | "next" | "prev" | "jump" | "set_queue" | "remove" | "move" => Some(true),
        _ => None,
    }
}

/// The songs `ids` name, as queue rows ([`ROW_COLS`]); unknown ids are not in the answer.
pub fn rows(db: &impl encore_db::Query, ids: &[i64]) -> HashMap<i64, Arc<Row>> {
    let mut out = HashMap::new();
    let mut wanted: Vec<i64> = ids.iter().copied().filter(|i| *i > 0).collect();
    wanted.sort_unstable();
    wanted.dedup();
    for chunk in wanted.chunks(500) {
        let marks = vec!["?"; chunk.len()].join(",");
        let params = chunk.iter().map(|i| json!(i)).collect();
        if let Ok(found) = db.query(&format!("SELECT {ROW_COLS} FROM tracks WHERE id IN ({marks})"), params) {
            for r in found {
                out.insert(r.i("id"), Arc::new(r));
            }
        }
    }
    out
}

/// Turns what was typed into a code: upper case, without spaces and dashes.
pub fn normalize_code(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace() && *c != '-').flat_map(char::to_uppercase).collect()
}

/// A random invite code ([`CODE_LEN`] of [`CODE_CHARS`]).
pub fn random_code() -> String {
    let n = CODE_CHARS.len();
    let limit = 256 - 256 % n;
    let mut out = String::with_capacity(CODE_LEN);
    while out.len() < CODE_LEN {
        let bytes = hex::decode(encore_core::ids::random_hex(16)).unwrap_or_default();
        for b in bytes {
            if (b as usize) < limit && out.len() < CODE_LEN {
                out.push(CODE_CHARS[b as usize % n] as char);
            }
        }
    }
    out
}

/// Every group listen of this server process. Clones share them.
#[derive(Clone)]
pub struct Groups {
    inner: Arc<Mutex<Inner>>,
    /// an id of this server process (random at start): a device tells a restart from an ended group by it
    pub boot: Arc<str>,
}

#[derive(Default)]
pub struct Inner {
    /// by id
    pub groups: HashMap<String, Group>,
    /// the saved groups were read back (or cleared) at start
    pub restored: bool,
    /// the groups that have rows in the database
    pub saved: HashSet<String>,
    /// saved groups that ended: their rows go at the next save
    pub ended: Vec<String>,
}

impl Default for Groups {
    fn default() -> Self {
        Groups { inner: Default::default(), boot: Arc::from(encore_core::ids::random_hex(3)) }
    }
}

impl Groups {
    pub fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Inner {
    /// A code no live group has.
    pub fn new_code(&self) -> String {
        loop {
            let c = random_code();
            if !self.groups.values().any(|g| g.code == c) {
                return c;
            }
        }
    }

    pub fn by_code(&self, code: &str) -> Option<&Group> {
        let code = normalize_code(code);
        if code.len() != CODE_LEN {
            return None;
        }
        self.groups.values().find(|g| g.code == code)
    }

    /// The group an account's device is a member of (a guest's group is the one its token names: member ids
    /// are only unique within a group).
    pub fn of(&self, user: i64, device: &str) -> Option<&Group> {
        let c = Caller::Account { user, device: device.to_string() };
        self.groups.values().find(|g| g.find(&c).is_some())
    }

    /// Ends a group: everyone's stream gets `ended` with `reason`. Its saved rows go at the next save.
    pub fn end(&mut self, id: &str, reason: &'static str) -> Option<Group> {
        let g = self.groups.remove(id)?;
        let _ = g.tx.send(Event::Ended { reason, member: None });
        if self.saved.remove(id) {
            self.ended.push(id.to_string());
        }
        Some(g)
    }
}

// ---- keeping groups through a restart (table group_listens, schema 27) ------------------------------------

/// What is saved of a group: its row and its members' rows.
pub struct Saved {
    pub id: String,
    pub group: Vec<Value>,
    pub members: Vec<Vec<Value>>,
}

impl Group {
    /// The group as it is saved at `now_ms`: the position where the music is now. No token is saved (a
    /// guest's token lives in `sessions`, as a digest); `data` holds what has no column of its own.
    pub fn saved(&self, now_ms: i64) -> Saved {
        let queue: Vec<Value> = self.queue.iter().map(|e| json!({ "id": e.track, "qid": e.qid, "added_by": e.added_by.json(), "next": e.next })).collect();
        let guests: Map<String, Value> = self.members.iter().filter_map(|m| Some((m.member.clone(), json!(m.expires_at?)))).collect();
        let data = json!({ "next_qid": self.next_qid, "next_member": self.next_member, "playing": self.playing, "active_at": self.active_at, "guest_expires": guests });
        let now = now_ms as f64 / 1000.0;
        Saved {
            id: self.id.clone(),
            group: p![self.id, self.code, self.host_user(), self.visibility, self.everyone_controls as i64, self.guests_allowed as i64, self.max_members,
                      Value::Array(queue).to_string(), self.index as i64, self.position_at(now_ms), self.rev as i64, data.to_string(), self.started_at, self.started_at, now],
            members: self.members.iter().map(|m| p![self.id, m.member, if m.is_guest() { Value::Null } else { json!(m.user) }, m.guest.clone(), m.name,
                                                      m.device_id, m.device_name, m.host as i64, m.added_by, m.joined_at]).collect(),
        }
    }
}

/// Writes `groups` and deletes the rows of `gone`, in one transaction.
pub fn save(db: &Db, groups: &[Saved], gone: &[String]) -> anyhow::Result<()> {
    db.write_now(|c| {
        c.transaction(|c| {
            for id in gone {
                c.execute("DELETE FROM group_listen_members WHERE group_id=?", p![id])?;
                c.execute("DELETE FROM group_listens WHERE id=?", p![id])?;
            }
            for g in groups {
                c.execute(
                    "INSERT OR REPLACE INTO group_listens (id, code, host_user_id, visibility, everyone_controls, guests_allowed, max_members, queue, idx, position, rev, data, started_at, created_at, updated_at)
                     VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                    g.group.clone(),
                )?;
                c.execute("DELETE FROM group_listen_members WHERE group_id=?", p![g.id])?;
                for m in &g.members {
                    c.execute(
                        "INSERT INTO group_listen_members (group_id, member, user_id, guest_id, name, device_id, device_name, is_host, added_by, joined_at) VALUES (?,?,?,?,?,?,?,?,?,?)",
                        m.clone(),
                    )?;
                }
            }
            Ok::<_, anyhow::Error>(())
        })
    })
}

/// Deletes every saved group (`group_listen_keep` off).
pub fn clear(db: &Db) -> anyhow::Result<()> {
    db.write_now(|c| {
        c.execute("DELETE FROM group_listen_members", p![])?;
        c.execute("DELETE FROM group_listens", p![])?;
        Ok::<_, anyhow::Error>(())
    })
}

/// The saved groups as they come back after a restart: paused at the saved position, with the same id,
/// code, settings, queue (songs no longer in the library left out) and members; their `rev` jumps by
/// [`REV_AFTER_RESTORE`]; every member has 2 minutes to open its event stream again. Groups idle for 12 h,
/// and groups without an account member, are left out and their rows deleted. `avatar`: an account's
/// picture path.
pub fn load(db: &Db, now_ms: i64, avatar: impl Fn(i64) -> String) -> Vec<Group> {
    let now = now_ms as f64 / 1000.0;
    let Ok(saved) = db.query("SELECT * FROM group_listens", p![]) else { return vec![] };
    let mut out = vec![];
    let mut stale = vec![];
    for r in saved {
        let id = r.s("id");
        let data: Value = serde_json::from_str(&r.s("data")).unwrap_or(json!({}));
        let active_at = data["active_at"].as_f64().unwrap_or(r.f("updated_at"));
        if now - active_at > IDLE_SECS {
            stale.push(id);
            continue;
        }
        let saved_queue: Vec<Value> = serde_json::from_str(&r.s("queue")).unwrap_or_default();
        let found = rows(db, &saved_queue.iter().filter_map(|e| e["id"].as_i64()).collect::<Vec<_>>());
        let saved_idx = r.i("idx").max(0) as usize;
        let mut queue = vec![];
        let mut index = 0;
        let mut position = r.f("position").max(0.0);
        for (i, e) in saved_queue.iter().enumerate() {
            let Some(t) = e["id"].as_i64() else { continue };
            if i == saved_idx {
                index = queue.len();
            }
            let Some(row) = found.get(&t) else {
                if i == saved_idx {
                    position = 0.0; // the current song is gone: the next one from its start
                }
                continue;
            };
            let by = &e["added_by"];
            queue.push(Entry {
                qid: e["qid"].as_str().unwrap_or_default().to_string(),
                track: t,
                added_by: AddedBy { user: by["user"].as_i64().unwrap_or(0), name: by["name"].as_str().unwrap_or_default().to_string(), guest: by["guest"].as_bool().unwrap_or(false) },
                row: row.clone(),
                next: e["next"].as_bool().unwrap_or(false),
            });
        }
        let index = index.min(queue.len().saturating_sub(1));
        let members: Vec<Member> = db.query("SELECT * FROM group_listen_members WHERE group_id=? ORDER BY joined_at", p![id]).unwrap_or_default()
            .into_iter().map(|m| {
                let guest = (!m.s("guest_id").is_empty()).then(|| m.s("guest_id"));
                let user = if guest.is_some() { 0 } else { m.i("user_id") };
                let expires_at = guest.as_ref().map(|_| data["guest_expires"][m.s("member")].as_f64().unwrap_or(now));
                Member {
                    member: m.s("member"), user, avatar: if guest.is_some() { String::new() } else { avatar(user) }, guest, name: m.s("name"),
                    device_id: m.s("device_id"), device_name: m.s("device_name"), joined_at: m.f("joined_at"), host: m.b("is_host"),
                    added_by: m.get("added_by").and_then(Value::as_i64), streams: 0, gone_since: Some(now), expires_at,
                }
            }).collect();
        if !members.iter().any(|m| m.guest.is_none()) {
            stale.push(id);
            continue;
        }
        let mut g = Group {
            id: id.clone(), code: r.s("code"), visibility: if r.s("visibility") == LINK { LINK.into() } else { ANYONE.into() },
            everyone_controls: r.b("everyone_controls"), guests_allowed: r.b("guests_allowed"), max_members: r.i("max_members").max(2),
            queue, index, playing: false, position, anchor_ms: now_ms, rev: r.i("rev").max(0) as u64 + REV_AFTER_RESTORE,
            started_at: r.f("started_at"), active_at, members, next_qid: data["next_qid"].as_u64().unwrap_or(1),
            next_member: data["next_member"].as_u64().unwrap_or(1), tx: broadcast::channel(64).0, unsaved: Some(now), saved_at: now, cache: None,
        };
        // a host is always there: the longest-present account member when the saved one is missing
        if g.host().is_none_or(Member::is_guest) {
            for m in g.members.iter_mut() {
                m.host = false;
            }
            if let Some(m) = g.members.iter_mut().find(|m| m.guest.is_none()) {
                m.host = true;
            }
        }
        match g.current().map(Entry::duration) {
            Some(d) if d > 0.0 => g.position = g.position.min(d),
            Some(_) => {}
            None => g.position = 0.0,
        }
        out.push(g);
    }
    if !stale.is_empty() {
        let _ = save(db, &[], &stale);
    }
    out
}

#[cfg(test)]
#[path = "group_tests.rs"]
mod tests;
