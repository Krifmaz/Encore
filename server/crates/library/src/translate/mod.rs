//! Translated and romanized titles, per listener.
//!
//! Each listener picks (Settings → Language, account prefs `translate_mode` / `translate_lang`):
//! off · show the translation under the original · show it instead · romanization only, and a target
//! language (English unless chosen). JSON answers to that listener's GETs then carry `title_t`,
//! `album_t`, `artist_t`, `albumartist_t` (and `name_t` on artist objects) with the translation and
//! the same names ending in `_r` with the romanization, wherever one is known; clients choose what to
//! show from the mode.
//!
//! Where translations come from, strongest first (tiers.rs): a manual correction (⋯ → Edit translation,
//! shared by everyone reading the same language), official ones (MusicBrainz pseudo-releases and aliases,
//! Apple's English names for Japanese releases; see official.rs), the romanization built into the server
//! (romanize.rs: kana, Hangul, Cyrillic, Greek and more from tables), DeepL with the owner's API key (deepl.rs),
//! then the local model when it is installed (llama.cpp with a Qwen3 GGUF; see llama.rs) – each optional. The
//! background worker goes through the library in small batches at low priority; titles a listener has on
//! screen are queued first.
pub mod deepl;
pub mod hangul;
pub mod llama;
pub mod official;
pub mod prompt;
pub mod romaji;
pub mod romanize;
pub mod script;
pub mod store;
pub mod tiers;

use crate::Ctx;
use encore_core::ids::is_mbid;
use encore_db::{Db, Row, RowExt};
use prompt::{Item, Kind};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// What the translations keep while the server runs: who has them on, the titles waiting, the model
/// server. Clones share it.
#[derive(Clone, Default)]
pub struct Translator(Arc<Inner>);

#[derive(Default)]
struct Inner {
    /// `active_targets` with the `Db::users_version` it was read at: looked up again only after a write
    /// to the accounts
    active: Mutex<Option<(u64, Vec<&'static str>)>>,
    queue: Mutex<Queue>,
    wake: tokio::sync::Notify,
    /// the model server while it runs
    server: tokio::sync::Mutex<Option<llama::Running>>,
    /// a client that never goes through a proxy (the model server is on 127.0.0.1)
    local: std::sync::OnceLock<reqwest::Client>,
    /// the loop that stops an idle model server runs
    reaper: AtomicBool,
    /// the library's texts to translate, per target language (see `library_backlog`)
    backlog: Mutex<HashMap<&'static str, Backlog>>,
    /// DeepL paused after an error (deepl::Hold)
    deepl_hold: Mutex<Option<deepl::Hold>>,
}

/// The library's candidates for one target language, worked out once per artist index (a change to the songs
/// makes a new one), and how many of the first ones are known to have a row already: each batch starts there
/// instead of reading the whole songs table again.
struct Backlog {
    index: Arc<crate::artists::Index>,
    cands: Vec<Pending>,
    known_upto: usize,
}

impl Translator {
    /// The target languages of everyone who has translations on, read again only after a write to the
    /// accounts.
    pub fn active_targets(&self, db: &Db) -> Vec<&'static str> {
        // the version is read before the rows: a prefs change meanwhile makes the next call look again
        let version = db.users_version();
        if let Some((v, t)) = self.0.active.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            if *v == version {
                return t.clone();
            }
        }
        let targets = active_targets(db);
        *self.0.active.lock().unwrap_or_else(|e| e.into_inner()) = Some((version, targets.clone()));
        targets
    }

    /// Does any account have translations on? (The API's enrichment pass skips translations when not;
    /// Diagnostics shows it.)
    pub fn anyone_on(&self, db: &Db) -> bool {
        !self.active_targets(db).is_empty()
    }

    /// Queue titles a listener has on screen (they go before the library pass).
    pub fn enqueue(&self, target: &'static str, items: Vec<(String, Kind, Option<String>)>) {
        if items.is_empty() {
            return;
        }
        let mut q = self.0.queue.lock().unwrap();
        let mut added = false;
        for (text, kind, artist) in items {
            let text = script::normalize(&text);
            if q.0.len() >= QUEUE_MAX || !q.1.insert((text.clone(), target)) {
                continue;
            }
            q.0.push_back(Pending { text, kind, artist, target });
            added = true;
        }
        drop(q);
        if added {
            self.0.wake.notify_one();
        }
    }

    fn take_queued(&self, n: usize) -> Vec<Pending> {
        let mut q = self.0.queue.lock().unwrap();
        let n = n.min(q.0.len());
        let items: Vec<Pending> = q.0.drain(..n).collect();
        for p in &items {
            q.1.remove(&(p.text.clone(), p.target));
        }
        items
    }

    /// How many titles wait.
    pub fn queued(&self) -> usize {
        self.0.queue.lock().unwrap().0.len()
    }

    /// Start a pass now (a listener turned translations on, the Translator was installed, the last group listen
    /// stopped).
    pub fn wake(&self) {
        self.0.wake.notify_one();
    }

    /// Until the next [`Translator::wake`] (one made while nothing waited counts: the worker's next wait ends at
    /// once).
    pub async fn woken(&self) {
        self.0.wake.notified().await
    }

    /// Why DeepL pauses now, if it does (DeepL's settings at `generation`, `Tiers::deepl_generation`: a change
    /// since the error lifts it).
    pub fn deepl_hold(&self, generation: u64) -> Option<deepl::Error> {
        let h = self.0.deepl_hold.lock().unwrap_or_else(|e| e.into_inner());
        h.as_ref().filter(|h| h.holds(encore_db::now(), generation)).map(|h| h.error.clone())
    }

    /// Whether DeepL may be asked now.
    pub fn deepl_usable(&self, generation: u64) -> bool {
        self.deepl_hold(generation).is_none()
    }

    /// DeepL failed: pause it (how long depends on the error).
    pub fn deepl_failed(&self, e: deepl::Error, generation: u64) {
        *self.0.deepl_hold.lock().unwrap_or_else(|e| e.into_inner()) = Some(deepl::Hold::after(e, encore_db::now(), generation));
    }

    /// DeepL answered: no pause.
    pub fn deepl_worked(&self) {
        *self.0.deepl_hold.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

// ---- listener settings --------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Off,
    Under,
    Instead,
    Romanized,
}

impl Mode {
    pub fn parse(s: &str) -> Mode {
        match s {
            "under" => Mode::Under,
            "instead" => Mode::Instead,
            "romanized" => Mode::Romanized,
            _ => Mode::Off,
        }
    }
}

/// The modes Settings → Language offers (id, label).
pub const MODES: [(&str, &str); 4] = [
    ("off", "Off"),
    ("under", "Show translation under the original"),
    ("instead", "Show translation instead"),
    ("romanized", "Romanization only"),
];

/// A user's (mode, target language) from their account prefs.
pub fn prefs_of(u: &Row) -> (Mode, &'static str) {
    let p: Value = serde_json::from_str(&u.s("prefs")).unwrap_or(json!({}));
    (Mode::parse(p["translate_mode"].as_str().unwrap_or("off")), script::target(p["translate_lang"].as_str().unwrap_or(script::DEFAULT_TARGET)))
}

/// The target languages of everyone who has translations on.
fn active_targets(db: &Db) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = db.query("SELECT prefs FROM users WHERE disabled=0 AND prefs LIKE '%translate_mode%'", encore_db::p![]).unwrap_or_default()
        .iter().map(prefs_of).filter(|(m, _)| *m != Mode::Off).map(|(_, t)| t).collect();
    out.sort();
    out.dedup();
    out
}

/// Does any account have translations on? (The API's enrichment pass skips translations when not;
/// Diagnostics shows it.)
pub fn anyone_on(st: &impl Ctx) -> bool {
    st.library().translator.anyone_on(st.db())
}

// ---- stamping JSON answers ----------------------------------------------------------------------

/// The fields that get `<field>_t` / `<field>_r`.
const FIELDS: [(&str, Kind); 4] = [("title", Kind::Title), ("album", Kind::Album), ("artist", Kind::Artist), ("albumartist", Kind::Artist)];

/// Artist objects (library artists, search results) name themselves in `name`.
fn is_artist(o: &serde_json::Map<String, Value>) -> bool {
    !o.contains_key("username") && !o.contains_key("title") && ["cover_album", "albums", "aliases", "disambiguation"].iter().any(|k| o.contains_key(*k))
}

fn texts_of(o: &serde_json::Map<String, Value>) -> impl Iterator<Item = (&'static str, Kind, &str)> {
    let artist_name = is_artist(o);
    FIELDS.iter().map(|(f, k)| (*f, *k)).chain(artist_name.then_some(("name", Kind::Artist)))
        .filter_map(move |(f, k)| o.get(f).and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()).map(|s| (f, k, s)))
}

fn collect(v: &Value, out: &mut Vec<(String, Kind, Option<String>)>) {
    match v {
        Value::Array(a) => a.iter().for_each(|x| collect(x, out)),
        Value::Object(o) => {
            let artist = o.get("artist").or_else(|| o.get("albumartist")).and_then(|a| a.as_str()).map(String::from);
            for (_, k, s) in texts_of(o) {
                out.push((s.to_string(), k, if k == Kind::Artist { None } else { artist.clone() }));
            }
            o.values().filter(|x| x.is_array() || x.is_object()).for_each(|x| collect(x, out));
        }
        _ => {}
    }
}

fn apply(v: &mut Value, found: &HashMap<String, store::Shown>) {
    match v {
        Value::Array(a) => a.iter_mut().for_each(|x| apply(x, found)),
        Value::Object(o) => {
            let adds: Vec<(String, Value)> = texts_of(o).filter_map(|(f, _, s)| found.get(&script::normalize(s)).map(|sh| (f, sh))).flat_map(|(f, sh)| {
                let mut a = vec![];
                if let Some(t) = &sh.translation { a.push((format!("{f}_t"), json!(t))); }
                if let Some(r) = &sh.romanization { a.push((format!("{f}_r"), json!(r))); }
                a
            }).collect();
            for (k, val) in adds {
                o.entry(k).or_insert(val);
            }
            o.values_mut().filter(|x| x.is_array() || x.is_object()).for_each(|x| apply(x, found));
        }
        _ => {}
    }
}

/// Add the known translations for `target` to every title, album and artist in `v`; returns the texts
/// worth translating that have no row yet (to queue).
pub fn stamp(db: &Db, target: &str, v: &mut Value) -> Vec<(String, Kind, Option<String>)> {
    let mut all = vec![];
    collect(v, &mut all);
    if all.is_empty() {
        return vec![];
    }
    let texts: Vec<String> = all.iter().map(|(t, _, _)| t.clone()).collect();
    let found = store::lookup(db, target, &texts);
    if !found.is_empty() {
        apply(v, &found);
    }
    let mut seen = HashSet::new();
    let wanted: Vec<_> = all.into_iter().filter(|(t, _, _)| script::candidate(t, target) && seen.insert(script::normalize(t))).collect();
    if wanted.is_empty() {
        return vec![];
    }
    let keys: Vec<String> = wanted.iter().map(|(t, _, _)| script::normalize(t)).collect();
    let known = store::known(db, target, &keys);
    wanted.into_iter().filter(|(t, _, _)| !known.contains(&script::normalize(t))).collect()
}

// ---- the work queue -----------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
struct Pending {
    text: String,
    kind: Kind,
    artist: Option<String>,
    target: &'static str,
}

const QUEUE_MAX: usize = 2000;
/// The waiting titles in order, and the (text, target) pairs among them so nothing is queued twice.
type Queue = (VecDeque<Pending>, HashSet<(String, &'static str)>);

/// Start a pass now (a listener turned translations on, the Translator was installed).
pub fn wake(st: &impl Ctx) {
    st.library().translator.wake();
}

/// Items per background batch, and the pause between batches (the work stays in the background).
const BATCH: usize = 40;
const PAUSE: Duration = Duration::from_secs(2);

/// Library rows that mention any of these texts (title, album, artist or album artist).
fn rows_for(db: &Db, texts: &[String]) -> Vec<Row> {
    let mut out = vec![];
    for chunk in texts.chunks(200) {
        let m = crate::artists::marks(chunk.len());
        let mut params = vec![];
        for _ in 0..4 {
            params.extend(chunk.iter().map(|t| json!(t)));
        }
        let sql = format!("SELECT title, artist, album, albumartist, mb_albumid, mb_artistid, mb_albumartistid FROM tracks
                           WHERE title IN ({m}) OR album IN ({m}) OR artist IN ({m}) OR albumartist IN ({m}) LIMIT 5000");
        out.extend(db.query(&sql, params).unwrap_or_default());
    }
    out
}

/// Translate these texts for `target`, source by source in the tier order (tiers.rs): the official names (by the
/// library's MusicBrainz ids), the built-in romanization, DeepL, then the model for what is still missing. A
/// source that is switched off is left out.
async fn process(st: &impl Ctx, target: &'static str, items: Vec<Pending>, model_ok: &mut bool) {
    if items.is_empty() {
        return;
    }
    let t = tiers::Tiers::read(st.cfg());
    if t.official {
        official_pass(st, target, &items).await;
    }
    if t.romanize {
        romanize_pass(st, target, &items).await;
    }
    deepl_pass(st, target, &items).await;
    if t.model {
        model_pass(st, target, items, model_ok).await;
    }
}

/// The official names of these texts (MusicBrainz releases and artists by the library's ids, Apple's English
/// titles); every text is stamped as asked.
async fn official_pass(st: &impl Ctx, target: &'static str, items: &[Pending]) {
    let want: HashMap<String, &Pending> = items.iter().map(|p| (p.text.clone(), p)).collect();
    let texts: Vec<String> = want.keys().cloned().collect();
    let db = st.db().clone();
    let rows = {
        let t = texts.clone();
        tokio::task::spawn_blocking(move || rows_for(&db, &t)).await.unwrap_or_default()
    };
    // official: one release lookup covers the album title and every song on it
    let mut albums: Vec<(String, String, String)> = vec![];
    let mut artists: Vec<(String, String)> = vec![];
    for r in &rows {
        let (title, album) = (script::normalize(&r.s("title")), script::normalize(&r.s("album")));
        let rid = r.s("mb_albumid");
        if (want.contains_key(&title) || want.contains_key(&album)) && !albums.iter().any(|(i, _, a)| *i == rid && *a == album) {
            albums.push((rid, r.s("albumartist"), r.s("album")));
        }
        for (name, id) in [(r.s("artist"), r.s("mb_artistid")), (r.s("albumartist"), r.s("mb_albumartistid"))] {
            if want.contains_key(&script::normalize(&name)) && is_mbid(&id) && !artists.iter().any(|(i, _)| *i == id) {
                artists.push((id, name));
            }
        }
    }
    let mut found: Vec<official::Found> = vec![];
    for (rid, artist, album) in albums.iter().take(BATCH) {
        if is_mbid(rid) {
            match official::release(st, rid, target).await {
                Ok(f) => found.extend(f),
                Err(e) => tracing::debug!("translate: MusicBrainz release {rid}: {e}"),
            }
        }
        match official::apple(st, artist, album, target).await {
            Ok(f) => found.extend(f),
            Err(e) => tracing::debug!("translate: Apple {artist} – {album}: {e}"),
        }
    }
    for (id, name) in artists.iter().take(BATCH) {
        match official::artist(st, id, name, target).await {
            Ok((t, r)) if t.is_some() || r.is_some() => found.push((Kind::Artist, name.clone(), t, r)),
            Ok(_) => {}
            Err(e) => tracing::debug!("translate: MusicBrainz artist {id}: {e}"),
        }
    }
    let db = st.db().clone();
    let items2 = items.to_vec();
    let _ = tokio::task::spawn_blocking(move || {
        for (kind, text, t, r) in &found {
            let _ = store::put(&db, text, target, kind.as_str(), t.as_deref(), r.as_deref(), store::Source::Official);
        }
        for p in &items2 {
            let _ = store::mark(&db, &p.text, target, p.kind.as_str(), store::Source::Official);
        }
    }).await;
}

/// The built-in romanization of these texts (those in a script it romanizes); each is stamped as seen, also
/// when it has none (Japanese with kanji).
async fn romanize_pass(st: &impl Ctx, target: &'static str, items: &[Pending]) {
    let found: Vec<(String, Kind, Option<String>)> = items.iter()
        .filter(|p| romanize::supported(script::detect(&p.text)))
        .map(|p| (p.text.clone(), p.kind, romanize::romanize(&p.text)))
        .collect();
    if found.is_empty() {
        return;
    }
    let db = st.db().clone();
    let _ = tokio::task::spawn_blocking(move || {
        for (text, kind, r) in &found {
            // a letter-by-letter spelling only fills a gap (romanize.rs)
            let source = if romanize::letters_only(script::detect(text)) { store::Source::Letters } else { store::Source::Romanized };
            let _ = store::put(&db, text, target, kind.as_str(), None, r.as_deref(), source);
        }
    }).await;
}

/// DeepL for the song and album titles among these that have no translation yet, or only the model's (deepl.rs
/// decides whether it may ask: switched on, a key, not paused, within the budget).
async fn deepl_pass(st: &impl Ctx, target: &'static str, items: &[Pending]) {
    let texts: Vec<String> = items.iter().filter(|p| deepl::wanted(p.kind)).map(|p| p.text.clone()).collect();
    if texts.is_empty() || !tiers::Tiers::read(st.cfg()).deepl_ready() {
        return;
    }
    let db = st.db().clone();
    let open = tokio::task::spawn_blocking(move || store::improvable(&db, target, &texts, store::Source::Deepl)).await.unwrap_or_default();
    let list: Vec<(String, Kind)> = items.iter().filter(|p| deepl::wanted(p.kind) && open.contains(&p.text)).map(|p| (p.text.clone(), p.kind)).collect();
    if list.is_empty() {
        return;
    }
    if let Err(e) = deepl::pass(st, target, &list).await {
        let level = if matches!(e, deepl::Error::Budget | deepl::Error::Busy) { "info" } else { "warning" };
        st.db().log(level, &format!("Translations: {e}"));
    }
}

/// A group listen is playing: the model's work waits (the rows stay waiting for it, and the worker is woken
/// when the last group listen stops), and its server goes now, so its memory and cores are free for the music.
async fn hold_for_groups(st: &impl Ctx) -> bool {
    if !st.library().busy.group_playing() {
        return false;
    }
    llama::stop(st).await;
    true
}

/// Ask the model for the items whose row still misses a translation or a romanization.
async fn model_pass(st: &impl Ctx, target: &'static str, items: Vec<Pending>, model_ok: &mut bool) {
    if !*model_ok || items.is_empty() || !st.cfg().bool("translate_local_model") || !llama::ready(st) || hold_for_groups(st).await {
        return;
    }
    let db = st.db().clone();
    let texts: Vec<String> = items.iter().map(|p| p.text.clone()).collect();
    let have = tokio::task::spawn_blocking(move || store::lookup(&db, target, &texts)).await.unwrap_or_default();
    let todo: Vec<&Pending> = items.iter().filter(|p| {
        let romanize = !matches!(script::detect(&p.text), "latn" | "");
        match have.get(&p.text) {
            Some(s) => s.translation.is_none() || (romanize && s.romanization.is_none()),
            None => true,
        }
    }).collect();
    let thinking = llama::model(st).thinking;
    for chunk in todo.chunks(prompt::BATCH) {
        if hold_for_groups(st).await {
            return;
        }
        let batch: Vec<Item> = chunk.iter().enumerate().map(|(i, p)| Item { id: i + 1, kind: p.kind, text: p.text.clone(), artist: p.artist.clone() }).collect();
        let reply = match llama::chat(st, prompt::messages(&batch, target, thinking), thinking).await {
            Ok(r) => r,
            Err(e) => {
                st.db().log("warning", &format!("Translator: {e}"));
                *model_ok = false;
                return;
            }
        };
        let got = prompt::parse(&reply, &batch);
        let db = st.db().clone();
        let _ = tokio::task::spawn_blocking(move || {
            for it in &batch {
                let (t, r) = got.get(&it.id).cloned().unwrap_or((None, None));
                let _ = store::put(&db, &it.text, target, it.kind.as_str(), t.as_deref(), r.as_deref(), store::Source::Model);
            }
        }).await;
    }
}

/// Library texts worth translating for `target`, with their kind and artist, in the order of the songs: every
/// distinct album artist, artist, album and title once (the first song that has it gives its artist).
fn candidates(idx: &crate::artists::Index, target: &'static str) -> Vec<Pending> {
    let mut seen = HashSet::new();
    let mut cands: Vec<Pending> = vec![];
    for (title, artist, album, albumartist) in idx.texts() {
        let ctx = Some(albumartist).filter(|a| !a.is_empty()).or_else(|| Some(artist).filter(|a| !a.is_empty())).map(str::to_string);
        for (text, kind, ctx) in [(albumartist, Kind::Artist, None), (artist, Kind::Artist, None), (album, Kind::Album, ctx), (title, Kind::Title, Some(artist.to_string()))] {
            let t = script::normalize(text);
            if !t.is_empty() && script::candidate(&t, target) && seen.insert(t.clone()) {
                cands.push(Pending { text: t, kind, artist: ctx, target });
            }
        }
    }
    cands
}

impl Translator {
    /// Library texts worth translating for `target` that have no row yet (at most `limit`), with their kind
    /// and artist: taken from the artist index (no scan of the songs table), checked against the stored
    /// rows from where the last batch left off.
    fn library_backlog(&self, db: &Db, artists: &crate::artists::Artists, target: &'static str, limit: usize) -> Vec<Pending> {
        let idx = artists.index(db);
        let mut all = self.0.backlog.lock().unwrap_or_else(|e| e.into_inner());
        let b = match all.get_mut(target) {
            Some(b) if Arc::ptr_eq(&b.index, &idx) => b,
            _ => {
                all.insert(target, Backlog { cands: candidates(&idx, target), index: idx, known_upto: 0 });
                all.get_mut(target).expect("just put in")
            }
        };
        let mut out = vec![];
        let mut at = b.known_upto;
        // rows are never taken back, so a text known once stays known: the start moves past the known ones
        let mut all_known_so_far = true;
        while at < b.cands.len() && out.len() < limit {
            let chunk = &b.cands[at..(at + 400).min(b.cands.len())];
            let keys: Vec<String> = chunk.iter().map(|p| p.text.clone()).collect();
            let known = store::known(db, target, &keys);
            for p in chunk {
                if known.contains(&p.text) {
                    if all_known_so_far {
                        b.known_upto += 1;
                    }
                } else {
                    all_known_so_far = false;
                    if out.len() < limit {
                        out.push(p.clone());
                    }
                }
            }
            at += chunk.len();
        }
        out
    }
}

/// One pass: queued titles first, then the library in batches, then rows that wait for the model.
async fn run_pass(st: &impl Ctx) {
    let db = st.db().clone();
    let tr = st.library().translator.clone();
    let targets = tokio::task::spawn_blocking(move || tr.active_targets(&db)).await.unwrap_or_default();
    let mut model_ok = true;
    // the rows each source was given in this pass: a row it cannot stamp is not given twice
    let mut backfilled: HashSet<(store::Source, String)> = HashSet::new();
    loop {
        let queued = st.library().translator.take_queued(BATCH);
        if !queued.is_empty() {
            let mut by_target: HashMap<&'static str, Vec<Pending>> = HashMap::new();
            for p in queued {
                by_target.entry(p.target).or_default().push(p);
            }
            for (t, items) in by_target {
                process(st, t, items, &mut model_ok).await;
            }
            continue;
        }
        if !st.cfg().bool("translate_background") {
            break;
        }
        let mut did = false;
        for &t in &targets {
            let (db, artists, tr) = (st.db().clone(), st.library().artists.clone(), st.library().translator.clone());
            let batch = tokio::task::spawn_blocking(move || tr.library_backlog(&db, &artists, t, BATCH)).await.unwrap_or_default();
            if !batch.is_empty() {
                did = true;
                process(st, t, batch, &mut model_ok).await;
                tokio::time::sleep(PAUSE).await;
            }
        }
        if !did {
            did = backfill(st, &targets, &mut backfilled).await;
        }
        if !did {
            break;
        }
    }
    // rows the official pass looked at before the Translator was installed
    if st.cfg().bool("translate_background") && st.cfg().bool("translate_local_model") && llama::ready(st) {
        for &t in &targets {
            loop {
                // (a group listen playing: these wait for the pass after it, which its end starts)
                if !model_ok || hold_for_groups(st).await {
                    return;
                }
                let db = st.db().clone();
                let rows = tokio::task::spawn_blocking(move || store::waiting_for_model(&db, t, prompt::BATCH * 2)).await.unwrap_or_default();
                if rows.is_empty() {
                    break;
                }
                let items: Vec<Pending> = rows.into_iter().map(|(text, kind)| Pending { text, kind: Kind::parse(&kind), artist: None, target: t }).collect();
                let texts: Vec<String> = items.iter().map(|p| p.text.clone()).collect();
                model_pass(st, t, items, &mut model_ok).await;
                if !model_ok || st.library().busy.group_playing() {
                    // the Translator failed, or a group listen started: leave these waiting for a later pass
                    return;
                }
                // whatever the answer, these were asked
                let db = st.db().clone();
                let _ = tokio::task::spawn_blocking(move || {
                    for text in texts {
                        let _ = store::mark(&db, &text, t, "title", store::Source::Model);
                    }
                }).await;
                if st.library().translator.queued() > 0 {
                    break;
                }
                tokio::time::sleep(PAUSE).await;
            }
        }
    }
}

/// Rows a source has not looked at yet – made while it was off, or before this version – one batch per source
/// and target language, in the tier order (the model's turn comes after, in `run_pass`). Whether there were
/// any: each batch is stamped, and `given` keeps a row from going to the same source twice in a pass, so the
/// passes come to an end.
async fn backfill(st: &impl Ctx, targets: &[&'static str], given: &mut HashSet<(store::Source, String)>) -> bool {
    use store::Source;
    let t = tiers::Tiers::read(st.cfg());
    let mut did = false;
    for &target in targets {
        let mut sources = vec![];
        if t.official {
            sources.push(Source::Official);
        }
        if t.romanize {
            sources.push(Source::Romanized);
        }
        if t.deepl_ready() && deepl::target_lang(target).is_some() && st.library().translator.deepl_usable(t.deepl_generation()) {
            sources.push(Source::Deepl);
        }
        for source in sources {
            let limit = match source {
                Source::Romanized => 500,
                Source::Deepl => deepl::BATCH,
                _ => BATCH,
            };
            let db = st.db().clone();
            let rows = tokio::task::spawn_blocking(move || store::unseen(&db, target, source, limit)).await.unwrap_or_default();
            let rows: Vec<(String, String)> = rows.into_iter().filter(|(text, _)| given.insert((source, format!("{target}\u{0}{text}")))).collect();
            if rows.is_empty() {
                continue;
            }
            did = true;
            let items: Vec<Pending> = rows.into_iter().map(|(text, kind)| Pending { text, kind: Kind::parse(&kind), artist: None, target }).collect();
            match source {
                Source::Official => official_pass(st, target, &items).await,
                Source::Romanized => romanize_pass(st, target, &items).await,
                // what DeepL did not get to (it paused) stays unseen until the pause ends
                _ => deepl_pass(st, target, &items).await,
            }
            if source != Source::Romanized {
                tokio::time::sleep(PAUSE).await;
            }
            if st.library().translator.queued() > 0 {
                return true;
            }
        }
    }
    did
}

/// One pass now, as the worker makes them (tests; the worker runs them by itself).
pub async fn pass_now(st: &impl Ctx) {
    run_pass(st).await;
}

/// The background worker: a pass when woken (titles on screen, settings changed) and every 30
/// minutes (new library songs).
pub async fn worker(st: impl Ctx) {
    tokio::time::sleep(Duration::from_secs(60)).await;
    let tr = st.library().translator.clone();
    loop {
        run_pass(&st).await;
        tokio::select! {
            _ = tr.woken() => tokio::time::sleep(Duration::from_secs(2)).await,
            _ = tokio::time::sleep(Duration::from_secs(1800)) => {}
        }
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
