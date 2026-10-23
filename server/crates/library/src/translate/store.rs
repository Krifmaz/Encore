//! The translation cache: one row per (text, source language, target language). A row keeps what
//! each source said, field by field, with the source – official (MusicBrainz, Apple) beats the built-in
//! romanization, which beats DeepL, which beats the model (the tier order of tiers.rs), and the built-in
//! letter-by-letter romanization of Hebrew, Arabic, Thai and Devanagari only fills a gap – and the manual
//! correction beside it, which beats them all and is never touched by the background work. Each source
//! stamps the rows it looked at (official_at, romanized_at, deepl_at, model_at), so a source switched on
//! later takes the rows it has not seen and replaces a weaker source's text.
use super::script;
use encore_db::{now, Db, RowExt};
use encore_db::p;
use serde_json::{json, Value};
use std::collections::HashMap;

/// Where a translation came from, weakest first (a manual correction is kept apart and beats them all).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Source {
    /// the built-in romanization spelled letter by letter (romanize.rs: Hebrew, Arabic, Thai, Devanagari):
    /// fills a romanization that is missing, and any other source replaces it
    Letters,
    /// the local model (llama.rs)
    Model,
    /// DeepL (deepl.rs): translations only
    Deepl,
    /// the built-in romanization (romanize.rs): romanizations only
    Romanized,
    /// MusicBrainz and Apple (official.rs)
    Official,
}

impl Source {
    /// Every source, weakest first.
    pub const ALL: [Source; 5] = [Source::Letters, Source::Model, Source::Deepl, Source::Romanized, Source::Official];

    /// Its name in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Letters => "letters",
            Source::Model => "model",
            Source::Deepl => "deepl",
            Source::Romanized => "romanized",
            Source::Official => "official",
        }
    }
    /// The strength of a stored source name (0 for none or unknown).
    fn rank(s: &str) -> u8 {
        Source::ALL.iter().position(|x| x.as_str() == s).map(|i| i as u8 + 1).unwrap_or(0)
    }
    /// The column with the time this source last looked at a row.
    fn stamp(self) -> &'static str {
        match self {
            Source::Model => "model_at",
            // the built-in romanization's tier, as the tables'
            Source::Letters => "romanized_at",
            Source::Deepl => "deepl_at",
            Source::Romanized => "romanized_at",
            Source::Official => "official_at",
        }
    }
}

/// The cache key of a text: (normalized text, source language). The source language follows from the
/// text (see script::detect), so the same title always lands on the same row.
pub fn key(text: &str) -> (String, &'static str) {
    let t = script::normalize(text);
    let src = script::detect(&t);
    (t, src)
}

/// What a listener sees for one text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shown {
    /// the translation shown
    pub translation: Option<String>,
    /// the romanization shown
    pub romanization: Option<String>,
    /// official | romanized | letters | deepl | model | manual (of the translation, or of the romanization when there is none)
    pub source: String,
}

/// Make the row for `text` when there is none yet.
fn ensure_row(db: &Db, text: &str, src: &str, target: &str, kind: &str) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO translations (text, src_lang, target, kind, updated_at) VALUES (?,?,?,?,?) ON CONFLICT(text, src_lang, target) DO NOTHING",
        p![text, src, target, kind, now()],
    )?;
    Ok(())
}

/// Store what a source found. A field is only replaced by a source at least as strong as the one that
/// filled it; None leaves the field as it is. Either way the source is marked as asked for this text.
pub fn put(db: &Db, text: &str, target: &str, kind: &str, translation: Option<&str>, romanization: Option<&str>, source: Source) -> rusqlite::Result<()> {
    let (t, src) = key(text);
    if t.is_empty() {
        return Ok(());
    }
    ensure_row(db, &t, src, target, kind)?;
    let row = db.one("SELECT translation_source, romanization_source FROM translations WHERE text=? AND src_lang=? AND target=?", p![t, src, target])?.unwrap_or_default();
    let mine = Source::rank(source.as_str());
    let tr = translation.map(str::trim).filter(|x| !x.is_empty() && !script::same(x, &t)).filter(|_| mine >= Source::rank(&row.s("translation_source")));
    let ro = romanization.map(str::trim).filter(|x| !x.is_empty() && !script::same(x, &t)).filter(|_| mine >= Source::rank(&row.s("romanization_source")));
    let stamp = source.stamp();
    db.execute(
        &format!(
            "UPDATE translations SET
               translation = CASE WHEN ? IS NULL THEN translation ELSE ? END,
               translation_source = CASE WHEN ? IS NULL THEN translation_source ELSE ? END,
               romanization = CASE WHEN ? IS NULL THEN romanization ELSE ? END,
               romanization_source = CASE WHEN ? IS NULL THEN romanization_source ELSE ? END,
               {stamp} = ?, updated_at = ?
             WHERE text=? AND src_lang=? AND target=?"
        ),
        p![tr, tr, tr, source.as_str(), ro, ro, ro, source.as_str(), now(), now(), t, src, target],
    )?;
    Ok(())
}

/// Mark a source as asked for a text without storing anything (it had nothing).
pub fn mark(db: &Db, text: &str, target: &str, kind: &str, source: Source) -> rusqlite::Result<()> {
    put(db, text, target, kind, None, None, source)
}

/// A manual correction (⋯ → Edit translation). Some("") means "show nothing"; None for both removes
/// the correction, and what the sources found shows again.
pub fn set_manual(db: &Db, text: &str, target: &str, kind: &str, translation: Option<&str>, romanization: Option<&str>) -> rusqlite::Result<()> {
    let (t, src) = key(text);
    if t.is_empty() {
        return Ok(());
    }
    ensure_row(db, &t, src, target, kind)?;
    db.execute(
        "UPDATE translations SET manual_translation=?, manual_romanization=?, updated_at=? WHERE text=? AND src_lang=? AND target=?",
        p![translation.map(str::trim), romanization.map(str::trim), now(), t, src, target],
    )?;
    Ok(())
}

/// What a row shows: a manual correction over the automatic text, for each of the two.
fn shown(r: &encore_db::Row) -> Shown {
    let pick = |manual: &str, auto: &str| -> (Option<String>, bool) {
        match r.get(manual) {
            Some(Value::String(m)) => (Some(m.clone()).filter(|m| !m.is_empty()), true),
            _ => (Some(r.s(auto)).filter(|a| !a.is_empty()), false),
        }
    };
    let (translation, tm) = pick("manual_translation", "translation");
    let (romanization, rm) = pick("manual_romanization", "romanization");
    let source = if tm || rm {
        "manual".to_string()
    } else if translation.is_some() {
        r.s("translation_source")
    } else {
        r.s("romanization_source")
    };
    Shown { translation, romanization, source }
}

/// What to show for these texts in `target` (normalized text → Shown); texts without a row, or
/// whose row has nothing to show, are left out.
pub fn lookup(db: &Db, target: &str, texts: &[String]) -> HashMap<String, Shown> {
    let mut out = HashMap::new();
    let keys: Vec<String> = {
        let mut k: Vec<String> = texts.iter().map(|t| script::normalize(t)).filter(|t| !t.is_empty()).collect();
        k.sort();
        k.dedup();
        k
    };
    for chunk in keys.chunks(400) {
        let mut params = vec![json!(target)];
        params.extend(chunk.iter().map(|t| json!(t)));
        let sql = format!(
            "SELECT text, translation, translation_source, romanization, romanization_source, manual_translation, manual_romanization
             FROM translations WHERE target=? AND text IN ({})",
            crate::artists::marks(chunk.len())
        );
        if let Ok(rows) = db.query(&sql, params) {
            for r in rows {
                let s = shown(&r);
                if s.translation.is_some() || s.romanization.is_some() {
                    out.insert(r.s("text"), s);
                }
            }
        }
    }
    out
}

/// Which of these texts have a row for `target` already (asked or corrected).
pub fn known(db: &Db, target: &str, texts: &[String]) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for chunk in texts.chunks(400) {
        let mut params = vec![json!(target)];
        params.extend(chunk.iter().map(|t| json!(t)));
        let sql = format!("SELECT text FROM translations WHERE target=? AND text IN ({})", crate::artists::marks(chunk.len()));
        if let Ok(rows) = db.query(&sql, params) {
            out.extend(rows.iter().map(|r| r.s("text")));
        }
    }
    out
}

/// The full row for one text, for the edit dialog.
pub fn entry(db: &Db, text: &str, target: &str) -> Value {
    let (t, src) = key(text);
    let row = db.one("SELECT * FROM translations WHERE text=? AND src_lang=? AND target=?", p![t, src, target]).ok().flatten();
    let s = row.as_ref().map(shown).unwrap_or_default();
    json!({
        "text": t, "src_lang": src, "target": target,
        "translation": s.translation, "romanization": s.romanization, "source": if s.source.is_empty() { Value::Null } else { json!(s.source) },
        "manual": row.as_ref().is_some_and(|r| r.get("manual_translation").is_some_and(|v| v.is_string()) || r.get("manual_romanization").is_some_and(|v| v.is_string())),
        "auto_translation": row.as_ref().map(|r| r.s("translation")).filter(|x| !x.is_empty()),
        "auto_romanization": row.as_ref().map(|r| r.s("romanization")).filter(|x| !x.is_empty()),
        "auto_source": row.as_ref().map(|r| r.s("translation_source")).filter(|x| !x.is_empty()),
    })
}

/// Rows of `target` the model has not looked at yet (texts that still need a romanization or a
/// translation the official sources did not give), oldest first.
pub fn waiting_for_model(db: &Db, target: &str, limit: usize) -> Vec<(String, String)> {
    db.query(
        "SELECT text, COALESCE(kind,'title') kind FROM translations WHERE target=? AND model_at IS NULL
           AND manual_translation IS NULL AND (translation IS NULL OR romanization IS NULL) ORDER BY updated_at LIMIT ?",
        p![target, limit as i64],
    ).unwrap_or_default().into_iter().map(|r| (r.s("text"), r.s("kind"))).collect()
}

/// Rows of `target` that `source` has not looked at yet and that it may still improve, oldest first: (text,
/// kind). Romanization: texts in a script it romanizes without a manual romanization. DeepL: song and album
/// titles without a manual translation whose translation is missing or the model's. Official: any row.
pub fn unseen(db: &Db, target: &str, source: Source, limit: usize) -> Vec<(String, String)> {
    let what = match source {
        Source::Romanized | Source::Letters => format!(
            "romanized_at IS NULL AND manual_romanization IS NULL AND src_lang IN ({})",
            super::romanize::SCRIPTS.iter().map(|s| format!("'{s}'")).collect::<Vec<_>>().join(",")
        ),
        Source::Deepl => "deepl_at IS NULL AND manual_translation IS NULL AND COALESCE(kind,'title') IN ('title','album')
                          AND (translation IS NULL OR translation_source='model')".to_string(),
        Source::Official => "official_at IS NULL".to_string(),
        Source::Model => return waiting_for_model(db, target, limit),
    };
    db.query(
        &format!("SELECT text, COALESCE(kind,'title') kind FROM translations WHERE target=? AND {what} ORDER BY updated_at LIMIT ?"),
        p![target, limit as i64],
    ).unwrap_or_default().into_iter().map(|r| (r.s("text"), r.s("kind"))).collect()
}

/// Of these texts, the ones whose translation `source` could still give: no correction by hand, and no
/// translation yet or one from a weaker source (no row at all counts too).
pub fn improvable(db: &Db, target: &str, texts: &[String], source: Source) -> std::collections::HashSet<String> {
    let mut out: std::collections::HashSet<String> = texts.iter().map(|t| script::normalize(t)).filter(|t| !t.is_empty()).collect();
    let keys: Vec<String> = out.iter().cloned().collect();
    let mine = Source::rank(source.as_str());
    for chunk in keys.chunks(400) {
        let mut params = vec![json!(target)];
        params.extend(chunk.iter().map(|t| json!(t)));
        let sql = format!(
            "SELECT text, translation, translation_source, manual_translation FROM translations WHERE target=? AND text IN ({})",
            crate::artists::marks(chunk.len())
        );
        for r in db.query(&sql, params).unwrap_or_default() {
            let manual = r.get("manual_translation").is_some_and(|v| v.is_string());
            let has = !r.s("translation").is_empty();
            if manual || (has && Source::rank(&r.s("translation_source")) >= mine) {
                out.remove(&r.s("text"));
            }
        }
    }
    out
}

/// How many titles have a correction by hand (any language).
pub fn manual_count(db: &Db) -> i64 {
    db.one("SELECT COUNT(*) n FROM translations WHERE manual_translation IS NOT NULL OR manual_romanization IS NOT NULL", p![])
        .ok().flatten().map(|r| r.i("n")).unwrap_or(0)
}

/// Counts for the status line: (rows, with something to show, waiting for the model).
pub fn counts(db: &Db) -> (i64, i64, i64) {
    let r = db.one(
        "SELECT COUNT(*) n,
                SUM(CASE WHEN COALESCE(manual_translation, translation, manual_romanization, romanization) IS NOT NULL THEN 1 ELSE 0 END) shown,
                SUM(CASE WHEN model_at IS NULL AND manual_translation IS NULL AND (translation IS NULL OR romanization IS NULL) THEN 1 ELSE 0 END) waiting
         FROM translations",
        p![],
    ).ok().flatten().unwrap_or_default();
    (r.i("n"), r.i("shown"), r.i("waiting"))
}
