//! The rules of a group listen that need no browser: reading codes and links, where the group is now, what to do
//! about drift, the clock offset, the seek lag and the words. The same rules as the phone's `GroupRules` and
//! `SeekLag` (clients/android …/features/group), tested here with `cargo test`.

use crate::json::J;
use serde_json::Value;

/// The code in what was typed or pasted: a bare code ("k7qm 4x" too), or a link `https://<server>/#/group/<code>`.
pub fn code_from(text: &str) -> Option<String> {
    let t = text.trim();
    if let Some(i) = t.find("#/group/") {
        let code: String = t[i + "#/group/".len()..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        return (4..=12).contains(&code.len()).then(|| code.to_ascii_uppercase());
    }
    let bare: String = t.chars().filter(|c| *c != ' ' && *c != '-').collect();
    (bare.len() == 6 && bare.chars().all(|c| c.is_ascii_alphanumeric())).then(|| bare.to_ascii_uppercase())
}

/// The server part of a group link ("https://music.example.com"), or None when the text is no link.
pub fn server_of(text: &str) -> Option<String> {
    let t = text.trim();
    let at = t.find("#/group/")?;
    let head = &t[..at];
    let start = head.find("https://").or_else(|| head.find("http://"))?;
    let url = &head[start..];
    let scheme_end = url.find("://")? + 3;
    let rest = &url[scheme_end..];
    let host_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let host = &rest[..host_end];
    if host.is_empty() || host.contains(char::is_whitespace) { return None; }
    let path = rest[host_end..].split('?').next().unwrap_or("").trim_end_matches('/');
    Some(format!("{}{host}{path}", &url[..scheme_end]))
}

/// The host (and port) of a link's server part: "music.example.com:8443".
pub fn host_of(server: &str) -> String {
    let rest = server.split_once("://").map(|(_, r)| r).unwrap_or(server);
    rest.split('/').next().unwrap_or("").to_string()
}

/// "K7QM4X" → "K7Q M4X" (easier to read out).
pub fn spaced(code: &str) -> String {
    if code.chars().count() == 6 && code.is_ascii() { format!("{} {}", &code[..3], &code[3..]) } else { code.to_string() }
}

/// The invite link: the server's own when it sent one, else `<origin>/#/group/<code>`.
pub fn invite_link(state: &Value, origin: &str) -> String {
    let own = state.s("invite_url");
    if !own.is_empty() { return own; }
    format!("{}/#/group/{}", origin.trim_end_matches('/'), state.s("code"))
}

/// Where the group is now: the entry's index and the position in it (seconds), given the server's clock (Unix ms).
/// While playing the position runs on from the anchor (position + (server_now − anchor_ms) / 1000), and a song that
/// would have ended rolls over into the next one (the server moves on by itself at the end of a song); the end of
/// the last song stays there. (-1, 0) for an empty queue.
pub fn expected(state: &Value, server_now_ms: f64) -> (i64, f64) {
    let queue = state.a("queue");
    if queue.is_empty() { return (-1, 0.0); }
    let mut index = (state.i("index").max(0) as usize).min(queue.len() - 1);
    let mut pos = state.f("position").max(0.0);
    let anchor = state.f("anchor_ms");
    if state.truthy("playing") && anchor > 0.0 { pos += ((server_now_ms - anchor).max(0.0)) / 1000.0; }
    loop {
        let dur = queue[index].f("duration");
        if dur <= 0.0 || pos < dur { break; }
        if index + 1 >= queue.len() { pos = dur; break; }
        pos -= dur;
        index += 1;
    }
    (index as i64, pos.max(0.0))
}

/// The entry `expected` names, when the group has reached the end of its last song.
pub fn at_end(state: &Value, index: i64, pos: f64) -> bool {
    let q = state.a("queue");
    let Some(e) = q.get(index.max(0) as usize) else { return true };
    index as usize + 1 >= q.len() && e.f("duration") > 0.0 && pos >= e.f("duration") - 0.3
}

/// What to do about the distance between this device and the group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drift { None, Seek, Faster, Slower, Normal }

/// The sync rule (the same on web, desktop and phone): above 0.5 s → seek; between 0.15 and 0.5 s → play at 0.97
/// (ahead) or 1.03 (behind) until within 0.05 s; `rate_ok` false (the browser would change the pitch) → seek only.
/// `drift_ms` = this device's position − the group's (positive: ahead); `adjusting`: the rate is off 1.0 now;
/// `seek_above_ms`: 3 s instead of 0.5 s in a song where seeks kept missing ([`SeekLag`]).
pub fn drift(drift_ms: f64, adjusting: bool, rate_ok: bool, seek_above_ms: f64) -> Drift {
    let d = drift_ms.abs();
    if d > seek_above_ms { return Drift::Seek; }
    if !rate_ok { return if adjusting { Drift::Normal } else { Drift::None }; }
    if d > 150.0 || (adjusting && d > 50.0) { return if drift_ms > 0.0 { Drift::Slower } else { Drift::Faster }; }
    if adjusting { Drift::Normal } else { Drift::None }
}

/// The clock offset (server − this device, ms) from round trips `(sent, server now, received)`, local times in ms:
/// the one with the smallest round trip wins, the server's time taken as its middle.
pub fn clock_offset(samples: &[(f64, f64, f64)]) -> Option<f64> {
    let best = samples.iter().filter(|s| s.2 >= s.0 && s.1 > 0.0).min_by(|a, b| (a.2 - a.0).total_cmp(&(b.2 - b.0)))?;
    Some(best.1 - (best.0 + best.2) / 2.0)
}

/// The host's name ("the host" when the state has none).
pub fn host_name(state: &Value) -> String {
    let n = state.g("host").s("name");
    if !n.is_empty() { return n; }
    state.a("members").iter().find(|m| m.truthy("host")).map(|m| m.s("name")).filter(|n| !n.is_empty()).unwrap_or_else(|| "the host".into())
}

/// "Only Alex controls playback": why a member's control is off.
pub fn only_host(state: &Value) -> String { format!("Only {} controls playback", host_name(state)) }

/// The pill on the player: "Group · 3".
pub fn pill(state: &Value) -> String { format!("Group · {}", state.a("members").len().max(1)) }

/// "Alex · 3 listening" on a tile of the server's list.
pub fn tile_line(g: &Value) -> String {
    let host = g.g("host").s("name");
    format!("{} · {} listening", if host.is_empty() { "Someone".to_string() } else { host }, g.i("members_count").max(1))
}

/// "Playing now: <title> – <artist>." ("Playing now: <title>." without an artist), "" without a song.
pub fn playing_now(g: &Value) -> String {
    let t = g.g("track");
    let (title, artist) = (t.s("title"), t.s("artist"));
    if title.is_empty() { return String::new(); }
    if artist.is_empty() { format!("Playing now: {title}.") } else { format!("Playing now: {title} – {artist}.") }
}

/// "Join <host>'s group listen?" ("Join this group listen?" when the host is not known).
pub fn join_title(g: Option<&Value>) -> String {
    match g.map(|g| g.g("host").s("name")).filter(|n| !n.is_empty()) {
        Some(n) => format!("Join {n}'s group listen?"),
        None => "Join this group listen?".to_string(),
    }
}

/// The toasts for people who came or went between two states ("Sam joined", "Sam left", a new host); this device
/// left out – also its membership before (a device that was dropped joins again with a new membership id).
pub fn comings(old: &Value, new: &Value) -> Vec<String> {
    if old.s("id") != new.s("id") { return vec![]; }
    let me: Vec<String> = [old.g("you").s("member"), new.g("you").s("member")].into_iter().filter(|m| !m.is_empty()).collect();
    let keys = |s: &Value| -> Vec<(String, String)> { s.a("members").iter().map(|m| (m.s("member"), m.s("name"))).collect() };
    let (before, after) = (keys(old), keys(new));
    let mut out = vec![];
    for (k, n) in &after { if !before.iter().any(|(b, _)| b == k) && !me.contains(k) { out.push(format!("{n} joined")); } }
    for (k, n) in &before { if !after.iter().any(|(a, _)| a == k) && !me.contains(k) { out.push(format!("{n} left")); } }
    let (oh, nh) = (old.g("host").i("user"), new.g("host").i("user"));
    if oh != nh && oh != 0 && nh != 0 {
        out.push(if new.g("you").truthy("host") { "You host the group listen now".to_string() } else { format!("{} hosts the group listen now", host_name(new)) });
    }
    out
}

/// What the end of a group listen is called, by the `ended` event's reason ("restart": the server restarted).
pub fn ended_text(reason: &str, host: &str, you_host: bool) -> String {
    match reason {
        "removed" => "You were removed from the group listen".into(),
        "off" => "Group listens are turned off on this server".into(),
        "restart" => "The group listen ended (the server restarted)".into(),
        "ended" if you_host => "You ended the group listen".into(),
        "ended" if !host.is_empty() => format!("{host} ended the group listen"),
        _ => "The group listen ended".into(),
    }
}

/// Why this device is out of its group when joining again after being dropped was refused (client-shapes §3.9,
/// §3.17): the server's own sentence, else one by the status.
pub fn rejoin_refused(status: u16, detail: &str) -> String {
    let d = detail.trim();
    if !d.is_empty() && !d.starts_with("HTTP ") { return d.to_string(); }
    match status {
        429 => "Too many tries. Wait a moment and try again.".into(),
        403 => "Group listens are turned off on this server".into(),
        _ => "Couldn't join".into(),
    }
}

/// At most `max` song ids around `index` (the songs before it count less: 50 of them at most when the list is
/// longer), and where `index` is in the window.
pub fn window(ids: &[i64], index: usize, max: usize) -> (Vec<i64>, usize) {
    if ids.len() <= max { return (ids.to_vec(), index.min(ids.len().saturating_sub(1))); }
    let from = index.saturating_sub(50).min(ids.len() - max);
    (ids[from..from + max].to_vec(), index - from)
}

/// What a seek made to catch up with the group costs on this device (the phone's `SeekLag`): a seek starts playing
/// some time after it was asked (a whole file buffers at the new spot, a server-made stream is asked for again at
/// `t=`), so it is aimed ahead by what the last ones took – learnt from where each one landed, for whole files and
/// server-made streams apart, at most 3 s. When two seeks within 15 s still land more than 0.5 s off, that song only
/// seeks for a drift above 3 s and the rate nudge closes the rest.
#[derive(Default)]
pub struct SeekLag {
    lag: [f64; 2],
    wait: Option<(usize, String)>,
    miss_at: Option<f64>,
    off_for: Option<String>,
}

impl SeekLag {
    pub const SEEK_MS: f64 = 500.0;
    pub const BACKED_OFF_MS: f64 = 3000.0;
    pub const MAX_LAG_MS: f64 = 3000.0;
    pub const MISS_WINDOW_MS: f64 = 15000.0;

    /// A seek's result is awaited: the next look while playing calls [`SeekLag::landed`].
    pub fn waiting(&self) -> bool { self.wait.is_some() }
    /// Where to seek in `song` (its queue entry id) for the group's `expected_ms`: ahead by the lag of that kind.
    pub fn target(&mut self, song: &str, expected_ms: f64, server_made: bool) -> f64 {
        let k = usize::from(server_made);
        self.wait = Some((k, song.to_string()));
        expected_ms + self.lag[k]
    }
    /// The first look while playing after a seek: `drift_ms` = this device − the group, in `song`, at `now_ms`.
    pub fn landed(&mut self, song: &str, drift_ms: f64, now_ms: f64) {
        let Some((k, waited)) = self.wait.take() else { return };
        if waited != song { return; }
        self.lag[k] = (self.lag[k] - drift_ms).clamp(0.0, Self::MAX_LAG_MS);
        if drift_ms.abs() > Self::SEEK_MS {
            if self.miss_at.is_some_and(|at| now_ms - at <= Self::MISS_WINDOW_MS) { self.off_for = Some(song.to_string()); }
            self.miss_at = Some(now_ms);
        }
    }
    /// The seek awaited is not one to learn from (a pause, the listener's own seek, leaving the group).
    pub fn cancel(&mut self) { self.wait = None; }
    /// Above this drift (ms) a seek is made.
    pub fn seek_above_ms(&self, song: &str) -> f64 { if self.off_for.as_deref() == Some(song) { Self::BACKED_OFF_MS } else { Self::SEEK_MS } }
    pub fn lag_ms(&self, server_made: bool) -> f64 { self.lag[usize::from(server_made)] }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn codes_and_links() {
        assert_eq!(code_from("k7qm 4x").as_deref(), Some("K7QM4X"));
        assert_eq!(code_from(" K7Q-M4X ").as_deref(), Some("K7QM4X"));
        assert_eq!(code_from("https://music.example.com/#/group/k7qm4x").as_deref(), Some("K7QM4X"));
        assert_eq!(code_from("Listen with me on Encore (code K7QM4X):\nhttps://m.example.com:8443/encore/#/group/K7QM4X").as_deref(), Some("K7QM4X"));
        assert_eq!(code_from("hello"), None);
        assert_eq!(code_from("K7QM4"), None);
        assert_eq!(code_from("#/group/ab"), None);
        assert_eq!(server_of("https://music.example.com/#/group/K7QM4X").as_deref(), Some("https://music.example.com"));
        assert_eq!(server_of("see http://10.0.0.2:4533/encore/#/group/K7QM4X").as_deref(), Some("http://10.0.0.2:4533/encore"));
        assert_eq!(server_of("K7QM4X"), None);
        assert_eq!(host_of("http://10.0.0.2:4533/encore"), "10.0.0.2:4533");
        assert_eq!(spaced("K7QM4X"), "K7Q M4X");
        assert_eq!(invite_link(&json!({"code": "K7QM4X", "invite_url": ""}), "https://a.b/"), "https://a.b/#/group/K7QM4X");
        assert_eq!(invite_link(&json!({"code": "K7QM4X", "invite_url": "https://x.y/#/group/K7QM4X"}), "https://a.b"), "https://x.y/#/group/K7QM4X");
    }

    #[test]
    fn where_the_group_is_now() {
        let s = json!({"queue": [{"duration": 100.0}, {"duration": 50.0}], "index": 0, "position": 90.0, "anchor_ms": 1_000_000.0, "playing": true});
        assert_eq!(expected(&s, 1_000_000.0), (0, 90.0));
        assert_eq!(expected(&s, 1_005_000.0), (0, 95.0));
        // rolls over into the next song
        let (i, p) = expected(&s, 1_015_000.0);
        assert_eq!(i, 1);
        assert!((p - 5.0).abs() < 1e-9);
        // the end of the last song stays there
        assert_eq!(expected(&s, 1_200_000.0), (1, 50.0));
        assert!(at_end(&s, 1, 50.0));
        // paused: the position is the position
        let mut p2 = s.clone();
        p2["playing"] = json!(false);
        assert_eq!(expected(&p2, 1_200_000.0), (0, 90.0));
        assert_eq!(expected(&json!({"queue": []}), 0.0), (-1, 0.0));
        // an anchor ahead of this device's idea of the server clock never goes backwards
        assert_eq!(expected(&s, 999_000.0), (0, 90.0));
    }

    #[test]
    fn the_sync_rule() {
        assert_eq!(drift(600.0, false, true, 500.0), Drift::Seek);
        assert_eq!(drift(-600.0, false, false, 500.0), Drift::Seek);
        assert_eq!(drift(300.0, false, true, 500.0), Drift::Slower);
        assert_eq!(drift(-300.0, false, true, 500.0), Drift::Faster);
        assert_eq!(drift(100.0, false, true, 500.0), Drift::None);
        assert_eq!(drift(100.0, true, true, 500.0), Drift::Slower);
        assert_eq!(drift(40.0, true, true, 500.0), Drift::Normal);
        // a browser that would change the pitch: seek only
        assert_eq!(drift(300.0, false, false, 500.0), Drift::None);
        assert_eq!(drift(300.0, true, false, 500.0), Drift::Normal);
        // a song where seeks kept missing: only above 3 s
        assert_eq!(drift(2000.0, false, true, 3000.0), Drift::Slower);
    }

    #[test]
    fn clock_offset_takes_the_shortest_round_trip() {
        assert_eq!(clock_offset(&[(0.0, 1000.0, 100.0), (200.0, 1210.0, 220.0), (300.0, 1400.0, 500.0)]), Some(1000.0));
        assert_eq!(clock_offset(&[]), None);
        assert_eq!(clock_offset(&[(10.0, 0.0, 20.0)]), None);
    }

    #[test]
    fn words() {
        let s = json!({"host": {"user": 3, "name": "Alex"}, "members": [{"member": "a"}, {"member": "b"}, {"member": "c"}]});
        assert_eq!(only_host(&s), "Only Alex controls playback");
        assert_eq!(pill(&s), "Group · 3");
        assert_eq!(tile_line(&json!({"host": {"name": "Alex"}, "members_count": 3})), "Alex · 3 listening");
        assert_eq!(playing_now(&json!({"track": {"title": "Crazy", "artist": "Gnarls Barkley"}})), "Playing now: Crazy – Gnarls Barkley.");
        assert_eq!(playing_now(&json!({"track": {"title": "Crazy"}})), "Playing now: Crazy.");
        assert_eq!(playing_now(&json!({"track": null})), "");
        assert_eq!(join_title(Some(&json!({"host": {"name": "Alex"}}))), "Join Alex's group listen?");
        assert_eq!(join_title(None), "Join this group listen?");
        assert_eq!(ended_text("ended", "Alex", false), "Alex ended the group listen");
        assert_eq!(ended_text("ended", "Alex", true), "You ended the group listen");
        assert_eq!(ended_text("removed", "", false), "You were removed from the group listen");
        assert_eq!(ended_text("restart", "", false), "The group listen ended (the server restarted)");
        assert_eq!(ended_text("idle", "Alex", false), "The group listen ended");
        assert_eq!(rejoin_refused(409, "This group listen is full"), "This group listen is full");
        assert_eq!(rejoin_refused(429, ""), "Too many tries. Wait a moment and try again.");
    }

    #[test]
    fn comings_and_goings() {
        let old = json!({"id": "g", "host": {"user": 3, "name": "Alex"}, "you": {"member": "m1", "host": false},
            "members": [{"member": "m0", "name": "Alex"}, {"member": "m1", "name": "Me"}, {"member": "m2", "name": "Sam"}]});
        let new = json!({"id": "g", "host": {"user": 5, "name": "Kim"}, "you": {"member": "m9", "host": false},
            "members": [{"member": "m3", "name": "Kim"}, {"member": "m9", "name": "Me"}, {"member": "m2", "name": "Sam"}]});
        assert_eq!(comings(&old, &new), vec!["Kim joined", "Alex left", "Kim hosts the group listen now"]);
        assert!(comings(&old, &json!({"id": "other"})).is_empty());
    }

    #[test]
    fn a_window_of_ids() {
        let ids: Vec<i64> = (0..2000).collect();
        let (w, at) = window(&ids, 500, 1000);
        assert_eq!(w.len(), 1000);
        assert_eq!(w[at], 500);
        assert_eq!(at, 50);
        // near the end the window ends with the list
        let (w, at) = window(&ids, 1900, 1000);
        assert_eq!((w[0], w[at], w[999]), (1000, 1900, 1999));
        let (w, at) = window(&ids, 10, 1000);
        assert_eq!((w[0], at), (0, 10));
        assert_eq!(window(&[1, 2, 3], 1, 1000), (vec![1, 2, 3], 1));
        assert_eq!(window(&[], 0, 1000), (vec![], 0));
    }

    #[test]
    fn the_seek_lag_is_learnt() {
        let mut l = SeekLag::default();
        assert_eq!(l.target("q1", 10_000.0, true), 10_000.0);
        // landed 800 ms behind: the next seek aims 800 ms ahead
        l.landed("q1", -800.0, 1000.0);
        assert_eq!(l.lag_ms(true), 800.0);
        assert_eq!(l.lag_ms(false), 0.0);
        assert_eq!(l.target("q1", 20_000.0, true), 20_800.0);
        // two misses within 15 s: that song seeks only above 3 s
        l.landed("q1", 900.0, 5000.0);
        assert_eq!(l.seek_above_ms("q1"), 3000.0);
        assert_eq!(l.seek_above_ms("q2"), 500.0);
        assert!(!l.waiting());
    }
}
