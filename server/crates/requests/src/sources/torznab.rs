//! Torrent and Usenet releases: search every indexer through Prowlarr, download with the chosen torrent
//! client (qBittorrent, Transmission, Deluge, rTorrent or a blackhole folder) or Usenet client (SABnzbd,
//! NZBGet or a blackhole folder), then import the finished folder. Replaces Lidarr. The clients are in
//! sources/dl.
use encore_core::text::ascii_words;
use encore_http::Ctx;
pub use super::dl::Poll;
use super::dl::Client;
use regex::Regex;
use serde_json::{json, Value};
use std::sync::LazyLock;

/// The chosen torrent client is set up (the name is from when qBittorrent was the only one).
pub fn qbit_ready(st: &impl crate::Ctx) -> bool {
    let c = Client::torrent(st);
    st.plugins().enabled(c.plugin_id()) && c.ready(st)
}
/// The chosen Usenet client is set up (and its plugin installed and switched on).
pub fn sab_ready(st: &impl crate::Ctx) -> bool {
    let c = Client::usenet(st);
    st.plugins().enabled(c.plugin_id()) && c.ready(st)
}
/// Torrents and Usenet are a source now: the torznab plugin is installed and switched on, Prowlarr is set up
/// and a client is ready.
pub fn enabled(st: &impl crate::Ctx) -> bool {
    st.plugins().enabled("torznab") && st.cfg().bool("torznab_enabled") && !st.cfg().str("prowlarr_url").is_empty() && !st.cfg().str("prowlarr_api_key").is_empty() && (qbit_ready(st) || sab_ready(st))
}

/// The Prowlarr base URL a setting names.
fn base(st: &impl Ctx, key: &str) -> String {
    prowlarr_base(&st.cfg().str(key))
}

/// Prowlarr's address as typed, made its base address: spaces and line breaks around it go (a pasted
/// address), and so does the path of an API or of one indexer's Torznab feed that is sometimes pasted
/// instead (".../api/v1", ".../1/api", ".../api"), since Encore asks Prowlarr's own search.
pub fn prowlarr_base(url: &str) -> String {
    static API: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(/\d+)?/api(/v\d+)?$").unwrap());
    let u = url.trim().trim_end_matches('/');
    API.replace(u, "").trim_end_matches('/').to_string()
}

// ---- Prowlarr ------------------------------------------------------------------------------------

/// What stands in a release link for Prowlarr's API key, which Prowlarr writes into every download link
/// it hands out ("…/download?apikey=<key>&link=…"): followed by the link's seal ([`seal_key`]).
const SEAL: &str = "encore-seal";
/// An `apikey=` query parameter.
static APIKEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)([?&]apikey=)([^&#]*)").unwrap());
/// A sealed link's seal.
static SEALED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"=encore-seal-([0-9a-f]{40})").unwrap());

/// HMAC-SHA1 of `msg` with `key`, in hex.
fn hmac(key: &str, msg: &str) -> String {
    use sha1::{Digest, Sha1};
    let mut k = key.as_bytes().to_vec();
    if k.len() > 64 {
        k = Sha1::digest(&k).to_vec();
    }
    k.resize(64, 0);
    let pad = |x: u8| k.iter().map(|b| b ^ x).collect::<Vec<u8>>();
    let inner = Sha1::new().chain_update(pad(0x36)).chain_update(msg.as_bytes()).finalize();
    hex::encode(Sha1::new().chain_update(pad(0x5c)).chain_update(inner).finalize())
}

/// A release link as the apps see it (the picker's rows): Prowlarr's API key out, a seal in its place – a
/// keyed hash of the link, so only links Encore handed out get the key back ([`unseal_key`]). Whoever may
/// pick downloads needn't learn the key to Prowlarr (and with it every indexer's login), and a link
/// written by hand can't carry the key to another address. Links without the key stay as they are.
pub fn seal_key(st: &impl Ctx, url: &str) -> String {
    let key = st.cfg().str("prowlarr_api_key").trim().to_string();
    if key.is_empty() {
        return url.to_string();
    }
    let mut found = false;
    let plain = APIKEY.replace_all(url, |c: &regex::Captures| {
        if c[2] == key {
            found = true;
            format!("{}{SEAL}", &c[1])
        } else {
            c[0].to_string()
        }
    }).into_owned();
    if !found {
        return url.to_string();
    }
    let tag = hmac(&key, &plain);
    plain.replace(&format!("={SEAL}"), &format!("={SEAL}-{tag}"))
}

/// A release link with Prowlarr's API key back where [`seal_key`] sealed it; a link whose seal doesn't
/// fit (written or changed by hand, or sealed with an earlier key) stays as it is.
pub fn unseal_key(st: &impl Ctx, url: &str) -> String {
    use subtle::ConstantTimeEq;
    let key = st.cfg().str("prowlarr_api_key").trim().to_string();
    let tags: Vec<String> = SEALED.captures_iter(url).map(|c| c[1].to_string()).collect();
    if key.is_empty() || tags.is_empty() {
        return url.to_string();
    }
    let plain = SEALED.replace_all(url, format!("={SEAL}").as_str()).into_owned();
    let want = hmac(&key, &plain);
    if !tags.iter().all(|t| bool::from(t.as_bytes().ct_eq(want.as_bytes()))) {
        return url.to_string();
    }
    plain.replace(&format!("={SEAL}"), &format!("={key}"))
}

/// Audio (3000), MP3 (3010) and lossless (3040): where single-track releases are filed.
pub const SONG_CATS: [&str; 3] = ["3000", "3010", "3040"];

/// A request's album search (in the audio category). The requests' own indexer searches take turns in the
/// indexers' line, two at a time ([`crate::line::Lines::indexers`], taken by the worker around its searches):
/// "Request discography" queues every release at once, and each album's search would otherwise reach Prowlarr
/// in the same second – indexers allow only so many searches a day, and Prowlarr spaces its searches per
/// indexer anyway. The picker's searches (someone waiting for them) don't wait in that line.
pub async fn search(st: &impl Ctx, query: &str) -> anyhow::Result<Vec<Value>> {
    search_in(st, query, &["3000"]).await
}

/// How long Prowlarr is waited for: it waits for every indexer, each up to about 100 s, and then answers
/// with what the others found.
const SEARCH_WAIT: std::time::Duration = std::time::Duration::from_secs(120);

/// Search every indexer in these Newznab categories.
pub async fn search_in(st: &impl Ctx, query: &str, cats: &[&str]) -> anyhow::Result<Vec<Value>> {
    // indexers spell Ke$ha "Kesha": the query too ([`fold_symbols`])
    let query = fold_symbols(query);
    let mut q: Vec<(&str, &str)> = vec![("query", query.as_str()), ("type", "search"), ("limit", "100")];
    q.extend(cats.iter().map(|c| ("categories", *c)));
    let r = st.http().get(format!("{}/api/v1/search", base(st, "prowlarr_url")))
        .query(&q)
        .header("X-Api-Key", st.cfg().str("prowlarr_api_key")).timeout(SEARCH_WAIT).send().await
        .map_err(|e| if e.is_timeout() {
            anyhow::anyhow!("Prowlarr took more than {} minutes to answer – one of its indexers is slow or not answering (Prowlarr → System → Status names it)", SEARCH_WAIT.as_secs() / 60)
        } else {
            anyhow::anyhow!("Prowlarr unreachable: {}", super::dl::http_error(e))
        })?;
    if !r.status().is_success() {
        anyhow::bail!("Prowlarr answered {}{}", r.status().as_u16(), if r.status().as_u16() == 401 { " (check the API key)" } else { "" });
    }
    Ok(r.json::<Value>().await?.as_array().cloned().unwrap_or_default())
}

/// (enabled indexers, those of them that search music): an indexer whose categories Prowlarr lists
/// counts for music when one of them is Audio (3000) or one of its 30xx kinds; one without a list of
/// categories counts too.
pub fn music_indexers(list: &Value) -> (usize, usize) {
    let on: Vec<&Value> = list.as_array().map(|a| a.iter().filter(|i| i["enable"].as_bool().unwrap_or(true)).collect()).unwrap_or_default();
    let audio = |c: &Value| c["id"].as_i64().is_some_and(|id| (3000..4000).contains(&id));
    let music = on.iter().filter(|i| match i["capabilities"]["categories"].as_array() {
        Some(cats) if !cats.is_empty() => cats.iter().any(|c| audio(c) || c["subCategories"].as_array().is_some_and(|s| s.iter().any(audio))),
        _ => true,
    }).count();
    (on.len(), music)
}

/// Prowlarr's state: reachable, its music indexers, and the clients.
pub async fn status(st: &impl Ctx) -> Value {
    let p = st.http().get(format!("{}/api/v1/indexer", base(st, "prowlarr_url"))).header("X-Api-Key", st.cfg().str("prowlarr_api_key")).send().await;
    let indexers = match p {
        Ok(r) if r.status().is_success() => r.json::<Value>().await.ok().map(|v| music_indexers(&v)),
        _ => None,
    };
    let Some((all, n)) = indexers else { return json!({ "ok": false, "message": "Prowlarr not reachable or wrong API key" }) };
    // searches ask for music only: an indexer without music categories finds nothing
    let first = match (all, n) {
        (0, _) => "no indexers in Prowlarr – add your trackers and Usenet indexers there".to_string(),
        (_, 0) => format!("none of the {all} indexer(s) in Prowlarr has music (Audio) categories – add one that has"),
        (all, n) if n < all => format!("{n} of {all} indexer(s) in Prowlarr search music"),
        (_, n) => format!("{n} indexer(s) in Prowlarr"),
    };
    let (mut ok, mut parts) = (n > 0, vec![first]);
    for c in [Client::torrent(st), Client::usenet(st)] {
        if !c.ready(st) {
            parts.push(format!("no {}", c.name()));
            continue;
        }
        match c.status(st).await {
            Ok(extra) => parts.push(format!("{} ok{extra}", c.name())),
            Err(e) => {
                ok = false;
                parts.push(e.to_string());
            }
        }
    }
    json!({ "ok": ok, "message": parts.join(" · ") })
}

/// Lossless words in a release name.
static LOSSLESS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(flac|lossless|alac|24[\s-]?bit|16[\s-]?bit|wav|hi-?res)\b").unwrap());
/// 320 kbps / V0 words.
static MP3_320: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(320|v0)\b").unwrap());
/// Versions nobody asking for the album means.
static BAD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(live|remix(es)?|karaoke|instrumental|8d|nightcore|tribute|cover)\b").unwrap());
/// Several albums in one release.
static BIG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(discography|collection|anthology|complete works|box ?set)\b").unwrap());
/// A whole CD as one file with a cue sheet ("[FLAC image+cue]"): Encore doesn't split it, so it would arrive
/// as one long song.
static IMAGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(image|img)\b").unwrap());

/// A bracketed part.
static BRACKET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*[\(\[][^\)\]]*[\)\]]").unwrap());
/// Extras of a catalogue title that file and release names often leave out.
static EXTRA: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(feat|ft|featuring|with|remaster(ed)?|version|mono|stereo|edit|bonus|deluxe|edition|expanded|anniversary|reissue|special|collector'?s)\b").unwrap());
/// A featured-artist tail.
static FEAT_TAIL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\s+(feat\.?|ft\.?|featuring)\s.*$").unwrap());

/// A song or album title without "(feat. X)", "(Remastered 2011)", " - 2009 Version" or "(Expanded
/// Edition)" (Deezer and Apple add them, files and release names usually don't). Versions that matter
/// (live, remix …) stay.
fn core_title(song: &str) -> String {
    let keep = |x: &str| !EXTRA.is_match(x) || BAD.is_match(x);
    let out = BRACKET.replace_all(song, |c: &regex::Captures| if keep(&c[0]) { c[0].to_string() } else { String::new() }).into_owned();
    let out = match out.split_once(" - ") {
        Some((l, r)) if !keep(r) => l.to_string(),
        _ => out,
    };
    FEAT_TAIL.replace(&out, "").trim().to_string()
}

/// `contains` of the song title, or of its core when the extras are what's missing.
fn contains_song(hay: &str, song: &str) -> f64 {
    let t = contains(hay, song);
    let core = core_title(song);
    if core.is_empty() || core == song { t } else { t.max(contains(hay, &core)) }
}

/// The share of an album title's words in a release name, or of its core's ([`core_title`]): a release
/// of "Rodeo (Expanded Edition)" or "Random Access Memories (10th Anniversary Edition)" is usually named
/// without the edition, as the search for it is.
pub fn album_share(hay: &str, album: &str) -> f64 {
    contains_song(hay, album)
}

/// [`contains`] for callers outside: the share of `needle`'s words in `hay`, whatever their spelling.
pub fn share(hay: &str, needle: &str) -> f64 {
    contains(hay, needle)
}

/// The words of a release or file name ([`encore_core::text::ascii_words`]), "&" read as "and".
fn title_words(s: &str) -> String {
    ascii_words(&s.replace('&', " and "))
}

/// A name with the symbols that stand for letters spelled out, everything else as it is: "$" next to a
/// letter (or next to a run of "$" that is) reads as "s" (Ke$ha → Kesha, A$AP → AsAP, Bada$$ → Badass,
/// $uicideboy$ → suicideboys, Ty Dolla $ign → Ty Dolla sign), "!" between two letters as "i" (P!nk →
/// Pink; "Panic! at" and "Wham!" keep it). Indexers and scene names spell those artists with letters.
/// For search queries too: "Ke$ha Cannibal" → "Kesha Cannibal", where stripping the symbol gave "Ke ha".
pub fn fold_symbols(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let letter = |i: usize| c.get(i).is_some_and(|x| x.is_alphabetic());
    // past a run of "$" from i in direction `step`: is the first other character a letter?
    let letter_past_dollars = |i: usize, step: isize| {
        let mut j = i as isize;
        while j >= 0 && (j as usize) < c.len() && c[j as usize] == '$' {
            j += step;
        }
        j >= 0 && letter(j as usize)
    };
    c.iter().enumerate().map(|(i, &x)| match x {
        '$' if letter_past_dollars(i, -1) || letter_past_dollars(i, 1) => 's',
        '!' if i > 0 && letter(i - 1) && letter(i + 1) => 'i',
        x => x,
    }).collect()
}

/// [`title_words`] for comparing a name with release and file names whatever their spelling:
/// [`fold_symbols`], apostrophes dropped (Don't → dont, the way scene names write it) and accents folded
/// (Beyoncé → beyonce).
pub fn match_words(s: &str) -> String {
    // a "$" left over is a symbol, not a letter ("$100 Bill"): a break, before matcher::fold reads it as "s"
    let t: String = fold_symbols(s).chars().filter(|c| *c != '\'' && *c != '’').map(|c| if c == '$' { ' ' } else { c }).collect();
    title_words(&encore_library::matcher::fold(&t))
}

/// The share of `needle`'s words found in `hay`, the better of two readings of both: the plain words
/// ("Ke$ha" → "ke ha") and [`match_words`] ("Ke$ha" → "kesha"). So "Ke$ha" matches a release named
/// "Kesha" and one named "Ke$ha", and no match of the plain reading is lost.
fn contains(hay: &str, needle: &str) -> f64 {
    contains_with(hay, needle, title_words).max(contains_with(hay, needle, match_words))
}

/// The share of `needle`'s words (as `words` makes them) in `hay`.
fn contains_with(hay: &str, needle: &str, words: fn(&str) -> String) -> f64 {
    let n = words(needle);
    let w: Vec<&str> = n.split_whitespace().collect();
    if w.is_empty() {
        return 1.0;
    }
    let h = format!(" {} ", words(hay));
    // a name split by a symbol, written as one word: AC/DC as "ACDC"
    if w.len() > 1 && h.contains(&format!(" {} ", w.concat())) {
        return 1.0;
    }
    w.iter().filter(|x| h.contains(&format!(" {x} "))).count() as f64 / w.len() as f64
}

/// What an album request looks for in the indexers' releases.
#[derive(Debug, Default, Clone, Copy)]
pub struct Want<'a> {
    /// the artist asked for
    pub artist: &'a str,
    /// the album asked for
    pub album: &'a str,
    /// the release's year, when known: a release named with another year ranks lower
    pub year: Option<i32>,
    /// the release's number of tracks, when known: the sizes a release of it can have
    pub tracks: Option<usize>,
    /// only lossless releases
    pub lossless_only: bool,
}

/// A year in a name.
static YEAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(19\d\d|20\d\d)\b").unwrap());
/// Words of release names that say nothing about which album it is: formats, sources, editions.
static NOISE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(flac|mp3|aac|alac|ogg|opus|wav|m4a|ape|wv|dsd|dsf|lossless|lossy|cbr|vbr|v0|v2|kbps|khz|bit|\d+bit|\d+khz|hi|res|hires|24|16|44|48|88|96|176|192|320|256|web|webflac|webrip|cd|cdrip|cdda|2cd|3cd|vinyl|vinylrip|lp|ep|sacd|dvd|bluray|rip|dl|scene|remaster|remastered|deluxe|edition|expanded|anniversary|reissue|bonus|tracks?|album|the|and|of|a|ost|retail|proper|repack|disc|discs|cue|log|tag|tagged|mixed|stereo|mono|[a-z]?\d{1,3})$").unwrap());
/// The word after a self-titled album's name that makes it another album: "Led Zeppelin II", "Peter
/// Gabriel 3".
static NUMBERED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(i{1,3}|iv|vi{0,3}|ix|x|\d{1,2})$").unwrap());

/// How many times `name`'s words appear one after another in `hay` (both as [`title_words`] and
/// [`match_words`] read them; the better reading counts), and the word after the second appearance.
fn appearances(hay: &str, name: &str) -> (usize, Option<String>) {
    let mut best = (0, None);
    for words in [title_words as fn(&str) -> String, match_words] {
        let (h, n) = (words(hay), words(name));
        let (h, n): (Vec<&str>, Vec<&str>) = (h.split_whitespace().collect(), n.split_whitespace().collect());
        if n.is_empty() || n.len() > h.len() {
            continue;
        }
        let (mut count, mut after, mut i) = (0, None, 0);
        while i + n.len() <= h.len() {
            if h[i..i + n.len()] == n[..] {
                count += 1;
                if count == 2 {
                    after = h.get(i + n.len()).map(|w| w.to_string());
                }
                i += n.len();
            } else {
                i += 1;
            }
        }
        if count > best.0 {
            best = (count, after);
        }
    }
    best
}

/// The years named in a release name.
fn years(title: &str) -> Vec<i32> {
    YEAR.find_iter(title).filter_map(|m| m.as_str().parse().ok()).collect()
}

/// Words of a release name beyond the artist's and the album's (and its core's), years, formats and
/// sources: a longer name is more likely another release that shares some words.
fn extra_words(title: &str, artist: &str, album: &str) -> usize {
    let known: Vec<String> = [artist, album, &core_title(album)].iter().flat_map(|s| {
        let (a, b) = (title_words(s), match_words(s));
        a.split_whitespace().chain(b.split_whitespace()).map(String::from).collect::<Vec<_>>()
    }).collect();
    match_words(title).split_whitespace().filter(|w| !known.iter().any(|k| k == w) && !NOISE.is_match(w) && !YEAR.is_match(w)).count()
}

/// Best releases for an album, best first (at most 5). A release must name the artist and the album (or
/// the album without its edition); a self-titled album ("Weezer" by Weezer) must name it twice or carry
/// its year, since every release of the artist names it once. Within that, lossless first, then fewer
/// words beyond the album's, the album's own year, and seeders.
pub fn rank_album(st: &impl Ctx, results: &[Value], want: &Want) -> Vec<Value> {
    let (artist, album) = (want.artist, want.album);
    let min_seed = st.cfg().int("torznab_min_seeders");
    let self_titled = !match_words(artist).is_empty() && (match_words(artist) == match_words(album) || match_words(artist) == match_words(&core_title(album)));
    let named = format!("{} {}", match_words(artist), match_words(album));
    let named: Vec<&str> = named.split_whitespace().collect();
    // MB: a single's MP3 can be a few MB, a hi-res box set many GB
    let (lo, hi) = match want.tracks.filter(|n| *n > 0) {
        Some(n) => ((2 * n) as f64, (400.0 * n as f64).max(4000.0)),
        None => (15.0, 4000.0),
    };
    let mut out = vec![];
    for r in results {
        let title = r["title"].as_str().unwrap_or("");
        let proto = r["protocol"].as_str().unwrap_or("torrent");
        // a release only its own kind of client can take (the client's plugin was checked before the search)
        if (proto == "torrent" && !Client::torrent(st).ready(st)) || (proto == "usenet" && !Client::usenet(st).ready(st)) {
            continue;
        }
        let (a, b) = (album_share(title, album), contains(title, artist));
        if a < 0.8 || b < 0.5 {
            continue;
        }
        // the years in the name ("1989" by Taylor Swift is the album's name, not its year)
        let ys: Vec<i32> = years(title).into_iter().filter(|y| !named.iter().any(|w| *w == y.to_string())).collect();
        let year_near = want.year.is_some_and(|y| ys.iter().any(|t| (t - y).abs() <= 1));
        // "Led Zeppelin II" is another album than "Led Zeppelin", unless it is from the year asked for
        let mut numbered = false;
        if self_titled {
            let (n, after) = appearances(title, artist);
            numbered = after.is_some_and(|w| NUMBERED.is_match(&w));
            let named_twice = n >= 2 && !numbered;
            if !named_twice && !year_near {
                continue;
            }
        }
        let size = r["size"].as_f64().unwrap_or(0.0) / 1e6;
        if size > 0.0 && !(lo..=hi).contains(&size) {
            continue;
        }
        if proto == "torrent" && r["seeders"].as_i64().unwrap_or(0) < min_seed {
            continue;
        }
        let lossless = LOSSLESS.is_match(title);
        if want.lossless_only && !lossless {
            continue;
        }
        let mut score = a * 30.0 + b * 15.0;
        score += if lossless { 30.0 } else if MP3_320.is_match(title) { 12.0 } else { 0.0 };
        if BAD.is_match(title) && !BAD.is_match(album) {
            score -= 20.0;
        }
        if BIG.is_match(title) {
            score -= 25.0;
        }
        // a release in tracks first, even an MP3 one: an image would be imported as one long song
        if IMAGE.is_match(title) && !IMAGE.is_match(album) {
            score -= 25.0;
        }
        score -= (2 * extra_words(title, artist, album)).min(12) as f64;
        if numbered {
            score -= 15.0;
        }
        if want.year.is_some() && !ys.is_empty() && !year_near {
            score -= 10.0;
        }
        score += if proto == "usenet" { 8.0 } else { (r["seeders"].as_f64().unwrap_or(0.0) + 1.0).log2().min(10.0) };
        let url = r["magnetUrl"].as_str().filter(|m| !m.is_empty()).or(r["downloadUrl"].as_str()).unwrap_or("").to_string();
        if url.is_empty() {
            continue;
        }
        out.push(json!({ "title": title, "indexer": r["indexer"], "protocol": proto, "size": r["size"], "seeders": r["seeders"],
            "url": url, "guid": r["guid"], "lossless": lossless, "score": (score * 10.0).round() / 10.0 }));
    }
    out.sort_by(|x, y| y["score"].as_f64().partial_cmp(&x["score"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
    out.truncate(5);
    out
}

// ---- single songs --------------------------------------------------------------------------------

/// How well a release matches one song (higher is better), and whether it looks like a single-track
/// release. None when it isn't that song. `size` in bytes (0 = unknown).
pub fn score_song(title: &str, size: f64, seeders: Option<i64>, proto: &str, artist: &str, song: &str) -> Option<(f64, bool)> {
    let (t, a) = (contains_song(title, song), contains(title, artist));
    if t < 0.8 || a < 0.5 {
        return None;
    }
    // another version than the one asked for (live, remix, karaoke …): never picked
    if encore_library::matcher::unwanted_version(song, artist, title).is_some() {
        return None;
    }
    let mb = size / 1e6;
    let single = mb > 0.0 && mb <= 250.0 && !BIG.is_match(title);
    let lossless = LOSSLESS.is_match(title);
    let mut score = t * 40.0 + a * 15.0 + if lossless { 30.0 } else if MP3_320.is_match(title) { 12.0 } else { 0.0 };
    if single {
        score += 12.0;
    }
    if BAD.is_match(title) && !BAD.is_match(song) {
        score -= 20.0;
    }
    if BIG.is_match(title) {
        score -= 25.0;
    }
    // extra words beyond artist and title: a longer name is more likely another release that mentions the song.
    // Both readings of every name, so "Beyonce" or "Dont" in a release name is no extra word for "Beyoncé" or
    // "Don't"
    let known: Vec<String> = [song, artist].iter().flat_map(|s| [title_words(s), match_words(s)]).flat_map(|w| w.split_whitespace().map(String::from).collect::<Vec<_>>()).collect();
    let extra = [title_words(title), match_words(title)].iter().map(|t| t.split_whitespace().filter(|w| !known.iter().any(|k| k == w)).count()).min().unwrap_or(0);
    score -= (extra as f64).min(10.0);
    score += if proto == "usenet" { 8.0 } else { (seeders.unwrap_or(0) as f64 + 1.0).log2().min(10.0) };
    Some(((score * 10.0).round() / 10.0, single))
}

/// Best releases for one song, best first (at most 5): the song's own releases ([`score_song`]), then
/// releases of the album it is on (`album`: its name and that search's results, ranked as [`rank_album`]
/// ranks them, below the song's own) – of those only the song's file downloads (torrents) or is imported
/// (Usenet). A release its kind of client can't take, a torrent below the minimum seeders, and with
/// `lossless_only` a lossy release are left out.
pub fn rank_song(st: &impl Ctx, songs: &[Value], album: Option<(&str, &[Value])>, artist: &str, song: &str, lossless_only: bool) -> Vec<Value> {
    let min_seed = st.cfg().int("torznab_min_seeders");
    let mut out = vec![];
    for r in songs {
        let title = r["title"].as_str().unwrap_or("");
        let proto = r["protocol"].as_str().unwrap_or("torrent");
        if (proto == "torrent" && !Client::torrent(st).ready(st)) || (proto == "usenet" && !Client::usenet(st).ready(st)) {
            continue;
        }
        if proto == "torrent" && r["seeders"].as_i64().unwrap_or(0) < min_seed {
            continue;
        }
        let lossless = LOSSLESS.is_match(title);
        if lossless_only && !lossless {
            continue;
        }
        let Some((score, _)) = score_song(title, r["size"].as_f64().unwrap_or(0.0), r["seeders"].as_i64(), proto, artist, song) else { continue };
        let url = r["magnetUrl"].as_str().filter(|m| !m.is_empty()).or(r["downloadUrl"].as_str()).unwrap_or("").to_string();
        if url.is_empty() {
            continue;
        }
        out.push(json!({ "title": title, "indexer": r["indexer"], "protocol": proto, "size": r["size"], "seeders": r["seeders"],
            "url": url, "guid": r["guid"], "lossless": lossless, "score": score }));
    }
    if let Some((name, results)) = album {
        let want = Want { artist, album: name, year: None, tracks: None, lossless_only };
        for mut c in rank_album(st, results, &want) {
            if out.iter().any(|o| o["guid"] == c["guid"] && !c["guid"].is_null()) {
                continue;
            }
            // the whole album has to arrive first: below the song's own releases of the same quality
            c["score"] = json!(((c["score"].as_f64().unwrap_or(0.0) - 10.0) * 10.0).round() / 10.0);
            c["album"] = json!(name);
            c["song_only"] = json!(true);
            out.push(c);
        }
    }
    out.sort_by(|x, y| y["score"].as_f64().partial_cmp(&x["score"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
    out.truncate(5);
    out
}

/// A file name with an audio extension.
pub fn is_audio(name: &str) -> bool {
    encore_core::audio::DOWNLOAD_EXTS.contains(&super::slskd::ext(name).as_str())
}

/// A file name's leading track number ("03 - …", "CD2 - 05 …", "1-04 …").
static LEAD_NO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(?:(?:cd|disc)\s*\d+\s*[-_. ]\s*)?(?:\d-)?(\d{1,3})(?:\D|$)").unwrap());

/// The file of `names` (paths inside a release) that is this song: its name holds every word of the
/// title (or its track number and most of the title), with as few other words as possible. Versions the
/// title doesn't ask for (live, remix, instrumental …) lose.
pub fn pick_file(names: &[String], artist: &str, song: &str, track_no: Option<u32>) -> Option<usize> {
    let want = title_words(song);
    if want.is_empty() {
        return None;
    }
    let mut best: Option<(f64, usize)> = None;
    for (i, n) in names.iter().enumerate() {
        if !is_audio(n) {
            continue;
        }
        let file = n.replace('\\', "/");
        let file = file.rsplit('/').next().unwrap_or("");
        let stem = file.rsplit_once('.').map(|x| x.0).unwrap_or(file);
        let t = contains_song(stem, song);
        let no = LEAD_NO.captures(stem).and_then(|c| c[1].parse::<u32>().ok());
        let by_no = track_no.is_some() && no == track_no;
        if t < 0.999 && !(by_no && t >= 0.5) {
            continue;
        }
        if encore_library::matcher::unwanted_version(song, artist, stem).is_some() {
            continue;
        }
        let words = title_words(stem);
        let extra = words.split_whitespace().filter(|w| !want.split_whitespace().any(|x| x == *w) && !title_words(artist).split_whitespace().any(|x| x == *w) && w.parse::<u32>().is_err()).count();
        let mut sc = t * 10.0 - extra as f64 + if by_no { 5.0 } else { 0.0 };
        if BAD.is_match(stem) && !BAD.is_match(song) {
            sc -= 8.0;
        }
        if best.is_none_or(|(b, _)| sc > b) {
            best = Some((sc, i));
        }
    }
    best.map(|(_, i)| i)
}

// ---- one download, through the chosen client ----------------------------------------------------

/// The client a release of this protocol goes to.
pub fn client_for(st: &impl Ctx, proto: &str) -> Client {
    if proto == "usenet" { Client::usenet(st) } else { Client::torrent(st) }
}

/// "qBittorrent", "Transmission", "SABnzbd" … for messages.
pub fn client_name(st: &impl Ctx, proto: &str) -> &'static str {
    client_for(st, proto).name()
}

/// Hand a release to the matching download client. → the id used for polling. A picked release's link
/// gets Prowlarr's key back ([`unseal_key`]).
pub async fn add(st: &impl Ctx, cand: &Value, rid: i64) -> anyhow::Result<String> {
    let url = unseal_key(st, cand["url"].as_str().unwrap_or(""));
    let c = client_for(st, cand["protocol"].as_str().unwrap_or("torrent"));
    c.add(st, &url, cand["title"].as_str().unwrap_or(""), rid).await
}

/// The id prefix of a finished download Encore found in its category but didn't add ([`crate::worker`]'s
/// client scan): it is imported from a copy and never removed from its client (it may still seed).
pub const KEEP: &str = "keep:";

/// The client and its own id behind a download id, a kept one's too.
fn client_of(id: &str) -> (Client, &str) {
    Client::for_id(id.strip_prefix(KEEP).unwrap_or(id))
}

/// Where the client is with the download `id`.
pub async fn poll(st: &impl Ctx, id: &str) -> anyhow::Result<Poll> {
    let (c, own) = client_of(id);
    c.poll(st, own).await
}

/// Which files of a release are the song.
pub enum SongFile {
    /// The client doesn't know the files yet (a magnet's metadata is loading): ask again later.
    Pending,
    /// The client downloads whole releases (Usenet, blackholes) or there is nothing to skip.
    Whole,
    /// Only this file downloads now.
    Only(String),
    /// No file in the release is this song.
    Missing,
}

/// A song from a release with several files: tell the torrent client to download only the song's file.
pub async fn select_song(st: &impl Ctx, id: &str, artist: &str, song: &str, track_no: Option<u32>) -> anyhow::Result<SongFile> {
    let (c, own) = client_of(id);
    if !c.can_select() {
        return Ok(SongFile::Whole);
    }
    let Some(names) = c.files(st, own).await? else { return Ok(SongFile::Pending) };
    if names.iter().filter(|n| is_audio(n)).count() <= 1 {
        return Ok(SongFile::Whole);
    }
    let Some(i) = pick_file(&names, artist, song, track_no) else { return Ok(SongFile::Missing) };
    c.select(st, own, &[i]).await?;
    Ok(SongFile::Only(names[i].clone()))
}

/// Remove the download (and what's left of its files) from the client – never one added outside Encore
/// ([`KEEP`]).
pub async fn remove(st: &impl Ctx, id: &str) {
    if id.starts_with(KEEP) {
        return;
    }
    let (c, own) = Client::for_id(id);
    c.remove(st, own).await
}

#[cfg(test)]
#[path = "torznab_tests.rs"]
mod song_tests;
