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

// ---- the sync rule (client-shapes §5.6: the same on web, phone and desktop) -----------------------------------------

/// Below this drift (ms) nothing is done …
pub const DEAD_MS: f64 = 40.0;
/// … a nudge goes on until the drift is below this …
pub const SETTLED_MS: f64 = 20.0;
/// … and above this one hard seek is made (never for less: the nudge closes it) …
pub const SEEK_MS: f64 = 1500.0;
/// … above this for a server-made stream (anything but the original file served with Range): its seek is a new
/// stream, which the server has to start again …
pub const SERVER_MADE_SEEK_MS: f64 = 3000.0;
/// … and after a hard seek none again for this long (ms): nudges only.
pub const SEEK_GAP_MS: f64 = 8000.0;
/// The most the playback rate moves off 1.0 (0.98–1.02).
pub const MAX_NUDGE: f64 = 0.02;
/// The rate's distance from 1.0 per second of drift: proportional, 0.2 s off is the whole 2 %.
pub const NUDGE_PER_S: f64 = 0.1;
/// The most the rate changes from one look (every 500 ms) to the next: eased in and out, never a jump.
pub const EASE_STEP: f64 = 0.004;

/// What to do about the distance between this device and the group.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Act {
    /// leave it as it is
    Keep,
    /// play at this rate (1.0: the nudge is over)
    Rate(f64),
    /// one hard seek to where the group is (aimed ahead by the seek lag)
    Seek,
}

/// The sync rule. `drift_ms` = this device's position − the group's (positive: ahead); `rate`: the playback rate
/// now; `since_seek_ms`: how long ago this device's last hard seek (or song load) was; `server_made`: the stream is
/// made by the server (a conversion, a lighter rung, server processing), whose seek starts a new stream.
/// Under 40 ms nothing; from 40 ms to the seek threshold a rate nudge proportional to the drift (at most ±2 %,
/// eased in and out, kept until under 20 ms); above 1.5 s (3 s for a server-made stream) one hard seek, then none
/// for 8 s (nudges only).
pub fn sync_step(drift_ms: f64, rate: f64, since_seek_ms: f64, server_made: bool) -> Act {
    let d = drift_ms.abs();
    let seek_above = if server_made { SERVER_MADE_SEEK_MS } else { SEEK_MS };
    if d > seek_above && since_seek_ms >= SEEK_GAP_MS { return Act::Seek; }
    let nudging = (rate - 1.0).abs() > 1e-9;
    let target = if d < SETTLED_MS || (!nudging && d < DEAD_MS) { 1.0 } else { 1.0 - (drift_ms / 1000.0 * NUDGE_PER_S).clamp(-MAX_NUDGE, MAX_NUDGE) };
    let next = ease(rate, target);
    if (next - rate).abs() < 1e-9 { Act::Keep } else { Act::Rate(next) }
}

/// One look's step of the rate towards `target`, at most [`EASE_STEP`]; within the ±2 % and on a round value.
pub fn ease(rate: f64, target: f64) -> f64 {
    let r = rate + (target - rate).clamp(-EASE_STEP, EASE_STEP);
    let r = r.clamp(1.0 - MAX_NUDGE, 1.0 + MAX_NUDGE);
    (r * 10_000.0).round() / 10_000.0
}

/// The median of `v` (the middle two averaged); None when empty.
pub fn median(v: &[f64]) -> Option<f64> {
    if v.is_empty() { return None; }
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    let m = s.len() / 2;
    Some(if s.len() % 2 == 1 { s[m] } else { (s[m - 1] + s[m]) / 2.0 })
}

// ---- the server's clock ------------------------------------------------------------------------------------------

/// Round trips to `/api/time` when joining …
pub const CLOCK_JOIN_SAMPLES: usize = 8;
/// … and at each refresh, this often (ms) while in a group.
pub const CLOCK_REFRESH_SAMPLES: usize = 3;
pub const CLOCK_REFRESH_MS: u32 = 60_000;
/// The estimate: the median of the offsets of this many round trips with the shortest round trip.
pub const CLOCK_KEEP: usize = 3;
/// A refresh ignores round trips longer than this many times the shortest seen …
pub const CLOCK_RTT_LIMIT: f64 = 2.0;
/// … but never turns away one this short (ms): on a fast network twice 1–2 ms would refuse every refresh. After
/// [`CLOCK_RESET_AFTER`] refreshes in a row with nothing kept (the network changed: Wi-Fi to mobile data) the
/// shortest round trip seen becomes the shortest of the last refresh. (The same rule on the phone and the desktop.)
pub const CLOCK_MIN_BAR_MS: f64 = 20.0;
pub const CLOCK_RESET_AFTER: u32 = 5;
/// … and moves the offset by this share of the difference (an exponentially weighted average) …
pub const CLOCK_BLEND: f64 = 0.3;
/// … never by more than this (ms) at once.
pub const CLOCK_MAX_STEP_MS: f64 = 50.0;

/// The offset between the server's clock and this device's (server − device, ms), from round trips
/// `(sent, server now, received)` in this device's ms: at join the median of the 3 shortest of 8; then every
/// minute 3 more, those over twice the shortest round trip seen (at least 20 ms) left out, blended in (weight 0.3)
/// and never moved by more than 50 ms at once – a slow answer of a busy server can't throw the group off.
#[derive(Debug, Clone, Default)]
pub struct Clock {
    pub offset: Option<f64>,
    best_rtt: Option<f64>,
    /// refreshes in a row that kept nothing
    empty: u32,
}

impl Clock {
    /// The usable round trips as (offset, round trip): the server's time taken at the middle of each.
    fn usable(samples: &[(f64, f64, f64)]) -> Vec<(f64, f64)> {
        samples.iter().filter(|s| s.2 >= s.0 && s.1 > 0.0).map(|s| (s.1 - (s.0 + s.2) / 2.0, s.2 - s.0)).collect()
    }

    /// The estimate of `samples`: the median offset of the [`CLOCK_KEEP`] with the shortest round trip, and the
    /// shortest round trip; None without a usable one.
    pub fn estimate(samples: &[(f64, f64, f64)]) -> Option<(f64, f64)> {
        let mut u = Self::usable(samples);
        u.sort_by(|a, b| a.1.total_cmp(&b.1));
        u.truncate(CLOCK_KEEP);
        let best = u.first()?.1;
        Some((median(&u.iter().map(|x| x.0).collect::<Vec<_>>())?, best))
    }

    /// Joining: the estimate as it is (8 round trips), which replaces what was measured before. False when none
    /// was usable.
    pub fn join(&mut self, samples: &[(f64, f64, f64)]) -> bool {
        let Some((o, rtt)) = Self::estimate(samples) else { return false };
        self.offset = Some(o);
        self.best_rtt = Some(rtt);
        self.empty = 0;
        true
    }

    /// A refresh (3 round trips): those over twice the shortest seen (at least [`CLOCK_MIN_BAR_MS`]) left out, the
    /// estimate blended in, at most 50 ms of change. After [`CLOCK_RESET_AFTER`] refreshes in a row with nothing
    /// kept, the shortest seen becomes the last refresh's, so a network that got slower for good is followed.
    pub fn refresh(&mut self, samples: &[(f64, f64, f64)]) {
        let Some(old) = self.offset else { self.join(samples); return };
        let Some(newest) = Self::usable(samples).iter().map(|u| u.1).min_by(f64::total_cmp) else { return };
        let best = self.best_rtt.map_or(newest, |b| b.min(newest));
        self.best_rtt = Some(best);
        let bar = (best * CLOCK_RTT_LIMIT).max(CLOCK_MIN_BAR_MS);
        let kept: Vec<(f64, f64, f64)> = samples.iter().copied().filter(|s| s.2 - s.0 <= bar).collect();
        let Some((est, _)) = Self::estimate(&kept) else {
            self.empty += 1;
            if self.empty >= CLOCK_RESET_AFTER { self.best_rtt = Some(newest); self.empty = 0; }
            return;
        };
        self.empty = 0;
        self.offset = Some(old + (CLOCK_BLEND * (est - old)).clamp(-CLOCK_MAX_STEP_MS, CLOCK_MAX_STEP_MS));
    }
}

// ---- quality while in a group --------------------------------------------------------------------------------------

/// Auto quality in a group listen: the rung is chosen when a song starts and kept for the whole song – no change
/// from the bandwidth estimate mid-song (a reload lands behind the group and costs a second one). A real stall (the
/// buffer ran empty) may drop it once per song. The original file is preferred when this device plays it; after a
/// song that had to drop, the next one starts where the estimate says.
#[derive(Debug, Clone, Default)]
pub struct QualityLock {
    song: String,
    dropped: bool,
    dropped_before: bool,
}

impl QualityLock {
    /// A song (its queue entry id) starts.
    pub fn start(&mut self, song: &str) {
        if self.song == song { return; }
        self.dropped_before = self.dropped;
        self.song = song.to_string();
        self.dropped = false;
    }
    /// The rung `song` starts on with Auto: the original when this device plays the file (`can_play`) and the song
    /// before it did not have to drop, else `auto_rung` (what the estimate allows). Asked for the song playing, or
    /// ahead of time for the next one (its pre-buffer), it gives what that song will start on.
    pub fn start_rung(&self, song: &str, can_play: bool, auto_rung: i64) -> i64 {
        let dropped_before = if song == self.song { self.dropped_before } else { self.dropped };
        if can_play && !dropped_before { 0 } else { auto_rung }
    }
    /// May the rung change now, in `song`? `stall`: a real stall. Only a stall, and only once per song.
    pub fn may_change(&mut self, song: &str, stall: bool) -> bool {
        self.start(song);
        if !stall || self.dropped { return false; }
        self.dropped = true;
        true
    }
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

/// A list's Shuffle in a group: the songs in a random order (Fisher–Yates; `rand` gives 0 ≤ x < 1), at most `max`
/// of them, so a list longer than that sends `max` songs picked from all of it (as the phone does), not its first.
pub fn pick_shuffled(ids: &[i64], max: usize, mut rand: impl FnMut() -> f64) -> Vec<i64> {
    let mut v = ids.to_vec();
    for i in (1..v.len()).rev() {
        let j = ((rand() * (i + 1) as f64).floor() as usize).min(i);
        v.swap(i, j);
    }
    v.truncate(max);
    v
}

// ---- Play and Pause in a group (the owner on Oct 21: "it should just add it to the que", "it may pause for me but dont
// stop the other users playback") – the same on web, phone and desktop ---------------------------------------------------

/// Play (or Shuffle) of an album, a playlist, an artist or a song in a group starts the group – the songs become its
/// queue (`set_queue`) – only when it plays nothing (paused, or past its last song) and has nothing in Up next, or
/// its queue is empty. Otherwise the songs are added to its Up next (`enqueue`).
pub fn play_starts(state: &Value, server_now_ms: f64) -> bool {
    let q = state.a("queue");
    if q.is_empty() { return true; }
    let (ei, epos) = expected(state, server_now_ms);
    let nothing_playing = !state.truthy("playing") || at_end(state, ei, epos);
    nothing_playing && ei.max(0) as usize + 1 >= q.len()
}

/// The songs Play adds to the group's Up next: those that would have played – from the one asked for (`index`, among
/// the library songs `ids`) to the end of the list, at most `max`.
pub fn to_add(ids: &[i64], index: usize, max: usize) -> Vec<i64> {
    ids.iter().skip(index).take(max).copied().collect()
}

/// What the toast says once the group took songs into its Up next: "Added 12 songs to the group's Up next", "Added 1
/// song to the group's Up next", "…, shuffled".
pub fn added_text(n: usize, shuffled: bool) -> String {
    let songs = if n == 1 { "1 song".to_string() } else { format!("{n} songs") };
    format!("Added {songs} to the group's Up next{}", if shuffled { ", shuffled" } else { "" })
}

/// Pause in a group pauses only this device; the group plays on.
pub const PAUSED_FOR_YOU: &str = "Paused for you – the group keeps playing";
/// The group's own pause (the player's ⋯ and the group panel, for those who control the group).
pub const PAUSE_FOR_EVERYONE: &str = "Pause for everyone";

/// Whether "Pause for everyone" is offered: this device may control the group and the group plays.
pub fn offers_pause_for_everyone(state: &Value) -> bool {
    state.g("you").truthy("can_control") && state.truthy("playing")
}

/// What the player's ⋯ offers about group listening: in a group, its dialog ("Group listen") and – for those who
/// control it while it plays (`group`: the group's state) – [`PAUSE_FOR_EVERYONE`]; outside one, "Start a group
/// listen" when this browser may start one. Never both "Group listen" and "Start a group listen".
pub fn menu_entries(group: Option<&Value>, may_start: bool) -> Vec<&'static str> {
    match group {
        Some(s) if offers_pause_for_everyone(s) => vec!["Group listen", PAUSE_FOR_EVERYONE],
        Some(_) => vec!["Group listen"],
        None if may_start => vec!["Start a group listen"],
        None => vec![],
    }
}

/// What a seek made to catch up with the group costs on this device: a seek starts playing some time after it was
/// asked (a whole file buffers at the new spot, a server-made stream is asked for again at `t=`), so it is aimed
/// ahead by the running median of what the last 5 took – learnt from where each one landed, for whole files and
/// server-made streams apart, at most 5 s. It also remembers when the last seek was ([`SEEK_GAP_MS`]).
#[derive(Default)]
pub struct SeekLag {
    took: [Vec<f64>; 2],
    /// the seek awaited: (kind, song, the lag it was aimed with)
    wait: Option<(usize, String, f64)>,
    /// when the last one was made (this device's ms)
    last_at: Option<f64>,
}

impl SeekLag {
    pub const KEEP: usize = 5;
    pub const MAX_LAG_MS: f64 = 5000.0;

    /// A seek's result is awaited: the next look while playing calls [`SeekLag::landed`].
    pub fn waiting(&self) -> bool { self.wait.is_some() }
    /// Where to seek in `song` (its queue entry id) for the group's `expected_ms`, at `now_ms`: ahead by the lag of
    /// that kind of stream.
    pub fn target(&mut self, song: &str, expected_ms: f64, server_made: bool, now_ms: f64) -> f64 {
        let k = usize::from(server_made);
        let lag = self.lag_ms(server_made);
        self.wait = Some((k, song.to_string(), lag));
        self.last_at = Some(now_ms);
        expected_ms + lag
    }
    /// The first look while playing after a seek: `drift_ms` = this device − the group, in `song`. What the seek
    /// really took is the lag it was aimed with less the drift.
    pub fn landed(&mut self, song: &str, drift_ms: f64) {
        let Some((k, waited, aimed)) = self.wait.take() else { return };
        if waited != song { return; }
        let v = &mut self.took[k];
        v.push((aimed - drift_ms).clamp(0.0, Self::MAX_LAG_MS));
        if v.len() > Self::KEEP { v.remove(0); }
    }
    /// The seek awaited is not one to learn from (a pause, the listener's own seek, leaving the group).
    pub fn cancel(&mut self) { self.wait = None; }
    /// The running median of what the last seeks of this kind took (0 before the first).
    pub fn lag_ms(&self, server_made: bool) -> f64 { median(&self.took[usize::from(server_made)]).unwrap_or(0.0) }
    /// How long ago the last seek (or song load) was, at `now_ms` (infinite before the first).
    pub fn since_ms(&self, now_ms: f64) -> f64 { self.last_at.map_or(f64::INFINITY, |t| (now_ms - t).max(0.0)) }
}

/// A song change near the boundary settles by itself (the pre-buffered next song plays): this device is on the next
/// entry within 3 s of its start while the group still ends the one before, or it still plays the last moments of
/// the one before (within 3 s of its end) while the group started the next. `cur_dur`: the length of the song here.
pub fn near_boundary(cur_i: i64, pos: f64, cur_dur: f64, ei: i64, epos: f64) -> bool {
    (cur_i == ei + 1 && pos < BOUNDARY_S) || (cur_i + 1 == ei && (epos < BOUNDARY_S || (cur_dur > 0.0 && cur_dur - pos < BOUNDARY_S)))
}

/// How close (s) to the expected end of a song a song change uses the pre-buffered next song.
pub const BOUNDARY_S: f64 = 3.0;

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

    /// Runs the rule every 500 ms on a device `drift0` ms off, with the drift closing at the rate's distance from 1.0:
    /// the rates it played at, and the drift at the end.
    fn run(drift0: f64, server_made: bool, looks: usize) -> (Vec<f64>, f64, usize) {
        let (mut d, mut rate, mut seeks) = (drift0, 1.0, 0);
        let mut rates = vec![];
        for _ in 0..looks {
            match sync_step(d, rate, f64::INFINITY, server_made) {
                Act::Seek => { seeks += 1; d = 0.0; rate = 1.0; }
                Act::Rate(r) => rate = r,
                Act::Keep => {}
            }
            rates.push(rate);
            d += (rate - 1.0) * 500.0;
        }
        (rates, d, seeks)
    }

    #[test]
    fn drift_to_action() {
        // under 40 ms: nothing
        assert_eq!(sync_step(39.0, 1.0, f64::INFINITY, false), Act::Keep);
        assert_eq!(sync_step(-39.0, 1.0, f64::INFINITY, false), Act::Keep);
        // 40 ms – 1.5 s: a nudge, eased in (one step of 0.4 % at a time), slower when ahead, faster when behind
        assert_eq!(sync_step(300.0, 1.0, f64::INFINITY, false), Act::Rate(0.996));
        assert_eq!(sync_step(-300.0, 1.0, f64::INFINITY, false), Act::Rate(1.004));
        assert_eq!(sync_step(-1400.0, 1.016, f64::INFINITY, false), Act::Rate(1.02), "never past 2 %");
        assert_eq!(sync_step(-1400.0, 1.02, f64::INFINITY, false), Act::Keep);
        // proportional: 60 ms ahead wants 0.6 % slower
        assert_eq!(sync_step(60.0, 0.996, f64::INFINITY, false), Act::Rate(0.994));
        assert_eq!(sync_step(60.0, 0.994, f64::INFINITY, false), Act::Keep);
        // kept until under 20 ms, then eased out
        assert_eq!(sync_step(25.0, 0.998, f64::INFINITY, false), Act::Rate(0.9975));
        assert_eq!(sync_step(15.0, 0.99, f64::INFINITY, false), Act::Rate(0.994));
        assert_eq!(sync_step(15.0, 0.998, f64::INFINITY, false), Act::Rate(1.0));
        // above 1.5 s: one hard seek – never for less, whatever happened before
        assert_eq!(sync_step(1501.0, 1.0, f64::INFINITY, false), Act::Seek);
        assert_eq!(sync_step(-1501.0, 1.0, 9000.0, false), Act::Seek);
        assert_ne!(sync_step(1499.0, 1.0, f64::INFINITY, false), Act::Seek);
        // no seek again within 8 s of the last one: a nudge (at most 2 %) instead
        assert_eq!(sync_step(2500.0, 1.0, 7999.0, false), Act::Rate(0.996));
        assert_eq!(sync_step(2500.0, 1.0, 8000.0, false), Act::Seek);
        // a server-made stream is never seeked for under 3 s
        assert_eq!(sync_step(2900.0, 1.0, f64::INFINITY, true), Act::Rate(0.996));
        assert_eq!(sync_step(3001.0, 1.0, f64::INFINITY, true), Act::Seek);
    }

    #[test]
    fn a_nudge_closes_the_drift_smoothly_and_never_seeks() {
        for start in [45.0f64, 150.0, -400.0, 1400.0, -2900.0] {
            let server_made = start.abs() > 1500.0;
            let (rates, end, seeks) = run(start, server_made, 600);
            assert_eq!(seeks, 0, "{start}");
            assert!(end.abs() < SETTLED_MS, "{start}: still {end} ms off");
            assert!(rates.iter().all(|r| (0.98..=1.02).contains(r)), "{start}");
            // eased: no step of more than 0.4 % from one look to the next, and it ends at 1.0
            let mut last = 1.0;
            for r in &rates { assert!((r - last).abs() <= EASE_STEP + 1e-9, "{start}: {last} → {r}"); last = *r; }
            assert_eq!(*rates.last().unwrap(), 1.0, "{start}");
        }
        // 1.6 s off a whole file: one seek, then nothing more
        let (_, end, seeks) = run(1600.0, false, 50);
        assert_eq!((seeks, end), (1, 0.0));
    }

    #[test]
    fn the_median() {
        assert_eq!(median(&[]), None);
        assert_eq!(median(&[3.0]), Some(3.0));
        assert_eq!(median(&[9.0, 1.0, 5.0]), Some(5.0));
        assert_eq!(median(&[4.0, 1.0, 3.0, 2.0]), Some(2.5));
    }

    #[test]
    fn the_clock_estimate() {
        // (sent, server now, received): offsets 1000 (rtt 100), 1000 (rtt 20), 990 (rtt 30), 1010 (rtt 40), 2000 (rtt 900)
        let samples = [(0.0, 1050.0, 100.0), (200.0, 1210.0, 220.0), (300.0, 1305.0, 330.0), (400.0, 1430.0, 440.0), (500.0, 2950.0, 1400.0)];
        // the 3 shortest round trips (20, 30, 40): the median of their offsets (1000, 990, 1010)
        assert_eq!(Clock::estimate(&samples), Some((1000.0, 20.0)));
        assert_eq!(Clock::estimate(&[]), None);
        assert_eq!(Clock::estimate(&[(10.0, 0.0, 20.0), (30.0, 5.0, 20.0)]), None, "no server time, or received before sent");
        let mut c = Clock::default();
        assert!(c.join(&samples));
        assert_eq!(c.offset, Some(1000.0));
        // a refresh 100 ms off: blended in at 0.3, and never more than 50 ms at once
        c.refresh(&[(0.0, 1110.0, 20.0), (100.0, 1210.0, 120.0), (200.0, 1310.0, 220.0)]);
        assert_eq!(c.offset, Some(1030.0));
        c.refresh(&[(0.0, 1530.0, 20.0)]);
        assert_eq!(c.offset, Some(1080.0), "500 ms off moves it 50 ms");
        // round trips over twice the shortest seen (20 ms) are left out
        c.refresh(&[(0.0, 9000.0, 41.0), (0.0, 9000.0, 300.0)]);
        assert_eq!(c.offset, Some(1080.0));
        // … but a network that stays slower is followed: after 5 refreshes in a row with nothing kept, the shortest
        // round trip seen is the last refresh's (100 ms), and the next one counts
        let mut n = 0;
        while c.offset == Some(1080.0) && n < 10 { c.refresh(&[(0.0, 1250.0, 100.0)]); n += 1; }
        assert_eq!(n, 5, "the 41/300 ms refresh and 4 more keep nothing (the 5th in a row resets), the next counts: {:?}", c.offset);
        assert!(c.offset.unwrap() > 1080.0 && c.offset.unwrap() <= 1130.0, "{:?}", c.offset);
        // on a fast network the bar is at least 20 ms: a 1 ms best does not turn a 3 ms round trip away
        let mut lan = Clock::default();
        assert!(lan.join(&[(0.0, 501.0, 1.0)]));
        assert_eq!(lan.offset, Some(500.5));
        lan.refresh(&[(0.0, 602.0, 3.0)]);
        assert_eq!(lan.offset, Some(530.5), "kept and blended in");
        // a join replaces the shortest round trip seen (a reload on another network)
        let mut again = Clock::default();
        again.join(&[(0.0, 501.0, 1.0)]);
        again.join(&[(0.0, 550.0, 100.0)]);
        again.refresh(&[(0.0, 570.0, 120.0)]);
        assert_eq!(again.offset, Some(503.0), "120 ms is within twice the new join's 100 ms");
        // a refresh before any join is a join
        let mut fresh = Clock::default();
        fresh.refresh(&[(0.0, 500.0, 10.0)]);
        assert_eq!(fresh.offset, Some(495.0));
    }

    #[test]
    fn the_quality_lock() {
        let mut q = QualityLock::default();
        q.start("q1");
        // the original when this device plays the file; what the estimate says when it can't
        assert_eq!((q.start_rung("q1", true, 256), q.start_rung("q1", false, 256)), (0, 256));
        // the estimate never changes the rung mid-song; a real stall once
        assert!(!q.may_change("q1", false));
        assert!(q.may_change("q1", true));
        assert!(!q.may_change("q1", true), "at most once per song");
        // the next song, asked ahead (its pre-buffer) and once it starts: after a song that had to drop, the estimate
        // decides where it starts
        assert_eq!(q.start_rung("q2", true, 128), 128);
        q.start("q2");
        assert_eq!(q.start_rung("q2", true, 128), 128);
        assert_eq!(q.start_rung("q3", true, 128), 0, "q2 has not dropped (yet)");
        assert!(q.may_change("q2", true));
        // a song without a drop: the one after it prefers the original again
        q.start("q3");
        q.start("q4");
        assert_eq!(q.start_rung("q4", true, 128), 0);
        // the same song started again (a reload) keeps its one drop used
        assert!(q.may_change("q4", true));
        q.start("q4");
        assert!(!q.may_change("q4", true));
    }

    #[test]
    fn song_changes_near_the_boundary_settle_by_themselves() {
        // here on the next entry's start, the group still ending the one before
        assert!(near_boundary(1, 2.5, 180.0, 0, 179.0));
        // the group 2.9 s into the next one, here still on the one before
        assert!(near_boundary(0, 120.0, 180.0, 1, 2.9));
        // here 2 s before the end of the song (by its own length), the group moved on
        assert!(near_boundary(0, 178.5, 180.5, 1, 4.0));
        // further: no
        assert!(!near_boundary(1, 3.5, 180.0, 0, 176.0));
        assert!(!near_boundary(0, 100.0, 180.0, 1, 4.0));
        assert!(!near_boundary(0, 10.0, 180.0, 2, 1.0));
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

    /// Play in a group adds to Up next unless the group plays nothing and has nothing after its song (or nothing at all).
    #[test]
    fn play_starts_the_group_only_when_it_has_nothing_to_play() {
        let q = json!([{"qid": "a", "duration": 200.0}, {"qid": "b", "duration": 200.0}]);
        let at = |index: i64, position: f64, playing: bool| json!({"queue": q, "index": index, "position": position, "playing": playing, "anchor_ms": 1_000_000.0});
        assert!(play_starts(&json!({"queue": []}), 1_000_000.0), "an empty queue");
        assert!(!play_starts(&at(0, 10.0, true), 1_000_000.0), "playing, with Up next");
        assert!(!play_starts(&at(0, 10.0, false), 1_000_000.0), "paused, with Up next");
        assert!(!play_starts(&at(1, 10.0, true), 1_000_000.0), "playing its last song");
        assert!(play_starts(&at(1, 10.0, false), 1_000_000.0), "paused on its last song: nothing in Up next");
        // the last song ran out: nothing plays
        assert!(play_starts(&at(1, 10.0, true), 1_000_000.0 + 300_000.0));
        assert!(!play_starts(&at(0, 10.0, true), 1_000_000.0 + 250_000.0), "into its last song: still playing");
    }

    #[test]
    fn what_play_adds_and_says() {
        assert_eq!(to_add(&[1, 2, 3, 4], 0, 1000), vec![1, 2, 3, 4]);
        assert_eq!(to_add(&[1, 2, 3, 4], 2, 1000), vec![3, 4], "from the song asked for");
        assert_eq!(to_add(&[1, 2, 3, 4], 1, 2), vec![2, 3]);
        assert!(to_add(&[1, 2], 2, 1000).is_empty());
        assert_eq!(added_text(12, false), "Added 12 songs to the group's Up next");
        assert_eq!(added_text(1, false), "Added 1 song to the group's Up next");
        assert_eq!(added_text(40, true), "Added 40 songs to the group's Up next, shuffled");
        assert_eq!(PAUSED_FOR_YOU, "Paused for you – the group keeps playing");
        assert_eq!(PAUSE_FOR_EVERYONE, "Pause for everyone");
        assert!(offers_pause_for_everyone(&json!({"you": {"can_control": true}, "playing": true})));
        assert!(!offers_pause_for_everyone(&json!({"you": {"can_control": true}, "playing": false})));
        assert!(!offers_pause_for_everyone(&json!({"you": {"can_control": false}, "playing": true})));
    }

    /// The player's ⋯ in a group never offers to start another one, whoever may control it and whether it
    /// plays; outside a group only "Start a group listen", when this browser may.
    #[test]
    fn the_player_menu_offers_one_group_entry_set() {
        let (ctl_playing, ctl_paused, member) = (json!({"you": {"can_control": true}, "playing": true}),
            json!({"you": {"can_control": true}, "playing": false}), json!({"you": {"can_control": false}, "playing": true}));
        assert_eq!(menu_entries(Some(&ctl_playing), true), ["Group listen", "Pause for everyone"]);
        assert_eq!(menu_entries(Some(&ctl_paused), true), ["Group listen"]);
        assert_eq!(menu_entries(Some(&member), true), ["Group listen"]);
        assert_eq!(menu_entries(None, true), ["Start a group listen"]);
        assert!(menu_entries(None, false).is_empty());
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
    fn a_shuffle_of_a_long_list_picks_from_all_of_it() {
        // a steady stand-in for the random numbers
        let mut x = 0.5f64;
        let mut rand = move || { x = (x * 9301.0 + 0.49297).fract(); x };
        let ids: Vec<i64> = (0..3000).collect();
        let got = pick_shuffled(&ids, 1000, &mut rand);
        assert_eq!(got.len(), 1000);
        let mut seen = got.clone();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 1000, "no song twice");
        assert!(got.iter().filter(|i| **i >= 1000).count() > 500, "songs past the first 1000 come too");
        // a short list: every song once, in another order
        let short: Vec<i64> = (0..20).collect();
        let mixed = pick_shuffled(&short, 1000, &mut rand);
        let mut back = mixed.clone();
        back.sort_unstable();
        assert_eq!(back, short);
        assert_ne!(mixed, short);
        assert!(pick_shuffled(&[], 1000, &mut rand).is_empty());
    }

    #[test]
    fn the_seek_lag_is_a_running_median() {
        let mut l = SeekLag::default();
        assert_eq!(l.since_ms(0.0), f64::INFINITY);
        assert_eq!(l.target("q1", 10_000.0, true, 1000.0), 10_000.0);
        assert!(l.waiting());
        assert_eq!(l.since_ms(4000.0), 3000.0);
        // landed 800 ms behind: that seek took 800 ms
        l.landed("q1", -800.0);
        assert_eq!((l.lag_ms(true), l.lag_ms(false)), (800.0, 0.0));
        // one slow seek among quick ones doesn't throw the aim off: the median of the last 5
        for took in [700.0, 2900.0, 750.0, 820.0, 760.0] {
            let aimed = l.lag_ms(true);
            l.target("q1", 0.0, true, 0.0);
            l.landed("q1", aimed - took);
        }
        assert_eq!(l.lag_ms(true), 760.0, "the last 5: 700, 2900, 750, 820, 760");
        // a seek in another song, or one cancelled, teaches nothing
        l.target("q1", 0.0, false, 0.0);
        l.landed("q2", -500.0);
        assert_eq!(l.lag_ms(false), 0.0);
        l.target("q1", 0.0, false, 0.0);
        l.cancel();
        l.landed("q1", -500.0);
        assert_eq!(l.lag_ms(false), 0.0);
        // never under 0 or over 5 s
        l.target("q1", 0.0, false, 0.0);
        l.landed("q1", 900.0);
        assert_eq!(l.lag_ms(false), 0.0);
        l.target("q1", 0.0, false, 0.0);
        l.landed("q1", -20_000.0);
        assert_eq!(l.lag_ms(false), 2500.0, "the median of 0 and 5000");
        l.target("q1", 0.0, false, 0.0);
        l.landed("q1", -20_000.0);
        assert_eq!(l.lag_ms(false), 5000.0, "the median of 0, 5000 and 5000");
    }
}
