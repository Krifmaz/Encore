//! The settings table from settings.toml (generated into settings_gen.rs by build.rs): `Keys`, `TABLE` (what
//! the loader reads), `SCHEMA` (the Settings pages, their cards and fields) and a typed accessor per key on
//! `Config`. `Config::schema` is the answer of GET /api/admin/settings/schema (docs/contracts/settings-schema.md).
use crate::Config;
use serde_json::{json, Value};

/// A setting's default. Its kind is the setting's type: the loader coerces saved and environment values to it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DefaultValue {
    /// a checkbox
    Bool(bool),
    /// a whole number
    Int(i64),
    /// a real number
    Float(f64),
    /// text
    Text(&'static str),
}

impl DefaultValue {
    /// The default as JSON.
    pub fn to_json(self) -> Value {
        match self {
            DefaultValue::Bool(b) => json!(b),
            DefaultValue::Int(i) => json!(i),
            DefaultValue::Float(f) => json!(f),
            DefaultValue::Text(s) => json!(s),
        }
    }
}

/// One setting.
#[derive(Debug)]
pub struct KeyDef {
    /// the setting's name
    pub key: &'static str,
    /// the environment variable that sets it (and locks it in the UI)
    pub env: Option<&'static str>,
    /// the value until one is saved
    pub default: DefaultValue,
    /// never sent to a browser
    pub secret: bool,
    /// text, password, number, bool, select, checks, textarea, path or url
    pub kind: &'static str,
    /// the card on the Settings pages that shows it; None: edited elsewhere, or kept by the server itself
    pub group: Option<&'static str>,
    /// the field's label
    pub label: Option<&'static str>,
    /// the explanation under the field
    pub help: Option<&'static str>,
}

/// A field on a card.
#[derive(Debug)]
pub struct Field {
    /// the setting it edits
    pub key: &'static str,
    /// its label
    pub label: &'static str,
    /// the explanation under it
    pub help: Option<&'static str>,
    /// the input type (text, password, number, bool, select, checks, textarea, path, url)
    pub kind: &'static str,
    /// (value, label) for select and checks
    pub options: &'static [(&'static str, &'static str)],
    /// visibility rule, `key=value` or `key!=value` (see [`crate::when`])
    pub when: Option<&'static str>,
    /// shown after the input ("min", "MB", …)
    pub unit: Option<&'static str>,
    /// the lowest number accepted
    pub min: Option<f64>,
    /// the highest number accepted
    pub max: Option<f64>,
    /// the step of a number input
    pub step: Option<f64>,
    /// takes the card's whole width
    pub full: bool,
    /// hidden unless "Show advanced" is on
    pub advanced: bool,
}

/// A card.
#[derive(Debug)]
pub struct Group {
    /// the card's id (also its anchor)
    pub id: &'static str,
    /// its title
    pub title: &'static str,
    /// the heading over a run of cards on the page ("" for none)
    pub part: &'static str,
    /// its description
    pub desc: &'static str,
    /// the card's Test button: POST /api/system/test/<test>
    pub test: Option<&'static str>,
    /// the Test button's text
    pub test_label: Option<&'static str>,
    /// its fields, in order
    pub fields: &'static [Field],
}

/// A page under Settings → Server. A page with no groups is drawn by its own code in the web app.
#[derive(Debug)]
pub struct Section {
    /// the page's id (`?s=`)
    pub id: &'static str,
    /// its title in the nav
    pub title: &'static str,
    /// old `?s=` names that open this page
    pub aliases: &'static [&'static str],
    /// its cards, in order
    pub groups: &'static [Group],
}

include!(concat!(env!("OUT_DIR"), "/settings_gen.rs"));

/// The table's entry for `key`.
pub fn def(key: &str) -> Option<&'static KeyDef> {
    TABLE.iter().find(|d| d.key == key)
}

/// The default of `key`, as the loader uses it.
pub fn default_of(key: &str) -> Option<Value> {
    def(key).map(|d| d.default.to_json())
}

/// A whole number without a fraction (65535, not 65535.0).
fn number(n: f64) -> Value {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        json!(n as i64)
    } else {
        json!(n)
    }
}

impl Config {
    /// GET /api/admin/settings/schema: the pages, cards and fields of Settings → Server, with each field's
    /// words, type, options, visibility rule and whether the environment locks it. Values are not in it
    /// (GET /api/admin/settings has them).
    pub fn schema(&self) -> Value {
        let field = |f: &Field| {
            let d = def(f.key).expect("SCHEMA fields are in TABLE");
            json!({
                "key": f.key,
                "label": f.label,
                "help": f.help,
                "type": f.kind,
                "options": f.options.iter().map(|(value, label)| json!({ "value": value, "label": label })).collect::<Vec<_>>(),
                "when": f.when,
                "secret": d.secret,
                "env": d.env,
                "env_locked": self.env_locked(f.key),
                "default": d.default.to_json(),
                "unit": f.unit,
                "min": f.min.map(number),
                "max": f.max.map(number),
                "step": f.step.map(number),
                "full": f.full,
                "advanced": f.advanced,
            })
        };
        let sections: Vec<Value> = SCHEMA
            .iter()
            .map(|s| {
                let groups: Vec<Value> = s
                    .groups
                    .iter()
                    .map(|g| {
                        json!({
                            "id": g.id,
                            "title": g.title,
                            "part": g.part,
                            "desc": g.desc,
                            "test": g.test,
                            "test_label": g.test_label,
                            "fields": g.fields.iter().map(field).collect::<Vec<_>>(),
                        })
                    })
                    .collect();
                json!({ "id": s.id, "title": s.title, "aliases": s.aliases, "groups": groups })
            })
            .collect();
        json!({ "sections": sections })
    }
}
