//! The one-time move of Encore's own download settings into Lantern: the indexers (Prowlarr), the torrent
//! and Usenet clients and the Soulseek account and preferences.
//!
//! [`plan`] is the dry run: every setting the move would carry, read from the cards of the settings table
//! (the Torrents & Usenet card and the Soulseek card, each field only while its card's own visibility rule
//! shows it, so only the chosen clients' settings), secrets masked. A card that is switched off moves
//! nothing and says so. The plan follows the settings table, so a field added to one of those cards is in
//! the move without a change here.
//!
//! [`apply`] writes the plan into Lantern and marks Lantern as having taken over. Writing into Lantern
//! needs its API, which is final with Lantern 1.0, so until then it refuses with [`super::NOT_YET`] and
//! changes nothing. Encore's own settings are never deleted by the move: disconnecting Lantern brings them
//! back as they were.
use encore_config::settings::{Group, ENCORE};
use encore_config::{Config, MASK};
use serde::Serialize;
use serde_json::Value;

/// The settings cards the move carries, with the setting that switches each one on.
const CARDS: [(&str, &str); 2] = [("torznab", "torznab_enabled"), ("slskd", "slskd_enabled")];

/// One setting the move carries.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Item {
    /// what it belongs to in Lantern's terms: "Indexers (Prowlarr)", "Torrent client", "Usenet client" or
    /// "Soulseek"
    pub part: &'static str,
    /// Encore's setting
    pub key: &'static str,
    /// its label on the settings card
    pub label: &'static str,
    /// its value; a secret's is [`MASK`] when set, "" when not (never the secret itself)
    pub value: Value,
    /// whether it is a secret
    pub secret: bool,
    /// whether it still has its default (nothing was changed there)
    pub default: bool,
}

/// What the move would carry (the dry run's answer).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Plan {
    /// every setting it carries, card by card in the order of the settings pages
    pub items: Vec<Item>,
    /// the cards that carry nothing, with why ("Soulseek is switched off")
    pub skipped: Vec<String>,
}

impl Plan {
    /// Whether the move would carry nothing at all.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// The dry run: what the move would carry, from the settings in effect now. Changes nothing.
pub fn plan(cfg: &Config) -> Plan {
    let mut plan = Plan { items: vec![], skipped: vec![] };
    for (card, switch) in CARDS {
        let Some(group) = group(card) else { continue };
        if !cfg.bool(switch) {
            plan.skipped.push(format!("{} is switched off: nothing to move", group.title));
            continue;
        }
        for f in group.fields.iter().filter(|f| shown(cfg, f.when)) {
            let def = encore_config::settings::def(f.key);
            let secret = def.is_some_and(|d| d.secret);
            let value = cfg.get(f.key);
            let default = encore_config::settings::default_of(f.key).is_some_and(|d| same(&d, &value));
            let value = if secret { Value::from(if text(&value).is_empty() { "" } else { MASK }) } else { value };
            plan.items.push(Item { part: part(card, f.key, f.when), key: f.key, label: f.label, value, secret, default });
        }
    }
    plan
}

/// The move itself: write the plan into Lantern, then mark Lantern as having taken over
/// (`lantern_took_over`). Refused until Lantern 1.0's API is final ([`super::NOT_YET`]); nothing is changed.
pub fn apply() -> Result<(), &'static str> {
    Err(super::NOT_YET)
}

/// The settings table's card `id`.
fn group(id: &str) -> Option<&'static Group> {
    ENCORE.sections.iter().flat_map(|s| s.groups.iter()).find(|g| g.id == id)
}

/// Whether a field with visibility rule `when` shows with the settings in effect (no rule: always).
fn shown(cfg: &Config, when: Option<&str>) -> bool {
    let Some(rule) = when else { return true };
    // (the build checks every rule, so one that doesn't parse can't be in the table)
    let Ok(w) = encore_config::when::parse(rule) else { return true };
    (text(&cfg.get(w.key)) == w.value) == w.equal
}

/// Where a field belongs in Lantern's terms: a Soulseek card field is Soulseek's; on the Torrents & Usenet
/// card, the client selects and the fields they show belong to that client, the rest to the indexers.
fn part(card: &str, key: &str, when: Option<&str>) -> &'static str {
    if card == "slskd" {
        return "Soulseek";
    }
    let of = |select: &str| key == select || when.is_some_and(|w| w.starts_with(&format!("{select}=")) || w.starts_with(&format!("{select}!=")));
    if of("torrent_client") {
        "Torrent client"
    } else if of("nzb_client") {
        "Usenet client"
    } else {
        "Indexers (Prowlarr)"
    }
}

/// A setting's value as the text a visibility rule compares (`true`, `qbittorrent`, `2`).
fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Whether a value is its default: numbers by value (2 and 2.0 are the same), the rest by their text.
fn same(default: &Value, value: &Value) -> bool {
    match (default.as_f64(), value.as_f64()) {
        (Some(a), Some(b)) => a == b,
        _ => text(default) == text(value),
    }
}
