//! Recommendations made on this server alone: smart shuffle, "For you" mixes, genre mixes and "More like this"
//! from the library's own songs, without any outside service.
//!
//! Three parts, all plain data in and plain data out (the API reads the database and caches them):
//!
//! - [`Catalog`]: every song of the library reduced to what the model looks at – its artists, genres (from the
//!   files' tags, or the artist's usual genres when a song has none), decade and album. Made once per state of the
//!   library.
//! - [`CoListen`]: which artists the listeners of this server play together. Two artists heard in the same
//!   listening session (plays less than [`SESSION_GAP`] apart, a few songs from each other) are neighbours; the
//!   counts become a cosine similarity. With one listener it still learns their own sessions; with many it is a
//!   small collaborative filter.
//! - [`Profile`]: one listener's taste, from their [`Signals`]: plays (fading with age), Liked Songs (a strong
//!   "yes"), songs on their own playlists, skips (a mild "not now") and dislikes (a hard "no"). Each signal adds to
//!   the song itself, to its artists, its genres and its decade.
//!
//! A song's score ([`Profile::score`]) mixes how much the listener likes its artists, genres and era, how close
//! its artists are to the ones they play (co-listening), and the song's own history, less a penalty for songs
//! heard in the last hours or skipped lately. Shuffles draw songs at random *weighted* by that score (so favourites
//! come up more often but anything can), then spread the artists out so no artist plays twice in a row.
//! Disliked songs, disliked artists and songs skipped again and again are never drawn.

use std::collections::{HashMap, HashSet};

/// Plays further apart than this (seconds) are separate listening sessions for [`CoListen`].
pub const SESSION_GAP: f64 = 30.0 * 60.0;
/// How many songs either side of a play in a session count as heard "together" with it.
const SESSION_REACH: usize = 3;
/// Most neighbours kept per artist in [`CoListen`].
const MAX_NEIGHBOURS: usize = 40;
/// A play counts half as much after this long (seconds): taste drifts, old plays fade.
const PLAY_HALF_LIFE: f64 = 120.0 * 86400.0;
/// A skip counts half as much after this long (seconds): "not now" is forgotten faster than a play.
const SKIP_HALF_LIFE: f64 = 21.0 * 86400.0;
/// Only skips this recent (seconds) count at all.
pub const SKIP_WINDOW: f64 = 90.0 * 86400.0;
/// This many recent skips of one song (weighted by age, see [`SKIP_HALF_LIFE`]) and it is left out like a
/// disliked one until they fade.
const SKIPS_TO_DROP: f64 = 2.5;
/// What one play adds (before fading).
const W_PLAY: f64 = 1.0;
/// What a liked song adds: a like says more than a play.
const W_LIKE: f64 = 4.0;
/// What a song on one of the listener's own playlists adds.
const W_PLAYLIST: f64 = 0.75;
/// What a skip takes away (before fading).
const W_SKIP: f64 = -0.6;
/// What a disliked song takes from its artists, genres and decade (the song itself is left out anyway).
const W_DISLIKE: f64 = -3.0;
/// The temperature of the weighted draw: lower follows the score more closely, higher is closer to a plain
/// shuffle. At 0.3 the best-liked songs come up about 20 times as often as ones the listener never played.
const TEMPERATURE: f64 = 0.3;
/// Tag values that say nothing about the music.
const NOT_GENRES: &[&str] = &["other", "unknown", "misc", "miscellaneous", "music", "none", "genre", "various", "n/a", "default"];

/// One song as the API reads it from the database: the input of [`Catalog::build`].
#[derive(Clone, Debug, Default)]
pub struct SongIn {
    /// the song's id (`tracks.id`)
    pub id: i64,
    /// every artist the song credits, first the main one (names as written)
    pub artists: Vec<String>,
    /// the album the song is on (`tracks.album_id`)
    pub album_id: String,
    /// the genre tag as the file has it ("Rock; Indie Rock", "Hip-Hop/Rap" …; empty when none)
    pub genre: String,
    /// the year or date tag ("1997", "1997-05-21"; empty when none)
    pub year: String,
}

/// One song as the model keeps it.
#[derive(Clone, Debug)]
pub struct Song {
    /// the song's id
    pub id: i64,
    /// its artists (indexes into [`Catalog::artists`]), the main one first; never empty
    pub artists: Vec<u32>,
    /// its genres (indexes into [`Catalog::genres`]): the song's own tags, else its main artist's usual ones
    pub genres: Vec<u16>,
    /// the decade it came out (1990 for 1997), if known
    pub decade: Option<u16>,
    /// its album (index into the catalog's album list), so a mix can spread albums out too
    pub album: u32,
}

/// Every song of the library as the model sees it, with the artist and genre names its indexes point at.
#[derive(Debug, Default)]
pub struct Catalog {
    /// the songs
    pub songs: Vec<Song>,
    /// the artists' names as first written, by index
    pub artists: Vec<String>,
    /// the genres (lower-case, "hip hop" for "Hip-Hop"), by index
    pub genres: Vec<String>,
    /// the songs of each genre (indexes into `songs`), by genre index
    pub genre_songs: Vec<Vec<usize>>,
    /// the songs of each artist (indexes into `songs`), by artist index
    pub artist_songs: Vec<Vec<usize>>,
    /// song id → index into `songs`
    by_id: HashMap<i64, usize>,
    /// lower-cased artist name → artist index
    artist_by_key: HashMap<String, u32>,
    /// genre name → genre index
    genre_by_key: HashMap<String, u16>,
}

/// The key two spellings of one artist share: lower-case, trimmed.
pub fn artist_key(name: &str) -> String {
    name.trim().to_lowercase()
}

/// The genres a genre tag names: split at `;`, `/`, `,`, `|` and NUL (how taggers join several), lower-cased,
/// with `-` and `_` read as spaces ("Hip-Hop" and "hip hop" are one genre), without values that are no genre.
pub fn split_genres(tag: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for part in tag.split([';', '/', ',', '|', '\0']) {
        let g: String = part.trim().to_lowercase().replace(['-', '_'], " ").split_whitespace().collect::<Vec<_>>().join(" ");
        if g.is_empty() || g.len() > 40 || NOT_GENRES.contains(&g.as_str()) || g.chars().all(|c| c.is_ascii_digit() || c == '(' || c == ')') {
            continue;
        }
        if !out.contains(&g) {
            out.push(g);
        }
    }
    out
}

/// The decade of a year or date tag ("1997-05-21" → 1990); none for anything that doesn't start with a
/// plausible year.
pub fn decade_of(year: &str) -> Option<u16> {
    let y: u16 = year.trim().get(..4)?.parse().ok()?;
    (1000..=2999).contains(&y).then_some(y / 10 * 10)
}

impl Catalog {
    /// The catalog of these songs. Songs without a genre tag take their main artist's two most common genres
    /// (from the artist's other songs), so an untagged song still lands in the right genre mixes.
    pub fn build(input: Vec<SongIn>) -> Catalog {
        let mut c = Catalog::default();
        let mut album_by_key: HashMap<String, u32> = HashMap::new();
        // the genres each song's own tag names, before the artist fallback
        let mut own: Vec<Vec<u16>> = Vec::with_capacity(input.len());
        for s in input {
            let mut artists: Vec<u32> = vec![];
            for name in s.artists.iter().filter(|n| !n.trim().is_empty()) {
                let a = c.artist(name);
                if !artists.contains(&a) {
                    artists.push(a);
                }
            }
            if artists.is_empty() {
                artists.push(c.artist("Unknown artist"));
            }
            let genres: Vec<u16> = split_genres(&s.genre).iter().map(|g| c.genre(g)).collect();
            let next = album_by_key.len() as u32;
            let album = *album_by_key.entry(s.album_id).or_insert(next);
            c.by_id.insert(s.id, c.songs.len());
            own.push(genres.clone());
            c.songs.push(Song { id: s.id, artists, genres, decade: decade_of(&s.year), album });
        }
        // each artist's usual genres: the two its tagged songs name most
        let mut counts: HashMap<u32, HashMap<u16, u32>> = HashMap::new();
        for (song, gs) in c.songs.iter().zip(&own) {
            for g in gs {
                *counts.entry(song.artists[0]).or_default().entry(*g).or_default() += 1;
            }
        }
        let usual: HashMap<u32, Vec<u16>> = counts.into_iter().map(|(a, m)| {
            let mut v: Vec<(u16, u32)> = m.into_iter().collect();
            v.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.cmp(&y.0)));
            (a, v.into_iter().take(2).map(|(g, _)| g).collect())
        }).collect();
        for song in c.songs.iter_mut().filter(|s| s.genres.is_empty()) {
            if let Some(gs) = usual.get(&song.artists[0]) {
                song.genres = gs.clone();
            }
        }
        c.genre_songs = vec![vec![]; c.genres.len()];
        c.artist_songs = vec![vec![]; c.artists.len()];
        for (i, s) in c.songs.iter().enumerate() {
            for g in &s.genres {
                c.genre_songs[*g as usize].push(i);
            }
            for a in &s.artists {
                c.artist_songs[*a as usize].push(i);
            }
        }
        c
    }

    /// The index of an artist, added when new.
    fn artist(&mut self, name: &str) -> u32 {
        let key = artist_key(name);
        if let Some(i) = self.artist_by_key.get(&key) {
            return *i;
        }
        let i = self.artists.len() as u32;
        self.artists.push(name.trim().to_string());
        self.artist_by_key.insert(key, i);
        i
    }

    /// The index of a genre (already split and lower-cased), added when new.
    fn genre(&mut self, g: &str) -> u16 {
        if let Some(i) = self.genre_by_key.get(g) {
            return *i;
        }
        let i = self.genres.len() as u16;
        self.genres.push(g.to_string());
        self.genre_by_key.insert(g.to_string(), i);
        i
    }

    /// The index into `songs` of a song id.
    pub fn index(&self, id: i64) -> Option<usize> {
        self.by_id.get(&id).copied()
    }

    /// The index of an artist by name (any case).
    pub fn artist_index(&self, name: &str) -> Option<u32> {
        self.artist_by_key.get(&artist_key(name)).copied()
    }

    /// The index of a genre by name ("Hip-Hop", "hip hop" …).
    pub fn genre_index(&self, name: &str) -> Option<u16> {
        split_genres(name).first().and_then(|g| self.genre_by_key.get(g)).copied()
    }

    /// The number of songs.
    pub fn len(&self) -> usize {
        self.songs.len()
    }

    /// No songs at all.
    pub fn is_empty(&self) -> bool {
        self.songs.is_empty()
    }
}

/// Which artists the listeners of this server play together: for each artist its closest neighbours with a
/// cosine similarity in 0..=1.
#[derive(Debug, Default)]
pub struct CoListen {
    /// artist index → its neighbours (artist index, similarity), best first
    neighbours: HashMap<u32, Vec<(u32, f32)>>,
}

impl CoListen {
    /// Co-listening from every listener's plays `(user, song id, played_at)`, in any order. Songs the catalog
    /// doesn't know (deleted since) are left out.
    pub fn build(cat: &Catalog, mut plays: Vec<(i64, i64, f64)>) -> CoListen {
        plays.sort_by(|a, b| a.0.cmp(&b.0).then(a.2.total_cmp(&b.2)));
        // how often each artist is heard, and each pair of artists together (key: smaller index first)
        let mut seen: HashMap<u32, f64> = HashMap::new();
        let mut pairs: HashMap<(u32, u32), f64> = HashMap::new();
        let mut session: Vec<(u32, f64)> = vec![]; // (main artist, played_at) of the session so far
        let mut user = i64::MIN;
        for (u, id, at) in plays {
            let Some(i) = cat.index(id) else { continue };
            let a = cat.songs[i].artists[0];
            if u != user || session.last().is_some_and(|(_, t)| at - t > SESSION_GAP) {
                session.clear();
                user = u;
            }
            *seen.entry(a).or_default() += 1.0;
            // the last few songs of this session were heard together with this one; nearer ones count more
            for (k, (b, _)) in session.iter().rev().take(SESSION_REACH).enumerate() {
                if *b != a {
                    *pairs.entry((a.min(*b), a.max(*b))).or_default() += 1.0 / (k as f64 + 1.0);
                }
            }
            session.push((a, at));
        }
        let mut neighbours: HashMap<u32, Vec<(u32, f32)>> = HashMap::new();
        for ((a, b), n) in pairs {
            // cosine: together / √(heard a · heard b), damped for pairs heard together only once
            let sim = n / (seen[&a] * seen[&b]).sqrt() * (n / (n + 1.0));
            neighbours.entry(a).or_default().push((b, sim as f32));
            neighbours.entry(b).or_default().push((a, sim as f32));
        }
        for list in neighbours.values_mut() {
            list.sort_by(|x, y| y.1.total_cmp(&x.1).then(x.0.cmp(&y.0)));
            list.truncate(MAX_NEIGHBOURS);
        }
        CoListen { neighbours }
    }

    /// The neighbours of an artist (artist index, similarity), best first.
    pub fn of(&self, artist: u32) -> &[(u32, f32)] {
        self.neighbours.get(&artist).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// How close two artists are (0 when never heard together; 1 for the same artist).
    pub fn sim(&self, a: u32, b: u32) -> f64 {
        if a == b {
            return 1.0;
        }
        self.of(a).iter().find(|(x, _)| *x == b).map(|(_, s)| *s as f64).unwrap_or(0.0)
    }
}

/// What one listener told the server, read from the database: the input of [`Profile::build`].
#[derive(Clone, Debug, Default)]
pub struct Signals {
    /// their plays: (song id, played_at)
    pub plays: Vec<(i64, f64)>,
    /// their Liked Songs (song ids)
    pub likes: Vec<i64>,
    /// the songs on their own playlists (song ids, each once)
    pub playlisted: Vec<i64>,
    /// their skips: (song id, skipped_at)
    pub skips: Vec<(i64, f64)>,
    /// the songs they disliked
    pub disliked_songs: HashSet<i64>,
    /// the artists they disliked (lower-cased names, see [`artist_key`])
    pub disliked_artists: HashSet<String>,
}

/// One listener's taste: how much they like each artist, genre and decade, each song's own history, and what
/// is never to be played for them.
#[derive(Debug, Default)]
pub struct Profile {
    /// artist index → affinity in -1..=1
    pub artist: HashMap<u32, f64>,
    /// genre index → affinity in -1..=1
    pub genre: HashMap<u16, f64>,
    /// decade → affinity in -1..=1
    pub decade: HashMap<u16, f64>,
    /// artist index → how close it is to what they play (co-listening), 0..=1
    pub near: HashMap<u32, f64>,
    /// song index → the song's own affinity in -1..=1
    pub song: HashMap<usize, f64>,
    /// song index → when they last played it
    pub last_played: HashMap<usize, f64>,
    /// song index → how many times they played it (unweighted)
    pub play_count: HashMap<usize, u32>,
    /// song index → the faded weight of their recent skips of it
    pub skipped: HashMap<usize, f64>,
    /// the songs in Liked Songs (indexes)
    pub liked: HashSet<usize>,
    /// songs never to be drawn: disliked, by a disliked artist, or skipped again and again (indexes)
    pub blocked: HashSet<usize>,
    /// the time the profile was made for (unix seconds)
    pub now: f64,
    /// no signal at all yet: every song scores the same and a smart shuffle is a plain one
    pub cold: bool,
}

/// `x` halved every `half_life` seconds since `at` (no growth for times in the future).
fn fade(at: f64, now: f64, half_life: f64) -> f64 {
    0.5f64.powf(((now - at).max(0.0)) / half_life)
}

/// Raw affinities squeezed into -1..=1: divided by a "typical strong" value (the mean of the ten largest, at
/// least 1) and passed through tanh, so one artist with thousands of plays doesn't flatten everyone else.
fn squash<K: std::hash::Hash + Eq + Copy>(raw: HashMap<K, f64>) -> HashMap<K, f64> {
    let mut top: Vec<f64> = raw.values().copied().filter(|v| *v > 0.0).collect();
    top.sort_by(|a, b| b.total_cmp(a));
    top.truncate(10);
    let scale = if top.is_empty() { 1.0 } else { (top.iter().sum::<f64>() / top.len() as f64).max(1.0) };
    raw.into_iter().map(|(k, v)| (k, (v / scale * 1.5).tanh())).collect()
}

impl Profile {
    /// The taste of a listener at time `now` from their signals, with the server's co-listening.
    pub fn build(cat: &Catalog, co: &CoListen, sig: &Signals, now: f64) -> Profile {
        let mut p = Profile { now, ..Default::default() };
        let mut artist: HashMap<u32, f64> = HashMap::new();
        let mut genre: HashMap<u16, f64> = HashMap::new();
        let mut decade: HashMap<u16, f64> = HashMap::new();
        let mut song: HashMap<usize, f64> = HashMap::new();
        // one signal of weight `w` on song `i`: to the song, its artists, its genres and its decade (shared out, so a
        // song with five genres doesn't count five times)
        let mut add = |i: usize, w: f64| {
            let s = &cat.songs[i];
            *song.entry(i).or_default() += w;
            for a in &s.artists {
                *artist.entry(*a).or_default() += w / s.artists.len() as f64;
            }
            for g in &s.genres {
                *genre.entry(*g).or_default() += w / s.genres.len() as f64;
            }
            if let Some(d) = s.decade {
                *decade.entry(d).or_default() += w * 0.5;
            }
        };
        let mut any = false;
        for (id, at) in &sig.plays {
            let Some(i) = cat.index(*id) else { continue };
            any = true;
            add(i, W_PLAY * fade(*at, now, PLAY_HALF_LIFE));
            let last = p.last_played.entry(i).or_insert(*at);
            *last = last.max(*at);
            *p.play_count.entry(i).or_default() += 1;
        }
        for id in &sig.likes {
            let Some(i) = cat.index(*id) else { continue };
            any = true;
            add(i, W_LIKE);
            p.liked.insert(i);
        }
        for id in &sig.playlisted {
            let Some(i) = cat.index(*id) else { continue };
            any = true;
            add(i, W_PLAYLIST);
        }
        for (id, at) in &sig.skips {
            let Some(i) = cat.index(*id) else { continue };
            if now - at > SKIP_WINDOW {
                continue;
            }
            any = true;
            let w = fade(*at, now, SKIP_HALF_LIFE);
            add(i, W_SKIP * w);
            *p.skipped.entry(i).or_default() += w;
        }
        for id in &sig.disliked_songs {
            let Some(i) = cat.index(*id) else { continue };
            any = true;
            add(i, W_DISLIKE);
            p.blocked.insert(i);
        }
        for name in &sig.disliked_artists {
            let Some(a) = cat.artist_index(name) else { continue };
            any = true;
            // a disliked artist is out with every song it is the main artist of; a feature on someone else's song
            // only pulls that song down
            for i in &cat.artist_songs[a as usize] {
                if cat.songs[*i].artists[0] == a {
                    p.blocked.insert(*i);
                }
            }
            artist.insert(a, -1e6);
        }
        for (i, w) in &p.skipped {
            // a song still liked is never dropped for its skips: it only sinks for a while
            if *w >= SKIPS_TO_DROP && !p.liked.contains(i) {
                p.blocked.insert(*i);
            }
        }
        p.artist = squash(artist);
        p.genre = squash(genre);
        p.decade = squash(decade);
        p.song = song.into_iter().map(|(k, v)| (k, (v / 3.0).tanh())).collect();
        // co-listening: the artists near the ones they like, each by the sum of liked artist × similarity
        let mut near: HashMap<u32, f64> = HashMap::new();
        for (a, aff) in p.artist.iter().filter(|(_, v)| **v > 0.0) {
            for (b, sim) in co.of(*a) {
                *near.entry(*b).or_default() += aff * *sim as f64;
            }
        }
        let max = near.values().copied().fold(0.0, f64::max);
        if max > 0.0 {
            p.near = near.into_iter().map(|(k, v)| (k, v / max)).collect();
        }
        p.cold = !any;
        p
    }

    /// How well a song fits this listener, about -1..=1 (higher is better). Songs heard in the last hours score
    /// lower for a while, so a shuffle doesn't repeat itself, and recent skips pull a song down.
    pub fn score(&self, cat: &Catalog, i: usize) -> f64 {
        let s = &cat.songs[i];
        let mean = |it: &mut dyn Iterator<Item = f64>, n: usize| if n == 0 { 0.0 } else { it.sum::<f64>() / n as f64 };
        let artist = mean(&mut s.artists.iter().map(|a| self.artist.get(a).copied().unwrap_or(0.0)), s.artists.len());
        let genre = mean(&mut s.genres.iter().map(|g| self.genre.get(g).copied().unwrap_or(0.0)), s.genres.len());
        let era = s.decade.and_then(|d| self.decade.get(&d)).copied().unwrap_or(0.0);
        let near = s.artists.iter().map(|a| self.near.get(a).copied().unwrap_or(0.0)).fold(0.0, f64::max);
        let own = self.song.get(&i).copied().unwrap_or(0.0);
        let mut score = 0.30 * artist + 0.25 * genre + 0.08 * era + 0.17 * near + 0.20 * own;
        if let Some(last) = self.last_played.get(&i) {
            // just heard: strongly down for the next hours, a little for the next days
            let ago = (self.now - last).max(0.0);
            score -= 0.8 * (-ago / (6.0 * 3600.0)).exp() + 0.25 * (-ago / (3.0 * 86400.0)).exp();
        }
        if let Some(w) = self.skipped.get(&i) {
            score -= 0.25 * w;
        }
        score
    }

    /// Whether a song may be drawn at all (not disliked, not by a disliked artist, not skipped again and again).
    pub fn allowed(&self, i: usize) -> bool {
        !self.blocked.contains(&i)
    }
}

/// A small, fast random source (SplitMix64): the library crate has no `rand`, and a seed makes tests repeatable.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    /// A source started from `seed` (the API seeds it from the clock).
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    /// The next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in 0..1 (never exactly 0, so its logarithm is finite).
    pub fn unit(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
}

/// `n` songs out of `candidates` (song index, score), drawn at random weighted by `exp(score / temperature)`
/// without repeats (Efraimidis–Spirakis: each song's key is `ln(u) / weight`, the largest keys win), then put in
/// an order that keeps one artist from playing twice within three songs and holds each artist (and each album)
/// to about an eighth of the list while others are left. Returns song indexes.
pub fn weighted_draw(cat: &Catalog, candidates: &[(usize, f64)], n: usize, temperature: f64, rng: &mut Rng) -> Vec<usize> {
    if n == 0 || candidates.is_empty() {
        return vec![];
    }
    // the key in log space: ln(ln(1/u)) - score/T, smallest first (the same order as u^(1/w), largest first,
    // without overflowing for big weights)
    let mut keyed: Vec<(f64, usize)> = candidates.iter().map(|(i, s)| ((-rng.unit().ln()).ln() - s / temperature, *i)).collect();
    keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
    let order: Vec<usize> = keyed.into_iter().map(|(_, i)| i).take(n.saturating_mul(20).max(n + 500)).collect();
    spread(cat, &order, n)
}

/// Up to `n` songs out of `order` (best first), keeping the order where it can while spreading artists and albums
/// out (see [`weighted_draw`]). Each place takes the first song (within the next [`LOOK_AHEAD`]) that breaks no
/// rule; when none does, the rules are eased one by one – first the share per artist and album, then the gap
/// between songs of one artist – so the list is always as long as the songs allow.
fn spread(cat: &Catalog, order: &[usize], n: usize) -> Vec<usize> {
    let cap = (n / 8).max(2);
    let mut rest: Vec<usize> = order.to_vec();
    let mut out: Vec<usize> = Vec::with_capacity(n.min(rest.len()));
    let mut per_artist: HashMap<u32, usize> = HashMap::new();
    let mut per_album: HashMap<u32, usize> = HashMap::new();
    while out.len() < n && !rest.is_empty() {
        // the main artists of the last three songs
        let recent: Vec<u32> = out.iter().rev().take(3).map(|j| cat.songs[*j].artists[0]).collect();
        let near = |i: &usize| !recent.contains(&cat.songs[*i].artists[0]);
        let share = |i: &usize| {
            let s = &cat.songs[*i];
            per_artist.get(&s.artists[0]).copied().unwrap_or(0) < cap && per_album.get(&s.album).copied().unwrap_or(0) < cap
        };
        let window = &rest[..rest.len().min(LOOK_AHEAD)];
        let at = window.iter().position(|i| near(i) && share(i)).or_else(|| window.iter().position(near)).unwrap_or(0);
        let i = rest.remove(at);
        let s = &cat.songs[i];
        *per_artist.entry(s.artists[0]).or_default() += 1;
        *per_album.entry(s.album).or_default() += 1;
        out.push(i);
    }
    out
}

/// How far down the drawn order [`spread`] looks for a song that fits the rules.
const LOOK_AHEAD: usize = 400;

/// Which songs a smart shuffle or a mix may draw from.
#[derive(Clone, Debug, Default)]
pub enum Scope {
    /// the whole library
    #[default]
    All,
    /// one genre's songs
    Genre(u16),
    /// only these songs (a playlist or an album being shuffled), by index
    Songs(Vec<usize>),
    /// songs the listener never played and hasn't liked: new finds
    Fresh,
    /// songs they played or liked before but not in the last month: old favourites
    Rediscover,
}

impl Profile {
    /// The songs a scope covers that may be drawn, with their scores, leaving out `exclude` (song ids).
    pub fn candidates(&self, cat: &Catalog, scope: &Scope, exclude: &HashSet<i64>) -> Vec<(usize, f64)> {
        let ok = |i: &usize| self.allowed(*i) && !exclude.contains(&cat.songs[*i].id);
        let month = 30.0 * 86400.0;
        let list: Box<dyn Iterator<Item = usize> + '_> = match scope {
            Scope::All => Box::new(0..cat.len()),
            Scope::Genre(g) => Box::new(cat.genre_songs.get(*g as usize).into_iter().flatten().copied()),
            Scope::Songs(v) => Box::new(v.iter().copied().filter(|i| *i < cat.len())),
            Scope::Fresh => Box::new((0..cat.len()).filter(|i| !self.play_count.contains_key(i) && !self.liked.contains(i))),
            Scope::Rediscover => Box::new((0..cat.len()).filter(move |i| {
                let known = self.liked.contains(i) || self.play_count.get(i).copied().unwrap_or(0) >= 2;
                known && self.last_played.get(i).is_none_or(|t| self.now - t > month)
            })),
        };
        list.filter(ok).map(|i| (i, self.score(cat, i))).collect()
    }

    /// A smart shuffle: `n` songs of the scope weighted by taste (see [`weighted_draw`]). A listener with no
    /// signals yet gets a plain shuffle (every score is the same).
    pub fn shuffle(&self, cat: &Catalog, scope: &Scope, n: usize, exclude: &HashSet<i64>, rng: &mut Rng) -> Vec<i64> {
        let cands = self.candidates(cat, scope, exclude);
        weighted_draw(cat, &cands, n, TEMPERATURE, rng).into_iter().map(|i| cat.songs[i].id).collect()
    }

    /// "More like this": `n` songs that sound like the seed song – the same or co-listened artists, shared genres,
    /// a close decade – nudged towards this listener's taste, without the seed itself or `exclude`. Up to a quarter
    /// of the list may be the seed's own artist.
    pub fn like_this(&self, cat: &Catalog, co: &CoListen, seed: usize, n: usize, exclude: &HashSet<i64>, rng: &mut Rng) -> Vec<i64> {
        let s = &cat.songs[seed];
        let main = s.artists[0];
        let mut cands: Vec<(usize, f64)> = vec![];
        for i in 0..cat.len() {
            let t = &cat.songs[i];
            if i == seed || !self.allowed(i) || exclude.contains(&t.id) {
                continue;
            }
            let sim = similarity(co, s, t);
            if sim <= 0.05 {
                continue;
            }
            // mostly how alike they are, a little how much this listener likes the candidate
            cands.push((i, sim + 0.25 * self.score(cat, i)));
        }
        // the closest few hundred, then a sharper weighted draw than a shuffle's (the list should stay on topic)
        cands.sort_by(|a, b| b.1.total_cmp(&a.1));
        cands.truncate(400.max(n * 8));
        let mut picked = weighted_draw(cat, &cands, n * 2, 0.12, rng);
        // the seed's own artist may fill at most a quarter: the point is to find others like it
        let own_cap = (n / 4).max(1);
        let mut own = 0;
        picked.retain(|i| {
            if cat.songs[*i].artists[0] != main {
                return true;
            }
            own += 1;
            own <= own_cap
        });
        picked.truncate(n);
        picked.into_iter().map(|i| cat.songs[i].id).collect()
    }
}

/// How alike two songs are, 0..=1: their artists (the same, or how often heard together), the genres they share
/// (Jaccard) and how close their decades are.
pub fn similarity(co: &CoListen, a: &Song, b: &Song) -> f64 {
    let artist = a.artists.iter().flat_map(|x| b.artists.iter().map(move |y| co.sim(*x, *y))).fold(0.0, f64::max);
    let shared = a.genres.iter().filter(|g| b.genres.contains(g)).count();
    let union = a.genres.len() + b.genres.len() - shared;
    let genre = if union == 0 { 0.0 } else { shared as f64 / union as f64 };
    let era = match (a.decade, b.decade) {
        (Some(x), Some(y)) => (1.0 - (x as f64 - y as f64).abs() / 30.0).max(0.0),
        _ => 0.3,
    };
    0.45 * artist + 0.40 * genre + 0.15 * era
}

/// One ready-made mix for the "For you" page.
#[derive(Clone, Debug)]
pub struct Mix {
    /// what to ask for it again (`/api/taste/mix/{id}`): `you`, `fresh`, `rediscover`, `genre:<name>`,
    /// `artist:<name>`
    pub id: String,
    /// its name ("Your mix", "Indie rock mix" …)
    pub title: String,
    /// one line under the name
    pub subtitle: String,
    /// its songs (ids), in play order
    pub songs: Vec<i64>,
}

/// A genre written for a title: "indie rock" → "Indie rock", "r&b" → "R&B".
pub fn genre_title(g: &str) -> String {
    if g.len() <= 3 {
        return g.to_uppercase();
    }
    let mut c = g.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

impl Profile {
    /// The genres this listener likes most (genre index, affinity), best first, each with at least `min_songs`
    /// songs in the library. A cold listener gets the library's biggest genres instead.
    pub fn top_genres(&self, cat: &Catalog, k: usize, min_songs: usize) -> Vec<(u16, f64)> {
        let mut v: Vec<(u16, f64)> = (0..cat.genres.len() as u16)
            .filter(|g| cat.genre_songs[*g as usize].len() >= min_songs)
            .map(|g| (g, self.genre.get(&g).copied().unwrap_or(0.0)))
            .filter(|(_, a)| self.cold || *a > 0.0)
            .collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1).then(cat.genre_songs[b.0 as usize].len().cmp(&cat.genre_songs[a.0 as usize].len())));
        v.truncate(k);
        v
    }

    /// The artists this listener likes most (artist index, affinity), best first.
    pub fn top_artists(&self, k: usize) -> Vec<(u32, f64)> {
        let mut v: Vec<(u32, f64)> = self.artist.iter().filter(|(_, a)| **a > 0.0).map(|(k, a)| (*k, *a)).collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        v.truncate(k);
        v
    }

    /// An artist mix: songs by the artist and the artists heard with it, weighted by taste.
    pub fn artist_mix(&self, cat: &Catalog, co: &CoListen, a: u32, n: usize, exclude: &HashSet<i64>, rng: &mut Rng) -> Vec<i64> {
        let mut pool: Vec<usize> = cat.artist_songs.get(a as usize).cloned().unwrap_or_default();
        for (b, _) in co.of(a).iter().take(12) {
            pool.extend(cat.artist_songs[*b as usize].iter().copied());
        }
        // artists that share the main genres when nobody has heard this one with others yet
        if pool.len() < n * 2 {
            if let Some(g) = cat.artist_songs.get(a as usize).and_then(|v| v.first()).and_then(|i| cat.songs[*i].genres.first()) {
                pool.extend(cat.genre_songs[*g as usize].iter().copied());
            }
        }
        pool.sort_unstable();
        pool.dedup();
        let cands: Vec<(usize, f64)> = self.candidates(cat, &Scope::Songs(pool), exclude).into_iter().map(|(i, s)| {
            // the artist itself a little ahead, so the mix stays recognisably theirs
            (i, s + if cat.songs[i].artists.contains(&a) { 0.3 } else { 0.0 })
        }).collect();
        weighted_draw(cat, &cands, n, TEMPERATURE, rng).into_iter().map(|i| cat.songs[i].id).collect()
    }

    /// The "For you" mixes: *Your mix* (the whole library by taste), *Fresh finds* (songs never played that fit),
    /// *Rediscover* (old favourites not heard for a month), a mix for each of the top genres and for the top
    /// artists. Mixes with fewer than 5 songs are left out.
    pub fn mixes(&self, cat: &Catalog, co: &CoListen, n: usize, rng: &mut Rng) -> Vec<Mix> {
        let none = HashSet::new();
        let mut out = vec![
            Mix { id: "you".into(), title: "Your mix".into(), subtitle: "The library, by what you like".into(), songs: self.shuffle(cat, &Scope::All, n, &none, rng) },
            Mix { id: "fresh".into(), title: "Fresh finds".into(), subtitle: "Songs you haven't played yet that fit your taste".into(), songs: self.shuffle(cat, &Scope::Fresh, n, &none, rng) },
            Mix { id: "rediscover".into(), title: "Rediscover".into(), subtitle: "Favourites you haven't heard in a while".into(), songs: self.shuffle(cat, &Scope::Rediscover, n, &none, rng) },
        ];
        for (g, _) in self.top_genres(cat, 6, 8) {
            let name = &cat.genres[g as usize];
            out.push(Mix { id: format!("genre:{name}"), title: format!("{} mix", genre_title(name)), subtitle: "Genre mix".into(), songs: self.shuffle(cat, &Scope::Genre(g), n, &none, rng) });
        }
        for (a, _) in self.top_artists(3) {
            let name = &cat.artists[a as usize];
            out.push(Mix { id: format!("artist:{name}"), title: format!("{name} radio"), subtitle: "The artist and others like it".into(), songs: self.artist_mix(cat, co, a, n, &none, rng) });
        }
        out.retain(|m| m.songs.len() >= 5);
        out
    }

    /// One mix by its id (see [`Mix::id`]), made again with `exclude` left out (the songs already queued, so a
    /// mix can go on). None for an id that names nothing (an unknown genre or artist).
    pub fn mix(&self, cat: &Catalog, co: &CoListen, id: &str, n: usize, exclude: &HashSet<i64>, rng: &mut Rng) -> Option<Vec<i64>> {
        let scope = match id {
            "you" => Scope::All,
            "fresh" => Scope::Fresh,
            "rediscover" => Scope::Rediscover,
            _ => {
                if let Some(g) = id.strip_prefix("genre:") {
                    Scope::Genre(cat.genre_index(g)?)
                } else if let Some(a) = id.strip_prefix("artist:") {
                    return Some(self.artist_mix(cat, co, cat.artist_index(a)?, n, exclude, rng));
                } else {
                    return None;
                }
            }
        };
        Some(self.shuffle(cat, &scope, n, exclude, rng))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A song for the tests.
    fn song(id: i64, artist: &str, genre: &str, year: &str) -> SongIn {
        SongIn { id, artists: vec![artist.into()], album_id: format!("{artist}-{}", id / 10), genre: genre.into(), year: year.into() }
    }

    /// A library of 4 artists × 20 songs: two rock artists, one jazz, one hip hop (whose songs have no genre tag
    /// but one, so the artist fallback is used).
    fn library() -> Catalog {
        let mut v = vec![];
        for i in 0..20 {
            v.push(song(100 + i, "Rocker", "Rock; Indie Rock", "1997"));
            v.push(song(200 + i, "Other Rocker", "rock", "2003"));
            v.push(song(300 + i, "Jazzer", "Jazz", "1962"));
            v.push(song(400 + i, "Rapper", if i == 0 { "Hip-Hop" } else { "" }, "2015"));
        }
        Catalog::build(v)
    }

    /// How many of `ids` are in the hundred `base..base+100`.
    fn count(ids: &[i64], base: i64) -> usize {
        ids.iter().filter(|i| (base..base + 100).contains(*i)).count()
    }

    #[test]
    fn genres_are_split_and_normalised() {
        assert_eq!(split_genres("Hip-Hop/Rap; R&B"), vec!["hip hop", "rap", "r&b"]);
        assert_eq!(split_genres("Other, Rock,  rock "), vec!["rock"]);
        assert_eq!(split_genres("(17)"), Vec::<String>::new());
        assert_eq!(decade_of("1997-05-21"), Some(1990));
        assert_eq!(decade_of("unknown"), None);
    }

    #[test]
    fn untagged_songs_take_the_artists_genre() {
        let c = library();
        let hip = c.genre_index("hip hop").unwrap();
        let s = &c.songs[c.index(405).unwrap()];
        assert_eq!(s.genres, vec![hip]);
        assert_eq!(c.genre_songs[hip as usize].len(), 20);
    }

    #[test]
    fn a_cold_listener_gets_a_plain_shuffle() {
        let c = library();
        let p = Profile::build(&c, &CoListen::default(), &Signals::default(), 1e9);
        assert!(p.cold);
        let ids = p.shuffle(&c, &Scope::All, 40, &HashSet::new(), &mut Rng::new(1));
        assert_eq!(ids.len(), 40);
        // every artist comes up
        for base in [100, 200, 300, 400] {
            assert!(count(&ids, base) > 0, "artist {base} missing");
        }
    }

    #[test]
    fn plays_and_likes_steer_the_shuffle() {
        // 10 rock and 10 hip hop artists with 5 songs each: rock artist k has ids 1000+10k.., hip hop 2000+10k..
        let mut v = vec![];
        for k in 0..10 {
            for j in 0..5 {
                v.push(song(1000 + 10 * k + j, &format!("Rock {k}"), "Rock", "1997"));
                v.push(song(2000 + 10 * k + j, &format!("Rap {k}"), "Hip Hop", "2015"));
            }
        }
        let c = Catalog::build(v);
        let now = 1e9;
        // a week-old history of three rock artists, and a like for one more rock song
        let sig = Signals { plays: (0..3).flat_map(|k| (0..5).map(move |j| (1000 + 10 * k + j, now - 7.0 * 86400.0))).collect(), likes: vec![1050], ..Default::default() };
        let p = Profile::build(&c, &CoListen::default(), &sig, now);
        let (mut rock, mut rap) = (0, 0);
        for seed in 0..20 {
            let ids = p.shuffle(&c, &Scope::All, 20, &HashSet::new(), &mut Rng::new(seed));
            rock += ids.iter().filter(|i| **i < 2000).count();
            rap += ids.iter().filter(|i| **i >= 2000).count();
        }
        assert!(rock > rap * 2, "rock {rock} vs rap {rap}");
        // genre affinity carries to rock artists never played
        assert!(p.score(&c, c.index(1094).unwrap()) > p.score(&c, c.index(2094).unwrap()));
    }

    #[test]
    fn dislikes_are_never_drawn() {
        let c = library();
        let sig = Signals { disliked_songs: [101, 102].into(), disliked_artists: ["rapper".to_string()].into(), ..Default::default() };
        let p = Profile::build(&c, &CoListen::default(), &sig, 1e9);
        for seed in 0..10 {
            let ids = p.shuffle(&c, &Scope::All, 80, &HashSet::new(), &mut Rng::new(seed));
            assert!(!ids.contains(&101) && !ids.contains(&102));
            assert_eq!(count(&ids, 400), 0);
        }
        // the disliked songs' genres sink
        let rock = c.genre_index("rock").unwrap();
        assert!(p.genre[&rock] < 0.0);
    }

    #[test]
    fn songs_skipped_again_and_again_drop_out_until_liked() {
        let c = library();
        let now = 1e9;
        // three skips in the last minutes: out
        let skips = vec![(105, now - 100.0), (105, now - 200.0), (105, now - 300.0)];
        let p = Profile::build(&c, &CoListen::default(), &Signals { skips: skips.clone(), ..Default::default() }, now);
        assert!(!p.allowed(c.index(105).unwrap()));
        let p = Profile::build(&c, &CoListen::default(), &Signals { skips, likes: vec![105], ..Default::default() }, now);
        assert!(p.allowed(c.index(105).unwrap()));
    }

    #[test]
    fn just_played_songs_wait() {
        let c = library();
        let now = 1e9;
        let sig = Signals { plays: vec![(100, now - 60.0), (101, now - 30.0 * 86400.0)], ..Default::default() };
        let p = Profile::build(&c, &CoListen::default(), &sig, now);
        assert!(p.score(&c, c.index(100).unwrap()) < p.score(&c, c.index(101).unwrap()));
    }

    #[test]
    fn co_listening_links_artists_heard_together() {
        let c = library();
        // user 1 plays Jazzer and Rapper back to back, every day
        let mut plays = vec![];
        for day in 0..10 {
            let t = day as f64 * 86400.0;
            plays.push((1, 300, t));
            plays.push((1, 400, t + 200.0));
            plays.push((1, 301, t + 400.0));
        }
        let co = CoListen::build(&c, plays);
        let jazz = c.artist_index("jazzer").unwrap();
        let rap = c.artist_index("RAPPER").unwrap();
        let rock = c.artist_index("rocker").unwrap();
        assert!(co.sim(jazz, rap) > 0.3);
        assert_eq!(co.sim(jazz, rock), 0.0);
        // a listener of Jazzer now leans to Rapper over the rock artists
        let sig = Signals { likes: vec![300, 301, 302], ..Default::default() };
        let p = Profile::build(&c, &co, &sig, 1e9);
        assert!(p.score(&c, c.index(410).unwrap()) > p.score(&c, c.index(110).unwrap()));
        // and "More like this" on a jazz song brings Rapper before the rockers
        let like = p.like_this(&c, &co, c.index(300).unwrap(), 12, &HashSet::new(), &mut Rng::new(3));
        assert!(!like.contains(&300));
        assert!(count(&like, 400) > count(&like, 100) + count(&like, 200), "{like:?}");
        assert!(count(&like, 300) <= 3);
    }

    #[test]
    fn artists_are_spread_out() {
        let c = library();
        let sig = Signals { likes: (100..120).collect(), ..Default::default() };
        let p = Profile::build(&c, &CoListen::default(), &sig, 1e9);
        let ids = p.shuffle(&c, &Scope::All, 16, &HashSet::new(), &mut Rng::new(9));
        assert_eq!(ids.len(), 16);
        for w in ids.windows(2) {
            assert!(!((100..120).contains(&w[0]) && (100..120).contains(&w[1])), "back to back: {ids:?}");
        }
        // a scope with only one artist still fills the list
        let one: Vec<usize> = (0..20).map(|i| c.index(100 + i).unwrap()).collect();
        assert_eq!(p.shuffle(&c, &Scope::Songs(one), 10, &HashSet::new(), &mut Rng::new(2)).len(), 10);
    }

    #[test]
    fn mixes_cover_the_taste() {
        let c = library();
        let now = 1e9;
        let sig = Signals {
            plays: (0..20).map(|i| (100 + i, now - 40.0 * 86400.0)).chain((0..5).map(|i| (100 + i, now - 41.0 * 86400.0))).collect(),
            likes: vec![300, 301],
            ..Default::default()
        };
        let p = Profile::build(&c, &CoListen::default(), &sig, now);
        let mixes = p.mixes(&c, &CoListen::default(), 15, &mut Rng::new(5));
        let ids: Vec<&str> = mixes.iter().map(|m| m.id.as_str()).collect();
        assert!(ids.contains(&"you") && ids.contains(&"fresh") && ids.contains(&"rediscover"), "{ids:?}");
        assert!(ids.contains(&"genre:rock"), "{ids:?}");
        assert!(ids.iter().any(|i| i.starts_with("artist:")), "{ids:?}");
        // fresh finds never holds a played or liked song
        let fresh = mixes.iter().find(|m| m.id == "fresh").unwrap();
        assert!(fresh.songs.iter().all(|i| !(100..120).contains(i) && *i != 300 && *i != 301));
        // a genre mix only holds that genre
        let rock = p.mix(&c, &CoListen::default(), "genre:Rock", 30, &HashSet::new(), &mut Rng::new(1)).unwrap();
        assert!(rock.iter().all(|i| (100..120).contains(i) || (200..220).contains(i)));
        assert!(p.mix(&c, &CoListen::default(), "genre:polka", 10, &HashSet::new(), &mut Rng::new(1)).is_none());
    }
}
