//! The Settings → Server pages as the server describes them: GET /api/admin/settings/schema (written down in
//! docs/contracts/settings-schema.md) lists the pages (sections), the cards on each page (groups) and the fields
//! on each card, with their words, type, options and visibility rule. The server's settings table
//! (server/crates/config/settings.toml) is the one place these are kept; the values themselves still come from
//! GET /api/admin/settings, with `_meta` saying where each one is set.

use crate::json::{js_string, J};
use serde_json::Value;
use std::rc::Rc;

/// A field's visibility rule: `key=value` (shown while the select `key` has that value) or `key!=value`
/// (shown while it has any other).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct When {
    pub key: String,
    pub value: String,
    /// true for `=`, false for `!=`
    pub equal: bool,
}

impl When {
    /// Reads `key=value` or `key!=value`; None for anything else (the field is then always shown).
    pub fn parse(rule: &str) -> Option<When> {
        let (key, value, equal) = match rule.split_once("!=") {
            Some((k, v)) => (k, v, false),
            None => { let (k, v) = rule.split_once('=')?; (k, v, true) }
        };
        let (key, value) = (key.trim(), value.trim());
        if key.is_empty() || value.is_empty() { return None; }
        Some(When { key: key.to_string(), value: value.to_string(), equal })
    }

    /// Whether the field shows while the select holds `current`.
    pub fn shows(&self, current: &str) -> bool { (current == self.value) == self.equal }
}

/// One field of a card.
#[derive(Clone, Debug)]
pub struct Field {
    pub key: String,
    pub label: String,
    /// the line under the field
    pub help: Option<String>,
    /// text | password | number | bool | select | checks | textarea | path | url (path and url are drawn as text)
    pub kind: String,
    /// (value, label) for select and checks
    pub options: Vec<(String, String)>,
    pub when: Option<When>,
    /// the server never sends the value back (passwords, the WireGuard config, pasted cookies)
    pub secret: bool,
    /// the field takes the card's whole width
    pub full: bool,
    /// a number input's step, as the attribute reads it ("0.1")
    pub step: Option<String>,
}

impl Field {
    pub fn from_json(f: &Value) -> Field {
        let text = |k: &str| f.st(k).map(str::to_string);
        Field {
            key: f.s("key"),
            label: f.s("label"),
            help: text("help"),
            kind: text("type").unwrap_or_else(|| "text".to_string()),
            options: f.a("options").iter().map(|o| (o.s("value"), o.s("label"))).collect(),
            when: f.st("when").and_then(When::parse),
            secret: f.truthy("secret"),
            full: f.truthy("full"),
            step: if f.is_nullish("step") { None } else { Some(js_string(f.g("step"))) },
        }
    }

    /// A password field or a secret text area: the input starts empty and an empty input keeps the saved value.
    pub fn is_secret(&self) -> bool { self.kind == "password" || self.secret }
}

/// A card: a run of fields with a Save button (and a Test button when `test` is set).
#[derive(Clone, Debug)]
pub struct Group {
    pub id: String,
    pub title: String,
    pub desc: String,
    /// the heading over a run of cards; empty for none (no page has one since R2)
    pub part: String,
    /// POST /api/system/test/<test>
    pub test: Option<String>,
    pub test_label: Option<String>,
    pub fields: Vec<Field>,
}

impl Group {
    pub fn from_json(g: &Value) -> Group {
        Group {
            id: g.s("id"),
            title: g.s("title"),
            desc: g.s("desc"),
            part: g.s("part"),
            test: g.st("test").map(str::to_string),
            test_label: g.st("test_label").map(str::to_string),
            fields: g.a("fields").iter().map(Field::from_json).collect(),
        }
    }

    pub fn field(&self, key: &str) -> Option<&Field> { self.fields.iter().find(|f| f.key == key) }
}

/// A page under Settings → Server. A page with no cards (Auto-download, Contribute, Blocked) is drawn by its own code.
#[derive(Clone, Debug)]
pub struct Section {
    pub id: String,
    pub title: String,
    /// older `?s=` names that open this page
    pub aliases: Vec<String>,
    pub groups: Vec<Rc<Group>>,
}

/// The whole answer of GET /api/admin/settings/schema.
#[derive(Clone, Debug, Default)]
pub struct Schema {
    pub sections: Vec<Section>,
}

impl Schema {
    pub fn from_json(v: &Value) -> Schema {
        Schema {
            sections: v.a("sections").iter().map(|s| Section {
                id: s.s("id"),
                title: s.s("title"),
                aliases: s.a("aliases").iter().map(js_string).collect(),
                groups: s.a("groups").iter().map(|g| Rc::new(Group::from_json(g))).collect(),
            }).collect(),
        }
    }

    pub fn section(&self, id: &str) -> Option<&Section> { self.sections.iter().find(|s| s.id == id) }

    pub fn group(&self, id: &str) -> Option<Rc<Group>> {
        self.sections.iter().flat_map(|s| s.groups.iter()).find(|g| g.id == id).cloned()
    }

    /// The page that shows `id`: a page's own id, one of its aliases (`apps`, the page Apps & sources was
    /// until R2, opens Downloads), or the id of one of its cards (old `#/settings?s=` links name cards), in that
    /// order. Anything else is returned as it is.
    pub fn page_of(&self, id: &str) -> String {
        let page = self.sections.iter().find(|s| s.id == id)
            .or_else(|| self.sections.iter().find(|s| s.aliases.iter().any(|a| a == id)))
            .or_else(|| self.sections.iter().find(|s| s.groups.iter().any(|g| g.id == id)));
        page.map(|s| s.id.clone()).unwrap_or_else(|| id.to_string())
    }

    /// Whether page `id` holds the card `card` (the Download tools card and How requests are fulfilled go on
    /// the page with the Download pipeline card).
    pub fn holds(&self, id: &str, card: &str) -> bool {
        self.section(id).is_some_and(|s| s.groups.iter().any(|g| g.id == card))
    }
}

/// Release kinds and extras compare without case, spaces and punctuation: a saved "Spoken word" ticks the
/// "Spokenword" box.
pub fn type_key(s: &str) -> String { s.to_lowercase().chars().filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit()).collect() }
