//! The lines (Download now, the owner on Oct 21: "for the que can i get a download now button"): the sources
//! that run only so many at a time – streamrip one download per service (Deezer, Qobuz and Tidal are separate
//! accounts, so one of each runs at once; [`Lines::streamrip`]), yt-dlp two, the indexers two searches, the
//! worker's Soulseek searches one, each plugin program one download ([`Lines::plugin`]) – keep the requests
//! that wait for them in a line, keyed by request id, so
//! the Requests page can say where each one stands ("2nd in line for Deezer") and Download now can move one to
//! the front.
//!
//! Order inside a line: requests someone pressed Download now on first, in the order they were pressed, then
//! everyone else in the order they arrived (first come, first served – what the semaphores did before). A
//! slot is given only to the first in that order and only while fewer than the limit run: a press never
//! gives a source more slots and never touches a running one. A press is kept ([`Pressed`]) until the request
//! leaves the active states, so a pressed request goes first in every later line it enters too (Soulseek
//! finds nothing → it goes first in the Deezer line); a restart of Encore forgets the presses.
//!
//! A waiter that is dropped (Cancel aborts the request's task) leaves its line at once; a [`Slot`] that is
//! dropped frees its place for the next. Every change bumps a `watch` counter the waiters follow, so no
//! wake-up is lost between a waiter's look and its wait.
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::watch;

/// The line keys (`line.source` in the API).
pub const STREAMRIP: &str = "streamrip";
/// yt-dlp's line.
pub const YTDLP: &str = "ytdlp";
/// The torrent and usenet indexers' line.
pub const INDEXERS: &str = "indexers";
/// Soulseek's line.
pub const SOULSEEK: &str = "soulseek";
/// The line of a plugin program's sources (one per plugin, [`Lines::plugin`]).
pub const PLUGIN: &str = "plugin";
/// How many downloads one plugin program runs at a time (its search and its download hold the slot, as
/// streamrip's do): an uploaded plugin's service is unknown to Encore, so it gets streamrip's limit.
pub const PLUGIN_LIMIT: usize = 1;

/// A request someone asked to download now: who, when (unix seconds), and the order of the presses.
#[derive(Clone, Debug, PartialEq)]
pub struct Press {
    /// who pressed
    pub by: String,
    /// when (unix seconds)
    pub at: f64,
    /// the order of the presses
    pub seq: u64,
}

/// The presses, by request id. Clones share them.
#[derive(Clone, Default)]
pub struct Pressed(Arc<Mutex<PressState>>);

/// The presses and what was served, behind [`Pressed`]'s lock.
#[derive(Default)]
struct PressState {
    /// the press per request id
    map: HashMap<i64, Press>,
    /// the last press's number
    seq: u64,
    /// pressed requests that waited in a line and got its slot: (request id, line), until the worker logs it
    served: Vec<(i64, &'static str)>,
}

impl Pressed {
    /// The state, even after a panic poisoned the lock.
    fn lock(&self) -> MutexGuard<'_, PressState> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The press kept for request `id`.
    pub fn get(&self, id: i64) -> Option<Press> {
        self.lock().map.get(&id).cloned()
    }

    /// Each pressed request's place in the order of presses.
    fn order(&self) -> HashMap<i64, u64> {
        self.lock().map.iter().map(|(id, p)| (*id, p.seq)).collect()
    }

    /// Request `id` is pressed by `by` at `at`: it goes behind the requests pressed before it. A request
    /// pressed before keeps its place (and who pressed it). → the press, and whether it is new
    fn press(&self, id: i64, by: &str, at: f64) -> (Press, bool) {
        let mut s = self.lock();
        if let Some(p) = s.map.get(&id) {
            return (p.clone(), false);
        }
        s.seq += 1;
        let p = Press { by: by.to_string(), at, seq: s.seq };
        s.map.insert(id, p.clone());
        (p, true)
    }

    /// Forget the presses of the requests `keep` says no to (they left the active states).
    fn retain(&self, keep: impl Fn(i64) -> bool) -> bool {
        let mut s = self.lock();
        let n = s.map.len();
        s.map.retain(|id, _| keep(*id));
        s.served.retain(|(id, _)| keep(*id));
        s.map.len() != n
    }

    /// A pressed request waited in line `source` and got its slot: the worker says so in its log.
    fn served(&self, id: i64, source: &'static str) {
        let mut s = self.lock();
        if s.map.contains_key(&id) && !s.served.contains(&(id, source)) {
            s.served.push((id, source));
        }
    }

    /// Request `id` went first in line `source` because of a press (once): the press, for the request's log
    /// ("streamrip: started first in line (Download now by carol)").
    pub fn take_served(&self, id: i64, source: &str) -> Option<Press> {
        let mut s = self.lock();
        let i = s.served.iter().position(|(x, l)| *x == id && *l == source)?;
        s.served.remove(i);
        s.map.get(&id).cloned()
    }
}

/// One request waiting in a line.
struct Waiter {
    /// the request's id
    id: i64,
    /// its place in the order of arrival
    ticket: u64,
    /// what the line is for this request ("Deezer", "bandcamp.com")
    label: String,
}

/// A line's slots and waiters.
#[derive(Default)]
struct LineState {
    /// the slots in use: (ticket, request id)
    running: Vec<(u64, i64)>,
    /// the requests waiting, in arrival order
    waiters: Vec<Waiter>,
    /// the last ticket handed out
    seq: u64,
}

/// A source's line: at most `limit` at once, the others wait in order.
pub struct Line {
    /// the line's key ([`STREAMRIP`], [`YTDLP`], [`INDEXERS`], [`SOULSEEK`], [`PLUGIN`])
    pub source: &'static str,
    /// how many at once
    pub limit: usize,
    /// the slots and waiters
    state: Mutex<LineState>,
    /// bumped on every change, for waiters to look again
    version: watch::Sender<u64>,
}

/// A slot of a line, held while the source runs for the request; dropping it lets the next one go.
pub struct Slot {
    /// the line it belongs to
    line: Arc<Line>,
    /// the ticket it was given in the line
    ticket: u64,
}

impl Drop for Slot {
    fn drop(&mut self) {
        {
            let mut s = self.line.lock();
            s.running.retain(|(t, _)| *t != self.ticket);
        }
        self.line.bump();
    }
}

/// A waiter's place, given up when its future is dropped before it got a slot (Cancel aborts the task).
struct Leave<'a> {
    /// the line waited in
    line: &'a Line,
    /// the waiter's ticket
    ticket: u64,
    /// still waiting (disarmed once it got a slot)
    armed: bool,
}

impl Drop for Leave<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        {
            let mut s = self.line.lock();
            s.waiters.retain(|w| w.ticket != self.ticket);
        }
        self.line.bump();
    }
}

/// The waiters in line order: pressed first in the order of the presses, then in the order of arrival.
fn ordered<'a>(waiters: &'a [Waiter], presses: &HashMap<i64, u64>) -> Vec<&'a Waiter> {
    let mut v: Vec<&Waiter> = waiters.iter().collect();
    v.sort_by_key(|w| (presses.get(&w.id).copied().unwrap_or(u64::MAX), w.ticket));
    v
}

impl Line {
    /// A line of `source` with `limit` slots (at least 1).
    pub fn new(source: &'static str, limit: usize) -> Line {
        let (version, _) = watch::channel(0);
        Line { source, limit: limit.max(1), state: Mutex::new(LineState::default()), version }
    }

    /// The state, even after a panic poisoned the lock.
    fn lock(&self) -> MutexGuard<'_, LineState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Something changed (a slot freed, a waiter left, a press): the waiters look again.
    pub fn bump(&self) {
        self.version.send_modify(|v| *v = v.wrapping_add(1));
    }

    /// Wait for a slot for request `id` (`label`: what the line is for it). `on_wait` runs once when the
    /// request has to wait (it writes the row's "In line for Deezer"); a slot that is free and first in line
    /// is taken at once without it.
    pub async fn acquire(self: &Arc<Self>, id: i64, label: &str, pressed: &Pressed, on_wait: impl FnOnce()) -> Slot {
        let mut rx = self.version.subscribe();
        let ticket = {
            let mut s = self.lock();
            s.seq += 1;
            let ticket = s.seq;
            s.waiters.push(Waiter { id, ticket, label: label.to_string() });
            ticket
        };
        let mut leave = Leave { line: self, ticket, armed: true };
        let mut on_wait = Some(on_wait);
        loop {
            rx.borrow_and_update();
            if self.try_take(ticket, id, pressed, on_wait.is_none()) {
                leave.armed = false;
                return Slot { line: self.clone(), ticket };
            }
            if let Some(f) = on_wait.take() {
                f();
            }
            // the sender lives as long as the line (self)
            let _ = rx.changed().await;
        }
    }

    /// The slot for `ticket`, when one is free and it is first in line (`waited`: it waited for it).
    fn try_take(&self, ticket: u64, id: i64, pressed: &Pressed, waited: bool) -> bool {
        let presses = pressed.order();
        {
            let mut s = self.lock();
            if s.running.len() >= self.limit {
                return false;
            }
            if ordered(&s.waiters, &presses).first().map(|w| w.ticket) != Some(ticket) {
                return false;
            }
            s.waiters.retain(|w| w.ticket != ticket);
            s.running.push((ticket, id));
        }
        if waited && presses.contains_key(&id) {
            pressed.served(id, self.source);
        }
        // the others moved up
        self.bump();
        true
    }

    /// Request `id`'s place among the waiters (1 = next), None when it doesn't wait here.
    pub fn position(&self, id: i64, pressed: &Pressed) -> Option<usize> {
        let presses = pressed.order();
        let s = self.lock();
        ordered(&s.waiters, &presses).iter().position(|w| w.id == id).map(|i| i + 1)
    }

    /// Request `id` holds a slot of this line.
    pub fn holds(&self, id: i64) -> bool {
        self.lock().running.iter().any(|(_, i)| *i == id)
    }

    /// How many hold a slot now.
    pub fn running(&self) -> usize {
        self.lock().running.len()
    }

    /// How many wait now.
    pub fn waiting(&self) -> usize {
        self.lock().waiters.len()
    }

    /// The waiters in line order: (request id, label).
    pub fn waiters(&self, pressed: &Pressed) -> Vec<(i64, String)> {
        let presses = pressed.order();
        let s = self.lock();
        ordered(&s.waiters, &presses).into_iter().map(|w| (w.id, w.label.clone())).collect()
    }

    /// What the line is for request `id` while it waits ("Deezer").
    pub fn label_of(&self, id: i64) -> Option<String> {
        self.lock().waiters.iter().find(|w| w.id == id).map(|w| w.label.clone())
    }

    /// "1 download at a time", "2 searches at a time".
    pub fn limit_text(&self) -> String {
        limit_text(self.source, self.limit)
    }
}

/// Where a request stands in a line (`line` of a request row in `GET /api/requests`).
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    /// the line's key
    pub source: &'static str,
    /// what the line is for this request ("Deezer", "bandcamp.com")
    pub label: String,
    /// 1 = next
    pub position: usize,
    /// how many wait in the line
    pub waiting: usize,
    /// how many slots are in use
    pub running: usize,
    /// how many at once
    pub limit: usize,
    /// someone pressed Download now on it
    pub first: bool,
}

impl Place {
    /// "2nd in line for Deezer"
    pub fn text(&self) -> String {
        line_text(self.position, &self.label)
    }

    /// "1 at a time" in words, as [`limit_text`] gives it.
    pub fn limit_text(&self) -> String {
        limit_text(self.source, self.limit)
    }

    /// `{source, label, position, waiting, running, limit, limit_text, first, text}`
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({ "source": self.source, "label": self.label, "position": self.position, "waiting": self.waiting, "running": self.running,
            "limit": self.limit, "limit_text": self.limit_text(), "first": self.first, "text": self.text() })
    }
}

/// The streamrip lines, the other three, the plugin programs' lines and the presses. Clones share them.
#[derive(Clone)]
pub struct Lines {
    /// streamrip: one download at a time per service, in [`SERVICES`] order (Qobuz, Tidal, Deezer)
    streamrip: [Arc<Line>; 3],
    /// streamrip's lookups (Encore's own calls to Deezer's and Tidal's APIs, a few seconds): one at a time, as
    /// when they held the one streamrip line; nobody waits here long enough for a line of its own
    pub streamrip_search: Arc<tokio::sync::Mutex<()>>,
    /// yt-dlp: two downloads at a time
    pub ytdlp: Arc<Line>,
    /// the requests' indexer searches: two at a time (the picker's don't wait here)
    pub indexers: Arc<Line>,
    /// the worker's Soulseek searches, before the search gate's turn (people never wait here)
    pub soulseek: Arc<Line>,
    /// each plugin program's line, by plugin id, made when a request first needs it ([`Lines::plugin`])
    plugins: Arc<Mutex<std::collections::BTreeMap<String, Arc<Line>>>>,
    /// who pressed Download now on which request
    pub pressed: Pressed,
}

impl Default for Lines {
    fn default() -> Self {
        Lines {
            streamrip: SERVICES.map(|_| Arc::new(Line::new(STREAMRIP, 1))),
            streamrip_search: Arc::default(),
            ytdlp: Arc::new(Line::new(YTDLP, 2)),
            indexers: Arc::new(Line::new(INDEXERS, 2)),
            soulseek: Arc::new(Line::new(SOULSEEK, 1)),
            plugins: Arc::default(),
            pressed: Pressed::default(),
        }
    }
}

impl Lines {
    /// The lines, in the order the Requests page names them: streamrip's (by service), the other three, then the plugin programs' by plugin id.
    pub fn all(&self) -> Vec<Arc<Line>> {
        let mut out = self.streamrip.to_vec();
        out.extend([self.ytdlp.clone(), self.indexers.clone(), self.soulseek.clone()]);
        out.extend(self.plugins.lock().unwrap_or_else(|p| p.into_inner()).values().cloned());
        out
    }

    /// The streamrip line of `service` ("deezer", "qobuz", "tidal"; the service a link names, see
    /// [`service_of`]). Anything else gets Deezer's.
    pub fn streamrip(&self, service: &str) -> Arc<Line> {
        let i = SERVICES.iter().position(|s| *s == service).unwrap_or(SERVICES.len() - 1);
        self.streamrip[i].clone()
    }

    /// The line of plugin program `plugin` (all of its sources): [`PLUGIN_LIMIT`] download at a time.
    pub fn plugin(&self, plugin: &str) -> Arc<Line> {
        let mut m = self.plugins.lock().unwrap_or_else(|p| p.into_inner());
        m.entry(plugin.to_string()).or_insert_with(|| Arc::new(Line::new(PLUGIN, PLUGIN_LIMIT))).clone()
    }

    /// Where request `id` waits, if it waits in a line.
    pub fn place(&self, id: i64) -> Option<Place> {
        let first = self.pressed.get(id).is_some();
        self.all().into_iter().find_map(|l| {
            let position = l.position(id, &self.pressed)?;
            Some(Place { source: l.source, label: l.label_of(id).unwrap_or_default(), position, waiting: l.waiting(), running: l.running(), limit: l.limit, first })
        })
    }

    /// The line whose slot request `id` holds.
    pub fn holding(&self, id: i64) -> Option<Arc<Line>> {
        self.all().into_iter().find(|l| l.holds(id))
    }

    /// Download now on request `id` by `by`: it goes first in its line (and in every line it enters later),
    /// behind the requests pressed before it. → the press, and whether it is new
    pub fn press(&self, id: i64, by: &str, at: f64) -> (Press, bool) {
        let p = self.pressed.press(id, by, at);
        for l in self.all() {
            l.bump();
        }
        p
    }

    /// Press `ids` in this order (Download all now: each line's waiters in the order they stand, so nobody
    /// overtakes anybody among them). → how many were pressed now
    pub fn press_in_order(&self, ids: &[i64], by: &str, at: f64) -> usize {
        let n = ids.iter().filter(|id| self.pressed.press(**id, by, at).1).count();
        for l in self.all() {
            l.bump();
        }
        n
    }

    /// The press kept for request `id`.
    pub fn pressed(&self, id: i64) -> Option<Press> {
        self.pressed.get(id)
    }

    /// Forget the presses of requests that left the active states (`active`: the ids still active).
    pub fn retain(&self, active: &HashSet<i64>) {
        if self.pressed.retain(|id| active.contains(&id)) {
            for l in self.all() {
                l.bump();
            }
        }
    }
}

// ---- words ------------------------------------------------------------------------------------------

/// "1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "101st", "111th".
pub fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// "Next in line for Deezer" (position 1), "2nd in line for Deezer", "21st in line for YouTube".
pub fn line_text(position: usize, label: &str) -> String {
    let at = if position <= 1 { "Next".to_string() } else { ordinal(position) };
    format!("{at} in line for {label}")
}

/// "1 download at a time", "2 downloads at a time" (streamrip, yt-dlp, a plugin); "2 searches at a time", "1
/// search at a time" (the indexers, Soulseek).
pub fn limit_text(source: &str, limit: usize) -> String {
    let (one, many) = if source == INDEXERS || source == SOULSEEK { ("search", "searches") } else { ("download", "downloads") };
    format!("{limit} {} at a time", if limit == 1 { one } else { many })
}

/// A streamrip service's name ("deezer" → "Deezer").
pub fn service_name(s: &str) -> String {
    match s {
        "deezer" => "Deezer".into(),
        "tidal" => "Tidal".into(),
        "qobuz" => "Qobuz".into(),
        other => other.to_string(),
    }
}

/// streamrip's services, in the order of its lines (`sources::streamrip::SERVICES` is this list).
pub const SERVICES: [&str; 3] = ["qobuz", "tidal", "deezer"];

/// The streamrip service a link is on ("deezer"), None for another site.
pub fn service_of(url: &str) -> Option<&'static str> {
    let u = url.to_lowercase();
    SERVICES.into_iter().find(|s| u.contains(s))
}

/// What the streamrip line is for a request: the service of the pick's or link's `url`, else the only
/// enabled service, else the enabled services in source order joined by "/" ("Tidal/Deezer").
pub fn streamrip_label(url: Option<&str>, services: &[String]) -> String {
    if let Some(u) = url.map(str::to_lowercase) {
        for s in ["deezer", "tidal", "qobuz"] {
            if u.contains(s) {
                return service_name(s);
            }
        }
    }
    match services {
        [] => "Qobuz/Tidal/Deezer".into(),
        list => list.iter().map(|s| service_name(s)).collect::<Vec<_>>().join("/"),
    }
}

/// The site of a link as the request names its source ("YouTube Music", "bandcamp.com").
pub fn link_site(url: &str) -> String {
    let host = reqwest::Url::parse(url).ok().and_then(|u| u.host_str().map(String::from)).unwrap_or("link".into());
    if host.contains("music.youtube") { "YouTube Music".to_string() } else { host.trim_start_matches("www.").to_string() }
}

/// The yt-dlp line's label for a search: "YouTube".
pub const YOUTUBE: &str = "YouTube";
/// The indexers' line's label.
pub const TORRENTS: &str = "torrents and Usenet";
/// The Soulseek line's label.
pub const SOULSEEK_LABEL: &str = "Soulseek";

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// Wait until the line has `n` waiters (tasks were spawned and took their tickets).
    async fn waiters(l: &Line, n: usize) {
        for _ in 0..1000 {
            if l.waiting() == n {
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        panic!("{} waiters, want {n}", l.waiting());
    }

    #[tokio::test(start_paused = true)]
    async fn never_more_than_the_limit() {
        let l = Arc::new(Line::new(YTDLP, 2));
        let p = Pressed::default();
        let (now, most, done) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let t0 = tokio::time::Instant::now();
        let mut tasks = vec![];
        for id in 1..=6 {
            let (l, p, now, most, done) = (l.clone(), p.clone(), now.clone(), most.clone(), done.clone());
            tasks.push(tokio::spawn(async move {
                let _slot = l.acquire(id, YOUTUBE, &p, || {}).await;
                let n = now.fetch_add(1, Ordering::SeqCst) + 1;
                most.fetch_max(n, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_secs(10)).await;
                now.fetch_sub(1, Ordering::SeqCst);
                done.fetch_add(1, Ordering::SeqCst);
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }
        assert_eq!(most.load(Ordering::SeqCst), 2, "never more than two at once");
        assert_eq!(done.load(Ordering::SeqCst), 6);
        assert!(t0.elapsed() <= Duration::from_secs(31), "all done by 30 s: {:?}", t0.elapsed());
        assert_eq!((l.running(), l.waiting()), (0, 0));
    }

    /// Deezer, Qobuz and Tidal are separate accounts: one download of each at once, never two on one.
    #[tokio::test(start_paused = true)]
    async fn streamrip_runs_one_download_per_service() {
        let lines = Lines::default();
        let p = lines.pressed.clone();
        let deezer = lines.streamrip("deezer").acquire(1, "Deezer", &p, || {}).await;
        let _qobuz = lines.streamrip("qobuz").acquire(2, "Qobuz", &p, || {}).await;
        let _tidal = lines.streamrip(service_of("https://tidal.com/browse/album/9").unwrap()).acquire(3, "Tidal", &p, || {}).await;
        let (l, p2) = (lines.streamrip("deezer"), p.clone());
        let second = tokio::spawn(async move { l.acquire(4, "Deezer", &p2, || {}).await });
        waiters(&lines.streamrip("deezer"), 1).await;
        assert_eq!(lines.place(4).map(|pl| pl.text()), Some("Next in line for Deezer".to_string()));
        assert_eq!(lines.all().iter().filter(|l| l.source == STREAMRIP).map(|l| l.running()).collect::<Vec<_>>(), [1, 1, 1]);
        drop(deezer);
        let _s = second.await.unwrap();
        assert_eq!((lines.streamrip("deezer").running(), lines.streamrip("deezer").waiting()), (1, 0));
        assert_eq!(service_of("https://www.deezer.com/album/1"), Some("deezer"));
        assert_eq!(service_of("https://youtube.com/watch?v=1"), None);
        assert!(Arc::ptr_eq(&lines.streamrip("unknown"), &lines.streamrip("deezer")));
    }

    /// a, b, c, d arrive behind a running request; d is pressed, then b: d, b, a, c.
    #[tokio::test(start_paused = true)]
    async fn download_now_goes_first_behind_earlier_presses() {
        let lines = Lines::default();
        let l = lines.streamrip("deezer");
        let busy = l.acquire(100, "Deezer", &lines.pressed, || {}).await;
        let order = Arc::new(Mutex::new(vec![]));
        let mut tasks = vec![];
        for id in [1, 2, 3, 4] {
            let (l2, p, order) = (l.clone(), lines.pressed.clone(), order.clone());
            tasks.push(tokio::spawn(async move {
                let _s = l2.acquire(id, "Deezer", &p, || {}).await;
                order.lock().unwrap().push(id);
                tokio::time::sleep(Duration::from_secs(1)).await;
            }));
            waiters(&l, id as usize).await;
        }
        assert_eq!((1..=4).map(|id| l.position(id, &lines.pressed)).collect::<Vec<_>>(), [Some(1), Some(2), Some(3), Some(4)]);
        lines.press(4, "carol", 1.0);
        lines.press(2, "dave", 2.0);
        assert_eq!(l.waiters(&lines.pressed).iter().map(|w| w.0).collect::<Vec<_>>(), [4, 2, 1, 3]);
        assert_eq!([4, 2, 1, 3].map(|id| l.position(id, &lines.pressed)), [Some(1), Some(2), Some(3), Some(4)]);
        let place = lines.place(2).unwrap();
        assert_eq!((place.position, place.first, place.text(), place.limit_text()), (2, true, "2nd in line for Deezer".to_string(), "1 download at a time".to_string()));
        assert!(!lines.place(1).unwrap().first);
        // pressed again: its place stays (it was pressed before 2)
        assert!(!lines.press(4, "erin", 3.0).1);
        assert_eq!(lines.pressed(4).unwrap().by, "carol");
        drop(busy);
        for t in tasks {
            t.await.unwrap();
        }
        assert_eq!(*order.lock().unwrap(), [4, 2, 1, 3]);
        // the worker logs who moved it up, once
        assert_eq!(lines.pressed.take_served(4, STREAMRIP).map(|p| p.by), Some("carol".to_string()));
        assert_eq!(lines.pressed.take_served(4, STREAMRIP), None);
        assert_eq!(lines.pressed.take_served(1, STREAMRIP), None, "not pressed");
    }

    #[tokio::test(start_paused = true)]
    async fn a_press_never_takes_a_running_slot() {
        let lines = Lines::default();
        let l = lines.streamrip("deezer");
        let running = l.acquire(1, "Deezer", &lines.pressed, || {}).await;
        let (l2, p2) = (l.clone(), lines.pressed.clone());
        let waiting = tokio::spawn(async move { l2.acquire(2, "Deezer", &p2, || {}).await });
        waiters(&l, 1).await;
        lines.press(2, "carol", 1.0);
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert!(l.holds(1) && !l.holds(2), "the running one keeps its slot");
        assert_eq!((l.running(), l.position(2, &lines.pressed)), (1, Some(1)));
        drop(running);
        let slot = waiting.await.unwrap();
        assert!(l.holds(2));
        drop(slot);
        assert_eq!(l.running(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn a_cancelled_waiter_leaves_and_the_rest_move_up() {
        let lines = Lines::default();
        let l = lines.ytdlp.clone();
        let a = l.acquire(1, YOUTUBE, &lines.pressed, || {}).await;
        let b = l.acquire(2, YOUTUBE, &lines.pressed, || {}).await;
        let mut tasks = vec![];
        for id in [3, 4, 5] {
            let (l2, p) = (l.clone(), lines.pressed.clone());
            tasks.push(tokio::spawn(async move {
                let _s = l2.acquire(id, YOUTUBE, &p, || {}).await;
                tokio::time::sleep(Duration::from_secs(1)).await;
            }));
            waiters(&l, id as usize - 2).await;
        }
        assert_eq!(l.position(5, &lines.pressed), Some(3));
        // Cancel aborts 4's task while it waits
        tasks.remove(1).abort();
        waiters(&l, 2).await;
        assert_eq!((l.position(3, &lines.pressed), l.position(4, &lines.pressed), l.position(5, &lines.pressed)), (Some(1), None, Some(2)));
        drop(a);
        drop(b);
        for t in tasks {
            t.await.unwrap();
        }
        assert_eq!((l.running(), l.waiting()), (0, 0), "no slot leaked");
        // and the line still gives both slots
        let _x = l.acquire(6, YOUTUBE, &lines.pressed, || {}).await;
        let _y = l.acquire(7, YOUTUBE, &lines.pressed, || {}).await;
        assert_eq!(l.running(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_press_before_it_reaches_a_line_counts_when_it_arrives() {
        let lines = Lines::default();
        let l = lines.indexers.clone();
        let _a = l.acquire(1, TORRENTS, &lines.pressed, || {}).await;
        let _b = l.acquire(2, TORRENTS, &lines.pressed, || {}).await;
        let (l3, p3) = (l.clone(), lines.pressed.clone());
        let _w = tokio::spawn(async move { l3.acquire(3, TORRENTS, &p3, || {}).await });
        waiters(&l, 1).await;
        // 9 is pressed while it searches Soulseek, and reaches the indexers' line later
        lines.press(9, "carol", 1.0);
        let waited = Arc::new(AtomicUsize::new(0));
        let (l9, p9, w9) = (l.clone(), lines.pressed.clone(), waited.clone());
        let _n = tokio::spawn(async move { l9.acquire(9, TORRENTS, &p9, move || { w9.fetch_add(1, Ordering::SeqCst); }).await });
        waiters(&l, 2).await;
        assert_eq!((l.position(9, &lines.pressed), l.position(3, &lines.pressed)), (Some(1), Some(2)));
        assert_eq!(waited.load(Ordering::SeqCst), 1, "it says once that it waits");
        let p = lines.place(9).unwrap();
        assert_eq!((p.source, p.label.as_str(), p.waiting, p.running, p.limit, p.first), (INDEXERS, TORRENTS, 2, 2, 2, true));
        assert_eq!(p.limit_text(), "2 searches at a time");
        // it leaves the active states: the press is forgotten, and it stands where it arrived
        lines.retain(&HashSet::from([3]));
        assert_eq!((l.position(3, &lines.pressed), l.position(9, &lines.pressed)), (Some(1), Some(2)));
    }

    #[test]
    fn the_bulk_press_keeps_each_lines_order() {
        let lines = Lines::default();
        // a line as it stands: 1, 2, 3, 4 (2 pressed earlier: 2, 1, 3, 4)
        lines.press(2, "carol", 1.0);
        let standing = [2, 1, 3, 4];
        // all of them: pressed in the order they stand, nobody overtakes anybody
        let n = lines.press_in_order(&standing, "boss", 2.0);
        assert_eq!(n, 3, "2 was pressed already");
        let order: HashMap<i64, u64> = lines.pressed.order();
        let mut by: Vec<i64> = standing.to_vec();
        by.sort_by_key(|id| order[id]);
        assert_eq!(by, standing);
        // mine: the admin's (3, 4) first in their order, the rest behind
        let lines = Lines::default();
        lines.press_in_order(&[3, 4], "boss", 1.0);
        let l = Line::new(STREAMRIP, 1);
        {
            let mut s = l.lock();
            for (i, id) in [1, 2, 3, 4].into_iter().enumerate() {
                s.waiters.push(Waiter { id, ticket: i as u64 + 1, label: "Deezer".into() });
            }
        }
        assert_eq!(l.waiters(&lines.pressed).iter().map(|w| w.0).collect::<Vec<_>>(), [3, 4, 1, 2]);
    }

    #[test]
    fn ordinals_and_line_texts() {
        let want = [(1, "1st"), (2, "2nd"), (3, "3rd"), (4, "4th"), (11, "11th"), (12, "12th"), (13, "13th"), (21, "21st"), (22, "22nd"), (101, "101st"), (111, "111th")];
        for (n, w) in want {
            assert_eq!(ordinal(n), w);
        }
        assert_eq!(line_text(1, "Deezer"), "Next in line for Deezer");
        assert_eq!(line_text(2, "Deezer"), "2nd in line for Deezer");
        assert_eq!(line_text(3, "YouTube"), "3rd in line for YouTube");
        assert_eq!(line_text(11, TORRENTS), "11th in line for torrents and Usenet");
        assert_eq!(line_text(21, SOULSEEK_LABEL), "21st in line for Soulseek");
        let lines = Lines::default();
        assert_eq!(lines.all().iter().map(|l| l.limit_text()).collect::<Vec<_>>(), ["1 download at a time", "1 download at a time", "1 download at a time", "2 downloads at a time", "2 searches at a time", "1 search at a time"]);
        // a plugin program's line: made when first needed, one per plugin, one download at a time
        let ia = lines.plugin("internet-archive");
        assert!(Arc::ptr_eq(&ia, &lines.plugin("internet-archive")) && !Arc::ptr_eq(&ia, &lines.plugin("other")));
        assert_eq!((ia.source, ia.limit, ia.limit_text()), (PLUGIN, 1, "1 download at a time".to_string()));
        assert_eq!(lines.all().len(), 8, "three streamrip lines, the other three, two plugins");
        // the streamrip label: from the link, from the one service, from several (in source order)
        let svc = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(streamrip_label(Some("https://www.deezer.com/album/302127"), &svc(&["tidal", "deezer"])), "Deezer");
        assert_eq!(streamrip_label(Some("https://tidal.com/browse/album/1"), &svc(&["deezer"])), "Tidal");
        assert_eq!(streamrip_label(Some("https://play.qobuz.com/album/x"), &[]), "Qobuz");
        assert_eq!(streamrip_label(None, &svc(&["deezer"])), "Deezer");
        assert_eq!(streamrip_label(None, &svc(&["tidal", "deezer"])), "Tidal/Deezer");
        assert_eq!(link_site("https://music.youtube.com/watch?v=x"), "YouTube Music");
        assert_eq!(link_site("https://www.bandcamp.com/album/x"), "bandcamp.com");
    }
}
