//! The acceptance list of the port: docs/ui/strings-reference.txt holds every user-visible string of the
//! JavaScript UI (web/js, frozen since 0.26). Every string web/js still ships has to appear in web-rs, so the
//! two interfaces read the same – the rule of docs/archive/ui-rewrite/PORTING.md. The Settings → Server cards'
//! words are counted where they live since 0.29, the server's settings table (server/crates/config/settings.toml),
//! which the web app draws them from, and since 0.32 the plugins' manifests (server/crates/plugins/catalog).
//! Run with `cargo test` in web-rs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Strings only the JavaScript UI has, on purpose – listed here as (part of the reference text, why) so the
/// test skips them knowingly. The About card's interface switch (web/js/pages/settings.js, gone from web-rs
/// with the switch of 0.26) was never extracted into the reference, so it needs no entry. An entry that
/// matches no reference line fails the test, so a stale one is noticed.
const LEGACY_ONLY: &[(&str, &str)] = &[
    ("Tick at least one kind of release", "the Request discography dialog of 0.26 ticks releases, not kinds (\"Tick at least one release\")"),
    // 0.29: the web app runs only in browsers; what the apps hung on window.EncoreNative went with the bridge
    ("Offline music", "the apps' offline screen (EncoreNative.openOffline), gone with the bridge in 0.29"),
    ("Change server…", "the apps' server picker (EncoreNative.changeServer), gone with the bridge in 0.29"),
    ("This computer", "Settings → This computer of the classic desktop app (EncoreNative.desktopSettings), gone in 0.29"),
    ("Save for offline", "songs kept on the phone by the Tauri Android app (EncoreNative.saveOffline), gone in 0.29; browsers use Download for offline"),
    ("Saving to this device…", "the Tauri Android app's Save for offline, gone in 0.29"),
    ("Saving song(s) to this device…", "the Tauri Android app's Save for offline, gone in 0.29"),
    // R1 (the owner, Oct 19): streamrip and links are staged in that folder too
    ("Staging path for yt-dlp downloads", "renamed \"Staging folder for downloads\" in the server's settings table (0.29, R1): streamrip and links use the folder too"),
    // R2 (the owner, Oct 19): Apps & sources became four Server pages, so the page and its part headings are gone
    ("Apps & sources", "the page became Library, Downloads, Integrations and Server processing in R2 (`apps` links open Downloads)"),
    ("Download sources", "a part heading of Apps & sources; its cards are the Downloads page since R2"),
    ("Services", "a part heading of Apps & sources; its cards are the Integrations and Server processing pages since R2"),
    ("Add a free AcoustID API key in Settings → Verification first", "there is no Verification page: the key is in Settings → Library → Verification & duplicates since R2, which the toast now names"),
    // 0.31 (p6 item 9, the owner on Oct 18): the Library's box searches the whole library, whatever the tab
    ("Filter library…", "the Library's search box reads \"Search artists, albums, songs in your library…\" since 0.31 (one search for the whole library)"),
    // 0.32 (plugins): every download tool is a plugin with its own card, Update and auto-update switch
    ("Download tools", "the card became Settings → Plugins in 0.32 (its Check for updates, Update to and the Translator's model moved there)"),
];

/// Whole files of the JavaScript UI whose page web-rs no longer has, as (file, why). Like [`LEGACY_ONLY`], an
/// entry that matches no reference line fails the test.
const LEGACY_FILES: &[(&str, &str)] = &[
    ("web/js/pages/computer.js", "Settings → This computer, a page of the classic desktop app only, gone with the EncoreNative bridge in 0.29"),
];


/// A line of the reference: `<file>:<line>  [<category>]  <text>` (the line number is missing for a few entries).
struct Entry {
    file: String,
    category: String,
    text: String,
}

fn parse(line: &str) -> Option<Entry> {
    let (head, rest) = line.split_once("  [")?;
    let (category, text) = rest.split_once("]  ")?;
    // `web/js/app.js:103`, or `web/js/app.js:None` for the entries the extraction found without a line
    let file = head.rsplit_once(':').filter(|(_, n)| *n == "None" || n.chars().all(|c| c.is_ascii_digit())).map(|(f, _)| f).unwrap_or(head);
    // the extraction appended what the string belongs to: `   (key: open)`, `   (page id: tasks)`, `   (route #/library)`
    let text = match text.rfind("   (") {
        Some(i) if text.ends_with(')') && ["(key: ", "(page id: ", "(route "].iter().any(|p| text[i + 3..].starts_with(p)) => &text[..i],
        _ => text,
    };
    Some(Entry { file: file.to_string(), category: category.to_string(), text: text.trim().to_string() })
}

/// Template residue: the reference was taken from JavaScript template literals, and a few entries kept a
/// broken piece of an expression (`cleared${files ?`) – not a string anyone sees.
fn residue(text: &str) -> bool {
    text.contains("${") || text.contains('`') || text.contains('}') || text.ends_with(" ?") || text.ends_with(" :")
}

/// One space between words, so a string split over lines or around a placeholder still matches.
fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Rust sources with their format placeholders taken out: `Removed {} song(s)` and `Removed {n} song(s)` read
/// `Removed song(s)`, the way the reference has them (its extraction dropped the `${n}`). Only short, simple
/// placeholders go (`{}`, `{n}`, `{x:.1}`, `{:?}`); code blocks keep their braces. `{{`/`}}` become `{`/`}`.
fn without_placeholders(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let b = src.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'{' {
            if b.get(i + 1) == Some(&b'{') { out.push('{'); i += 2; continue; }
            if let Some(end) = src[i + 1..].find('}') {
                let inner = &src[i + 1..i + 1 + end];
                if inner.len() <= 24 && inner.bytes().all(|c| c.is_ascii_alphanumeric() || b"_:.#?<>^".contains(&c)) {
                    i += end + 2;
                    continue;
                }
            }
        } else if b[i] == b'}' && b.get(i + 1) == Some(&b'}') {
            out.push('}');
            i += 2;
            continue;
        }
        let ch = src[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    collapse(&out)
}

/// JavaScript sources the same way: the plain `${expr}` pieces taken out (`Removed ${n} song(s)` reads
/// `Removed song(s)`), so the reference text matches its own source again. An expression that holds a nested
/// template (`${n ? `Request ${n} missing` : ''}`) stays, with its inner pieces taken out, so the text inside
/// it is still found.
fn without_js_expressions(src: &str) -> String {
    let mut s = src.to_string();
    loop {
        let mut out = String::with_capacity(s.len());
        let mut rest = s.as_str();
        let mut changed = false;
        while let Some(i) = rest.find("${") {
            out.push_str(&rest[..i]);
            let body = &rest[i + 2..];
            match body.find(['}', '{', '`']) {
                Some(j) if body.as_bytes()[j] == b'}' => { rest = &body[j + 1..]; changed = true; }
                _ => { out.push_str("${"); rest = body; }
            }
        }
        out.push_str(rest);
        s = out;
        if !changed { break; }
    }
    collapse(&s)
}

/// The entities HTML attributes use for quotes and line breaks, as the extraction decoded them.
fn decode_entities(s: &str) -> String {
    s.replace("&quot;", "\"").replace("&#39;", "'").replace("&#10;", " ").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

/// The shapes a reference string may take in a source: as written, with `\n` written out, with HTML entities
/// decoded, and with its quotes escaped.
fn variants(text: &str) -> Vec<String> {
    let t = collapse(text);
    let mut v = vec![text.to_string(), t.clone(), t.replace("\\n", " "), t.replace('\'', "\\'"), t.replace('"', "\\\"")];
    v.push(collapse(&decode_entities(&t)));
    v.sort();
    v.dedup();
    v
}

fn read_all(dir: &Path, ext: &str) -> String {
    let mut out = String::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    let mut paths: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        if p.is_dir() {
            out.push_str(&read_all(&p, ext));
        } else if p.extension().and_then(|e| e.to_str()) == Some(ext) {
            out.push_str(&std::fs::read_to_string(&p).unwrap_or_default());
            out.push('\n');
        }
    }
    out
}

#[test]
fn every_string_the_javascript_ui_ships_is_in_web_rs() {
    let repo = repo();
    let reference = std::fs::read_to_string(repo.join("docs/ui/strings-reference.txt")).expect("docs/ui/strings-reference.txt");
    // since 0.29 the words of the Settings → Server cards (titles, labels, help, options) are in the server's
    // settings table: the web app draws those cards from GET /api/admin/settings/schema, which answers with them
    let table = std::fs::read_to_string(repo.join("server/crates/config/settings.toml")).expect("server/crates/config/settings.toml");
    // since 0.32 the words of the plugin cards that the settings table has no field for (the Translator's model,
    // once on the Download tools card) are in the plugins' manifests, which GET /api/plugins/{id}/settings
    // answers with
    let catalog = read_all(&repo.join("server/crates/plugins/catalog"), "toml");
    assert!(catalog.contains("[plugin]"), "server/crates/plugins/catalog/*.toml read");
    let rs_raw = read_all(&repo.join("web-rs/src"), "rs") + &table + &catalog;
    assert!(rs_raw.len() > 100_000, "web-rs sources read");
    let rs_norm = without_placeholders(&rs_raw);
    let rs_decoded = collapse(&decode_entities(&rs_norm));
    // web/js, while it is still in the tree: a string it no longer ships is not required. Once web/js is gone
    // (0.27), the reference stands on its own and every string counts.
    let js_dir = repo.join("web/js");
    let js_present = js_dir.is_dir();
    // per file: the source as written, with its `${…}` expressions taken out, and with HTML entities decoded
    let mut js_files: BTreeMap<String, (String, String, String)> = BTreeMap::new();

    let mut checked = 0;
    let mut skipped_residue = Vec::new();
    let mut skipped_gone = Vec::new();
    let mut skipped_legacy = Vec::new();
    let mut legacy_used = vec![false; LEGACY_ONLY.len()];
    let mut legacy_files_used = vec![false; LEGACY_FILES.len()];
    let mut missing = Vec::new();
    for line in reference.lines() {
        if line.is_empty() || line.starts_with('#') { continue; }
        let Some(e) = parse(line) else { panic!("unreadable reference line: {line}") };
        if residue(&e.text) { skipped_residue.push(e.text.clone()); continue; }
        if let Some((i, (_, why))) = LEGACY_FILES.iter().enumerate().find(|(_, (f, _))| e.file == *f) {
            legacy_files_used[i] = true;
            skipped_legacy.push(format!("{} – {why}", e.text));
            continue;
        }
        if let Some((i, (_, why))) = LEGACY_ONLY.iter().enumerate().find(|(_, (s, _))| e.text.contains(s)) {
            legacy_used[i] = true;
            skipped_legacy.push(format!("{} – {why}", e.text));
            continue;
        }
        if js_present {
            let (raw, norm, decoded) = js_files.entry(e.file.clone()).or_insert_with(|| {
                let raw = std::fs::read_to_string(repo.join(&e.file)).unwrap_or_default();
                let norm = without_js_expressions(&raw);
                let decoded = collapse(&decode_entities(&norm));
                (raw, norm, decoded)
            });
            let ships = raw.is_empty() || variants(&e.text).iter().any(|v| raw.contains(v) || norm.contains(v) || decoded.contains(v));
            if !ships {
                skipped_gone.push(format!("{}  {}", e.file, e.text));
                continue;
            }
        }
        checked += 1;
        let found = variants(&e.text).iter().any(|v| rs_raw.contains(v) || rs_norm.contains(v) || rs_decoded.contains(v));
        if !found {
            missing.push(format!("{}  [{}]  {}", e.file, e.category, e.text));
        }
    }
    eprintln!("strings reference: {checked} checked, {} skipped as template residue, {} no longer in web/js, {} legacy-only",
        skipped_residue.len(), skipped_gone.len(), skipped_legacy.len());
    for s in &skipped_residue { eprintln!("  residue: {s}"); }
    for s in &skipped_gone { eprintln!("  gone from web/js: {s}"); }
    for s in &skipped_legacy { eprintln!("  legacy-only: {s}"); }
    let stale: Vec<&str> = LEGACY_ONLY.iter().zip(&legacy_used).filter(|(_, used)| !**used).map(|((s, _), _)| *s).collect();
    assert!(stale.is_empty(), "LEGACY_ONLY entries that match no line of the reference: {stale:?}");
    let stale_files: Vec<&str> = LEGACY_FILES.iter().zip(&legacy_files_used).filter(|(_, used)| !**used).map(|((f, _), _)| *f).collect();
    assert!(stale_files.is_empty(), "LEGACY_FILES entries that match no line of the reference: {stale_files:?}");
    assert!(checked > 850, "the reference lists about 935 strings that web/js still ships; only {checked} were checked");
    assert!(skipped_residue.len() < 20, "too many lines read as template residue: {skipped_residue:?}");
    assert!(missing.is_empty(), "{} string(s) of the JavaScript UI are not in web-rs:\n  {}", missing.len(), missing.join("\n  "));
}

#[test]
fn the_reference_parser_reads_the_three_line_shapes() {
    let e = parse("web/js/pages/account.js:63  [toast / message]  Password updated – other sessions signed out").unwrap();
    assert_eq!((e.file.as_str(), e.category.as_str(), e.text.as_str()), ("web/js/pages/account.js", "toast / message", "Password updated – other sessions signed out"));
    let e = parse("web/js/pages/admin.js:137  [tab / segment / option pair]  Invite only   (key: invite)").unwrap();
    assert_eq!(e.text, "Invite only");
    let e = parse("web/js/pages/settings.js  [settings nav entry, Server group]  Scheduled tasks   (page id: tasks)").unwrap();
    assert_eq!((e.file.as_str(), e.text.as_str()), ("web/js/pages/settings.js", "Scheduled tasks"));
    let e = parse("web/js/app.js:None  [sidebar nav]  For you   (route #/for-you)").unwrap();
    assert_eq!((e.file.as_str(), e.text.as_str()), ("web/js/app.js", "For you"));
    assert!(parse("# a comment").is_none());
}

#[test]
fn placeholders_are_taken_out_the_way_the_reference_dropped_them() {
    assert_eq!(without_placeholders(r#"format!("Removed {} song(s)", n)"#), r#"format!("Removed song(s)", n)"#);
    assert_eq!(without_placeholders(r#"format!("Plex connected ({name})")"#), r#"format!("Plex connected ()")"#);
    assert_eq!(without_placeholders(r#"format!("Imported {}: {} tracks", a, b)"#), r#"format!("Imported : tracks", a, b)"#);
    assert_eq!(without_placeholders("{:.1} kHz"), "kHz");
    // code blocks and escaped braces stay
    assert_eq!(without_placeholders("if x { y(); } else {{ z }}"), "if x { y(); } else { z }");
    assert_eq!(without_js_expressions("Removed ${n} song(s) from ${esc(p.name || 'it')}"), "Removed song(s) from");
    // a nested template keeps its text: only the plain pieces go
    assert_eq!(without_js_expressions("${missing.length ? `<button>Request ${missing.length} missing</button>` : ''}"), "${missing.length ? `<button>Request missing</button>` : ''}");
    assert_eq!(decode_entities("[tidal]&#10;user_id = &quot;…&quot;"), "[tidal] user_id = \"…\"");
    assert!(variants("accept: */*\\ncookie: …").contains(&"accept: */* cookie: …".to_string()));
    assert!(residue("cleared${files ?"));
    assert!(!residue("Removed song(s)"));
}
