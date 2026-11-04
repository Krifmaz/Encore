//! Tests of the loudness columns: the album aggregate, the stamp on track rows, the signal path's `gain`,
//! what the scan writes, and the "Measure loudness" task going on where it stopped.
use super::*;
use encore_db::{p, Db};
use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::{ItemKey, Tag, TagType};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

fn song(album: &str, track: Option<f64>, peak: Option<f64>, album_gain: Option<f64>, album_peak: Option<f64>, lufs: Option<f64>) -> SongLoudness {
    SongLoudness { album_id: album.into(), track_gain: track, track_peak: peak, album_gain, album_peak, lufs, ..Default::default() }
}

fn agg(songs: &[SongLoudness]) -> AlbumLoudness {
    let mut a = AlbumLoudness::default();
    songs.iter().for_each(|s| a.add(s));
    a
}

fn close(a: Option<f64>, b: f64) -> bool {
    a.is_some_and(|a| (a - b).abs() < 1e-9)
}

/// The album's gain and peak (client-shapes §2.1): the tags' album values when every song has them, else
/// the energy mean of the songs' loudness once every song has one; omitted while one is missing.
#[test]
fn the_album_aggregate() {
    // every song has the album tags: each song's own album gain and peak
    let a = song("x", Some(-7.21), Some(0.988), Some(-6.84), Some(1.0), Some(-10.79));
    let b = song("x", Some(-6.5), Some(0.9), Some(-6.84), Some(1.0), Some(-11.5));
    let ag = agg(&[a.clone(), b]);
    assert_eq!(ag.for_song(&a), (Some(-6.84), Some(1.0)));
    // one song without an album peak tag: the loudest track peak instead
    let b2 = song("x", Some(-6.5), Some(0.9), Some(-6.84), None, Some(-11.5));
    assert_eq!(agg(&[a.clone(), b2]).for_song(&a), (Some(-6.84), Some(0.988)));
    // one song without the album gain tag: the energy mean of the loudness, tagged and measured alike:
    // -18 - 10·log10((10^-1 + 10^-2) / 2)
    let c = song("x", Some(-8.0), Some(0.5), None, None, Some(-10.0));
    let d = song("x", Some(2.0), None, None, None, Some(-20.0));
    let (g, pk) = agg(&[c.clone(), d.clone()]).for_song(&d);
    let want = -18.0 - 10.0 * ((0.1f64 + 0.01) / 2.0).log10();
    assert!(close(g, want), "{g:?} vs {want}");
    assert!((want - -5.4036).abs() < 1e-3);
    assert_eq!(pk, Some(0.5), "the album peak is the loudest track peak known");
    // tags on some songs only: the energy mean, not the tags
    let e = song("x", Some(-8.0), None, Some(-3.0), None, Some(-10.0));
    let (g, _) = agg(&[e.clone(), d]).for_song(&e);
    assert!(close(g, want));
    // a song not measured yet: no album gain; the gain object keeps the track's own
    let f = song("x", None, None, None, None, None);
    let ag = agg(&[c.clone(), f.clone()]);
    assert_eq!(ag.for_song(&c).0, None);
    let o = gain_object(&c, Some(&ag)).unwrap();
    assert_eq!(Value::Object(o), json!({ "track": -8.0, "track_peak": 0.5, "album_peak": 0.5 }));
    // nothing known: no gain object at all
    assert!(gain_object(&f, Some(&ag)).is_none());
    assert!(gain_object(&f, None).is_none());
    // only the album tags (a file with album tags only, all its album's songs too): the album alone
    let g1 = song("y", None, None, Some(-5.0), Some(0.7), None);
    let ag = agg(&[g1.clone(), g1.clone()]);
    assert_eq!(Value::Object(gain_object(&g1, Some(&ag)).unwrap()), json!({ "album": -5.0, "album_peak": 0.7 }));
    // a single song album measured: album = track
    let h = song("z", Some(-4.5), Some(0.8), None, None, Some(-13.5));
    assert_eq!(Value::Object(gain_object(&h, Some(&agg(std::slice::from_ref(&h)))).unwrap()), json!({ "track": -4.5, "album": -4.5, "track_peak": 0.8, "album_peak": 0.8 }));
}

/// What the scan writes from the tags: a track gain makes it 'tags' (loudness −18 − gain); no track gain
/// keeps a measurement of the same audio, else everything but the album tags is cleared.
#[test]
fn the_columns_the_scan_writes() {
    let tags = GainTags { track_gain: Some(-7.21), track_peak: Some(0.988123), album_gain: Some(-6.84), album_peak: Some(1.0) };
    assert_eq!(tag_values(&tags), [json!(-7.21), json!(0.9881), json!(-6.84), json!(1.0), json!(-10.79), json!("tags")]);
    let album_only = GainTags { album_gain: Some(-6.84), ..Default::default() };
    assert_eq!(tag_values(&album_only), [Value::Null, Value::Null, json!(-6.84), Value::Null, Value::Null, Value::Null]);
    assert!(matches!(tag_columns(&album_only, true), TagColumns::AlbumOnly(v) if v == [json!(-6.84), Value::Null]));
    assert!(matches!(tag_columns(&tags, true), TagColumns::All(_)), "a track gain tag wins over a measurement");
    assert!(keeps_measurement("measured", 200.0, 200.3));
    assert!(!keeps_measurement("measured", 200.0, 230.0));
    assert!(!keeps_measurement("tags", 200.0, 200.0));
    assert!(!keeps_measurement("none", 200.0, 200.0));
}

fn db(name: &str) -> (Db, std::path::PathBuf) {
    let d = encore_core::tmp::dir(name);
    (Db::open(&d.join("cfg")).unwrap(), d)
}

#[allow(clippy::too_many_arguments)]
fn insert(db: &Db, id: i64, album: &str, track: Option<f64>, peak: Option<f64>, ag: Option<f64>, ap: Option<f64>, lufs: Option<f64>, source: Option<&str>, rate: Option<i64>, bits: Option<i64>) {
    db.execute("INSERT INTO tracks (id, path, title, artist, album, albumartist, album_id, format, duration, mtime, rg_track_gain, rg_track_peak, rg_album_gain, rg_album_peak, loudness_lufs, loudness_source, sample_rate, bit_depth)
                VALUES (?, ?, ?, 'A', 'Alb', 'A', ?, 'flac', 100, 1, ?, ?, ?, ?, ?, ?, ?, ?)",
        p![id, format!("/m/{id}.flac"), format!("Song {id}"), album, track, peak, ag, ap, lufs, source, rate, bits]).unwrap();
}

/// Every track row in an answer gets `gain`, `sample_rate` and `bit_depth` (when known), wherever it sits:
/// lists, nested `item`s, catalogue items' `library`, playlist entries (by `track_id`); raw columns of a
/// `SELECT t.*` give way to `gain`; anything that is not a library row is left alone; twice is once.
#[test]
fn the_stamp_on_track_rows() {
    let (db, d) = db("loudness-stamp");
    insert(&db, 1, "al1", Some(-7.21), Some(0.988), Some(-6.84), Some(1.0), Some(-10.79), Some("tags"), Some(96000), Some(24));
    insert(&db, 2, "al1", Some(-6.5), Some(0.95), Some(-6.84), Some(0.99), Some(-11.5), Some("tags"), Some(96000), Some(24));
    insert(&db, 3, "al2", Some(-4.0), Some(0.5), None, None, Some(-14.0), Some("measured"), Some(44100), None);
    insert(&db, 4, "al2", None, None, None, None, None, None, Some(44100), None);
    insert(&db, 5, "al3", None, None, None, None, None, Some("none"), None, None);
    let row = |id: i64| json!({ "id": id, "title": format!("Song {id}"), "artist": "A", "album_id": format!("al{}", (id + 1) / 2), "format": "flac", "duration": 100 });
    let mut v = json!({
        "songs": [row(1), row(3), row(4), row(5)],
        "top": { "kind": "song", "item": row(2) },
        // a playlist entry: its own id is the entry's; its song is track_id
        "items": [{ "id": 3, "track_id": 1, "title": "Song 1", "album_id": "al1", "format": "flac" },
                  { "id": 9, "track_id": null, "title": "Not in the library", "album_id": null, "format": null }],
        // a catalogue song with its library copy
        "tracks": [{ "id": "mbid-1", "title": "Song 1", "album_id": "rg", "format": "flac", "library": row(1) }],
        // not track rows: a request (no album or format), an album
        "requests": [{ "id": 1, "title": "Song 1", "artist": "A" }],
        "albums": [{ "id": "al1", "album": "Alb", "album_id": "al1" }],
        // what `SELECT t.*` brings along
        "raw": [{ "id": 2, "title": "Song 2", "album_id": "al1", "format": "flac", "rg_track_gain": -6.5, "rg_track_peak": 0.95, "rg_album_gain": -6.84,
                  "rg_album_peak": 0.99, "loudness_lufs": -11.5, "loudness_source": "tags", "sample_rate": 96000, "bit_depth": 24 }],
    });
    stamp(&db, &mut v);
    assert_eq!(v["songs"][0]["gain"], json!({ "track": -7.21, "album": -6.84, "track_peak": 0.988, "album_peak": 1.0 }));
    assert_eq!((v["songs"][0]["sample_rate"].clone(), v["songs"][0]["bit_depth"].clone()), (json!(96000), json!(24)));
    // album 2: one song not measured yet, so no album gain; the track's own values; no bit depth (lossy-like)
    assert_eq!(v["songs"][1]["gain"], json!({ "track": -4.0, "track_peak": 0.5, "album_peak": 0.5 }));
    assert_eq!(v["songs"][1]["sample_rate"], 44100);
    assert!(v["songs"][1].get("bit_depth").is_none());
    assert!(v["songs"][2].get("gain").is_none(), "nothing known: no gain");
    assert!(v["songs"][3].get("gain").is_none() && v["songs"][3].get("sample_rate").is_none());
    assert_eq!(v["top"]["item"]["gain"], json!({ "track": -6.5, "album": -6.84, "track_peak": 0.95, "album_peak": 0.99 }));
    assert_eq!(v["items"][0]["gain"]["track"], -7.21, "a playlist entry gets its song's gain, not entry 3's");
    assert!(v["items"][1].get("gain").is_none());
    assert_eq!(v["tracks"][0]["library"]["gain"]["track"], -7.21);
    assert!(v["tracks"][0].get("gain").is_none(), "the catalogue song itself is not a library row");
    assert!(v["requests"][0].get("gain").is_none() && v["albums"][0].get("gain").is_none());
    let raw = v["raw"][0].as_object().unwrap();
    assert!(COLUMNS.iter().all(|c| !raw.contains_key(*c)), "{raw:?}");
    assert_eq!(raw["gain"]["track"], -6.5);
    assert_eq!(raw["sample_rate"], 96000);
    // stamped twice: the same
    let once = v.clone();
    stamp(&db, &mut v);
    assert_eq!(v, once);
    // nothing to stamp: untouched
    let mut plain = json!({ "title": "x" });
    stamp(&db, &mut plain);
    assert_eq!(plain, json!({ "title": "x" }));
    let _ = std::fs::remove_dir_all(&d);
}

/// The signal path's `gain`: the four numbers with `source` and `lufs`; nothing when nothing is known.
#[test]
fn the_signal_path_gain() {
    let (db, d) = db("loudness-info");
    insert(&db, 1, "al1", Some(-4.0), Some(0.5), None, None, Some(-14.0), Some("measured"), None, None);
    insert(&db, 2, "al2", Some(-7.21), Some(0.988), Some(-6.84), Some(1.0), Some(-10.79), Some("tags"), None, None);
    insert(&db, 3, "al3", None, None, Some(-5.0), None, None, Some("none"), None, None);
    insert(&db, 4, "al4", None, None, None, None, None, None, None, None);
    assert_eq!(info_gain(&db, 1).unwrap(), json!({ "track": -4.0, "album": -4.0, "track_peak": 0.5, "album_peak": 0.5, "source": "measured", "lufs": -14.0 }));
    assert_eq!(info_gain(&db, 2).unwrap(), json!({ "track": -7.21, "album": -6.84, "track_peak": 0.988, "album_peak": 1.0, "source": "tags", "lufs": -10.79 }));
    // album tags on a song that could not be measured: the album, without a source
    assert_eq!(info_gain(&db, 3).unwrap(), json!({ "album": -5.0 }));
    assert!(info_gain(&db, 4).is_none());
    assert!(info_gain(&db, 99).is_none());
    let _ = std::fs::remove_dir_all(&d);
}

/// `seconds` of a sine as FLAC (ffmpeg), None when ffmpeg isn't installed.
fn flac(path: &Path, rate: u32, secs: f64) -> Option<()> {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let ok = std::process::Command::new("ffmpeg").args(["-v", "error", "-nostdin", "-y", "-f", "lavfi", "-i", &format!("sine=f=440:d={secs}:sample_rate={rate}"), "-c:a", "flac"])
        .arg(path).status().is_ok_and(|s| s.success());
    if !ok {
        eprintln!("ffmpeg is not installed: skipped");
    }
    ok.then_some(())
}

/// Sets (or, with an empty value, removes) Vorbis comments of a FLAC file with lofty.
fn set_tags(path: &Path, items: &[(ItemKey, &str)]) {
    let mut f = lofty::read_from_path(path).unwrap();
    if f.tag(TagType::VorbisComments).is_none() {
        f.insert_tag(Tag::new(TagType::VorbisComments));
    }
    let t = f.tag_mut(TagType::VorbisComments).unwrap();
    for (k, v) in items {
        if v.is_empty() {
            t.remove_key(*k);
        } else {
            t.insert_text(*k, v.to_string());
        }
    }
    f.save_to_path(path, WriteOptions::default()).unwrap();
}

fn cols(db: &Db, path: &Path) -> Row {
    db.one("SELECT rg_track_gain, rg_track_peak, rg_album_gain, rg_album_peak, loudness_lufs, loudness_source, sample_rate, bit_depth FROM tracks WHERE path=?",
        p![path.to_string_lossy()]).unwrap().unwrap()
}

/// The scan reads the loudness tags, the sample rate and the bit depth; a file whose tags changed keeps its
/// measurement while its audio is the same length, and is measured again when it is not.
#[test]
fn the_scan_writes_loudness_and_properties() {
    let d = encore_core::tmp::dir("loudness-scan");
    let db = Db::open(&d.join("cfg")).unwrap();
    let a = d.join("music/Ann/Alb/01.flac");
    if flac(&a, 96000, 1.0).is_none() {
        return;
    }
    set_tags(&a, &[(ItemKey::TrackTitle, "One"), (ItemKey::ReplayGainTrackGain, "-7.21 dB"), (ItemKey::ReplayGainTrackPeak, "0.988"),
                   (ItemKey::ReplayGainAlbumGain, "-6.84 dB"), (ItemKey::ReplayGainAlbumPeak, "1.0")]);
    crate::index_file(&db, &a.to_string_lossy()).unwrap();
    let r = cols(&db, &a);
    assert_eq!((r.f("rg_track_gain"), r.f("rg_track_peak"), r.f("rg_album_gain"), r.f("rg_album_peak"), r.f("loudness_lufs"), r.s("loudness_source")),
               (-7.21, 0.988, -6.84, 1.0, -10.79, "tags".to_string()));
    assert_eq!((r.i("sample_rate"), r.i("bit_depth")), (96000, 16));
    // the track tags go, the album ones stay: measured again, the album tags kept
    set_tags(&a, &[(ItemKey::ReplayGainTrackGain, ""), (ItemKey::ReplayGainTrackPeak, "")]);
    crate::index_file(&db, &a.to_string_lossy()).unwrap();
    let r = cols(&db, &a);
    assert!(r.get("loudness_source").is_some_and(Value::is_null) && r.get("rg_track_gain").is_some_and(Value::is_null), "{r:?}");
    assert_eq!(r.f("rg_album_gain"), -6.84);
    // measured; then a tag edit (same audio): the measurement stays
    db.execute("UPDATE tracks SET rg_track_gain=-4, rg_track_peak=0.5, loudness_lufs=-14, loudness_source='measured' WHERE path=?", p![a.to_string_lossy()]).unwrap();
    set_tags(&a, &[(ItemKey::TrackTitle, "One (edited)"), (ItemKey::ReplayGainAlbumGain, "-5.00 dB")]);
    crate::index_file(&db, &a.to_string_lossy()).unwrap();
    let r = cols(&db, &a);
    assert_eq!((r.f("rg_track_gain"), r.s("loudness_source"), r.f("rg_album_gain")), (-4.0, "measured".to_string(), -5.0));
    // other audio of another length at the same path: measured again
    flac(&a, 44100, 3.0).unwrap();
    crate::index_file(&db, &a.to_string_lossy()).unwrap();
    let r = cols(&db, &a);
    assert!(r.get("loudness_source").is_some_and(Value::is_null), "{r:?}");
    assert_eq!(r.i("sample_rate"), 44100);
    let _ = std::fs::remove_dir_all(&d);
}

/// The task goes on where it stopped: a run that the busy gate stops leaves the rest for the next run, which
/// measures nothing twice; tags are read first (no ffmpeg for a tagged file); the quality scan's
/// measurement of the same file is taken; a file that can't be measured is marked and not tried again;
/// a file that changed while it was measured is left for the next run.
#[tokio::test]
async fn the_task_goes_on_where_it_stopped() {
    let d = encore_core::tmp::dir("loudness-task");
    let db = Db::open(&d.join("cfg")).unwrap();
    // song 1: a real FLAC with a track gain tag (the tags pass takes it); 2..=7 files to measure (fake)
    let tagged = d.join("music/1.flac");
    if flac(&tagged, 48000, 1.0).is_none() {
        return;
    }
    set_tags(&tagged, &[(ItemKey::ReplayGainTrackGain, "-3.00 dB"), (ItemKey::ReplayGainTrackPeak, "0.7")]);
    for id in 1..=7 {
        let path = if id == 1 { tagged.to_string_lossy().to_string() } else { format!("/nowhere/{id}.flac") };
        db.execute("INSERT INTO tracks (id, path, title, album_id, format, duration, mtime) VALUES (?, ?, ?, 'al', 'flac', 100, 5)", p![id, path, format!("S{id}")]).unwrap();
    }
    // song 4 was measured by the quality scan at this mtime (taken without ffmpeg, busy or not); song 5's
    // report is of an older file
    db.execute("INSERT INTO track_quality (track_id, status, issues, summary, lufs, true_peak, mtime, checked_at) VALUES (4, 'ok', '[]', '', -12.0, -1.0, 5, 1), (5, 'ok', '[]', '', -30.0, -1.0, 2, 1)", p![]).unwrap();
    let calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
    let busy = Busy::default();
    let stop_after = Arc::new(AtomicUsize::new(2));
    let fake = |calls: Arc<Mutex<Vec<String>>>, busy: Busy, stop_after: Arc<AtomicUsize>, db: Db| move |path: PathBuf| {
        let (calls, busy, stop_after, db) = (calls.clone(), busy.clone(), stop_after.clone(), db.clone());
        async move {
            let p = path.to_string_lossy().to_string();
            calls.lock().unwrap().push(p.clone());
            // somebody starts using the app after this many files
            if stop_after.fetch_sub(1, Ordering::SeqCst) == 1 {
                busy.touch();
            }
            match p.as_str() {
                "/nowhere/6.flac" => (None, None),
                // the file changes while it is measured: the scan wrote the row again
                "/nowhere/7.flac" => {
                    db.execute("UPDATE tracks SET mtime=9 WHERE id=7", p![]).unwrap();
                    (Some(-9.0), Some(-0.5))
                }
                _ => (Some(-14.0), Some(-1.0)),
            }
        }
    };
    let lim = Limits { slice: 2, budget: Duration::from_secs(60), busy_max: Duration::from_millis(50) };
    let first = run(&db, &busy, true, fake(calls.clone(), busy.clone(), stop_after.clone(), db.clone()), lim.clone()).await.unwrap();
    assert_eq!(first.stopped, Some(Stop::Busy), "{first:?}");
    assert_eq!(first.from_tags, 1);
    assert_eq!(*calls.lock().unwrap(), vec!["/nowhere/2.flac", "/nowhere/3.flac"], "two measured, then the app was in use");
    assert_eq!((first.measured, first.reused, first.failed), (2, 1, 0));
    assert_eq!(first.left, 3);
    let src = |id: i64| db.one("SELECT loudness_source FROM tracks WHERE id=?", p![id]).unwrap().unwrap().s("loudness_source");
    assert_eq!((src(2), src(3), src(4), src(5)), ("measured".to_string(), "measured".to_string(), "measured".to_string(), String::new()));
    let r = db.one("SELECT rg_track_gain, rg_track_peak, loudness_lufs FROM tracks WHERE id=2", p![]).unwrap().unwrap();
    assert_eq!((r.f("rg_track_gain"), r.f("rg_track_peak"), r.f("loudness_lufs")), (-4.0, 0.8913, -14.0));
    let r = db.one("SELECT rg_track_gain, loudness_lufs FROM tracks WHERE id=4", p![]).unwrap().unwrap();
    assert_eq!((r.f("rg_track_gain"), r.f("loudness_lufs")), (-6.0, -12.0), "the quality scan's measurement");
    let r = db.one("SELECT rg_track_gain, rg_track_peak, loudness_lufs, loudness_source, sample_rate, bit_depth FROM tracks WHERE id=1", p![]).unwrap().unwrap();
    assert_eq!((r.f("rg_track_gain"), r.f("rg_track_peak"), r.f("loudness_lufs"), r.s("loudness_source"), r.i("sample_rate"), r.i("bit_depth")),
               (-3.0, 0.7, -15.0, "tags".to_string(), 48000, 16), "the tagged file needed no ffmpeg");
    // the next run (the app quiet again) goes on with song 5: nothing measured twice
    calls.lock().unwrap().clear();
    let quiet_busy = Busy::default();
    let second = run(&db, &quiet_busy, true, fake(calls.clone(), quiet_busy.clone(), Arc::new(AtomicUsize::new(0)), db.clone()), lim.clone()).await.unwrap();
    assert_eq!(second.stopped, None, "{second:?}");
    assert_eq!(*calls.lock().unwrap(), vec!["/nowhere/5.flac", "/nowhere/6.flac", "/nowhere/7.flac"], "an older report is not taken");
    assert_eq!((second.measured, second.reused, second.failed, second.from_tags), (1, 0, 1, 0));
    assert_eq!(src(5), "measured");
    assert_eq!(src(6), "none", "could not be measured: marked");
    assert_eq!(src(7), "", "changed while it was measured: left for the next run");
    assert_eq!(second.left, 1);
    assert!(second.line().contains("1 still to measure"), "{}", second.line());
    // the third run: only song 7 (6 is not tried again)
    calls.lock().unwrap().clear();
    let third = run(&db, &quiet_busy, true, fake(calls.clone(), quiet_busy.clone(), Arc::new(AtomicUsize::new(0)), db.clone()), lim.clone()).await.unwrap();
    assert_eq!(*calls.lock().unwrap(), vec!["/nowhere/7.flac"]);
    assert_eq!((third.measured, third.left), (1, 0));
    assert_eq!(src(7), "measured");
    // nothing left: a run does nothing
    calls.lock().unwrap().clear();
    let last = run(&db, &quiet_busy, true, fake(calls.clone(), quiet_busy.clone(), Arc::new(AtomicUsize::new(0)), db.clone()), lim).await.unwrap();
    assert!(calls.lock().unwrap().is_empty());
    assert_eq!(last, Run::default());
    assert_eq!(last.line(), "Nothing left to measure");
    let _ = std::fs::remove_dir_all(&d);
}

/// A run that the busy gate stops inside the tags pass keeps what it read: the next run reads only the songs
/// after it and goes on to measure, so a server in use now and then still gets its songs measured (a tags pass
/// that started again from the first song every run would never get to the measuring).
#[cfg(unix)]
#[tokio::test]
async fn a_run_stopped_in_the_tags_pass_goes_on_from_there() {
    let d = encore_core::tmp::dir("loudness-tags-stop");
    let db = Db::open(&d.join("cfg")).unwrap();
    let real = d.join("music/1.flac");
    if flac(&real, 48000, 1.0).is_none() {
        return;
    }
    // song 2's file is a pipe: opening it waits until the test opens the other end
    let pipe = d.join("music/2.flac");
    assert!(std::process::Command::new("mkfifo").arg(&pipe).status().unwrap().success());
    for id in 1..=6 {
        let path = match id {
            1 => real.to_string_lossy().to_string(),
            2 => pipe.to_string_lossy().to_string(),
            _ => format!("/nowhere/{id}.flac"),
        };
        db.execute("INSERT INTO tracks (id, path, title, album_id, format, duration, mtime) VALUES (?, ?, ?, 'al', 'flac', 100, 5)", p![id, path, format!("S{id}")]).unwrap();
    }
    // somebody starts using the app while the pass waits on song 2; then the pipe is let go (empty)
    let busy = Busy::default();
    let writer = {
        let (pipe, busy) = (pipe.clone(), busy.clone());
        std::thread::spawn(move || {
            use std::os::unix::fs::OpenOptionsExt;
            std::thread::sleep(Duration::from_millis(300));
            busy.touch();
            // (opening the writing end without waiting works once the pass has the pipe open)
            let until = Instant::now() + Duration::from_secs(10);
            while Instant::now() < until {
                if std::fs::OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open(&pipe).is_ok() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        })
    };
    let calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
    let fake = |calls: Arc<Mutex<Vec<String>>>| move |path: PathBuf| {
        let calls = calls.clone();
        async move {
            calls.lock().unwrap().push(path.to_string_lossy().to_string());
            (Some(-14.0), Some(-1.0))
        }
    };
    let lim = Limits { slice: 2, budget: Duration::from_secs(60), busy_max: Duration::from_millis(50) };
    let first = run(&db, &busy, true, fake(calls.clone()), lim.clone()).await.unwrap();
    writer.join().unwrap();
    assert_eq!(first.stopped, Some(Stop::Busy), "{first:?}");
    assert!(calls.lock().unwrap().is_empty(), "stopped in the tags pass: nothing measured");
    let rate = |id: i64| db.one("SELECT sample_rate FROM tracks WHERE id=?", p![id]).unwrap().unwrap().get("sample_rate").and_then(Value::as_i64);
    assert_eq!((1..=6).map(rate).collect::<Vec<_>>(), vec![Some(48000), Some(0), None, None, None, None], "1 and 2 read (0: the file did not say), 3–6 not yet");
    // song 1's file gets a track gain tag now (its row is not scanned again): read again, it would become 'tags'
    set_tags(&real, &[(ItemKey::ReplayGainTrackGain, "-3.00 dB")]);
    let quiet = Busy::default();
    let second = run(&db, &quiet, true, fake(calls.clone()), lim).await.unwrap();
    assert_eq!((second.stopped, second.from_tags, second.measured, second.left), (None, 0, 6, 0), "{second:?}");
    assert_eq!(calls.lock().unwrap().len(), 6);
    assert_eq!((3..=6).map(rate).collect::<Vec<_>>(), vec![Some(0); 4], "3–6 were read this time");
    let src = db.one("SELECT loudness_source FROM tracks WHERE id=1", p![]).unwrap().unwrap().s("loudness_source");
    assert_eq!(src, "measured", "song 1 was not read again");
    let _ = std::fs::remove_dir_all(&d);
}

/// Without ffmpeg only the tags are read; the line says what is missing. A run whose time is up stops.
#[tokio::test]
async fn without_ffmpeg_and_out_of_time() {
    let d = encore_core::tmp::dir("loudness-noffmpeg");
    let db = Db::open(&d.join("cfg")).unwrap();
    for id in 1..=3 {
        db.execute("INSERT INTO tracks (id, path, title, album_id, format, duration, mtime) VALUES (?, ?, 'S', 'al', 'flac', 100, 5)", p![id, format!("/nowhere/{id}.flac")]).unwrap();
    }
    let never = |_: PathBuf| async { panic!("ffmpeg is not there") };
    let r = run(&db, &Busy::default(), false, never, Limits { slice: 2, ..Default::default() }).await.unwrap();
    assert_eq!((r.measured, r.left, r.no_ffmpeg), (0, 3, true));
    assert_eq!(r.line(), "0 measured, 3 still to measure. Measuring needs ffmpeg");
    let r = run(&db, &Busy::default(), true, |_: PathBuf| async { (Some(-14.0), None) }, Limits { slice: 2, budget: Duration::ZERO, ..Default::default() }).await.unwrap();
    assert_eq!((r.stopped, r.measured, r.left), (Some(Stop::Time), 0, 3));
    assert!(r.line().ends_with("(the run's time was up; the next run goes on)"), "{}", r.line());
    let _ = std::fs::remove_dir_all(&d);
}

/// While a group listen plays the measuring waits (no ffmpeg runs), and goes on once the last one stopped – also
/// when the group played longer than the run's time: the wait does not count against it.
#[tokio::test]
async fn measuring_waits_while_a_group_listen_plays() {
    let d = encore_core::tmp::dir("loudness-group");
    let db = Db::open(&d.join("cfg")).unwrap();
    for id in 1..=3 {
        // sample_rate set: the tags pass has nothing to read
        db.execute("INSERT INTO tracks (id, path, title, album_id, format, duration, mtime, sample_rate) VALUES (?, ?, ?, 'al', 'flac', 100, 5, 44100)",
            p![id, format!("/nowhere/{id}.flac"), format!("S{id}")]).unwrap();
    }
    let calls: Arc<Mutex<Vec<(String, bool)>>> = Arc::new(Mutex::new(vec![]));
    let busy = Busy::default();
    let fake = |calls: Arc<Mutex<Vec<(String, bool)>>>, busy: Busy| move |path: PathBuf| {
        let (calls, busy) = (calls.clone(), busy.clone());
        async move {
            calls.lock().unwrap().push((path.to_string_lossy().to_string(), busy.group_playing()));
            (Some(-14.0), Some(-1.0))
        }
    };
    // a group plays three times as long as the run's whole time: the run waits for it, then measures everything
    // (never while it played) with the time it had left
    busy.set_group_playing(true);
    let b2 = busy.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(1500)).await;
        b2.set_group_playing(false);
    });
    let lim = Limits { slice: 2, budget: Duration::from_millis(500), busy_max: Duration::from_millis(50) };
    let t0 = std::time::Instant::now();
    let r = run(&db, &busy, true, fake(calls.clone(), busy.clone()), lim).await.unwrap();
    assert!(t0.elapsed() >= Duration::from_millis(1400), "it waited for the group: {:?}", t0.elapsed());
    assert_eq!((r.stopped, r.measured, r.left), (None, 3, 0), "{r:?}");
    let got = calls.lock().unwrap().clone();
    assert_eq!(got.len(), 3);
    assert!(got.iter().all(|(_, playing)| !playing), "{got:?}");
    let _ = std::fs::remove_dir_all(&d);
}
