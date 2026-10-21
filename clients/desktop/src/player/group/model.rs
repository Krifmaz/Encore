//! Group listen (p6 item 8): the server's shapes (docs/contracts/client-shapes-0.31-0.33.md §2.2
//! GroupState, §2.3 GroupSummary) read leniently, so an older or newer server never costs the
//! whole answer, and the rules that need no window: codes and links, where the group is now,
//! the clock offset and the words. The phone's GroupModels.kt is the reference; the tests are
//! its GroupRulesTest cases.

use crate::fmt;
use serde_json::Value;

/// One person (one device) in a group listen.
#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    /// The server's id of this membership (`member`): what "Remove from the group" names.
    pub key: String,
    pub user: i64,
    pub name: String,
    /// A server path of the avatar ("/api/users/3/avatar"), or "" for the first letter.
    pub avatar: String,
    pub device_name: String,
    pub joined_at: f64,
    pub host: bool,
    pub guest: bool,
}

impl Member {
    pub fn from(v: &Value) -> Option<Member> {
        let name = fmt::s(v, "name");
        let mut key = fmt::s(v, "member");
        if key.is_empty() {
            key = fmt::s(v, "id");
        }
        if key.is_empty() {
            key = format!(
                "{}:{}:{}",
                if fmt::truthy(v, "guest") { "g" } else { "u" },
                fmt::s(v, "user"),
                fmt::s(v, "device_name")
            );
        }
        if name.is_empty() && key.is_empty() {
            return None;
        }
        Some(Member {
            key,
            user: fmt::i(v, "user"),
            name: if name.is_empty() { "Someone".into() } else { name },
            avatar: fmt::s(v, "avatar"),
            device_name: fmt::s(v, "device_name"),
            joined_at: fmt::f(v, "joined_at"),
            host: fmt::truthy(v, "host"),
            guest: fmt::truthy(v, "guest"),
        })
    }
}

/// One song of the group's queue, with who added it ("added by Sam").
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// The queue entry's own id (the same song can be queued twice): what remove / move / jump
    /// name.
    pub qid: String,
    /// The library song.
    pub id: i64,
    /// The track row as the server sent it (title, artist, album_id, format, duration, gain …):
    /// what the engine's queue entry is built from.
    pub row: Value,
    pub added_by: String,
    pub added_by_user: i64,
}

impl Entry {
    pub fn from(v: &Value, at: usize) -> Option<Entry> {
        let id = crate::engine::tracks::id_of(v)?;
        let by = &v["added_by"];
        let (added_by, added_by_user) = if by.is_object() {
            (fmt::s(by, "name"), fmt::i(by, "user"))
        } else {
            (fmt::s(v, "added_by"), 0)
        };
        let mut qid = fmt::s(v, "qid");
        if qid.is_empty() {
            qid = format!("i{at}-{id}");
        }
        Some(Entry {
            qid,
            id,
            row: v.clone(),
            added_by,
            added_by_user,
        })
    }

    pub fn title(&self) -> String {
        fmt::s(&self.row, "title")
    }

    pub fn artist(&self) -> String {
        let a = fmt::s(&self.row, "artist");
        if a.is_empty() {
            fmt::s(&self.row, "albumartist")
        } else {
            a
        }
    }

    pub fn album_id(&self) -> String {
        fmt::s(&self.row, "album_id")
    }

    pub fn duration(&self) -> f64 {
        fmt::f(&self.row, "duration")
    }
}

/// The group listen this computer is in, as the server last told it (REST answers and the event
/// stream).
#[derive(Clone, Debug, PartialEq)]
pub struct GroupState {
    pub id: String,
    pub code: String,
    pub invite_url: String,
    pub rev: i64,
    pub host_name: String,
    pub host_user: i64,
    pub host_avatar: String,
    pub everyone_controls: bool,
    /// `"anyone"` (anyone on this server can see and join) | `"link"` (only people with the link).
    pub visibility: String,
    /// The host's switch "Guests can join with the link".
    pub guests_allowed: bool,
    /// The server allows guests at all (`group_listen_guests`): the host's switch shows only then.
    pub guests_possible: bool,
    /// This group's cap ("Limit to N people") and the server's (`group_listen_max`).
    pub max_members: u32,
    pub server_max: u32,
    pub queue: Vec<Entry>,
    /// The current entry; None when the queue is empty.
    pub index: Option<usize>,
    pub playing: bool,
    /// Seconds into the song at `anchor_ms` (the server's clock, ms).
    pub position: f64,
    pub anchor_ms: i64,
    pub members: Vec<Member>,
    pub started_at: f64,
    /// The server process's id: another one after a restart.
    pub boot: String,
    /// This device's membership, and what it may do.
    pub you_member: String,
    pub you_host: bool,
    pub you_can_control: bool,
}

impl GroupState {
    /// A state from an answer or an event (`{state: {...}}` is unwrapped); None without an id.
    pub fn from(v: &Value) -> Option<GroupState> {
        let s = if v["state"].is_object() { &v["state"] } else { v };
        let id = fmt::s(s, "id");
        if id.is_empty() {
            return None;
        }
        let host = &s["host"];
        let queue: Vec<Entry> = fmt::items(&s["queue"])
            .iter()
            .enumerate()
            .filter_map(|(i, q)| Entry::from(q, i))
            .collect();
        let members: Vec<Member> = fmt::items(&s["members"]).iter().filter_map(Member::from).collect();
        let you = &s["you"];
        let you_host = fmt::truthy(you, "host");
        let everyone = fmt::truthy(s, "everyone_controls");
        // group_listen_max's default
        let server_max = match fmt::i(s, "server_max_members") {
            n if n > 0 => n as u32,
            _ => 20,
        };
        let mut host_name = fmt::s(host, "name");
        if host_name.is_empty() {
            host_name = members
                .iter()
                .find(|m| m.host)
                .map(|m| m.name.clone())
                .unwrap_or_else(|| "the host".into());
        }
        let index = (!queue.is_empty()).then(|| (fmt::i(s, "index").max(0) as usize).min(queue.len() - 1));
        Some(GroupState {
            id,
            code: fmt::s(s, "code").to_uppercase(),
            invite_url: fmt::s(s, "invite_url"),
            rev: fmt::i(s, "rev"),
            host_name,
            host_user: fmt::i(host, "user"),
            host_avatar: fmt::s(host, "avatar"),
            everyone_controls: everyone,
            visibility: if fmt::s(s, "visibility") == "link" { "link".into() } else { "anyone".into() },
            guests_allowed: fmt::truthy(s, "guests_allowed"),
            guests_possible: fmt::truthy(s, "guests_possible"),
            max_members: match fmt::i(s, "max_members") {
                n if n > 0 => n as u32,
                _ => server_max,
            },
            server_max,
            queue,
            index,
            playing: fmt::truthy(s, "playing"),
            position: fmt::f(s, "position").max(0.0),
            anchor_ms: fmt::i(s, "anchor_ms"),
            members,
            started_at: fmt::f(s, "started_at"),
            boot: fmt::s(s, "boot"),
            you_member: fmt::s(you, "member"),
            you_host,
            // an older answer without `you.can_control`: the host and "Everyone can control"
            you_can_control: match you.get("can_control") {
                Some(Value::Bool(b)) => *b,
                _ => you_host || everyone,
            },
        })
    }

    pub fn count(&self) -> usize {
        self.members.len()
    }

    pub fn link_only(&self) -> bool {
        self.visibility == "link"
    }

    /// The current entry.
    pub fn current(&self) -> Option<&Entry> {
        self.index.and_then(|i| self.queue.get(i))
    }
}

/// One live group listen in the server's list (`GET /api/group`): Home's "Listening together
/// now" and the Your devices panel.
#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    pub id: String,
    pub host_name: String,
    pub host_avatar: String,
    pub title: String,
    pub artist: String,
    pub album_id: String,
    pub cover_v: Value,
    pub playing: bool,
    pub members_count: usize,
    pub started_at: f64,
    pub visibility: String,
    /// This account is in it (on this or another device).
    pub mine: bool,
    /// `GET /api/group/code/{code}` only: a guest may join now.
    pub guests: bool,
}

impl Summary {
    pub fn from(v: &Value) -> Option<Summary> {
        let id = fmt::s(v, "id");
        if id.is_empty() {
            return None;
        }
        let host = &v["host"];
        let t = &v["track"];
        let host_name = fmt::s(host, "name");
        Some(Summary {
            id,
            host_name: if host_name.is_empty() { "Someone".into() } else { host_name },
            host_avatar: fmt::s(host, "avatar"),
            title: fmt::s(t, "title"),
            artist: fmt::s(t, "artist"),
            album_id: fmt::s(t, "album_id"),
            cover_v: t["cover_v"].clone(),
            playing: fmt::truthy(v, "playing"),
            members_count: fmt::i(v, "members_count").max(1) as usize,
            started_at: fmt::f(v, "started_at"),
            visibility: if fmt::s(v, "visibility") == "link" { "link".into() } else { "anyone".into() },
            mine: fmt::truthy(v, "mine"),
            guests: fmt::truthy(v, "guests"),
        })
    }

    /// The list of `GET /api/group`: a bare array, or `{groups: [...]}`.
    pub fn list(v: &Value) -> Vec<Summary> {
        let items = match v {
            Value::Array(a) => a.clone(),
            Value::Object(o) => o
                .get("groups")
                .or_else(|| o.get("items"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        items.iter().filter_map(Summary::from).collect()
    }
}

// ----- codes and links -------------------------------------------------------------------

/// The code in what was typed or pasted: a bare code ("k7qm 4x" too: spaces and dashes are left
/// out), or a link `https://<server>/#/group/<code>` (4–12 characters there).
pub fn code_from(text: &str) -> Option<String> {
    let t = text.trim();
    if let Some(at) = t.find("#/group/") {
        let code: String = t[at + 8..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        if (4..=12).contains(&code.len()) {
            return Some(code.to_uppercase());
        }
    }
    let bare: String = t.chars().filter(|c| *c != ' ' && *c != '-').collect();
    (bare.len() == 6 && bare.chars().all(|c| c.is_ascii_alphanumeric())).then(|| bare.to_uppercase())
}

/// The server part of a group link ("https://music.example.com", with a path prefix when the
/// server has one), or None when the text holds no link.
pub fn server_of(text: &str) -> Option<String> {
    let at = text.find("#/group/")?;
    let before = &text[..at];
    let start = before.rfind("https://").into_iter().chain(before.rfind("http://")).max()?;
    let url = &before[start..];
    if url.chars().any(char::is_whitespace) {
        return None;
    }
    let scheme_end = url.find("://")? + 3;
    let rest = &url[scheme_end..];
    let host_end = rest.find('/').unwrap_or(rest.len());
    let host = &rest[..host_end];
    if host.is_empty() || host.contains('#') || host.contains('?') {
        return None;
    }
    let path = rest[host_end..].trim_end_matches('/');
    Some(format!("{}{host}{path}", &url[..scheme_end]))
}

/// Whether a link's server (`server_of`) is the one this app is signed in to: the same host and
/// port.
pub fn same_server(link: &str, base: &str) -> bool {
    match (url::Url::parse(link), url::Url::parse(base)) {
        (Ok(a), Ok(b)) => a.host_str() == b.host_str() && a.port_or_known_default() == b.port_or_known_default(),
        _ => false,
    }
}

/// The host part of a link's server, for "That group listen is on another server (<host>)".
pub fn host_of(server: &str) -> String {
    server.split_once("://").map(|(_, r)| r.to_string()).unwrap_or_else(|| server.to_string())
}

/// "K7QM4X" → "K7Q M4X" (easier to read out).
pub fn spaced(code: &str) -> String {
    if code.len() == 6 && code.is_ascii() {
        format!("{} {}", &code[..3], &code[3..])
    } else {
        code.to_string()
    }
}

/// The invite link: the server's own when it sent one, else `<server>/#/group/<code>` as the web
/// builds it.
pub fn invite_link(s: &GroupState, base: &str) -> String {
    if s.invite_url.is_empty() {
        format!("{}/#/group/{}", base.trim_end_matches('/'), s.code)
    } else {
        s.invite_url.clone()
    }
}

// ----- where the group is ----------------------------------------------------------------

/// Where the group is now: the entry and the position in it (seconds), given the server's clock
/// (Unix ms). While playing the position runs on from the anchor (expected = position +
/// (server_now − anchor_ms) / 1000), and a song that would have ended rolls over into the next
/// one (the server moves on by itself at the end of a song); past the last one it stays at that
/// song's end. None when the queue is empty.
pub fn expected(s: &GroupState, server_now_ms: i64) -> Option<(usize, f64)> {
    let mut index = s.index?;
    let mut pos = s.position;
    if s.playing && s.anchor_ms > 0 {
        pos += (server_now_ms - s.anchor_ms).max(0) as f64 / 1000.0;
    }
    loop {
        let dur = s.queue[index].duration();
        if dur <= 0.0 || pos < dur {
            break;
        }
        if index + 1 >= s.queue.len() {
            pos = dur;
            break;
        }
        pos -= dur;
        index += 1;
    }
    Some((index, pos.max(0.0)))
}

// ----- words -----------------------------------------------------------------------------

/// "Only Alex controls playback": why a member's control is off.
pub fn only_host(s: &GroupState) -> String {
    format!("Only {} controls playback", s.host_name)
}

/// The pill on the player: "Group · 3".
pub fn pill(s: &GroupState) -> String {
    format!("Group · {}", s.count())
}

/// "Alex · 3 listening" on a tile of the server's list.
pub fn tile_line(g: &Summary) -> String {
    format!("{} · {} listening", g.host_name, g.members_count)
}

/// "Your group listen" / "Alex's group listen".
pub fn whose(s: &GroupState) -> String {
    if s.you_host {
        "Your group listen".into()
    } else {
        format!("{}'s group listen", s.host_name)
    }
}

/// The words of the people who came or went between two states ("Sam joined", "Sam left") and of
/// a new host; this device left out. Its membership before counts as this device too: after
/// being dropped (2 minutes without the event stream) the app joins again with the code and gets
/// a new membership id, which is not "<you> left".
pub fn comings(old: Option<&GroupState>, new: &GroupState) -> Vec<String> {
    let Some(old) = old.filter(|o| o.id == new.id) else {
        return Vec::new();
    };
    let me = [old.you_member.as_str(), new.you_member.as_str()];
    let mine = |k: &str| !k.is_empty() && me.contains(&k);
    let mut out = Vec::new();
    for m in &new.members {
        if !old.members.iter().any(|o| o.key == m.key) && !mine(&m.key) {
            out.push(format!("{} joined", m.name));
        }
    }
    for m in &old.members {
        if !new.members.iter().any(|n| n.key == m.key) && !mine(&m.key) {
            out.push(format!("{} left", m.name));
        }
    }
    if old.host_user != new.host_user && new.host_user != 0 && old.host_user != 0 {
        out.push(if new.you_host {
            "You host the group listen now".into()
        } else {
            format!("{} hosts the group listen now", new.host_name)
        });
    }
    out
}

/// Why this computer is out of its group when joining again after being dropped was refused
/// (§3.9, §3.17): the server's own sentence, else one by the status.
pub fn rejoin_refused(status: u16, detail: &str) -> String {
    let d = detail.trim();
    if !d.is_empty() && !d.starts_with("HTTP ") {
        return d.to_string();
    }
    match status {
        429 => "Too many tries. Wait a moment and try again.".into(),
        403 => "Group listens are turned off on this server".into(),
        _ => "Couldn't join".into(),
    }
}

/// What the end of a group listen is called, by the `ended` event's reason.
pub fn ended_text(reason: &str, host: &str) -> String {
    match reason {
        "removed" => "You were removed from the group listen".into(),
        "off" => "Group listens are turned off on this server".into(),
        "restart" => "The group listen ended (the server restarted)".into(),
        "ended" if !host.is_empty() => format!("{host} ended the group listen"),
        _ => "The group listen ended".into(),
    }
}

/// "Playing now: <title> – <artist>. " for "Join <host>'s group listen?" ("" without a song).
pub fn playing_now(g: Option<&Summary>) -> String {
    match g {
        Some(g) if !g.title.is_empty() && !g.artist.is_empty() => format!("Playing now: {} – {}. ", g.title, g.artist),
        Some(g) if !g.title.is_empty() => format!("Playing now: {}. ", g.title),
        _ => String::new(),
    }
}

/// The library songs of a queue (ids), at most `max` around the current one, and where the
/// current song is in them (the phone's `RemoteDevice.window`).
pub fn window(ids: &[Option<i64>], index: usize, max: usize) -> (Vec<i64>, usize) {
    let lib: Vec<i64> = ids.iter().flatten().copied().collect();
    if lib.is_empty() {
        return (lib, 0);
    }
    let idx = ids.iter().take(index).filter(|i| i.is_some()).count().min(lib.len() - 1);
    if lib.len() <= max {
        return (lib, idx);
    }
    let from = idx.saturating_sub(50).min(lib.len() - max);
    (lib[from..from + max].to_vec(), idx - from)
}

/// A list's Shuffle in a group: its library songs in a random order, at most `max` of them, so a
/// list longer than that sends `max` songs picked from all of it (as the phone does), not its
/// first ones.
pub fn pick_shuffled(ids: &[Option<i64>], max: usize, rng: &mut crate::player::queue::Rng) -> Vec<i64> {
    let mut lib: Vec<i64> = ids.iter().flatten().copied().collect();
    for i in (1..lib.len()).rev() {
        lib.swap(i, rng.below(i + 1));
    }
    lib.truncate(max);
    lib
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn state(v: Value) -> GroupState {
        GroupState::from(&v).unwrap()
    }

    fn two() -> Value {
        json!({"id": "g1", "code": "k7qm4x", "rev": 4,
         "host": {"user": 3, "name": "Alex", "avatar": "/api/users/3/avatar"},
         "queue": [
           {"id": 11, "title": "One", "artist": "A", "duration": 200, "qid": "q1", "added_by": {"user": 3, "name": "Alex"}},
           {"id": 12, "title": "Two", "artist": "B", "duration": 100, "qid": "q2", "added_by": {"user": 5, "name": "Sam"}}
         ],
         "index": 0, "playing": true, "position": 50.0, "anchor_ms": 1000000,
         "members": [
           {"member": "m3", "user": 3, "name": "Alex", "device_name": "Pixel", "host": true},
           {"member": "m5", "user": 5, "name": "Sam", "device_name": "Firefox"}
         ],
         "everyone_controls": false, "visibility": "anyone", "max_members": 20, "server_max_members": 20,
         "you": {"member": "m5", "host": false, "can_control": false}})
    }

    #[test]
    fn reads_the_state() {
        let s = state(two());
        assert_eq!(s.code, "K7QM4X");
        assert_eq!(s.host_name, "Alex");
        assert_eq!(s.queue.len(), 2);
        assert_eq!(s.queue[1].added_by, "Sam");
        assert_eq!(s.queue[1].qid, "q2");
        assert_eq!(s.current().map(|e| e.id), Some(11));
        assert_eq!(s.you_member, "m5");
        assert!(!s.you_can_control);
        assert_eq!(pill(&s), "Group · 2");
        assert_eq!(only_host(&s), "Only Alex controls playback");
        assert_eq!(whose(&s), "Alex's group listen");
    }

    #[test]
    fn an_older_answer_without_you_lets_the_host_and_everyone_control() {
        let s = state(json!({"id": "g", "everyone_controls": true, "queue": []}));
        assert!(s.you_can_control);
        assert_eq!(s.index, None);
        assert!(s.current().is_none());
        assert_eq!(s.max_members, 20);
        assert!(GroupState::from(&json!({"code": "X"})).is_none());
        // the event stream may wrap it
        assert_eq!(GroupState::from(&json!({"state": {"id": "g2"}})).unwrap().id, "g2");
    }

    #[test]
    fn codes_and_links() {
        assert_eq!(code_from("k7qm4x").as_deref(), Some("K7QM4X"));
        assert_eq!(code_from(" K7Q M4X ").as_deref(), Some("K7QM4X"));
        assert_eq!(code_from("k7q-m4x").as_deref(), Some("K7QM4X"));
        assert_eq!(code_from("https://music.example.com/#/group/K7QM4X").as_deref(), Some("K7QM4X"));
        assert_eq!(
            code_from("Listen with me on Encore (code K7QM4X):\nhttps://music.example.com/#/group/k7qm4x").as_deref(),
            Some("K7QM4X")
        );
        assert_eq!(code_from("hello there"), None);
        assert_eq!(code_from("ab"), None);
        assert_eq!(server_of("Listen: https://music.example.com/#/group/K7QM4X").as_deref(), Some("https://music.example.com"));
        assert_eq!(server_of("http://10.0.0.2:8080/encore/#/group/ABCDEF").as_deref(), Some("http://10.0.0.2:8080/encore"));
        assert_eq!(server_of("K7QM4X"), None);
        assert_eq!(spaced("K7QM4X"), "K7Q M4X");
        assert!(same_server("http://nas:8080", "http://nas:8080/"));
        assert!(same_server("https://m.example.com", "https://m.example.com:443"));
        assert!(!same_server("http://other:8080", "http://nas:8080"));
        assert!(!same_server("http://nas:9090", "http://nas:8080"));
        assert_eq!(host_of("https://music.example.com"), "music.example.com");
    }

    #[test]
    fn the_invite_link_is_the_servers_or_built_like_the_web() {
        let s = state(two());
        assert_eq!(invite_link(&s, "https://m.example/"), "https://m.example/#/group/K7QM4X");
        let with_url = state(json!({"id": "g", "code": "ABCDEF", "invite_url": "https://public.example/#/group/ABCDEF"}));
        assert_eq!(invite_link(&with_url, "http://lan:8080"), "https://public.example/#/group/ABCDEF");
    }

    #[test]
    fn the_expected_position_runs_on_from_the_anchor_and_rolls_over() {
        let s = state(two());
        // 10 s after the anchor: 50 + 10
        assert_eq!(expected(&s, 1_010_000), Some((0, 60.0)));
        // a clock behind the anchor never goes back
        assert_eq!(expected(&s, 999_000), Some((0, 50.0)));
        // 160 s after: 50 + 160 = 210 → 10 s into the second song
        assert_eq!(expected(&s, 1_160_000), Some((1, 10.0)));
        // past the end of the queue: the end of the last song
        assert_eq!(expected(&s, 2_000_000), Some((1, 100.0)));
        let paused = GroupState { playing: false, ..s.clone() };
        assert_eq!(expected(&paused, 5_000_000), Some((0, 50.0)));
        let empty = GroupState { queue: vec![], index: None, ..s };
        assert_eq!(expected(&empty, 0), None);
    }

    #[test]
    fn joins_and_leaves_are_said() {
        let before = state(two());
        let mut after = before.clone();
        after.members.retain(|m| m.key != "m3");
        after.members.push(Member {
            key: "m9".into(),
            user: 9,
            name: "Kim".into(),
            avatar: String::new(),
            device_name: "Phone".into(),
            joined_at: 0.0,
            host: false,
            guest: false,
        });
        after.rev = 5;
        let said = comings(Some(&before), &after);
        assert!(said.contains(&"Kim joined".to_string()));
        assert!(said.contains(&"Alex left".to_string()));
        // nothing to compare at the start, or with another group
        assert!(comings(None, &after).is_empty());
        let other = GroupState { id: "g9".into(), ..after.clone() };
        assert!(comings(Some(&before), &other).is_empty());
        // this device is never announced, also under its new membership after a silent rejoin
        let mut again = after.clone();
        again.members.retain(|m| m.key != "m5");
        again.members.push(Member { key: "m5b".into(), name: "Sam".into(), ..after.members[0].clone() });
        again.you_member = "m5b".into();
        assert!(!comings(Some(&after), &again).iter().any(|w| w.starts_with("Sam")));
        // a new host
        let mut handed = after.clone();
        handed.host_user = 5;
        handed.host_name = "Sam".into();
        handed.you_host = true;
        let mut was = after.clone();
        was.host_user = 3;
        assert_eq!(comings(Some(&was), &handed), vec!["You host the group listen now".to_string()]);
        handed.you_host = false;
        assert_eq!(comings(Some(&was), &handed), vec!["Sam hosts the group listen now".to_string()]);
    }

    /// The first `state` event the 0.31 server sent in G's smoke run of the real binary (two
    /// devices of one account, three songs, rows straight from SQLite with nulls).
    const FROM_THE_SERVER: &str = r#"{"anchor_ms":1792503497853,"boot":"0e9336","code":"J68ANU","everyone_controls":false,"guests_allowed":false,"guests_possible":false,"host":{"avatar":"","name":"admin","user":1},"id":"g_b442cdbc96998cf60bc8","index":0,"invite_url":"http://localhost:18931/#/group/J68ANU","max_members":20,"members":[{"avatar":"","device_name":"Smoke PC","guest":false,"host":true,"joined_at":1792503497,"member":"m1","name":"admin","user":1},{"avatar":"","device_name":"Smoke phone","guest":false,"host":false,"joined_at":1792503497,"member":"m2","name":"admin","user":1}],"playing":true,"position":0.0,"queue":[{"added_by":{"guest":false,"name":"admin","user":1},"album":"Record","album_id":"al1","albumartist":null,"artist":"Band","artists":[{"in_library":false,"name":"Band"}],"bitrate":null,"disc":null,"duration":3.0,"format":"mp3","id":1,"qid":"q1","title":"Song 1","track":null,"year":null},{"added_by":{"guest":false,"name":"admin","user":1},"album":"Record","album_id":"al1","albumartist":null,"artist":"Band","artists":[{"in_library":false,"name":"Band"}],"bitrate":null,"disc":null,"duration":3.0,"format":"mp3","id":2,"qid":"q2","title":"Song 2","track":null,"year":null},{"added_by":{"guest":false,"name":"admin","user":1},"album":"Record","album_id":"al1","albumartist":null,"artist":"Band","artists":[{"in_library":false,"name":"Band"}],"bitrate":null,"disc":null,"duration":100.0,"format":"mp3","id":3,"qid":"q3","title":"Song 3","track":null,"year":null}],"rev":2,"server_max_members":20,"started_at":1792503497,"track":{"added_by":{"guest":false,"name":"admin","user":1},"album":"Record","album_id":"al1","albumartist":null,"artist":"Band","artists":[{"in_library":false,"name":"Band"}],"bitrate":null,"disc":null,"duration":3.0,"format":"mp3","id":1,"qid":"q1","title":"Song 1","track":null,"year":null},"visibility":"anyone","you":{"can_control":true,"host":true,"member":"m2"}}"#;

    #[test]
    fn reads_the_servers_own_state() {
        let s = state(serde_json::from_str(FROM_THE_SERVER).unwrap());
        assert_eq!(s.id, "g_b442cdbc96998cf60bc8");
        assert_eq!((s.code.as_str(), s.rev, s.boot.as_str()), ("J68ANU", 2, "0e9336"));
        assert_eq!(invite_link(&s, "https://elsewhere.example"), "http://localhost:18931/#/group/J68ANU");
        assert_eq!((s.host_name.as_str(), s.host_user), ("admin", 1));
        assert_eq!(s.queue.iter().map(|e| e.qid.as_str()).collect::<Vec<_>>(), ["q1", "q2", "q3"]);
        assert_eq!((s.queue[1].added_by.as_str(), s.queue[1].added_by_user), ("admin", 1));
        assert_eq!((s.queue[0].title(), s.queue[0].artist(), s.queue[0].duration()), ("Song 1".to_string(), "Band".to_string(), 3.0));
        // one account on two devices: two members; `you` is this device's membership
        assert_eq!(s.count(), 2);
        assert_eq!(s.you_member, "m2");
        assert!(s.you_host && s.you_can_control);
        assert_eq!((s.max_members, s.server_max), (20, 20));
        assert!(!s.guests_possible && !s.link_only());
        // where the server's next two events put the group (3 s and 6 s later, index 1 and 2)
        assert_eq!(expected(&s, 1_792_503_500_853), Some((1, 0.0)));
        assert_eq!(expected(&s, 1_792_503_503_853), Some((2, 0.0)));
        assert_eq!(expected(&s, 1_792_503_501_353), Some((1, 0.5)));
        // the queue rows are what the engine's entries are built from
        let t = crate::engine::tracks::from_encore("http://e", &crate::engine::tracks::bearer("x"), &s.queue[2].row).unwrap();
        assert_eq!(crate::engine::tracks::encore_id(&t), Some(3));
        assert_eq!(t.title, "Song 3");
    }

    #[test]
    fn refusals_and_endings_say_why() {
        assert_eq!(rejoin_refused(409, "This group listen is full"), "This group listen is full");
        assert_eq!(rejoin_refused(409, "HTTP 409"), "Couldn't join");
        assert_eq!(rejoin_refused(429, ""), "Too many tries. Wait a moment and try again.");
        assert_eq!(rejoin_refused(403, " "), "Group listens are turned off on this server");
        assert_eq!(rejoin_refused(400, " "), "Couldn't join");
        assert_eq!(ended_text("restart", "Alex"), "The group listen ended (the server restarted)");
        assert_eq!(ended_text("ended", "Alex"), "Alex ended the group listen");
        assert_eq!(ended_text("ended", ""), "The group listen ended");
        assert_eq!(ended_text("idle", "Alex"), "The group listen ended");
        assert_eq!(ended_text("removed", "Alex"), "You were removed from the group listen");
        assert_eq!(ended_text("off", ""), "Group listens are turned off on this server");
    }

    #[test]
    fn the_list_and_its_words() {
        let list = Summary::list(&json!([
            {"id": "g1", "host": {"name": "Alex", "avatar": "/api/users/3/avatar"}, "track": {"title": "One", "artist": "A", "album_id": "al", "cover_v": 7}, "playing": true, "members_count": 3, "visibility": "anyone", "mine": false},
            {"host": {"name": "nobody"}}
        ]));
        assert_eq!(list.len(), 1);
        assert_eq!(tile_line(&list[0]), "Alex · 3 listening");
        assert_eq!(playing_now(Some(&list[0])), "Playing now: One – A. ");
        assert_eq!(Summary::list(&json!({"groups": [{"id": "g2"}]}))[0].host_name, "Someone");
        assert_eq!(playing_now(None), "");
        let bare = Summary::from(&json!({"id": "g3", "track": {"title": "Solo"}})).unwrap();
        assert_eq!(playing_now(Some(&bare)), "Playing now: Solo. ");
    }

    #[test]
    fn the_window_keeps_the_library_songs_around_the_current_one() {
        let ids = [Some(1), None, Some(2), Some(3)];
        assert_eq!(window(&ids, 2, 1000), (vec![1, 2, 3], 1));
        assert_eq!(window(&[None, None], 1, 1000), (vec![], 0));
        let many: Vec<Option<i64>> = (0..1200).map(Some).collect();
        let (w, at) = window(&many, 1100, 1000);
        assert_eq!(w.len(), 1000);
        assert_eq!(w[at], 1100);
        assert_eq!(w[0], 200);
    }

    #[test]
    fn a_shuffle_of_a_long_list_picks_from_all_of_it() {
        let mut rng = crate::player::queue::Rng::seeded(7);
        let many: Vec<Option<i64>> = (0..3000).map(Some).collect();
        let got = pick_shuffled(&many, 1000, &mut rng);
        assert_eq!(got.len(), 1000);
        let mut seen = got.clone();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 1000, "no song twice");
        assert!(got.iter().filter(|i| **i >= 1000).count() > 500, "songs past the first 1000 come too");
        // a short list: every library song once (files left out), in another order
        let short: Vec<Option<i64>> = (0..20).map(|i| (i != 3).then_some(i)).collect();
        let mixed = pick_shuffled(&short, 1000, &mut rng);
        let mut back = mixed.clone();
        back.sort_unstable();
        assert_eq!(back, short.iter().flatten().copied().collect::<Vec<_>>());
        assert_ne!(mixed, back);
        assert!(pick_shuffled(&[None], 1000, &mut rng).is_empty());
    }
}
