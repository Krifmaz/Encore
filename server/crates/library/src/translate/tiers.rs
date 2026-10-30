//! Where translated and romanized titles come from, strongest first; each source but the first can be switched
//! off (Settings → Library → Translated titles, and the list on Settings → Language for admins):
//!
//! 1. corrections by hand (⋯ → Edit translation): always, never replaced;
//! 2. official names (`translate_official`): MusicBrainz pseudo-releases and aliases, Apple's English names
//!    (official.rs);
//! 3. romanization built into the server (`translate_romanize`): kana, Hangul, Cyrillic, Greek and more from
//!    tables (romanize.rs) – the `_r` fields only, no model and no network;
//! 4. DeepL (`translate_deepl`, used once `deepl_api_key` is set): song and album titles, within the monthly
//!    budget `deepl_char_budget` (deepl.rs) – the `_t` fields only;
//! 5. the local model (`translate_local_model`), only when the Translator plugin is installed and switched on
//!    (Settings → Plugins; llama.rs): it is optional and not installed by default.
//!
//! Every result keeps its source (store::Source), and a stronger source replaces a weaker one's text when it
//! gets to the row, also later: each source stamps the rows it looked at, and the background pass takes the
//! rows a source has not seen (a source switched on later, or new in this version).
use super::{deepl, llama, store};
use crate::Ctx;
use serde_json::{json, Value};

/// The sources' switches and DeepL's key and budget, as the settings have them now.
#[derive(Clone, Debug, PartialEq)]
pub struct Tiers {
    /// official translations (MusicBrainz pseudo-releases)
    pub official: bool,
    /// the built-in romanization
    pub romanize: bool,
    /// DeepL is switched on
    pub deepl: bool,
    /// DeepL's key
    pub deepl_key: String,
    /// characters a month; 0: no limit
    pub deepl_budget: i64,
    /// the local model
    pub model: bool,
}

impl Tiers {
    /// The switches as the settings have them now.
    pub fn read(cfg: &encore_config::Config) -> Tiers {
        Tiers {
            official: cfg.bool("translate_official"),
            romanize: cfg.bool("translate_romanize"),
            deepl: cfg.bool("translate_deepl"),
            deepl_key: cfg.str("deepl_api_key").trim().to_string(),
            deepl_budget: cfg.int("deepl_char_budget").max(0),
            model: cfg.bool("translate_local_model"),
        }
    }

    /// DeepL is on and has a key.
    pub fn deepl_ready(&self) -> bool {
        self.deepl && !self.deepl_key.is_empty()
    }

    /// A number that changes with DeepL's own settings (the switch, the key, the budget) and with nothing
    /// else: a pause after an error (deepl::Hold) lasts until it changes, so saving another setting does not
    /// ask DeepL again.
    pub fn deepl_generation(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (self.deepl, &self.deepl_key, self.deepl_budget).hash(&mut h);
        h.finish()
    }
}

/// 12340 → "12 340" (a no-break space between the groups of three).
pub fn grouped(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, d) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push('\u{a0}');
        }
        out.push(d);
    }
    if n < 0 { format!("-{out}") } else { out }
}

/// What DeepL's line says: the month's characters against the budget, and why it pauses when it does.
pub fn deepl_state(t: &Tiers, used: i64, hold: Option<&deepl::Error>) -> String {
    if !t.deepl {
        return "DeepL: off".into();
    }
    if t.deepl_key.is_empty() {
        return "DeepL: no API key – add one under Settings → Library → Translated titles".into();
    }
    if hold == Some(&deepl::Error::Key) {
        return "DeepL: the API key was refused – check it under Settings → Library → Translated titles".into();
    }
    let count = if t.deepl_budget > 0 {
        format!("DeepL: {} of {} characters this month", grouped(used), grouped(t.deepl_budget))
    } else {
        format!("DeepL: {} characters this month (no limit)", grouped(used))
    };
    match hold {
        Some(e) => format!("{count} – paused: {e}"),
        None => count,
    }
}

/// Where the Translator plugin stands (Settings → Plugins), for the local model's line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Plugin {
    /// not installed, and not being installed
    Missing,
    /// the Plugins page installs it now
    Installing,
    /// installed, switched off on its card
    SwitchedOff,
    /// installed and switched on, but its llama.cpp program is not there (a failed install or update)
    NoProgram,
    /// installed, switched on, llama.cpp there
    Installed,
}

impl Plugin {
    /// The Translator plugin's state from the plugins' registry (and an adopted pre-0.32 install, llama.rs).
    pub fn of(st: &impl Ctx) -> Plugin {
        let reg = st.plugins();
        if !reg.installed("translator") {
            return if reg.busy_with("translator") == Some("installing") { Plugin::Installing } else { Plugin::Missing };
        }
        if !reg.enabled("translator") {
            Plugin::SwitchedOff
        } else if llama::installed(st).is_none() {
            Plugin::NoProgram
        } else {
            Plugin::Installed
        }
    }
}

/// What the local model's line says.
pub fn model_state(on: bool, plugin: Plugin, has_model: bool, model: &str, running: bool) -> String {
    match plugin {
        Plugin::Missing => return "Local model: not installed – install it in Plugins".into(),
        Plugin::Installing => return "Local model: installing now – see Plugins".into(),
        _ if !on => return "Local model: off".into(),
        Plugin::SwitchedOff => return "Local model: switched off in Plugins – switch the Translator on there".into(),
        Plugin::NoProgram => return "Local model: llama.cpp is missing – update the Translator in Plugins".into(),
        Plugin::Installed => {}
    }
    if !has_model {
        format!("Local model: installed, the {model} model is not downloaded yet – download it in Plugins")
    } else if running {
        format!("Local model: running now ({model} model)")
    } else {
        format!("Local model: ready ({model} model) – starts when there is something to translate")
    }
}

/// The sources for Settings → Language (admins), strongest first: id, name, the settings switch (none for
/// the corrections), on, the state line and a sentence on what it does. Reads the database: call it off the
/// async threads.
pub fn states(st: &impl Ctx) -> Vec<Value> {
    let t = Tiers::read(st.cfg());
    let db = st.db();
    let manual = store::manual_count(db);
    let used = deepl::used(db, &deepl::month(encore_db::now()));
    let hold = st.library().translator.deepl_hold(t.deepl_generation());
    let m = llama::model(st);
    let plugin = Plugin::of(st);
    let has_model = llama::model_path(st, &m).is_file();
    let onoff = |on: bool, what: &str| if on { what.to_string() } else { "off".to_string() };
    vec![
        json!({
            "id": "manual", "name": "Corrections by hand", "setting": null, "on": true,
            "state": match manual { 0 => "Corrections by hand: none yet".to_string(), 1 => "Corrections by hand: 1 title".to_string(), n => format!("Corrections by hand: {n} titles") },
            "help": "Made with ⋯ → Edit translation. They come first and are never replaced.",
        }),
        json!({
            "id": "official", "name": "Official names", "setting": "translate_official", "on": t.official,
            "state": format!("Official names: {}", onoff(t.official, "MusicBrainz and Apple")),
            "help": "MusicBrainz pseudo-releases and aliases, and Apple's English names for Japanese releases.",
        }),
        json!({
            "id": "romanize", "name": "Romanization", "setting": "translate_romanize", "on": t.romanize,
            "state": format!("Romanization: {}", onoff(t.romanize, "built in")),
            "help": "Japanese kana, Korean, Cyrillic, Greek, Hebrew, Arabic, Thai and Devanagari, made on this server. Titles with kanji and Chinese titles are left to the other sources.",
        }),
        json!({
            "id": "deepl", "name": "DeepL", "setting": "translate_deepl", "on": t.deepl, "ready": t.deepl_ready(),
            "used": used, "budget": t.deepl_budget,
            "state": deepl_state(&t, used, hold.as_ref()),
            "help": "Translates song and album titles with your DeepL API key, never lyrics or artist names. Free keys end in :fx.",
        }),
        json!({
            "id": "model", "name": "Local model", "setting": "translate_local_model", "on": t.model, "ready": plugin == Plugin::Installed && has_model,
            "state": model_state(t.model, plugin, has_model, m.id, llama::running(st)),
            "help": "Translates and romanizes what the other sources leave, on this server, with the Translator plugin (llama.cpp and a model). It needs several GB of memory and is not installed by default.",
        }),
    ]
}

/// One line for every listener on Settings → Language: which sources this server uses.
pub fn summary(st: &impl Ctx) -> String {
    let t = Tiers::read(st.cfg());
    let mut used: Vec<&str> = vec![];
    if t.official {
        used.push("official names");
    }
    if t.romanize {
        used.push("built-in romanization");
    }
    if t.deepl_ready() {
        used.push("DeepL");
    }
    if t.model && llama::ready(st) {
        used.push("the local model");
    }
    match used.len() {
        0 => "This server makes no translations of its own: only corrections by hand show.".into(),
        1 => format!("This server uses {} besides corrections by hand.", used[0]),
        n => format!("This server uses {} and {} besides corrections by hand.", used[..n - 1].join(", "), used[n - 1]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiers() -> Tiers {
        Tiers { official: true, romanize: true, deepl: true, deepl_key: "k:fx".into(), deepl_budget: 400_000, model: true }
    }

    #[test]
    fn state_lines() {
        assert_eq!(grouped(12_340), "12\u{a0}340");
        assert_eq!(grouped(400_000), "400\u{a0}000");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_000_000), "1\u{a0}000\u{a0}000");
        let t = tiers();
        assert_eq!(deepl_state(&t, 12_340, None), "DeepL: 12\u{a0}340 of 400\u{a0}000 characters this month");
        assert_eq!(deepl_state(&Tiers { deepl_budget: 0, ..t.clone() }, 5, None), "DeepL: 5 characters this month (no limit)");
        assert_eq!(deepl_state(&Tiers { deepl_key: String::new(), ..t }, 0, None), "DeepL: no API key – add one under Settings → Library → Translated titles");
        assert_eq!(deepl_state(&Tiers { deepl: false, ..t.clone() }, 0, None), "DeepL: off");
        assert!(deepl_state(&t, 0, Some(&deepl::Error::Key)).contains("refused"));
        assert_eq!(deepl_state(&t, 400_000, Some(&deepl::Error::Budget)), "DeepL: 400\u{a0}000 of 400\u{a0}000 characters this month – paused: this month's DeepL budget is used up");
        assert_eq!(model_state(true, Plugin::Missing, false, "small", false), "Local model: not installed – install it in Plugins");
        assert_eq!(model_state(false, Plugin::Missing, false, "small", false), "Local model: not installed – install it in Plugins");
        assert_eq!(model_state(true, Plugin::Installing, false, "small", false), "Local model: installing now – see Plugins");
        assert_eq!(model_state(false, Plugin::Installed, true, "small", false), "Local model: off");
        assert_eq!(model_state(false, Plugin::SwitchedOff, true, "small", false), "Local model: off", "the tier's own switch first");
        assert_eq!(model_state(true, Plugin::SwitchedOff, true, "small", false), "Local model: switched off in Plugins – switch the Translator on there");
        assert_eq!(model_state(true, Plugin::NoProgram, true, "small", false), "Local model: llama.cpp is missing – update the Translator in Plugins");
        assert_eq!(model_state(true, Plugin::Installed, false, "small", false), "Local model: installed, the small model is not downloaded yet – download it in Plugins");
        assert!(model_state(true, Plugin::Installed, true, "small", true).starts_with("Local model: running now"));
        assert_eq!(model_state(true, Plugin::Installed, true, "best", false), "Local model: ready (best model) – starts when there is something to translate");
        assert!(t.deepl_ready() && !Tiers { deepl: false, ..t.clone() }.deepl_ready() && !Tiers { deepl_key: String::new(), ..t }.deepl_ready());
    }
}
