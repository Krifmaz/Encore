//! Each user's last playback spot (`playback_spot`, read by `/api/remote/spot`), written from the devices'
//! check-ins, so any device can pick up where another one left off. Apart from the devices hub
//! ([`crate::hub`]): the hub only says whether the device that wrote the spot is online now.
use crate::hub::{Devices, ALIVE};
use encore_core::error::ApiResult;
use encore_db::{now, p, Db, RowExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Check-ins of a playing device write the spot this often (seconds) at most.
pub const SPOT_EVERY: f64 = 10.0;
/// Songs of the queue kept with the spot.
pub const SPOT_QUEUE_MAX: usize = 300;

/// The last spot written for a user, so check-ins only touch the database every SPOT_EVERY seconds
/// (and at once when the song changes or playback stops or starts).
pub struct Last {
    pub at: f64,
    pub track: i64,
    pub playing: bool,
    pub device: String,
}

/// Whether a check-in should write the spot. A playing device writes on its first check-in, on a song
/// change and every SPOT_EVERY seconds. Another playing device takes over once the last writer has
/// stopped or gone quiet for SPOT_EVERY seconds, so two devices playing at once still write at most
/// once per SPOT_EVERY between them. A paused device only writes when it is the one that wrote last and
/// it just paused or changed song, so an app that opens paused on an old song doesn't overwrite what
/// another device is playing.
pub fn spot_due(last: Option<&Last>, device: &str, track: i64, playing: bool, t: f64) -> bool {
    match last {
        None => playing,
        Some(l) if playing && l.device != device => !l.playing || t - l.at >= SPOT_EVERY,
        Some(l) if playing => l.track != track || !l.playing || t - l.at >= SPOT_EVERY,
        Some(l) => l.device == device && (l.playing || l.track != track),
    }
}

/// Library track ids of a check-in's queue (numbers or objects with a numeric `id`), at most
/// SPOT_QUEUE_MAX, and where the current song is in them.
pub fn spot_queue(state: &Value, track: i64) -> (Vec<i64>, usize) {
    let mut ids: Vec<i64> = state["queue"].as_array().map(|q| q.iter()
        .filter_map(|x| x.as_i64().or_else(|| x["id"].as_i64())).take(SPOT_QUEUE_MAX).collect()).unwrap_or_default();
    let at = state["index"].as_u64().map(|i| i as usize).filter(|&i| ids.get(i) == Some(&track))
        .or_else(|| ids.iter().position(|&x| x == track));
    match at {
        Some(i) => (ids, i),
        None => {
            ids = vec![track];
            (ids, 0)
        }
    }
}

/// The last spot written per user. The server keeps one (in [`crate::Playback`]); clones share it.
#[derive(Clone, Default)]
pub struct Spots(Arc<Mutex<HashMap<i64, Last>>>);

impl Spots {
    /// Forget what was written for `user` (the spot was cleared, or writing it failed).
    pub fn forget(&self, user: i64) {
        self.0.lock().unwrap().remove(&user);
    }

    /// Delete the user's spot (`DELETE /api/remote/spot`).
    pub fn clear(&self, db: &Db, user: i64) -> ApiResult<()> {
        db.execute("DELETE FROM playback_spot WHERE user_id = ?", p![user])?;
        self.forget(user);
        Ok(())
    }

    /// Writes the user's spot from a check-in when it is due (see `spot_due`).
    pub fn record(&self, db: &Db, user: i64, device: &str, name: &str, state: &Value) {
        // a remote for another device isn't playing anything itself; only library songs can be picked up elsewhere
        if state["remote_of"].as_str().is_some_and(|r| !r.is_empty()) {
            return;
        }
        let Some(track) = state["id"].as_i64() else { return };
        let playing = state["playing"].as_bool().unwrap_or(false);
        let t = now();
        {
            let mut last = self.0.lock().unwrap();
            if !spot_due(last.get(&user), device, track, playing, t) {
                return;
            }
            last.insert(user, Last { at: t, track, playing, device: device.to_string() });
        }
        let (queue, idx) = spot_queue(state, track);
        let position = state["position"].as_f64().filter(|p| p.is_finite() && *p >= 0.0).unwrap_or(0.0);
        let r = db.execute(
            "INSERT INTO playback_spot (user_id, track_id, position, queue, idx, device_id, device_name, playing, updated_at) VALUES (?,?,?,?,?,?,?,?,?)
             ON CONFLICT(user_id) DO UPDATE SET track_id=excluded.track_id, position=excluded.position, queue=excluded.queue, idx=excluded.idx,
             device_id=excluded.device_id, device_name=excluded.device_name, playing=excluded.playing, updated_at=excluded.updated_at",
            p![user, track, position, json!(queue).to_string(), idx, device, name, playing, t],
        );
        if let Err(e) = r {
            tracing::warn!("could not save the playback spot: {e}");
            self.forget(user);
        }
    }
}

/// The user's last spot with its song (`GET /api/remote/spot`), and whether the device that wrote it is
/// online now (and playing, going by its latest check-in), or null.
pub fn answer(db: &Db, devices: &Devices, user: i64) -> ApiResult<Value> {
    let Some(mut row) = db.one(
        "SELECT s.track_id, s.position, s.queue, s.idx, s.device_id, s.device_name, s.playing, s.updated_at,
                t.title, t.artist, t.album, t.album_id, t.duration
         FROM playback_spot s JOIN tracks t ON t.id = s.track_id WHERE s.user_id = ?",
        p![user],
    )? else {
        return Ok(Value::Null);
    };
    let queue: Value = serde_json::from_str(&row.s("queue")).unwrap_or_else(|_| json!([]));
    let device = row.s("device_id");
    let t = now();
    let live = {
        let all = devices.hubs();
        all.get(&user).and_then(|h| h.devices.get(&device)).filter(|d| t - d.seen < ALIVE).map(|d| d.state["playing"].as_bool().unwrap_or(false))
    };
    let index = row.i("idx");
    let playing = row.b("playing");
    row.remove("idx");
    row.insert("index".into(), json!(index));
    row.insert("queue".into(), queue);
    row.insert("playing".into(), json!(playing));
    row.insert("online".into(), json!(live.is_some()));
    row.insert("device_playing".into(), json!(live.unwrap_or(false)));
    Ok(Value::Object(row))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spot_is_written_on_changes_and_otherwise_every_ten_seconds() {
        let l = Last { at: 100.0, track: 1, playing: true, device: "a".into() };
        assert!(spot_due(None, "a", 1, true, 0.0));
        assert!(!spot_due(None, "a", 1, false, 0.0)); // an app that opens paused doesn't claim the spot
        assert!(!spot_due(Some(&l), "a", 1, true, 105.0));
        assert!(spot_due(Some(&l), "a", 1, true, 110.0));
        assert!(spot_due(Some(&l), "a", 2, true, 101.0)); // song change
        assert!(spot_due(Some(&l), "a", 1, false, 101.0)); // pause
        assert!(!spot_due(Some(&l), "b", 1, true, 101.0)); // another device plays while "a" still does: waits its turn
        assert!(!spot_due(Some(&l), "b", 7, true, 101.0)); // even with a different song
        assert!(spot_due(Some(&l), "b", 1, true, 110.0)); // "a" has gone quiet
        assert!(!spot_due(Some(&l), "b", 7, false, 101.0)); // another device sits paused
        let paused = Last { playing: false, ..l };
        assert!(!spot_due(Some(&paused), "a", 1, false, 500.0));
        assert!(spot_due(Some(&paused), "a", 2, false, 101.0)); // skipped while paused
        assert!(spot_due(Some(&paused), "a", 1, true, 101.0)); // resumed
        assert!(spot_due(Some(&paused), "b", 1, true, 101.0)); // another device plays after "a" stopped
    }

    #[test]
    fn spot_queue_keeps_library_ids_and_finds_the_current_song() {
        let s = json!({ "index": 1, "queue": [{ "id": 5 }, { "id": 6 }, { "title": "stream" }, 7] });
        assert_eq!(spot_queue(&s, 6), (vec![5, 6, 7], 1));
        // an index from before non-library songs were dropped: found by id instead
        let s = json!({ "index": 3, "queue": [{ "id": 5 }, { "title": "x" }, { "id": 6 }, { "id": 7 }] });
        assert_eq!(spot_queue(&s, 7), (vec![5, 6, 7], 2));
        assert_eq!(spot_queue(&json!({}), 9), (vec![9], 0));
        let big: Vec<i64> = (1..=400).collect();
        let (q, i) = spot_queue(&json!({ "queue": big, "index": 0 }), 1);
        assert_eq!((q.len(), i), (300, 0));
    }
}
