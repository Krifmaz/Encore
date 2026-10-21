//! DeepL (tier 4 of tiers.rs): song and album titles translated with the owner's DeepL API key
//! (`deepl_api_key`, or DEEPL_API_KEY) – titles only: never lyrics, and not artist names (a name is spelled, not
//! translated; the romanization and the official names cover those).
//!
//! A key ending in `:fx` belongs to DeepL's free plan and goes to api-free.deepl.com, any other key to
//! api.deepl.com. Texts go in batches of 50, one source script per batch. The characters sent are counted per
//! calendar month (UTC, table translate_usage), and DeepL is not asked again in a month once
//! `deepl_char_budget` is reached (0: no limit of Encore's own; DeepL's plan has its limit too). A refused key,
//! a used-up DeepL quota or a busy answer pauses DeepL for a while, or until DeepL's settings change (see
//! [`Hold`]), instead of asking again with every title on screen.
use super::prompt::Kind;
use super::{script, store, tiers::Tiers};
use crate::Ctx;
use encore_db::{p, Db, RowExt};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::Duration;

/// Texts per request.
pub const BATCH: usize = 50;
pub const FREE_API: &str = "https://api-free.deepl.com";
pub const PRO_API: &str = "https://api.deepl.com";
/// The name of the service in translate_usage.
pub const SERVICE: &str = "deepl";

/// The API a key belongs to: the free plan's for a key ending in `:fx`.
pub fn api_for(key: &str) -> &'static str {
    if key.trim().ends_with(":fx") { FREE_API } else { PRO_API }
}

/// DeepL's code for a target language of Settings → Language (None: DeepL does not translate into it).
pub fn target_lang(code: &str) -> Option<&'static str> {
    Some(match code {
        "en" => "EN-US", "de" => "DE", "fr" => "FR", "es" => "ES", "it" => "IT", "pt" => "PT-PT", "nl" => "NL",
        "sv" => "SV", "da" => "DA", "nb" => "NB", "fi" => "FI", "pl" => "PL", "cs" => "CS", "tr" => "TR",
        "id" => "ID", "vi" => "VI", "ru" => "RU", "uk" => "UK", "el" => "EL", "ar" => "AR", "he" => "HE",
        "th" => "TH", "ja" => "JA", "ko" => "KO", "zh" => "ZH-HANS",
        _ => return None,
    })
}

/// The source language to tell DeepL for a script (script::detect), where the script says it: short titles
/// are easy to mistake. Cyrillic is several languages, so DeepL tells those itself.
pub fn source_lang(script: &str) -> Option<&'static str> {
    Some(match script {
        "ja" => "JA",
        "ko" => "KO",
        "zh" => "ZH",
        "grek" => "EL",
        "arab" => "AR",
        _ => return None,
    })
}

/// Whether DeepL is asked for this kind of text: song and album titles.
pub fn wanted(kind: Kind) -> bool {
    matches!(kind, Kind::Title | Kind::Album)
}

/// Why DeepL gave nothing.
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// 403: the key is wrong or was revoked
    Key,
    /// 456: the DeepL account's own quota is used up
    Quota,
    /// 429 / 503: too many requests
    Busy,
    /// Encore's budget for this month (`deepl_char_budget`) is used up
    Budget,
    /// another answer (status, DeepL's message)
    Refused(u16, String),
    /// no answer, or not one DeepL gives, after the request was sent (DeepL may have counted it)
    Net(String),
    /// no connection (DNS, a refused connection, the proxy): nothing reached DeepL
    Unreached(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Key => write!(f, "DeepL refused the API key – check it under Settings → Library → Translated titles"),
            Error::Quota => write!(f, "the DeepL account has used its characters for this month"),
            Error::Busy => write!(f, "DeepL is busy (too many requests) – trying again in a minute"),
            Error::Budget => write!(f, "this month's DeepL budget is used up"),
            Error::Refused(code, msg) => write!(f, "DeepL answered {code}: {msg}"),
            Error::Net(e) | Error::Unreached(e) => write!(f, "DeepL could not be reached: {e}"),
        }
    }
}

/// Translate `texts` (at most [`BATCH`]) into `target` (DeepL's code) with the API at `api`; the answers in the
/// same order.
pub async fn translate(http: &reqwest::Client, api: &str, key: &str, texts: &[String], source: Option<&str>, target: &str) -> Result<Vec<String>, Error> {
    if texts.is_empty() {
        return Ok(vec![]);
    }
    let mut body = json!({
        "text": texts,
        "target_lang": target,
        // one title is one unit, whatever its punctuation
        "split_sentences": "0",
        // what the texts are (not translated, not counted by DeepL)
        "context": "Titles of songs and albums.",
    });
    if let Some(s) = source {
        body["source_lang"] = json!(s);
    }
    let r = http
        .post(format!("{}/v2/translate", api.trim_end_matches('/')))
        .header("Authorization", format!("DeepL-Auth-Key {}", key.trim()))
        .json(&body)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| if e.is_connect() { Error::Unreached(e.to_string()) } else { Error::Net(e.to_string()) })?;
    let code = r.status().as_u16();
    match code {
        200 => {}
        401 | 403 => return Err(Error::Key),
        456 => return Err(Error::Quota),
        429 | 503 | 529 => return Err(Error::Busy),
        _ => {
            let text = r.text().await.unwrap_or_default();
            let msg = serde_json::from_str::<Value>(&text).ok().and_then(|v| v["message"].as_str().map(String::from))
                .unwrap_or_else(|| text.chars().take(200).collect());
            return Err(Error::Refused(code, msg));
        }
    }
    let v: Value = r.json().await.map_err(|e| Error::Net(format!("the answer is not JSON ({e})")))?;
    let out: Vec<String> = v["translations"].as_array().into_iter().flatten().map(|t| t["text"].as_str().unwrap_or("").trim().to_string()).collect();
    if out.len() != texts.len() {
        return Err(Error::Net(format!("{} answers for {} titles", out.len(), texts.len())));
    }
    Ok(out)
}

// ---- the monthly budget -------------------------------------------------------------------------

/// The calendar month (UTC) of unix seconds: `2026-10`.
pub fn month(secs: f64) -> String {
    let (y, m, _) = encore_core::time::civil(secs.max(0.0));
    format!("{y:04}-{m:02}")
}

/// The first second of the month after the one of `secs`.
fn next_month(secs: f64) -> f64 {
    let (y, m, _) = encore_core::time::civil(secs.max(0.0));
    let (y, m) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    encore_core::time::days_from_civil(y, m, 1) as f64 * 86_400.0
}

/// The characters DeepL counts for a text.
pub fn chars(text: &str) -> i64 {
    text.chars().count() as i64
}

/// Characters sent to DeepL in `month`.
pub fn used(db: &Db, month: &str) -> i64 {
    db.one("SELECT chars FROM translate_usage WHERE service=? AND month=?", p![SERVICE, month]).ok().flatten().map(|r| r.i("chars")).unwrap_or(0)
}

/// Count `n` more characters sent in `month`.
pub fn add_used(db: &Db, month: &str, n: i64) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO translate_usage (service, month, chars) VALUES (?,?,?) ON CONFLICT(service, month) DO UPDATE SET chars = chars + excluded.chars",
        p![SERVICE, month, n],
    )?;
    Ok(())
}

/// How many of `texts`, from the first, fit in what is left of `budget` after `used` (0: no limit).
pub fn fit(texts: &[String], used: i64, budget: i64) -> usize {
    if budget <= 0 {
        return texts.len();
    }
    let mut left = budget - used;
    let mut n = 0;
    for t in texts {
        left -= chars(t);
        if left < 0 {
            break;
        }
        n += 1;
    }
    n
}

/// Count `n` characters (negative: give them back) for `month`; a count that can't be saved is said in the log
/// (this month's count may then read low).
async fn count(st: &impl Ctx, month: &str, n: i64) {
    let (db, m) = (st.db().clone(), month.to_string());
    let res = tokio::task::spawn_blocking(move || add_used(&db, &m, n)).await;
    if let Some(e) = match res {
        Ok(Ok(())) => None,
        Ok(Err(e)) => Some(e.to_string()),
        Err(e) => Some(e.to_string()),
    } {
        tracing::warn!("translate: DeepL's character count could not be saved: {e}");
        st.db().log("warning", &format!("Translations: DeepL's character count could not be saved ({e}) – this month's count may read low"));
    }
}

// ---- pausing after an error ---------------------------------------------------------------------

/// DeepL paused after an error: until a time, or until DeepL's own settings change (a new key, a higher
/// budget, the switch; `Tiers::deepl_generation`). Other settings leave the hold as it is.
#[derive(Clone, Debug)]
pub struct Hold {
    pub error: Error,
    pub until: f64,
    /// DeepL's settings when it failed (`Tiers::deepl_generation`): a change lifts the hold
    pub generation: u64,
}

impl Hold {
    /// The hold after `error` at `now`.
    pub fn after(error: Error, now: f64, generation: u64) -> Hold {
        let until = match &error {
            Error::Key => now + 24.0 * 3600.0,
            Error::Quota => now + 6.0 * 3600.0,
            Error::Busy => now + 60.0,
            Error::Budget => next_month(now),
            Error::Refused(..) => now + 3600.0,
            Error::Net(_) | Error::Unreached(_) => now + 300.0,
        };
        Hold { error, until, generation }
    }

    /// Whether DeepL still waits at `now` with DeepL's settings at `generation`.
    pub fn holds(&self, now: f64, generation: u64) -> bool {
        now < self.until && generation == self.generation
    }
}

// ---- the pass -----------------------------------------------------------------------------------

/// Translate these texts for `target` with DeepL, where DeepL is set up, not paused and the month's budget
/// reaches: song and album titles only, grouped by script, 50 to a request. Each answer is stored with the
/// source `deepl` (a weaker source's translation gives way, a stronger one stays) and every text sent is
/// stamped as seen. Returns how many got a translation.
pub async fn pass(st: &impl Ctx, target: &'static str, items: &[(String, Kind)]) -> Result<usize, Error> {
    let tiers = Tiers::read(st.cfg());
    let tr = &st.library().translator;
    let generation = tiers.deepl_generation();
    if !tiers.deepl_ready() || !tr.deepl_usable(generation) {
        return Ok(0);
    }
    let Some(lang) = target_lang(target) else { return Ok(0) };
    let api = if st.urls().deepl.is_empty() { api_for(&tiers.deepl_key).to_string() } else { st.urls().deepl.clone() };
    // one source language per request
    let mut groups: BTreeMap<Option<&'static str>, Vec<(String, Kind)>> = BTreeMap::new();
    for (text, kind) in items.iter().filter(|(_, k)| wanted(*k)) {
        groups.entry(source_lang(script::detect(text))).or_default().push((text.clone(), *kind));
    }
    let mut done = 0;
    for (source, group) in groups {
        for chunk in group.chunks(BATCH) {
            let texts: Vec<String> = chunk.iter().map(|(t, _)| t.clone()).collect();
            let now = encore_db::now();
            let m = month(now);
            let used_now = {
                let (db, m) = (st.db().clone(), m.clone());
                tokio::task::spawn_blocking(move || used(&db, &m)).await.unwrap_or(0)
            };
            let n = fit(&texts, used_now, tiers.deepl_budget);
            if n == 0 {
                tr.deepl_failed(Error::Budget, generation);
                return Err(Error::Budget);
            }
            let send = &texts[..n];
            let sent: i64 = send.iter().map(|t| chars(t)).sum();
            // DeepL counts what it receives, also when its answer is lost on the way back (a timeout, a reset):
            // the characters are counted before asking, and given back when DeepL refused the request or the
            // request never left (no connection)
            count(st, &m, sent).await;
            let got = match translate(st.http(), &api, &tiers.deepl_key, send, source, lang).await {
                Ok(g) => g,
                Err(e) => {
                    if !matches!(e, Error::Net(_)) {
                        count(st, &m, -sent).await;
                    }
                    tr.deepl_failed(e.clone(), generation);
                    return Err(e);
                }
            };
            tr.deepl_worked();
            let rows: Vec<(String, Kind, String)> = chunk[..n].iter().zip(got).map(|((t, k), g)| (t.clone(), *k, g)).collect();
            done += rows.iter().filter(|(t, _, g)| !g.is_empty() && !script::same(g, t)).count();
            let db = st.db().clone();
            let _ = tokio::task::spawn_blocking(move || -> rusqlite::Result<()> {
                for (text, kind, got) in &rows {
                    store::put(&db, text, target, kind.as_str(), Some(got.as_str()).filter(|g| !g.is_empty()), None, store::Source::Deepl)?;
                }
                Ok(())
            }).await;
            if n < texts.len() {
                tr.deepl_failed(Error::Budget, generation);
                return Err(Error::Budget);
            }
        }
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_languages_and_kinds() {
        assert_eq!(api_for("0f4e2d1c-aaaa:fx"), FREE_API);
        assert_eq!(api_for(" 0f4e2d1c-aaaa:fx \n"), FREE_API);
        assert_eq!(api_for("0f4e2d1c-aaaa"), PRO_API);
        assert_eq!(target_lang("en"), Some("EN-US"));
        assert_eq!(target_lang("zh"), Some("ZH-HANS"));
        assert_eq!(target_lang("hi"), None, "DeepL has no Hindi");
        assert!(super::super::script::LANGUAGES.iter().filter(|(c, _, _)| *c != "hi").all(|(c, _, _)| target_lang(c).is_some()));
        assert_eq!(source_lang("ja"), Some("JA"));
        assert_eq!(source_lang("cyrl"), None);
        assert!(wanted(Kind::Title) && wanted(Kind::Album) && !wanted(Kind::Artist));
    }

    #[test]
    fn the_budget() {
        let t = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // 5 + 5 + 5 characters: 12 left takes two
        assert_eq!(fit(&t(&["ハルジオン", "マリーゴー", "アイドルだ"]), 88, 100), 2);
        assert_eq!(fit(&t(&["ハルジオン"]), 95, 100), 1, "exactly at the limit");
        assert_eq!(fit(&t(&["ハルジオン"]), 96, 100), 0);
        assert_eq!(fit(&t(&["ハルジオン", "x"]), 10_000_000, 0), 2, "0: no limit");
        assert_eq!(chars("夜に駆ける"), 5, "DeepL counts characters, not bytes");
        assert_eq!(month(0.0), "1970-01");
        assert_eq!(month(1_792_800_000.0), "2026-10");
        assert_eq!(next_month(1_792_800_000.0), encore_core::time::days_from_civil(2026, 11, 1) as f64 * 86_400.0);
        assert_eq!(next_month(encore_core::time::days_from_civil(2026, 12, 15) as f64 * 86_400.0), encore_core::time::days_from_civil(2027, 1, 1) as f64 * 86_400.0);
        // the count per month, kept in the database
        let d = std::env::temp_dir().join(format!("encore-deepl-budget-{}-{}", std::process::id(), encore_core::ids::random_hex(4)));
        let db = Db::open(&d).unwrap();
        assert_eq!(used(&db, "2026-10"), 0);
        add_used(&db, "2026-10", 12_000).unwrap();
        add_used(&db, "2026-10", 340).unwrap();
        add_used(&db, "2026-11", 7).unwrap();
        assert_eq!((used(&db, "2026-10"), used(&db, "2026-11"), used(&db, "2026-09")), (12_340, 7, 0));
        drop(db);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn holds_end_with_time_or_new_settings() {
        let now = 1_792_800_000.0;
        let key = Hold::after(Error::Key, now, 7);
        assert!(key.holds(now + 3600.0, 7));
        assert!(!key.holds(now + 3600.0, 8), "a new key lifts it");
        let busy = Hold::after(Error::Busy, now, 7);
        assert!(busy.holds(now + 30.0, 7) && !busy.holds(now + 61.0, 7));
        let budget = Hold::after(Error::Budget, now, 7);
        assert!(budget.holds(next_month(now) - 1.0, 7) && !budget.holds(next_month(now), 7), "until the month is over");
        assert!(!budget.holds(now, 8), "or the budget was raised");
    }
}
