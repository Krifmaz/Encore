//! The relay with the real engine: two songs (files) played through Cadenza's null output, which
//! paces itself like a device and keeps every sample it is given. Checks where a load starts,
//! that the next song follows without a gap, that a seek is spliced in where the group is, that a
//! correction moves the song on faster than the clock, and that none of it clicks.

use super::*;
use cadenza::config::Settings;
use cadenza::engine::{EngineOptions, PlayState};
use cadenza_dsp::CoefficientCache;
use std::path::Path;

const RATE: u32 = 44_100;
const FREQ: f64 = 440.0;
const AMP: f64 = 0.5;

/// A song of `secs` seconds: a 440 Hz tone that fades in and out over 50 ms (so the change of
/// song is quiet and any step heard in the output is the relay's).
fn write_song(path: &Path, secs: f64) {
    let spec = hound::WavSpec { channels: 2, sample_rate: RATE, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    let n = (f64::from(RATE) * secs) as usize;
    let edge = (f64::from(RATE) * 0.05) as usize;
    for i in 0..n {
        let env = (i.min(n - 1 - i) as f64 / edge as f64).min(1.0);
        let s = AMP * env * (2.0 * std::f64::consts::PI * FREQ * i as f64 / f64::from(RATE)).sin();
        let v = (s * 32767.0).round() as i16;
        w.write_sample(v).unwrap();
        w.write_sample(v).unwrap();
    }
    w.finalize().unwrap();
}

fn song(path: &Path, title: &str, secs: f64) -> Track {
    Track {
        id: Some("1".into()),
        source: Source::Path { path: path.to_path_buf() },
        title: title.into(),
        duration: Some(secs),
        ..Default::default()
    }
}

fn wait_for(what: &str, secs: f64, mut ok: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs_f64(secs);
    while !ok() {
        assert!(Instant::now() < until, "waited too long for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The engine's position as a status fresh from the engine says it (it publishes twice a second),
/// and the group's position `aim` at that moment.
fn fresh(e: &Engine, aim: &Aim) -> (f64, f64) {
    let old = e.status();
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        let st = e.status();
        if !Arc::ptr_eq(&st, &old) || Instant::now() > until {
            return (st.position, aim.at(mono_ms()));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn songs_play_through_the_relay_gaplessly_seek_without_a_click_and_catch_up() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("a.wav"), dir.path().join("b.wav"));
    write_song(&a, 5.0);
    write_song(&b, 14.0);
    let capture = dir.path().join("out.raw");
    let mut s = Settings::default();
    s.output.backend = "null".into();
    s.output.device = format!("capture:{}", capture.display());
    s.dsp.bit_perfect = true;
    let engine = Engine::start(s, EngineOptions { cache: CoefficientCache::disabled(), realtime: true, persist: None });
    let relay = Relay::start(engine.clone());
    let (ta, tb) = (song(&a, "A", 5.0), song(&b, "B", 14.0));
    relay.add(vec![(live_id("rt-a"), ta.clone()), (live_id("rt-b"), tb.clone())]);
    let queue = vec![live_track(&ta, "rt-a"), live_track(&tb, "rt-b")];

    // the group is 2 s into song A: the load starts there
    let aim = Aim { pos: 2.0, at_ms: mono_ms(), playing: true, lag_ms: 0 };
    relay.load(live_id("rt-a"), aim, Then::Queue { tracks: queue, play: true });
    wait_for("the load", 10.0, || !relay.busy() && engine.status().state == PlayState::Playing);
    std::thread::sleep(Duration::from_millis(700));
    let (got, want) = fresh(&engine, &aim);
    eprintln!("load: here {got:.3} s, the group {want:.3} s");
    assert!((got - want).abs() < 0.25, "started at {got:.2}, the group at {want:.2}");

    // song A ends: the engine runs on into song B, which the relay had ready
    wait_for("song B", 8.0, || engine.status().index == Some(1));
    let st = engine.status();
    assert_eq!(st.state, PlayState::Playing);
    assert!(st.error.is_none(), "{:?}", st.error);

    // a seek in song B, 4 s ahead: spliced in where the group is while the song plays on
    std::thread::sleep(Duration::from_millis(500));
    let aim = Aim { pos: 5.0, at_ms: mono_ms(), playing: true, lag_ms: 0 };
    relay.seek(Some(live_id("rt-b")), 1, aim, false);
    wait_for("the seek", 10.0, || !relay.busy());
    std::thread::sleep(Duration::from_millis(1200));
    let (got, want) = fresh(&engine, &aim);
    eprintln!("seek: here {got:.3} s, the group {want:.3} s");
    assert!((got - want).abs() < 0.25, "after the seek at {got:.2}, the group at {want:.2}");

    // 400 ms behind: slices catch up, the song runs faster than the clock (at most 2 %)
    let clock = Aim { pos: 0.0, at_ms: mono_ms(), playing: true, lag_ms: 0 };
    let (p0, c0) = fresh(&engine, &clock);
    relay.correct(-400);
    std::thread::sleep(Duration::from_millis(2600));
    let (p1, c1) = fresh(&engine, &clock);
    let (gained, dt) = ((p1 - p0) - (c1 - c0), c1 - c0);
    eprintln!("slices: {:.3} s gained in {dt:.2} s", gained);
    assert!(gained > 0.01 && gained < 0.02 * dt + 0.03, "gained {gained:.3} s in {dt:.2} s");
    relay.correct(0);

    // what was played up to here (the pause writes nothing more)
    engine.send(Command::Pause);
    wait_for("the pause", 3.0, || engine.status().state == PlayState::Paused);
    std::thread::sleep(Duration::from_millis(100));
    let played = engine.status().output_frames as usize;

    // out of the group: the real songs are back in the engine's queue
    relay.leave(&engine, &engine.status());
    wait_for("the real songs", 5.0, || engine.status().queue.iter().all(|t| live_of(t).is_none()));
    assert_eq!(engine.status().queue[0].source, ta.source);
    engine.shutdown();
    std::thread::sleep(Duration::from_millis(300));

    // what was played: never a step larger than the tone's own (the slices and the splice are
    // crossfaded; the songs start and end quietly)
    let bytes = std::fs::read(&capture).unwrap();
    let words: Vec<f64> = bytes.as_chunks::<4>().0.iter().map(|c| f64::from(i32::from_le_bytes(*c))).collect();
    let left: Vec<f64> = words.iter().step_by(2).take(played).copied().collect();
    assert!(left.len() > 44_100 * 6, "only {} frames", left.len());
    let peak = left.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    assert!(peak > 0.0, "nothing was played");
    let natural = peak * 2.0 * std::f64::consts::PI * FREQ / f64::from(RATE);
    eprintln!("{} frames played", left.len());
    let (at, worst) = left.windows(2).map(|w| (w[1] - w[0]).abs()).enumerate().fold((0, 0.0), |m, (i, d)| if d > m.1 { (i, d) } else { m });
    let near: Vec<String> = left[at.saturating_sub(4)..(at + 5).min(left.len())].iter().map(|v| format!("{:.3}", v / peak)).collect();
    assert!(
        worst <= natural * 1.3,
        "a step of {:.4} of full scale (the tone's own: {:.4}) at {:.3} s: {}",
        worst / peak,
        natural / peak,
        at as f64 / f64::from(RATE),
        near.join(" ")
    );
}

/// A song of `secs` seconds whose right channel says where in the song it is (a slow ramp, 0.9 of
/// full scale at the end) under the left channel's tone, so what was played can be placed.
fn write_ramp_song(path: &Path, secs: f64) {
    let spec = hound::WavSpec { channels: 2, sample_rate: RATE, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    let n = (f64::from(RATE) * secs) as usize;
    for i in 0..n {
        let t = i as f64 / f64::from(RATE);
        let l = AMP * (2.0 * std::f64::consts::PI * FREQ * t).sin();
        let r = 0.9 * t / secs;
        w.write_sample((l * 32767.0).round() as i16).unwrap();
        w.write_sample((r * 32767.0).round() as i16).unwrap();
    }
    w.finalize().unwrap();
}

/// A paused group moved to another spot, and a group playing again far from where this computer
/// paused: the relay starts the engine's stream afresh at the new spot, so the engine's position
/// says it while still paused and, once it plays, nothing of the old spot is heard (the stream's
/// buffered audio from before the pause is gone).
#[test]
fn a_seek_while_paused_starts_afresh_and_nothing_of_the_old_spot_is_heard() {
    const SECS: f64 = 20.0;
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("ramp.wav");
    write_ramp_song(&a, SECS);
    let capture = dir.path().join("out.raw");
    let mut s = Settings::default();
    s.output.backend = "null".into();
    s.output.device = format!("capture:{}", capture.display());
    s.dsp.bit_perfect = true;
    let engine = Engine::start(s, EngineOptions { cache: CoefficientCache::disabled(), realtime: true, persist: None });
    let relay = Relay::start(engine.clone());
    let ta = song(&a, "Ramp", SECS);
    relay.add(vec![(live_id("rp-a"), ta.clone())]);
    let aim = Aim { pos: 3.0, at_ms: mono_ms(), playing: true, lag_ms: 0 };
    relay.load(live_id("rp-a"), aim, Then::Queue { tracks: vec![live_track(&ta, "rp-a")], play: true });
    wait_for("the load", 10.0, || !relay.busy() && engine.status().state == PlayState::Playing);
    std::thread::sleep(Duration::from_millis(900));

    // the group pauses here (about 4 s in)
    engine.send(Command::Pause);
    wait_for("the pause", 3.0, || engine.status().state == PlayState::Paused);
    std::thread::sleep(Duration::from_millis(300));
    let before = engine.status().position;
    assert!(before > 3.0 && before < 6.0, "paused at {before:.2}");

    // the host moves the paused group to 12 s: the engine says so at once, still paused
    relay.seek(Some(live_id("rp-a")), 0, Aim { pos: 12.0, at_ms: mono_ms(), playing: false, lag_ms: 0 }, false);
    wait_for("the seek", 10.0, || !relay.busy());
    wait_for("the new spot in the status", 3.0, || (engine.status().position - 12.0).abs() < 0.5);
    assert_eq!(engine.status().state, PlayState::Paused);
    std::thread::sleep(Duration::from_millis(100));
    let p0 = engine.status().output_frames as usize;

    // the group plays again: from the new spot
    engine.send(Command::Play);
    std::thread::sleep(Duration::from_millis(1000));
    engine.send(Command::Pause);
    wait_for("the second pause", 3.0, || engine.status().state == PlayState::Paused);
    std::thread::sleep(Duration::from_millis(100));
    let p1 = engine.status().output_frames as usize;

    // playing again far from where it paused (a long "Paused here"): the seek is made while paused
    // and the engine plays from the group's spot
    let aim = Aim { pos: 5.0, at_ms: mono_ms(), playing: true, lag_ms: 0 };
    relay.seek(Some(live_id("rp-a")), 0, aim, true);
    wait_for("the seek that plays", 10.0, || !relay.busy() && engine.status().state == PlayState::Playing);
    std::thread::sleep(Duration::from_millis(800));
    engine.send(Command::Pause);
    wait_for("the last pause", 3.0, || engine.status().state == PlayState::Paused);
    std::thread::sleep(Duration::from_millis(100));
    let p2 = engine.status().output_frames as usize;
    engine.shutdown();
    std::thread::sleep(Duration::from_millis(300));

    // where in the song each frame played from: the ramp against the tone's level
    let bytes = std::fs::read(&capture).unwrap();
    let words: Vec<f64> = bytes.as_chunks::<4>().0.iter().map(|c| f64::from(i32::from_le_bytes(*c))).collect();
    let frames: Vec<(f64, f64)> = words.as_chunks::<2>().0.iter().map(|f| (f[0], f[1])).collect();
    let rate = RATE as usize;
    let place = |from: usize, to: usize, lo: f64, hi: f64, what: &str| {
        assert!(to <= frames.len() && to > from + rate / 2, "{what}: frames {from}..{to} of {}", frames.len());
        let peak = frames[from..to].iter().fold(0.0f64, |m, f| m.max(f.0.abs()));
        assert!(peak > 0.0, "{what}: nothing played");
        // past the 20 ms fade-in from silence, 0.6 s of it
        for (i, f) in frames[from + rate * 3 / 100..(from + rate * 6 / 10).min(to)].iter().enumerate() {
            let t = f.1 / peak * (AMP / 0.9) * SECS;
            assert!(t > lo && t < hi, "{what}: {:.3} s after it played, a frame from {t:.2} s of the song (wanted {lo}..{hi})", (i + rate * 3 / 100) as f64 / RATE as f64);
        }
    };
    place(p0, p1, 11.8, 14.0, "after the paused group's seek");
    place(p1, p2, 4.8, 7.0, "after playing again far off");
}
