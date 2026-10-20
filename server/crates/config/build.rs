//! Turns settings.toml into settings_gen.rs (in OUT_DIR): the `Keys` names, the `TABLE` of definitions the
//! loader reads, `SCHEMA` (pages, cards and fields) and a typed accessor per key on `Config`. Anything in
//! the file that does not fit (an unknown attribute, a type that does not match the default, a card or rule
//! that names something missing) stops the build with the reason.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use toml::{Table, Value};

#[path = "src/when.rs"]
mod when;

const TYPES: &[&str] = &["text", "password", "number", "bool", "select", "checks", "textarea", "path", "url"];
const KEY_ATTRS: &[&str] =
    &["env", "default", "secret", "group", "label", "help", "type", "options", "when", "unit", "min", "max", "step", "full", "advanced"];
/// Names an accessor cannot take: Config's own methods, and Rust keywords.
const TAKEN: &[&str] = &[
    "load", "reload", "def", "defs", "get", "str", "bool", "int", "float", "env_locked", "set_many", "is_masked", "public_dump", "schema",
    "as", "async", "await", "box", "break", "const", "continue", "crate", "do", "dyn", "else", "enum", "extern", "false", "final", "fn",
    "for", "if", "impl", "in", "let", "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref", "return", "self",
    "static", "struct", "super", "trait", "true", "try", "type", "typeof", "unsafe", "unsized", "use", "virtual", "where", "while", "yield",
];

fn main() {
    println!("cargo:rerun-if-changed=settings.toml");
    println!("cargo:rerun-if-changed=src/when.rs");
    let text = std::fs::read_to_string("settings.toml").expect("settings.toml");
    let out = match generate(&text) {
        Ok(code) => code,
        Err(e) => panic!("settings.toml: {e}"),
    };
    let path = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("settings_gen.rs");
    std::fs::write(path, out).unwrap();
}

struct Key<'a> {
    name: &'a str,
    t: &'a Table,
    kind: String,
}

fn s<'a>(t: &'a Table, attr: &str, at: &str) -> Result<Option<&'a str>, String> {
    match t.get(attr) {
        None => Ok(None),
        Some(Value::String(v)) => Ok(Some(v)),
        Some(_) => Err(format!("{at}: {attr} must be a string")),
    }
}

fn flag(t: &Table, attr: &str, at: &str) -> Result<bool, String> {
    match t.get(attr) {
        None => Ok(false),
        Some(Value::Boolean(b)) => Ok(*b),
        Some(_) => Err(format!("{at}: {attr} must be true or false")),
    }
}

fn num(t: &Table, attr: &str, at: &str) -> Result<Option<f64>, String> {
    match t.get(attr) {
        None => Ok(None),
        Some(Value::Integer(i)) => Ok(Some(*i as f64)),
        Some(Value::Float(f)) => Ok(Some(*f)),
        Some(_) => Err(format!("{at}: {attr} must be a number")),
    }
}

fn table<'a>(root: &'a Table, name: &str) -> Result<&'a Table, String> {
    match root.get(name) {
        Some(Value::Table(t)) => Ok(t),
        _ => Err(format!("[{name}.*] tables are missing")),
    }
}

fn entries<'a>(t: &'a Table, what: &str) -> Result<Vec<(&'a str, &'a Table)>, String> {
    t.iter()
        .map(|(k, v)| match v {
            Value::Table(e) => Ok((k.as_str(), e)),
            _ => Err(format!("{what}.{k} must be a table")),
        })
        .collect()
}

fn options(t: &Table, at: &str) -> Result<Vec<(String, String)>, String> {
    let Some(v) = t.get("options") else { return Ok(vec![]) };
    let Value::Array(items) = v else { return Err(format!("{at}: options must be a list of [value, label]")) };
    items
        .iter()
        .map(|o| match o {
            Value::Array(p) if p.len() == 2 => match (&p[0], &p[1]) {
                (Value::String(a), Value::String(b)) => Ok((a.clone(), b.clone())),
                _ => Err(format!("{at}: an option is two strings")),
            },
            _ => Err(format!("{at}: an option is [value, label]")),
        })
        .collect()
}

/// A string as a Rust literal.
fn lit(v: &str) -> String {
    format!("{v:?}")
}

fn opt_lit(v: Option<&str>) -> String {
    v.map(|v| format!("Some({v:?})")).unwrap_or_else(|| "None".into())
}

fn opt_num(v: Option<f64>) -> String {
    v.map(|v| format!("Some({v:?})")).unwrap_or_else(|| "None".into())
}

fn generate(text: &str) -> Result<String, String> {
    let root: Table = text.parse().map_err(|e| format!("{e}"))?;
    for k in root.keys() {
        if !["listed_last", "sections", "groups", "keys"].contains(&k.as_str()) {
            return Err(format!("unknown top-level entry {k}"));
        }
    }
    let sections = entries(table(&root, "sections")?, "sections")?;
    let groups = entries(table(&root, "groups")?, "groups")?;
    let raw_keys = entries(table(&root, "keys")?, "keys")?;

    // sections and groups
    let section_ids: HashSet<&str> = sections.iter().map(|(id, _)| *id).collect();
    for (id, t) in &sections {
        for attr in t.keys() {
            if !["title", "aliases"].contains(&attr.as_str()) {
                return Err(format!("sections.{id}: unknown attribute {attr}"));
            }
        }
        s(t, "title", id)?.ok_or(format!("sections.{id}: title is missing"))?;
    }
    for (id, t) in &groups {
        for attr in t.keys() {
            if !["section", "part", "title", "desc", "test", "test_label"].contains(&attr.as_str()) {
                return Err(format!("groups.{id}: unknown attribute {attr}"));
            }
        }
        let sec = s(t, "section", id)?.ok_or(format!("groups.{id}: section is missing"))?;
        if !section_ids.contains(sec) {
            return Err(format!("groups.{id}: no section {sec}"));
        }
        s(t, "title", id)?.ok_or(format!("groups.{id}: title is missing"))?;
        if s(t, "test", id)?.is_some() != s(t, "test_label", id)?.is_some() {
            return Err(format!("groups.{id}: test and test_label go together"));
        }
    }
    let group_ids: HashSet<&str> = groups.iter().map(|(id, _)| *id).collect();
    // `#/settings?s=<name>` opens the page of that name, alias or card: each name leads to one page only
    let mut opens: HashMap<&str, &str> = HashMap::new();
    let mut names: Vec<(&str, &str)> = sections.iter().map(|(id, _)| (*id, *id)).collect();
    for (id, t) in &sections {
        if let Some(Value::Array(a)) = t.get("aliases") {
            names.extend(a.iter().filter_map(Value::as_str).map(|alias| (alias, *id)));
        }
    }
    names.extend(groups.iter().filter_map(|(id, t)| Some((*id, t.get("section")?.as_str()?))));
    for (name, page) in names {
        match opens.insert(name, page) {
            Some(other) if other != page => return Err(format!("?s={name} would open both {other} and {page}")),
            _ => {}
        }
    }

    // keys
    let mut keys: Vec<Key> = vec![];
    for (name, t) in &raw_keys {
        let at = format!("keys.{name}");
        if !name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_') || name.starts_with(|c: char| c.is_ascii_digit()) {
            return Err(format!("{at}: a key is lower case letters, digits and _"));
        }
        if TAKEN.contains(name) {
            return Err(format!("{at}: the name is taken by a Config method or a Rust keyword"));
        }
        for attr in t.keys() {
            if !KEY_ATTRS.contains(&attr.as_str()) {
                return Err(format!("{at}: unknown attribute {attr}"));
            }
        }
        let default = t.get("default").ok_or(format!("{at}: default is missing"))?;
        let secret = flag(t, "secret", &at)?;
        let derived = match default {
            Value::Boolean(_) => "bool",
            Value::Integer(_) | Value::Float(_) => "number",
            Value::String(_) if secret => "password",
            Value::String(_) => "text",
            _ => return Err(format!("{at}: default must be a bool, number or string")),
        };
        let kind = s(t, "type", &at)?.unwrap_or(derived).to_string();
        if !TYPES.contains(&kind.as_str()) {
            return Err(format!("{at}: unknown type {kind}"));
        }
        let fits = match kind.as_str() {
            "bool" => matches!(default, Value::Boolean(_)),
            "number" => matches!(default, Value::Integer(_) | Value::Float(_)),
            "select" => matches!(default, Value::String(_) | Value::Integer(_)),
            _ => matches!(default, Value::String(_)),
        };
        if !fits {
            return Err(format!("{at}: a {kind} field cannot have the default {default:?}"));
        }
        if kind == "password" && !secret {
            return Err(format!("{at}: a password field is secret"));
        }
        let opts = options(t, &at)?;
        if matches!(kind.as_str(), "select" | "checks") == opts.is_empty() {
            return Err(format!("{at}: options are for select and checks fields, and those need them"));
        }
        if let Some(env) = s(t, "env", &at)? {
            if env.is_empty() || !env.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_') {
                return Err(format!("{at}: env must be an environment variable name"));
            }
        }
        match s(t, "group", &at)? {
            Some(g) if !group_ids.contains(g) => return Err(format!("{at}: no group {g}")),
            Some(_) => {
                s(t, "label", &at)?.ok_or(format!("{at}: a field on a card needs a label"))?;
                if secret && !matches!(kind.as_str(), "password" | "textarea") {
                    return Err(format!("{at}: a secret on a card is a password or textarea field"));
                }
            }
            None => {
                for attr in ["when", "options", "unit", "min", "max", "step", "full", "advanced"] {
                    if t.contains_key(attr) && !(attr == "options" && kind == "select") {
                        return Err(format!("{at}: {attr} is for fields on a card (set group)"));
                    }
                }
            }
        }
        for attr in ["unit", "min", "max", "step"] {
            if t.contains_key(attr) && kind != "number" {
                return Err(format!("{at}: {attr} is for number fields"));
            }
        }
        for attr in ["label", "help", "unit"] {
            s(t, attr, &at)?;
        }
        for attr in ["min", "max", "step"] {
            num(t, attr, &at)?;
        }
        flag(t, "full", &at)?;
        flag(t, "advanced", &at)?;
        keys.push(Key { name, t, kind });
    }
    let by_name: BTreeMap<&str, &Key> = keys.iter().map(|k| (k.name, k)).collect();
    for k in &keys {
        let at = format!("keys.{}", k.name);
        if let Some(rule) = s(k.t, "when", &at)? {
            let w = when::parse(rule).map_err(|e| format!("{at}: when {e}"))?;
            let Some(other) = by_name.get(w.key) else { return Err(format!("{at}: when names no setting {}", w.key)) };
            if other.kind != "select" {
                return Err(format!("{at}: when names {}, which is not a select", w.key));
            }
            if !options(other.t, &at)?.iter().any(|(v, _)| v == w.value) {
                return Err(format!("{at}: when {rule}: {} has no option {}", w.key, w.value, ));
            }
            if w.equal && w.key == k.name {
                return Err(format!("{at}: when names the field itself"));
            }
        }
    }

    // TABLE order: A to Z, then listed_last in its order
    let last: Vec<&str> = match root.get("listed_last") {
        None => vec![],
        Some(Value::Array(a)) => a.iter().map(|v| v.as_str().ok_or("listed_last holds key names".to_string())).collect::<Result<_, _>>()?,
        Some(_) => return Err("listed_last must be a list of key names".into()),
    };
    let mut seen = HashSet::new();
    for k in &last {
        if !by_name.contains_key(k) {
            return Err(format!("listed_last: no key {k}"));
        }
        if !seen.insert(*k) {
            return Err(format!("listed_last: {k} twice"));
        }
    }
    let mut order: Vec<&str> = by_name.keys().copied().filter(|k| !seen.contains(k)).collect();
    order.extend(last.iter().copied());

    let mut o = String::new();
    o.push_str("// Generated by build.rs from settings.toml; edit that file instead.\n\n");

    // Keys
    o.push_str("/// Every setting's name: `Keys::APP_NAME` is `\"app_name\"`.\npub struct Keys;\n\nimpl Keys {\n");
    for name in &order {
        writeln!(o, "    pub const {}: &'static str = {};", name.to_uppercase(), lit(name)).unwrap();
    }
    o.push_str("}\n\n");

    // TABLE
    o.push_str("/// Every setting: A to Z, then the keys settings.toml lists last. `Config::defs` keeps this order.\n");
    o.push_str("pub static TABLE: &[KeyDef] = &[\n");
    for name in &order {
        let k = by_name[name];
        let at = format!("keys.{name}");
        let default = match &k.t["default"] {
            Value::Boolean(b) => format!("DefaultValue::Bool({b})"),
            Value::Integer(i) => format!("DefaultValue::Int({i})"),
            Value::Float(f) => format!("DefaultValue::Float({f:?})"),
            Value::String(v) => format!("DefaultValue::Text({})", lit(v)),
            _ => unreachable!(),
        };
        writeln!(
            o,
            "    KeyDef {{ key: {}, env: {}, default: {default}, secret: {}, kind: {}, group: {}, label: {}, help: {} }},",
            lit(name),
            opt_lit(s(k.t, "env", &at)?),
            flag(k.t, "secret", &at)?,
            lit(&k.kind),
            opt_lit(s(k.t, "group", &at)?),
            opt_lit(s(k.t, "label", &at)?),
            opt_lit(s(k.t, "help", &at)?),
        )
        .unwrap();
    }
    o.push_str("];\n\n");

    // SCHEMA
    o.push_str("/// The Settings pages in nav order, their cards in page order, and each card's fields.\n");
    o.push_str("pub static SCHEMA: &[Section] = &[\n");
    for (sid, st) in &sections {
        let aliases: Vec<String> = match st.get("aliases") {
            None => vec![],
            Some(Value::Array(a)) => {
                a.iter().map(|v| v.as_str().map(lit).ok_or(format!("sections.{sid}: aliases are strings"))).collect::<Result<_, _>>()?
            }
            Some(_) => return Err(format!("sections.{sid}: aliases must be a list")),
        };
        writeln!(o, "    Section {{ id: {}, title: {}, aliases: &[{}], groups: &[", lit(sid), lit(s(st, "title", sid)?.unwrap()), aliases.join(", ")).unwrap();
        for (gid, gt) in groups.iter().filter(|(_, gt)| gt.get("section").and_then(Value::as_str) == Some(sid)) {
            writeln!(
                o,
                "        Group {{ id: {}, title: {}, part: {}, desc: {}, test: {}, test_label: {}, fields: &[",
                lit(gid),
                lit(s(gt, "title", gid)?.unwrap()),
                lit(s(gt, "part", gid)?.unwrap_or("")),
                lit(s(gt, "desc", gid)?.unwrap_or("")),
                opt_lit(s(gt, "test", gid)?),
                opt_lit(s(gt, "test_label", gid)?),
            )
            .unwrap();
            for k in keys.iter().filter(|k| k.t.get("group").and_then(Value::as_str) == Some(gid)) {
                let at = format!("keys.{}", k.name);
                let opts: Vec<String> = options(k.t, &at)?.iter().map(|(v, l)| format!("({}, {})", lit(v), lit(l))).collect();
                writeln!(
                    o,
                    "            Field {{ key: {}, label: {}, help: {}, kind: {}, options: &[{}], when: {}, unit: {}, min: {}, max: {}, step: {}, full: {}, advanced: {} }},",
                    lit(k.name),
                    lit(s(k.t, "label", &at)?.unwrap()),
                    opt_lit(s(k.t, "help", &at)?),
                    lit(&k.kind),
                    opts.join(", "),
                    opt_lit(s(k.t, "when", &at)?),
                    opt_lit(s(k.t, "unit", &at)?),
                    opt_num(num(k.t, "min", &at)?),
                    opt_num(num(k.t, "max", &at)?),
                    opt_num(num(k.t, "step", &at)?),
                    flag(k.t, "full", &at)?,
                    flag(k.t, "advanced", &at)?,
                )
                .unwrap();
            }
            o.push_str("        ] },\n");
        }
        o.push_str("    ] },\n");
    }
    o.push_str("];\n\n");

    // accessors
    o.push_str("/// A typed accessor per setting (the value in effect: environment, then saved, then default).\nimpl crate::Config {\n");
    for name in &order {
        let k = by_name[name];
        let (ret, call) = match &k.t["default"] {
            Value::Boolean(_) => ("bool", "bool"),
            Value::Integer(_) => ("i64", "int"),
            Value::Float(_) => ("f64", "float"),
            _ => ("String", "str"),
        };
        let what = s(k.t, "label", name)?.or(s(k.t, "help", name)?).unwrap_or("");
        let doc = if what.is_empty() { format!("`{name}`") } else { format!("`{name}`: {what}") };
        writeln!(o, "    #[doc = {}]", lit(&doc)).unwrap();
        writeln!(o, "    pub fn {name}(&self) -> {ret} {{\n        self.{call}(Keys::{})\n    }}", name.to_uppercase()).unwrap();
    }
    o.push_str("}\n");
    Ok(o)
}
