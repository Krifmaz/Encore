//! The manifest on its own: the SDK's format reads unchanged, unknown keys and unsafe asks are refused, and the
//! problems name the table and key.
use crate::manifest::*;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// The `plugin.toml` of the owner's Internet Archive plugin 0.1.0, as its zip carries it. The format started
/// from it, so it has to keep reading unchanged. (The plugin is uploaded, never built in or in the catalog.)
const SDK_PLUGIN: &str = r#"# Encore plugin manifest. docs/CONTRACT.md describes every key.

[plugin]
id = "internet-archive"
name = "Internet Archive"
version = "0.1.0"
description = "Live Music Archive, netlabels, Creative Commons and public-domain recordings from archive.org. Downloads the files as uploaded (FLAC originals when there are any). No account needed."
kind = "source"
protocol = 1
homepage = "https://archive.org/details/audio"
license = "MIT"
commands = ["describe", "status", "test", "search", "download"]
kinds = ["track", "album"]
lossless = true
needs_sign_in = false

[run]
script = "plugin.py"
interpreter = "python3"

[run.timeouts]
search = 60
download = 3600
test = 30

# Where new versions come from: the GitHub release tagged internet-archive-v<version>, made by
# .github/workflows/plugin-release.yml. Encore's Update button installs it; with auto = true the
# scheduled "Update download tools" check installs it by itself.
[update]
github = "Krifmaz/Encore"
tag = "internet-archive-v{version}"
asset = "internet-archive-{version}.zip"
sums = "SHA256SUMS"
auto = true

[[settings]]
key = "format"
label = "Preferred format"
type = "select"
options = [["flac", "FLAC (lossless originals)"], ["mp3", "MP3"], ["any", "Best the item has"]]
default = "flac"
help = "Most items carry the uploaded original and an MP3 made from it. Items without the preferred format still show, marked with what they have."

[[settings]]
key = "collections"
label = "Collections"
type = "text"
default = ""
help = "Only these archive.org collections, comma-separated (for example: etree, netlabels, opensource_audio). Empty searches all audio."

[[settings]]
key = "max_items"
label = "Items to inspect per search"
type = "number"
default = 8
min = 1
max = 25
help = "Each archive.org item (a show, an album) is fetched to list its files. More items find more, slower."

[permissions]
network = ["archive.org", "*.archive.org"]
filesystem = "folder"
"#;

/// A small process plugin to vary in the tests.
const SMALL: &str = r#"
[plugin]
id = "small"
name = "Small"
version = "1.0.0"
kind = "source"
protocol = 1
commands = ["describe", "status", "search", "download"]
kinds = ["track"]

[run]
script = "plugin.py"
interpreter = "python3"

[[settings]]
key = "mode"
label = "Mode"
type = "select"
options = [["a", "A"], ["b", "B"]]
default = "a"

[[settings]]
key = "depth"
label = "Depth"
type = "number"
default = 3
min = 1
max = 9
when = "mode=b"
"#;

/// `SMALL` with `old` replaced by `new` (which must be there).
fn small_with(old: &str, new: &str) -> String {
    assert!(SMALL.contains(old), "{old:?} is not in SMALL");
    SMALL.replacen(old, new, 1)
}

/// The problems of a manifest that reads as TOML (parse errors fail the test).
fn problems_of(text: &str) -> Vec<String> {
    match parse(text) {
        Ok(_) => vec![],
        Err(e) => e.problems,
    }
}

fn has(problems: &[String], part: &str) -> bool {
    problems.iter().any(|p| p.contains(part))
}

#[test]
fn the_sdk_plugins_manifest_reads_unchanged() {
    let m = parse_for(SDK_PLUGIN, Origin::Upload).unwrap();
    assert_eq!((m.id(), m.plugin.name.as_str(), m.plugin.version.as_str()), ("internet-archive", "Internet Archive", "0.1.0"));
    assert_eq!((m.plugin.kind.as_str(), m.plugin.protocol), ("source", 1));
    assert_eq!(m.plugin.commands, ["describe", "status", "test", "search", "download"]);
    assert_eq!(m.plugin.kinds, ["track", "album"]);
    assert!(m.plugin.lossless && !m.plugin.needs_sign_in && !m.plugin.builtin && m.is_process());
    let run = m.run.as_ref().unwrap();
    assert_eq!((run.script.as_deref(), run.interpreter.as_deref(), run.shell), (Some("plugin.py"), Some("python3"), false));
    assert_eq!((m.timeout("search"), m.timeout("download"), m.timeout("test")), (60, 3600, 30));
    assert_eq!((m.timeout("status"), m.timeout("describe")), (15, 10), "the defaults where [run.timeouts] says nothing");
    let u = m.update.as_ref().unwrap();
    assert_eq!(u.source(), "github");
    assert_eq!(u.tag_for("0.2.0"), "internet-archive-v0.2.0");
    assert_eq!(u.version_in_tag("internet-archive-v0.2.0").as_deref(), Some("0.2.0"));
    assert_eq!(u.version_in_tag("v0.31.0"), None, "Encore's own releases in the same repository are not the plugin's");
    assert_eq!(fill(u.asset.as_deref().unwrap(), "0.2.0", "internet-archive-v0.2.0"), "internet-archive-0.2.0.zip");
    assert!(u.auto);
    assert_eq!(m.settings.len(), 3);
    let format = m.setting("format").unwrap();
    assert_eq!(format.kind.as_deref(), Some("select"));
    assert_eq!(format.options[0], ("flac".to_string(), "FLAC (lossless originals)".to_string()));
    assert_eq!(format.default, Some(json!("flac")));
    let items = m.setting("max_items").unwrap();
    assert_eq!((items.default.clone(), items.min, items.max), (Some(json!(8)), Some(1.0), Some(25.0)));
    assert_eq!(m.store_key(format), "plugin.internet-archive.format", "an uploaded plugin's keys are its own");
    assert!(m.permissions.network.allowed());
    assert_eq!(m.permissions.network.hosts(), ["archive.org", "*.archive.org"]);
    assert_eq!(m.permissions.filesystem.as_deref(), Some("folder"));
    let ids: Vec<String> = m.source_ids().into_iter().map(|s| s.id).collect();
    assert_eq!(ids, ["internet-archive"], "a source with no [[sources]] answers for its own id");
    assert!(m.fits("0.32.0").is_ok());
    assert!(!m.installs_program());
    // the JSON the API serves keeps the SDK's names
    let v = serde_json::to_value(&m).unwrap();
    assert_eq!(v["settings"][0]["type"], "select");
    assert_eq!(v["settings"][0]["options"][1], json!(["mp3", "MP3"]));
    assert_eq!(v["permissions"]["network"], json!(["archive.org", "*.archive.org"]));
}

#[test]
fn a_small_plugin_reads_and_its_rules_hold() {
    let m = parse_for(SMALL, Origin::Url).unwrap();
    assert_eq!(m.setting("depth").unwrap().when.as_deref(), Some("mode=b"));
    assert!(!m.permissions.network.allowed(), "no network unless asked for");
    assert_eq!(Origin::Url.trust(), "third-party");
    assert_eq!(Origin::Catalog.trust(), "first-party");
    assert_eq!(Origin::parse("upload"), Some(Origin::Upload));
}

#[test]
fn unknown_keys_are_refused() {
    for (old, new, key) in [
        ("[plugin]\n", "[plugin]\ncolour = \"red\"\n", "colour"),
        ("[run]\n", "[run]\nshel = false\n", "shel"),
        ("key = \"mode\"\n", "key = \"mode\"\nplaceholder = \"x\"\n", "placeholder"),
        ("[[settings]]\nkey = \"mode\"", "[extra]\nx = 1\n\n[[settings]]\nkey = \"mode\"", "extra"),
        ("[run]\n", "[run.limits]\ncpu = 1\n\n[run]\n", "limits"),
    ] {
        let err = parse(&small_with(old, new)).unwrap_err().to_string();
        assert!(err.contains("unknown field") && err.contains(key), "{key}: {err}");
        assert!(err.contains("line "), "the error names the line: {err}");
    }
}

#[test]
fn nothing_runs_through_a_shell() {
    let p = problems_of(&small_with("[run]\n", "[run]\nshell = true\n"));
    assert!(has(&p, "[run] shell"), "{p:?}");
    assert!(problems_of(&small_with("[run]\n", "[run]\nshell = false\n")).is_empty());
}

#[test]
fn ids_versions_kinds_and_commands() {
    for (old, new, want) in [
        ("id = \"small\"", "id = \"Small Plugin\"", "[plugin] id"),
        ("id = \"small\"", "id = \"-small\"", "[plugin] id"),
        ("id = \"small\"", "id = \"../etc\"", "[plugin] id"),
        ("version = \"1.0.0\"", "version = \"../1\"", "[plugin] version"),
        ("version = \"1.0.0\"", "version = \"latest\"", "[plugin] version"),
        ("kind = \"source\"", "kind = \"widget\"", "[plugin] kind"),
        ("protocol = 1", "protocol = 2", "protocol 2"),
        ("commands = [\"describe\", \"status\", \"search\", \"download\"]", "commands = [\"status\", \"search\", \"download\"]", "answers describe"),
        ("commands = [\"describe\", \"status\", \"search\", \"download\"]", "commands = [\"describe\", \"search\"]", "a source answers download"),
        ("commands = [\"describe\", \"status\", \"search\", \"download\"]", "commands = [\"describe\", \"search\", \"download\", \"rip\"]", "\"rip\" is not a command"),
        ("kinds = [\"track\"]", "kinds = []", "a source lists the requests"),
        ("kinds = [\"track\"]", "kinds = [\"playlist\"]", "\"playlist\" is not one of"),
        ("script = \"plugin.py\"", "script = \"../plugin.py\"", "[run] script"),
        ("script = \"plugin.py\"", "script = \"/usr/bin/python3\"", "[run] script"),
        ("interpreter = \"python3\"", "interpreter = \"bash -c\"", "[run] interpreter"),
    ] {
        let p = problems_of(&small_with(old, new));
        assert!(has(&p, want), "{new}: {p:?}");
    }
    let p = problems_of(&small_with("kind = \"source\"", "kind = \"tool\""));
    assert!(has(&p, "only a source takes requests"), "{p:?}");
    let p = problems_of(&small_with("[run]\nscript = \"plugin.py\"\ninterpreter = \"python3\"\n", ""));
    assert!(has(&p, "[run] script"), "a program needs its script: {p:?}");
}

#[test]
fn settings_values_must_fit_their_type() {
    for (old, new, want) in [
        ("default = \"a\"", "default = \"c\"", "not one of the options"),
        ("default = 3", "default = 12", "outside min and max"),
        ("default = 3", "default = \"3\"", "default is a number"),
        ("min = 1\nmax = 9", "min = 9\nmax = 1", "min is above max"),
        ("options = [[\"a\", \"A\"], [\"b\", \"B\"]]", "options = []", "lists its options"),
        ("options = [[\"a\", \"A\"], [\"b\", \"B\"]]", "options = [[\"a\", \"A\"], [\"a\", \"B\"]]", "listed twice"),
        ("type = \"number\"", "type = \"slider\"", "type \"slider\""),
        ("when = \"mode=b\"", "when = \"mode\"", "expected key=value"),
        ("when = \"mode=b\"", "when = \"colour=b\"", "not a select or switch of this plugin"),
        ("key = \"depth\"", "key = \"mode\"", "listed twice"),
        ("key = \"depth\"", "key = \"Depth\"", "[[settings]] key"),
        ("label = \"Depth\"\n", "", "label is the field's name"),
    ] {
        let p = problems_of(&small_with(old, new));
        assert!(has(&p, want), "{new}: {p:?}");
    }
    let p = problems_of(&small_with("label = \"Mode\"\ntype = \"select\"", "label = \"Mode\"\ntype = \"text\""));
    assert!(has(&p, "only a select or checks has options") && has(&p, "not a select or switch"), "{p:?}");
    let p = problems_of(&small_with("default = 3", "default = 3\nunit = \"seconds\"\nfull = true\nadvanced = true\npart = \"More\""));
    assert!(p.is_empty(), "{p:?}");
}

#[test]
fn uploads_may_not_ask_for_what_only_the_catalog_has() {
    let catalog_only = r#"
[plugin]
id = "tool-like"
name = "Tool like"
version = "1.0.0"
kind = "tool"
protocol = 1
commands = ["describe", "status"]
builtin = true
settings_on = "downloads"

[update]
github = "someone/tool"
sums = "github"

[python]
python = "3.14"
packages = ["tool=={version}"]
bin = "tool"

[[download]]
url = "https://example.org/model.bin"
sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
path = "models/model.bin"

[[settings]]
key = "library_path"

[permissions]
paths = [{ setting = "slskd_download_path", access = "write" }]

[migration]
from_tool = "ytdlp"
"#;
    let m = parse(catalog_only).unwrap();
    assert!(m.check_origin(Origin::Catalog).is_empty());
    let p = m.check_origin(Origin::Upload);
    for want in ["[plugin] builtin", "[plugin] settings_on", "[migration]", "[python]", "[[download]]", "may only read the library"] {
        assert!(has(&p, want), "{want}: {p:?}");
    }
    let p = parse(&small_with("[run]\n", "[run]\nenv = { LD_PRELOAD = \"/tmp/x.so\", ENCORE_PLUGIN_DATA = \"/\", COLUMNS = \"80\" }\n")).unwrap().check_origin(Origin::Upload);
    assert!(has(&p, "LD_PRELOAD") && has(&p, "ENCORE_PLUGIN_DATA") && !has(&p, "COLUMNS"), "{p:?}");
    let reads_library = small_with("[[settings]]\nkey = \"mode\"", "[permissions]\npaths = [{ setting = \"library_path\", access = \"read\" }]\n\n[[settings]]\nkey = \"mode\"");
    assert!(parse(&reads_library).unwrap().check_origin(Origin::Upload).is_empty(), "reading the library is allowed (with a warning on the card)");
    // a builtin plugin's keys are Encore's own, any other plugin's are namespaced
    assert_eq!(m.store_key(&m.settings[0]), "library_path");
}

#[test]
fn programs_are_checked_before_they_run() {
    let base = r#"
[plugin]
id = "prog"
name = "Prog"
version = "1.0.0"
kind = "tool"
protocol = 1
commands = ["describe", "status"]

[run]
script = "run.sh"
interpreter = "sh"

[update]
github = "someone/prog"
tag = "v{version}"

[[binary]]
arch = "x86_64"
url = "https://example.org/prog-x64.tar.gz"
sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
bin = "prog"
"#;
    let m = parse(base).unwrap();
    assert!(m.check_origin(Origin::Upload).is_empty(), "one machine is enough for an upload");
    assert!(has(&m.check_origin(Origin::Catalog), "aarch64"), "the catalog lists both machines");
    assert_eq!(m.binary_for("x86_64").unwrap().unpack_for("prog-x64.tar.gz"), Some("tar.gz"));
    assert_eq!(m.verify_argv(), ["{bin}", "--version"]);
    assert_eq!(m.max_bytes(), DEFAULT_MAX_MB * 1024 * 1024);
    let p = problems_of(&base.replace("sha256 = \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"\n", ""));
    assert!(has(&p, "sha256 or sums"), "{p:?}");
    let p = problems_of(&base.replace("https://example.org/prog-x64.tar.gz", "http://example.org/prog-x64.tar.gz"));
    assert!(has(&p, "url must be https"), "{p:?}");
    let p = problems_of(&base.replace("prog-x64.tar.gz", "prog-x64.bin"));
    assert!(has(&p, "does not tell"), "{p:?}");
    assert!(problems_of(&base.replace("prog-x64.tar.gz", "prog-x64.bin").replace("bin = \"prog\"", "unpack = \"file\"\nbin = \"prog\"")).is_empty());
    let p = problems_of(&base.replace("bin = \"prog\"", "bin = \"../prog\""));
    assert!(has(&p, "bin \"../prog\""), "{p:?}");
    let p = problems_of(&format!("{base}\n[install]\nverify = [\"sh\", \"-c\", \"prog --version\"]\n"));
    assert!(has(&p, "starts with \"{bin}\""), "{p:?}");
    let p = problems_of(&format!("{base}\n[install]\nversion_from = \"json\"\n"));
    assert!(has(&p, "version_from"), "{p:?}");
    let p = problems_of(&format!("{base}\n[[install.check]]\nfile = \"lib/x.py\"\nmessage = \"x\"\n"));
    assert!(has(&p, "either file and contains"), "{p:?}");
    let p = problems_of(&base.replace("[update]\ngithub = \"someone/prog\"\ntag = \"v{version}\"\n", ""));
    assert!(has(&p, "says where its versions come from"), "{p:?}");
    let with_asset = base.replace("url = \"https://example.org/prog-x64.tar.gz\"", "asset = [\"prog-{version}-x64.tar.gz\", \"prog-{version}-x64.zip\"]");
    let m = parse(&with_asset).unwrap();
    let b = m.binary_for("x86_64").unwrap();
    assert_eq!(b.names(), ["prog-{version}-x64.tar.gz", "prog-{version}-x64.zip"]);
    assert_eq!(b.unpack_for("prog-1.0-x64.zip"), Some("zip"), "the format of the asset the release had");
    let p = problems_of(&with_asset.replace("[update]\ngithub = \"someone/prog\"", "[update]\nfrom = \"none\""));
    assert!(has(&p, "asset needs [update] github"), "{p:?}");
}

#[test]
fn python_packages_are_requirements_not_options() {
    let base = r#"
[plugin]
id = "py"
name = "Py"
version = "0.32.0"
kind = "tool"
protocol = 1
commands = ["status"]
builtin = true

[update]
from = "pypi"
pypi = "py-tool"

[python]
python = "3.14"
packages = ["py-tool=={version}"]
bin = ["py-tool", "pytool"]
"#;
    let m = parse(base).unwrap();
    assert_eq!(m.python.as_ref().unwrap().bin.0, ["py-tool", "pytool"]);
    assert_eq!(m.update.as_ref().unwrap().source(), "pypi");
    let p = problems_of(&base.replace("[\"py-tool=={version}\"]", "[\"--index-url=https://evil.example\", \"py-tool\"]"));
    assert!(has(&p, "a requirement, not an option"), "{p:?}");
    let p = problems_of(&base.replace("python = \"3.14\"", "python = \"latest\""));
    assert!(has(&p, "[python] python"), "{p:?}");
    let p = problems_of(&base.replace("[python]\npython = \"3.14\"\npackages = [\"py-tool=={version}\"]\nbin = [\"py-tool\", \"pytool\"]\n", ""));
    assert!(has(&p, "installs a [python] venv"), "{p:?}");
    // overrides: requirements too (a git URL a package names, replaced by an archive)
    let with = |o: &str| base.replace("bin = [\"py-tool\", \"pytool\"]\n", &format!("bin = [\"py-tool\", \"pytool\"]\noverrides = [{o}]\n"));
    let m = parse(&with("\"dep @ https://example.com/dep.zip\"")).unwrap();
    assert_eq!(m.python.as_ref().unwrap().overrides, ["dep @ https://example.com/dep.zip"]);
    let p = problems_of(&with("\"--extra-index-url=https://evil.example\""));
    assert!(has(&p, "[python] overrides"), "{p:?}");
}

#[test]
fn update_tags_and_pins() {
    let u = Update { github: Some("o/r".into()), tag: Some("b{version}".into()), ..Default::default() };
    assert_eq!(u.source(), "github");
    assert_eq!(u.tag_for("6800"), "b6800");
    assert_eq!(u.version_in_tag("b6800").as_deref(), Some("6800"));
    assert_eq!(u.version_in_tag("6800"), None);
    let plain = Update { github: Some("o/r".into()), tag: Some("{version}".into()), ..Default::default() };
    assert_eq!(plain.version_in_tag("0.26.1").as_deref(), Some("0.26.1"));
    assert_eq!(plain.version_in_tag("nightly"), None, "a tag with no number is no version");
    let default_tag = Update { github: Some("o/r".into()), ..Default::default() };
    assert_eq!(default_tag.tag_for("2.4.10"), "v2.4.10");
    assert_eq!(Update::default().source(), "none");
    let pinned = r#"
[plugin]
id = "pinned"
name = "Pinned"
version = "1.0.0"
kind = "tool"
protocol = 1
commands = ["describe"]

[run]
script = "x.py"
interpreter = "python3"

[update]
github = "o/r"
asset = "pinned-{version}.zip"
sums = "SHA256SUMS"
pin = "v1.0.0"
pin_reports = "0.9.9"
"#;
    assert!(problems_of(pinned).is_empty(), "{:?}", problems_of(pinned));
    let p = problems_of(&pinned.replace("pin = \"v1.0.0\"", "pin = \"release-1\""));
    assert!(has(&p, "[update] pin"), "{p:?}");
    let p = problems_of(&pinned.replace("sums = \"SHA256SUMS\"\n", ""));
    assert!(has(&p, "sha256 must be checked"), "{p:?}");
    let p = problems_of(&pinned.replace("github = \"o/r\"", "github = \"https://github.com/o/r\""));
    assert!(has(&p, "owner/repo"), "{p:?}");
}

#[test]
fn permissions_read_as_the_sdk_writes_them() {
    let p = problems_of(&small_with("[run]\n", "[permissions]\nnetwork = [\"Bad Host\"]\nfilesystem = \"everything\"\nmemory = 10\n\n[run]\n"));
    assert!(has(&p, "network \"Bad Host\"") && has(&p, "filesystem") && has(&p, "memory"), "{p:?}");
    let err = parse(&small_with("[run]\n", "[permissions]\nnetwork = \"yes\"\n\n[run]\n")).unwrap_err().to_string();
    assert!(err.contains("network"), "{err}");
    let m = parse(&small_with("[run]\n", "[permissions]\nnetwork = true\nmemory = 0\n\n[run]\n")).unwrap();
    assert!(m.permissions.network.allowed() && m.permissions.network.hosts().is_empty());
    assert_eq!(m.permissions.memory, Some(0), "0 is no limit");
}

/// An update the scheduled task installs by itself may not ask for more than the version the admin accepted.
#[test]
fn a_new_release_asking_for_more_is_named() {
    let with = |perm: &str| parse(&small_with("[run]\n", &format!("{perm}\n\n[run]\n"))).unwrap();
    let off = with("[permissions]\nnetwork = false");
    let hosts = with("[permissions]\nnetwork = [\"archive.org\"]");
    assert!(off.asks_for_more(&off).is_empty() && hosts.asks_for_more(&hosts).is_empty(), "the same asks for no more");
    assert!(off.asks_for_more(&hosts).is_empty(), "less is fine");
    assert_eq!(hosts.asks_for_more(&off), vec!["the network (archive.org)".to_string()]);
    let more = with("[permissions]\nnetwork = [\"archive.org\", \"example.com\"]");
    assert_eq!(more.asks_for_more(&hosts), vec!["more hosts (example.com)".to_string()]);
    assert_eq!(with("[permissions]\nnetwork = true").asks_for_more(&hosts), vec!["the whole network, not only its hosts".to_string()]);
    let folder = with("[permissions]\nfilesystem = \"folder\"");
    assert_eq!(folder.asks_for_more(&off), vec!["the download folder".to_string()]);
    assert_eq!(with("[permissions]\nmemory = 4096\nprocesses = 0").asks_for_more(&off), vec!["a higher memory limit".to_string(), "a higher processes limit".to_string()]);
    assert!(with("[permissions]\nmemory = 512").asks_for_more(&off).is_empty(), "a lower limit is fine");
    let upd = |repo: &str| parse(&small_with("[run]\n", &format!("[update]\ngithub = \"{repo}\"\ntag = \"v{{version}}\"\nasset = \"small-{{version}}.zip\"\nsums = \"SHA256SUMS\"\nauto = true\n\n[run]\n"))).unwrap();
    assert!(upd("o/r").asks_for_more(&upd("o/r")).is_empty());
    assert_eq!(upd("x/y").asks_for_more(&upd("o/r")), vec!["updates from x/y".to_string()]);
}

#[test]
fn setting_keys_ids_and_sources_are_unique_across_plugins() {
    let a = parse(SMALL).unwrap();
    let b = parse(&SMALL.replace("id = \"small\"", "id = \"other\"")).unwrap();
    assert!(check_together(&[&a, &b]).is_empty(), "two uploads may both have a `mode`: their keys are their own");
    let p = check_together(&[&a, &a.clone()]);
    assert!(has(&p, "plugin id \"small\" is used 2 times"), "{p:?}");
    let builtin = |id: &str, key: &str, source: &str| {
        parse(&format!(
            "[plugin]\nid = \"{id}\"\nname = \"{id}\"\nversion = \"0.32.0\"\nkind = \"source\"\nprotocol = 1\ncommands = [\"search\", \"download\"]\nkinds = [\"album\"]\nbuiltin = true\n\n[[sources]]\nid = \"{source}\"\nname = \"{source}\"\n\n[[settings]]\nkey = \"{key}\"\n"
        ))
        .unwrap()
    };
    let (x, y) = (builtin("x", "shared_key", "one"), builtin("y", "shared_key", "two"));
    let p = check_together(&[&x, &y]);
    assert!(has(&p, "setting key \"shared_key\" belongs to both x and y"), "{p:?}");
    let (x, y) = (builtin("x", "k1", "same"), builtin("y", "k2", "same"));
    let p = check_together(&[&x, &y]);
    assert!(has(&p, "source id \"same\" belongs to both x and y"), "{p:?}");
    let up = parse(&SMALL.replace("id = \"small\"", "id = \"same\"")).unwrap();
    let p = check_together(&[&x, &up]);
    assert!(has(&p, "source id \"same\""), "an upload can't answer for a source id another plugin has: {p:?}");
}

#[test]
fn errors_say_every_problem_once() {
    let e = parse(&small_with("protocol = 1", "protocol = 3").replace("kind = \"source\"", "kind = \"gadget\"")).unwrap_err();
    assert!(e.problems.len() >= 2, "{e:?}");
    assert!(e.to_string().starts_with("plugin.toml has "), "{e}");
    let one = parse(&small_with("protocol = 1", "protocol = 3")).unwrap_err();
    assert!(one.to_string().starts_with("plugin.toml: [plugin] protocol 3"), "{one}");
    let broken = parse("[plugin\nid = 1").unwrap_err();
    assert!(broken.to_string().contains("line 1"), "{broken}");
    let big = format!("{SMALL}\n#{}", "x".repeat(MAX_MANIFEST_BYTES));
    assert!(parse(&big).unwrap_err().to_string().contains("larger than"));
}

#[test]
fn min_encore_is_compared_by_number() {
    let m = parse(&small_with("protocol = 1", "protocol = 1\nmin_encore = \"0.33.0\"")).unwrap();
    assert!(m.fits("0.33.0").is_ok() && m.fits("0.40.1").is_ok());
    let e = m.fits("0.32.9").unwrap_err();
    assert!(e.contains("needs Encore 0.33.0 or newer (this is 0.32.9)"), "{e}");
    assert!(has(&problems_of(&small_with("protocol = 1", "protocol = 1\nmin_encore = \"soon\"")), "min_encore"));
}

/// Settings and files for the condition tests.
struct Fake {
    values: BTreeMap<&'static str, Value>,
    saved: BTreeSet<&'static str>,
    files: BTreeSet<&'static str>,
}

impl Facts for Fake {
    fn value(&self, key: &str) -> Option<Value> {
        self.values.get(key).cloned()
    }
    fn saved(&self, key: &str) -> bool {
        self.saved.contains(key)
    }
    fn exists(&self, rel: &str) -> bool {
        self.files.contains(rel)
    }
}

#[test]
fn conditions_read_settings_and_files() {
    let f = Fake {
        values: BTreeMap::from([
            ("on", json!(true)),
            ("off", json!(false)),
            ("text", json!(" x ")),
            ("empty", json!("")),
            ("zero", json!(0)),
            ("count", json!(4)),
            ("order", json!("tidal, Deezer,slskd")),
            ("client", json!("qBittorrent")),
            ("auto", json!(true)),
        ]),
        saved: BTreeSet::from(["off"]),
        files: BTreeSet::from(["tools/venv/bin/yt-dlp"]),
    };
    let holds = |c: &str| Condition::parse(c).unwrap().holds(&f);
    assert!(holds("on") && holds("text") && holds("count") && !holds("off") && !holds("empty") && !holds("zero") && !holds("missing"));
    assert!(holds("!off") && holds("!missing") && !holds("!on"));
    assert!(holds("client=qbittorrent") && !holds("client=deluge") && holds("client!=deluge") && holds("count=4"));
    assert!(holds("order~deezer") && holds("order~slskd") && !holds("order~qobuz"));
    assert!(holds("file:tools/venv/bin/yt-dlp") && !holds("file:tools/slskd/current"));
    assert!(!holds("saved:auto"), "on by default is not set by the owner");
    assert!(!holds("saved:off"), "set by the owner, but off");
    for bad in ["", "Key", "a=", "a= b", "file:../etc", "file:/etc/passwd", "saved:X", "!", "a~b~c"] {
        assert!(Condition::parse(bad).is_err(), "{bad:?}");
    }
    assert_eq!(Condition::parse("order~deezer").unwrap().key(), Some("order"));
    assert_eq!(Condition::parse("file:x").unwrap().key(), None);
    let m = parse(&format!("{}\n[migration]\nin_use = [[\"on\", \"client=deluge\"], [\"file:tools/venv/bin/yt-dlp\"]]\n", SMALL)).unwrap();
    assert!(m.in_use(&f), "the second list holds");
    let m = parse(&format!("{}\n[migration]\nin_use = [[\"on\", \"client=deluge\"]]\n", SMALL)).unwrap();
    assert!(!m.in_use(&f), "every condition of a list must hold");
    assert!(!parse(SMALL).unwrap().in_use(&f), "no [migration]: never adopted");
    let p = problems_of(&format!("{SMALL}\n[migration]\nin_use = [[\"file:../x\"]]\nfrom_tool = \"lidarr\"\nauto_key = \"auto\"\nadopt = [\"/config\"]\n"));
    for want in ["in_use", "from_tool", "auto_key", "adopt"] {
        assert!(has(&p, want), "{want}: {p:?}");
    }
}

#[test]
fn small_checks() {
    assert!(is_id("internet-archive") && is_id("a") && is_id("x9") && !is_id("") && !is_id("a_b") && !is_id("a-") && !is_id(&"a".repeat(41)));
    assert!(is_key("max_items") && !is_key("1x") && !is_key("Max") && !is_key("a-b"));
    assert!(is_version("0.1.0") && is_version("2026.10.10") && is_version("1.0.0-beta.2") && !is_version(".1") && !is_version("v1/2") && !is_version("abc"));
    assert!(is_rel_path("lib/python3*/x.py", true) && !is_rel_path("lib/python3*/x.py", false));
    assert!(!is_rel_path("a//b", false) && !is_rel_path("./a", false) && !is_rel_path("a/../b", false) && !is_rel_path("a\\b", false));
    assert_eq!(fill("llama-{tag}-bin-{version}.zip", "6800", "b6800"), "llama-b6800-bin-6800.zip");
    for c in COMMANDS {
        assert!(default_timeout(c) > 0);
    }
    assert_eq!(ERROR_CODES.len(), 7);
}
