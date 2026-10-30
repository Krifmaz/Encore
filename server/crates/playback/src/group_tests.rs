//! The rules of a group listen without the server: the clock (`position` at `anchor_ms`), moving on at the end
//! of a song, the commands, who may do what, the host handing over, the cap, and saving and restoring.
use super::*;
use encore_db::p;

fn row(id: i64, duration: f64) -> Arc<Row> {
    let v = json!({ "id": id, "title": format!("Song {id}"), "artist": "Band", "album": "Record", "albumartist": "Band", "album_id": "al1",
                    "track": id, "disc": 1, "year": 2006, "duration": duration, "format": "flac", "bitrate": 900 });
    Arc::new(v.as_object().unwrap().clone())
}

fn rows_for(ids: &[(i64, f64)]) -> HashMap<i64, Arc<Row>> {
    ids.iter().map(|(i, d)| (*i, row(*i, *d))).collect()
}

fn alex() -> AddedBy {
    AddedBy { user: 3, name: "Alex".into(), guest: false }
}

fn account(user: i64, name: &str, device: &str, at: f64) -> Member {
    Member { member: String::new(), user, guest: None, name: name.into(), avatar: String::new(), device_id: device.into(), device_name: format!("{device} app"),
             joined_at: at, host: false, added_by: None, streams: 0, gone_since: Some(at), expires_at: None }
}

fn guest(name: &str, at: f64) -> Member {
    Member { member: String::new(), user: 0, guest: Some(format!("g-{name}")), name: name.into(), avatar: String::new(), device_id: String::new(),
             device_name: String::new(), joined_at: at, host: false, added_by: None, streams: 0, gone_since: Some(at), expires_at: Some(at + GUEST_SECS) }
}

const T0: i64 = 1_792_390_000_000;

/// A group hosted by Alex (user 3, device "pc") with songs 1, 2, 3 of 200, 180 and 240 s, playing song 1 at 0.
fn group() -> Group {
    let mut g = Group::new(NewGroup { id: "g_1".into(), code: "K7QM4X".into(), visibility: ANYONE.into(), everyone_controls: false, guests_allowed: false, max_members: 20 }, T0);
    g.add(account(3, "Alex", "pc", 1000.0));
    let r = rows_for(&[(1, 200.0), (2, 180.0), (3, 240.0)]);
    g.set_queue(&[1, 2, 3], &r, At { index: 0, position: 0.0, playing: true }, &alex(), T0);
    g
}

fn cmd(g: &mut Group, op: &str, body: Value, at_ms: i64) -> ApiResult<()> {
    let r = rows_for(&[(1, 200.0), (2, 180.0), (3, 240.0), (4, 100.0), (5, 90.0)]);
    g.command(op, &body, &r, &alex(), at_ms)
}

fn qids(g: &Group) -> Vec<String> {
    g.queue.iter().map(|e| e.qid.clone()).collect()
}

#[test]
fn the_anchor_math_runs_on_while_playing_and_stands_still_when_paused() {
    let mut g = group();
    assert_eq!(g.position_at(T0), 0.0);
    assert_eq!(g.position_at(T0 + 12_500), 12.5);
    // a pause at 30 s keeps the position there whatever the clock says
    cmd(&mut g, "pause", json!({}), T0 + 30_000).unwrap();
    assert!(!g.playing);
    assert_eq!((g.position, g.anchor_ms), (30.0, T0 + 30_000));
    assert_eq!(g.position_at(T0 + 90_000), 30.0);
    // play again a minute later: the anchor moves to then, the position stays 30
    cmd(&mut g, "play", json!({}), T0 + 90_000).unwrap();
    assert_eq!((g.position, g.anchor_ms), (30.0, T0 + 90_000));
    assert_eq!(g.position_at(T0 + 100_000), 40.0);
    // seek: the song at that position, now
    cmd(&mut g, "seek", json!({ "position": 150.25 }), T0 + 101_000).unwrap();
    assert_eq!((g.position, g.anchor_ms), (150.25, T0 + 101_000));
    assert_eq!(g.position_at(T0 + 103_000), 152.25);
    // past the end is the end
    cmd(&mut g, "seek", json!({ "position": 999.0 }), T0 + 104_000).unwrap();
    assert_eq!(g.position, 200.0);
    assert_eq!(cmd(&mut g, "seek", json!({}), T0).unwrap_err().status.as_u16(), 400, "position is required");
}

#[test]
fn the_server_moves_on_at_the_end_of_a_song_and_stops_after_the_last() {
    let mut g = group();
    assert!(!g.advance(T0 + 199_999), "not yet");
    assert!(g.advance(T0 + 200_000));
    // song 2 from 0, anchored at the moment song 1 ended
    assert_eq!((g.index, g.position, g.anchor_ms, g.playing), (1, 0.0, T0 + 200_000, true));
    // the server was busy for a while: both songs that ended are passed, each anchored at its own end
    assert!(g.advance(T0 + 200_000 + 180_000 + 240_000 + 5_000));
    assert_eq!((g.index, g.position, g.playing), (2, 240.0, false), "stopped at the end of the last entry");
    assert_eq!(g.anchor_ms, T0 + 620_000);
    assert!(!g.advance(T0 + 900_000), "stopped: nothing moves");
    // play at the very end of the last song plays it again from its start
    cmd(&mut g, "play", json!({}), T0 + 900_000).unwrap();
    assert_eq!((g.index, g.position, g.playing), (2, 0.0, true));
    // the queue ran out, then people added songs: play starts the first of them, not the finished one again
    assert!(g.advance(T0 + 900_000 + 240_000));
    assert_eq!((g.index, g.position, g.playing), (2, 240.0, false));
    cmd(&mut g, "enqueue", json!({ "tracks": [4] }), T0 + 1_200_000).unwrap();
    cmd(&mut g, "play_next", json!({ "tracks": [5] }), T0 + 1_200_000).unwrap();
    assert_eq!(g.queue.iter().map(|e| e.track).collect::<Vec<_>>(), vec![1, 2, 3, 5, 4]);
    cmd(&mut g, "play", json!({}), T0 + 1_210_000).unwrap();
    assert_eq!((g.index, g.position, g.playing, g.anchor_ms), (3, 0.0, true, T0 + 1_210_000), "song 5, the play-next one");
    assert!(!g.queue[3].next, "reached: no longer a play-next entry");
}

#[test]
fn next_prev_and_jump() {
    let mut g = group();
    cmd(&mut g, "next", json!({}), T0 + 10_000).unwrap();
    assert_eq!((g.index, g.position, g.anchor_ms), (1, 0.0, T0 + 10_000));
    // prev past 3 s: back to the start of this song; within 3 s: the previous song
    cmd(&mut g, "prev", json!({}), T0 + 15_000).unwrap();
    assert_eq!((g.index, g.position), (1, 0.0));
    cmd(&mut g, "prev", json!({}), T0 + 16_000).unwrap();
    assert_eq!((g.index, g.position), (0, 0.0));
    cmd(&mut g, "prev", json!({}), T0 + 16_500).unwrap();
    assert_eq!((g.index, g.position), (0, 0.0), "on the first entry: its start");
    // jump by qid; the qid wins over a disagreeing index; an unknown qid is a 409
    let q3 = g.queue[2].qid.clone();
    cmd(&mut g, "pause", json!({}), T0 + 17_000).unwrap();
    cmd(&mut g, "jump", json!({ "qid": q3, "index": 1 }), T0 + 18_000).unwrap();
    assert_eq!((g.index, g.position, g.playing), (2, 0.0, true), "jump plays that entry from 0");
    let e = cmd(&mut g, "jump", json!({ "qid": "q999", "index": 0 }), T0).unwrap_err();
    assert_eq!((e.status.as_u16(), e.detail.as_str()), (409, "That song is no longer in the queue"));
    assert_eq!(cmd(&mut g, "jump", json!({ "index": 7 }), T0).unwrap_err().status.as_u16(), 400, "out of range");
    // next on the last entry: stopped at its end, index unchanged
    cmd(&mut g, "next", json!({}), T0 + 20_000).unwrap();
    assert_eq!((g.index, g.position, g.playing), (2, 240.0, false));
    assert_eq!(cmd(&mut g, "rewind", json!({}), T0).unwrap_err().status.as_u16(), 400, "unknown op");
}

#[test]
fn the_queue_commands() {
    let mut g = group();
    let before = qids(&g);
    // enqueue appends; play_next goes after the current one and after earlier play_next songs not reached yet
    cmd(&mut g, "enqueue", json!({ "tracks": [4] }), T0).unwrap();
    cmd(&mut g, "play_next", json!({ "tracks": [5] }), T0).unwrap();
    cmd(&mut g, "play_next", json!({ "tracks": [4, 99] }), T0).unwrap();
    let tracks: Vec<i64> = g.queue.iter().map(|e| e.track).collect();
    assert_eq!(tracks, vec![1, 5, 4, 2, 3, 4], "99 is not in the library: left out");
    // the same song queued twice has two qids, all different, and the old ones stayed
    let all = qids(&g);
    assert_eq!(all.iter().collect::<HashSet<_>>().len(), all.len());
    assert!(before.iter().all(|q| all.contains(q)));
    assert_eq!(cmd(&mut g, "enqueue", json!({ "tracks": [99] }), T0).unwrap_err().detail, "Only songs from your library can be played in a group listen");
    // reaching a play_next song ends its run: the next play_next goes right after the new current one
    cmd(&mut g, "next", json!({}), T0 + 1000).unwrap();
    cmd(&mut g, "play_next", json!({ "tracks": [3] }), T0 + 1000).unwrap();
    let tracks: Vec<i64> = g.queue.iter().map(|e| e.track).collect();
    assert_eq!(tracks, vec![1, 5, 4, 3, 2, 3, 4]);
    // move: Media3's moveMediaItem(from, to); the current entry keeps playing where it moved to
    let mut m = group();
    for t in [4, 5, 1, 2, 3] {
        cmd(&mut m, "enqueue", json!({ "tracks": [t] }), T0).unwrap();
    }
    assert_eq!(m.queue.len(), 8);
    let moved = m.queue[2].qid.clone();
    cmd(&mut m, "move", json!({ "qid": moved, "from": 2, "to": 5 }), T0).unwrap();
    assert_eq!(m.queue[5].qid, moved, "moving 2 → 5 in a list of 8 leaves it at index 5");
    let cur = m.queue[0].qid.clone();
    cmd(&mut m, "move", json!({ "from": 0, "to": 3 }), T0).unwrap();
    assert_eq!((m.index, m.queue[3].qid.clone()), (3, cur), "the current entry is followed");
    assert_eq!(cmd(&mut m, "move", json!({ "from": 0, "to": 8 }), T0).unwrap_err().status.as_u16(), 400);
    // remove: before the current one, the current one (the next plays from 0), the last one while current
    let mut r = group();
    cmd(&mut r, "next", json!({}), T0 + 5_000).unwrap();
    let first = r.queue[0].qid.clone();
    cmd(&mut r, "remove", json!({ "qid": first }), T0 + 6_000).unwrap();
    assert_eq!((r.index, r.queue[r.index].track), (0, 2));
    cmd(&mut r, "seek", json!({ "position": 50.0 }), T0 + 7_000).unwrap();
    cmd(&mut r, "remove", json!({ "index": 0 }), T0 + 8_000).unwrap();
    assert_eq!((r.index, r.queue[r.index].track, r.position, r.playing), (0, 3, 0.0, true), "removing the current one plays the next");
    cmd(&mut r, "remove", json!({ "index": 0 }), T0 + 9_000).unwrap();
    assert!(r.queue.is_empty() && !r.playing);
    // set_queue: replaced, always playing, unknown ids out and the index follows its song
    let mut s = group();
    cmd(&mut s, "pause", json!({}), T0).unwrap();
    cmd(&mut s, "set_queue", json!({ "tracks": [99, 4, 5], "index": 2, "position": 12.0 }), T0 + 1000).unwrap();
    let tracks: Vec<i64> = s.queue.iter().map(|e| e.track).collect();
    assert_eq!((tracks, s.index, s.position, s.playing), (vec![4, 5], 1, 12.0, true));
    assert!(s.queue.iter().all(|e| e.added_by == alex()), "added by whoever sent them");
    let too_many: Vec<i64> = (0..1001).collect();
    assert_eq!(cmd(&mut s, "set_queue", json!({ "tracks": too_many }), T0).unwrap_err().status.as_u16(), 400);
}

/// A group of `n` songs (ids 1..=n, 100 s each) playing song 1 at 0, and rows for ids 1..=n+50.
fn long_group(n: i64) -> (Group, HashMap<i64, Arc<Row>>) {
    let r: HashMap<i64, Arc<Row>> = (1..=n + 50).map(|i| (i, row(i, 100.0))).collect();
    let mut g = Group::new(NewGroup { id: "g_2".into(), code: "K7QM4Y".into(), visibility: ANYONE.into(), everyone_controls: false, guests_allowed: false, max_members: 20 }, T0);
    g.add(account(3, "Alex", "pc", 1000.0));
    let ids: Vec<i64> = (1..=n).collect();
    g.set_queue(&ids, &r, At { index: 0, position: 0.0, playing: true }, &alex(), T0);
    (g, r)
}

fn tracks(g: &Group) -> Vec<i64> {
    g.queue.iter().map(|e| e.track).collect()
}

fn sorted(mut v: Vec<i64>) -> Vec<i64> {
    v.sort_unstable();
    v
}

#[test]
fn shuffle_mixes_the_songs_after_the_one_playing_and_clear_takes_them_out() {
    let (mut g, r) = long_group(40);
    g.command("next", &json!({}), &r, &alex(), T0 + 1000).unwrap();
    g.command("seek", &json!({ "position": 42.0 }), &r, &alex(), T0 + 2000).unwrap();
    g.command("play_next", &json!({ "tracks": [77] }), &r, &alex(), T0 + 2000).unwrap();
    assert!(g.queue[2].next);
    let before = tracks(&g);
    let playing = g.queue[1].qid.clone();
    let (anchor, position) = (g.anchor_ms, g.position);
    g.command("shuffle", &json!({}), &r, &alex(), T0 + 3000).unwrap();
    let after = tracks(&g);
    // the song before and the one playing stay where they are, and it plays on where it was
    assert_eq!((&after[..2], g.index, g.queue[1].qid.clone()), (&before[..2], 1, playing.clone()));
    assert_eq!((g.anchor_ms, g.position, g.playing), (anchor, position, true), "the song playing is not touched");
    // the rest: the same songs (and entries) in another order; 39 songs coming back in order is a 1 in 39! chance
    assert_eq!(sorted(after[2..].to_vec()), sorted(before[2..].to_vec()));
    assert_ne!(after[2..], before[2..]);
    assert!(g.queue.iter().all(|e| !e.next), "after a shuffle the next Play next goes right after the song playing");
    // nothing after the last one: a shuffle changes nothing
    let (mut one, r1) = long_group(1);
    one.command("shuffle", &json!({}), &r1, &alex(), T0).unwrap();
    assert_eq!(tracks(&one), vec![1]);
    // clear: everything after the song playing goes; that song plays on
    g.command("clear", &json!({}), &r, &alex(), T0 + 4000).unwrap();
    assert_eq!((tracks(&g), g.index, g.queue[1].qid.clone()), (before[..2].to_vec(), 1, playing));
    assert_eq!((g.anchor_ms, g.position, g.playing), (anchor, position, true));
    // songs added after a clear come right after the one playing
    g.command("enqueue", &json!({ "tracks": [5] }), &r, &alex(), T0 + 5000).unwrap();
    assert_eq!(tracks(&g)[2], 5);
    // an empty queue: clear and shuffle change nothing
    let mut e = Group::new(NewGroup { id: "g_3".into(), code: "K7QM4Z".into(), visibility: ANYONE.into(), everyone_controls: false, guests_allowed: false, max_members: 20 }, T0);
    e.command("clear", &json!({}), &r, &alex(), T0).unwrap();
    e.command("shuffle", &json!({}), &r, &alex(), T0).unwrap();
    assert!(e.queue.is_empty() && !e.playing);
}

#[test]
fn set_queue_and_enqueue_with_shuffle_mix_the_songs_first() {
    let (mut g, r) = long_group(3);
    let list: Vec<i64> = (10..50).collect();
    // set_queue with shuffle: the songs in a random order, playing from the first of it (index and position ignored)
    g.command("set_queue", &json!({ "tracks": list, "index": 7, "position": 30.0, "shuffle": true }), &r, &alex(), T0 + 1000).unwrap();
    let got = tracks(&g);
    assert_eq!(sorted(got.clone()), list);
    assert_ne!(got, list, "40 songs coming back in order is a 1 in 40! chance");
    assert_eq!((g.index, g.position, g.anchor_ms, g.playing), (0, 0.0, T0 + 1000, true));
    // without it the order and the place asked for stay
    g.command("set_queue", &json!({ "tracks": [10, 11, 12], "index": 2, "position": 5.0, "shuffle": false }), &r, &alex(), T0 + 2000).unwrap();
    assert_eq!((tracks(&g), g.index, g.position), (vec![10, 11, 12], 2, 5.0));
    // enqueue with shuffle: added at the end, in a random order, the song playing not touched
    let more: Vec<i64> = (13..53).collect();
    g.command("enqueue", &json!({ "tracks": more, "shuffle": true }), &r, &alex(), T0 + 3000).unwrap();
    let got = tracks(&g);
    assert_eq!((&got[..3], g.index, g.position), (&[10, 11, 12][..], 2, 5.0));
    assert_eq!(sorted(got[3..].to_vec()), more);
    assert_ne!(got[3..], more[..]);
    // unknown songs are still left out
    g.command("enqueue", &json!({ "tracks": [999, 20], "shuffle": true }), &r, &alex(), T0 + 4000).unwrap();
    assert_eq!(*tracks(&g).last().unwrap(), 20);
}

#[test]
fn every_change_bumps_rev_in_order_and_is_announced() {
    let mut g = group();
    let mut rx = g.tx.subscribe();
    let start = g.rev;
    for (i, op) in ["pause", "play", "next", "prev"].iter().enumerate() {
        cmd(&mut g, op, json!({}), T0 + i as i64 * 1000).unwrap();
        g.changed(0.0);
        assert_eq!(g.rev, start + 1 + i as u64);
    }
    let mut events = 0;
    while let Ok(e) = rx.try_recv() {
        assert!(matches!(e, Event::State));
        events += 1;
    }
    assert_eq!(events, 4);
}

#[test]
fn who_may_do_what() {
    let mut g = group();
    let sam = g.add(account(5, "Sam", "phone", 1001.0));
    let kim = g.add(guest("Kim", 1002.0));
    let alex = Caller::Account { user: 3, device: "pc".into() };
    let alex_phone = Caller::Account { user: 3, device: "phone".into() };
    let sam_c = Caller::Account { user: 5, device: "phone".into() };
    let kim_c = Caller::Guest { member: kim.clone() };
    let stranger = Caller::Account { user: 9, device: "x".into() };
    assert_eq!(g.you(&alex), You { member: "m1".into(), host: true, can_control: true, guest: false });
    assert_eq!(g.you(&alex_phone), You { member: String::new(), host: true, can_control: true, guest: false }, "the host's account on another device");
    assert_eq!(g.you(&sam_c), You { member: sam, host: false, can_control: false, guest: false });
    assert_eq!(g.you(&kim_c), You { member: kim, host: false, can_control: false, guest: true });
    assert!(!g.you(&stranger).can_control);
    g.everyone_controls = true;
    assert!(g.you(&sam_c).can_control);
    assert!(!g.you(&kim_c).can_control, "a guest never controls");
    assert!(!g.you(&stranger).can_control, "not in the group");
    // adding songs is for every account member; the rest needs control
    assert_eq!(needs_control("enqueue"), Some(false));
    assert_eq!(needs_control("play_next"), Some(false));
    for op in ["play", "pause", "seek", "next", "prev", "jump", "set_queue", "remove", "move", "shuffle", "clear"] {
        assert_eq!(needs_control(op), Some(true), "{op}");
    }
    assert_eq!(needs_control("ended"), None);
}

#[test]
fn the_host_hands_over_to_the_longest_present_account_and_the_last_account_ends_it() {
    let mut g = group();
    let kim = g.add(guest("Kim", 1001.0));
    let sam = g.add(account(5, "Sam", "phone", 1003.0));
    let pat = g.add(account(7, "Pat", "tablet", 1002.0));
    assert_eq!(g.members.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["Alex", "Kim", "Pat", "Sam"], "ordered by joined_at");
    assert!(!g.remove("m1"));
    assert_eq!(g.host_name(), "Pat", "the longest-present account member, never the guest who came earlier");
    assert_eq!(g.members.iter().filter(|m| m.host).count(), 1);
    assert!(!g.remove(&sam));
    assert!(g.remove(&pat), "only the guest is left: the group ends");
    assert!(g.member(&kim).is_some());

    // the host is in the group on a second device too: closing the first keeps the account hosting
    let mut g = group();
    g.add(account(5, "Sam", "phone", 1001.0));
    let alex_phone = g.add(account(3, "Alex", "phone", 1002.0));
    assert!(!g.remove("m1"));
    assert_eq!(g.host().map(|m| m.member.as_str()), Some(alex_phone.as_str()), "Alex's phone, not Sam who came earlier");
    assert!(g.you(&Caller::Account { user: 3, device: "phone".into() }).host);
    assert!(!g.you(&Caller::Account { user: 5, device: "phone".into() }).host);
}

#[test]
fn the_cap_counts_guests_and_the_server_cap_wins() {
    let mut g = group();
    g.max_members = 3;
    g.add(guest("Kim", 1001.0));
    assert!(!g.full(20));
    g.add(guest("Lee", 1002.0));
    assert!(g.full(20), "two guests and the host fill a cap of 3");
    g.max_members = 20;
    assert_eq!(g.cap(5), 5, "never above the server's");
    assert!(!g.full(5));
    assert!(g.full(3));
}

#[test]
fn codes_are_six_unambiguous_letters_and_typing_is_forgiving() {
    for _ in 0..200 {
        let c = random_code();
        assert_eq!(c.len(), CODE_LEN);
        assert!(c.bytes().all(|b| CODE_CHARS.contains(&b)), "{c}");
        assert!(!c.contains(['0', 'O', '1', 'I', 'L']), "{c}");
    }
    assert_eq!(normalize_code(" k7q-m4x "), "K7QM4X");
    let mut inner = Inner::default();
    inner.groups.insert("g_1".into(), group());
    assert!(inner.by_code("k7q m4x").is_some());
    assert!(inner.by_code("K7QM4").is_none());
    assert_ne!(inner.new_code(), "K7QM4X");
}

#[test]
fn a_saved_group_comes_back_paused_with_its_members_and_a_higher_rev() {
    let d = encore_core::tmp::dir("group-save");
    let db = Db::open(&d).unwrap();
    for (id, dur) in [(1, 200.0), (2, 180.0), (3, 240.0)] {
        db.execute("INSERT INTO tracks (id, path, title, artist, album, album_id, duration, format) VALUES (?,?,?,?,?,?,?,?)",
                   p![id, format!("/m/{id}.flac"), format!("Song {id}"), "Band", "Record", "al1", dur, "flac"]).unwrap();
    }
    let mut g = group();
    g.visibility = LINK.into();
    g.everyone_controls = true;
    g.guests_allowed = true;
    g.max_members = 8;
    let sam = g.add(account(5, "Sam", "phone", 1001.0));
    let kim = g.add(guest("Kim", 1002.0));
    cmd(&mut g, "next", json!({}), T0 + 1000).unwrap();
    g.changed((T0 + 1000) as f64 / 1000.0);
    let rev = g.rev;
    let qids_before = qids(&g);
    // saved 63.25 s into song 2 while playing
    save(&db, &[g.saved(T0 + 1000 + 63_250)], &[]).unwrap();
    // a song left the library meanwhile
    db.execute("DELETE FROM tracks WHERE id=1", p![]).unwrap();
    let later = T0 + 3_600_000;
    let back = load(&db, later, |u| format!("/api/users/{u}/avatar"));
    assert_eq!(back.len(), 1);
    let b = &back[0];
    assert_eq!((b.id.as_str(), b.code.as_str(), b.visibility.as_str(), b.everyone_controls, b.guests_allowed, b.max_members), ("g_1", "K7QM4X", LINK, true, true, 8));
    assert!(!b.playing, "restored paused");
    assert_eq!((b.index, b.position, b.anchor_ms), (0, 63.25, later), "song 1 left out: song 2 is now at index 0");
    assert_eq!(b.queue.iter().map(|e| e.qid.clone()).collect::<Vec<_>>(), qids_before[1..].to_vec(), "the qids are kept");
    assert_eq!(b.rev, rev + REV_AFTER_RESTORE);
    let names: Vec<_> = b.members.iter().map(|m| (m.member.clone(), m.name.clone(), m.host, m.is_guest(), m.device_id.clone())).collect();
    assert_eq!(names, vec![("m1".into(), "Alex".into(), true, false, "pc".into()), (sam.clone(), "Sam".into(), false, false, "phone".into()), (kim, "Kim".into(), false, true, String::new())]);
    assert_eq!(b.members[0].avatar, "/api/users/3/avatar");
    assert!(b.members.iter().all(|m| m.gone_since == Some(later as f64 / 1000.0) && m.streams == 0), "every member has 2 minutes to come back");
    assert_eq!(b.members[2].expires_at, Some(1002.0 + GUEST_SECS));
    // the you of a device is as before
    assert_eq!(b.you(&Caller::Account { user: 5, device: "phone".into() }).member, sam);
    // new members and entries do not reuse ids
    let mut b = back.into_iter().next().unwrap();
    assert_eq!(b.add(account(9, "Lee", "web", 2000.0)), "m4");
    // idle for 12 h: not restored, and its rows are gone
    let rows = db.query("SELECT * FROM group_listens", p![]).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(load(&db, T0 + 1000 + (IDLE_SECS as i64 + 10) * 1000, |_| String::new()).is_empty());
    assert!(db.query("SELECT * FROM group_listens", p![]).unwrap().is_empty());
    assert!(db.query("SELECT * FROM group_listen_members", p![]).unwrap().is_empty());
    // deleting a group's rows and clearing all
    save(&db, &[b.saved(T0)], &[]).unwrap();
    save(&db, &[], &["g_1".into()]).unwrap();
    assert!(db.query("SELECT * FROM group_listens", p![]).unwrap().is_empty());
    save(&db, &[b.saved(T0)], &[]).unwrap();
    clear(&db).unwrap();
    assert!(db.query("SELECT * FROM group_listen_members", p![]).unwrap().is_empty());
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn saving_waits_for_quiet_but_not_forever_and_playing_groups_are_saved_now_and_then() {
    let mut g = group();
    g.unsaved = None;
    g.saved_at = 100.0;
    g.changed(100.0);
    assert!(!g.save_due(101.0));
    assert!(g.save_due(102.0), "2 s of quiet");
    for t in 101..110 {
        g.changed(t as f64);
    }
    assert!(!g.save_due(109.5), "changing all the time: not yet");
    assert!(g.save_due(110.0), "10 s after the first unsaved change at the latest");
    g.unsaved = None;
    g.saved_at = 200.0;
    assert!(g.playing);
    assert!(!g.save_due(209.0));
    assert!(g.save_due(210.0));
    g.playing = false;
    assert!(!g.save_due(300.0), "a paused group with nothing new is not saved again");
}
