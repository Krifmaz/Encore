//! `plugin.toml`, the manifest every plugin carries: the format of the plugin SDK (protocol 1,
//! plugins/sdk/encore_plugin.py) with the extensions the catalog needs to install and adopt the tools Encore
//! used to manage itself (program downloads per architecture, Python venvs made with uv, a version check, the
//! settings that keep their old keys). docs/contracts/plugin-manifest.md describes every key; this module reads
//! a manifest (serde + toml, unknown keys refused) and checks it, so a manifest that passes here is one the
//! registry, the installer and the web page can rely on.
//!
//! Three checks, in the order an install runs them:
//! - [`parse`]: the file reads, every key is known and every value fits ([`Manifest::problems`]);
//! - [`Manifest::check_origin`]: what a plugin may ask for depends on where it came from: the catalog
//!   (first-party, built into Encore) or an upload or a URL (third-party, the strictest sandbox);
//! - [`check_together`]: ids, source ids and setting keys are unique across every plugin Encore knows.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// The process protocol this Encore speaks (`protocol` in `[plugin]` and in `describe`).
pub const PROTOCOL: u32 = 1;
/// The commands of protocol 1, in the SDK's order.
pub const COMMANDS: [&str; 6] = ["describe", "status", "test", "search", "download", "update"];
/// The error codes of protocol 1 (`{"ok": false, "error": "...", "code": "..."}`).
pub const ERROR_CODES: [&str; 7] = ["auth", "not_found", "network", "rate_limited", "unsupported", "cancelled", "internal"];
/// What a plugin is: a download source (searched for requests and in the picker), a download client a source
/// hands its downloads to, a tool other parts of Encore use, the server's playback processing, or a
/// connection to another music server.
pub const KINDS: [&str; 5] = ["source", "client", "tool", "playback", "connection"];
/// The request kinds a source answers (`kinds`): a song, an album, a pasted link.
pub const REQUEST_KINDS: [&str; 3] = ["track", "album", "url"];
/// The field types of `[[settings]]` (the SDK's select, text, number, bool, password and the settings table's
/// checks, textarea, path, url).
pub const SETTING_TYPES: [&str; 9] = ["text", "password", "number", "bool", "select", "checks", "textarea", "path", "url"];
/// The machines Encore's image is built for (`std::env::consts::ARCH`).
pub const ARCHES: [&str; 2] = ["x86_64", "aarch64"];
/// The programs a process plugin's script may be run with (none: the script is a program itself).
pub const INTERPRETERS: [&str; 3] = ["python3", "node", "sh"];
/// The ids of the old Download tools card (`/api/system/tools/{id}`), which `[migration] from_tool` names.
pub const OLD_TOOLS: [&str; 5] = ["ytdlp", "streamrip", "cadenza", "slskd", "translator"];
/// The largest manifest read.
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
/// The largest plugin zip an upload or a URL install takes.
pub const UPLOAD_MAX_BYTES: u64 = 200 * 1024 * 1024;
/// The largest program download when `[install] max_mb` does not say.
pub const DEFAULT_MAX_MB: u64 = 400;
/// How long `[install] verify` may run when `[install] timeout` does not say (seconds).
pub const DEFAULT_VERIFY_SECS: u64 = 60;

/// How long a command may run when `[run.timeouts]` does not say (seconds).
pub fn default_timeout(command: &str) -> u64 {
    match command {
        "describe" => 10,
        "status" => 15,
        "test" => 30,
        "search" => 60,
        "download" => 3600,
        "update" => 600,
        _ => 60,
    }
}

/// Where a plugin came from: the catalog Encore ships (first-party), or an upload or a URL (third-party).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Catalog,
    Upload,
    Url,
}

impl Origin {
    /// The `origin` column of the plugins table.
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::Catalog => "catalog",
            Origin::Upload => "upload",
            Origin::Url => "url",
        }
    }

    pub fn parse(s: &str) -> Option<Origin> {
        [Origin::Catalog, Origin::Upload, Origin::Url].into_iter().find(|o| o.as_str() == s)
    }

    /// The `trust` column: the catalog is first-party, everything else third-party.
    pub fn trust(self) -> &'static str {
        if self == Origin::Catalog { "first-party" } else { "third-party" }
    }
}

// ---- the manifest -------------------------------------------------------------------------------

/// A whole `plugin.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub plugin: Plugin,
    /// The source ids a source plugin answers for (`source_order`, the picker). None listed: the plugin's id.
    #[serde(default)]
    pub sources: Vec<SourceEntry>,
    /// How a process plugin is started. Builtin plugins have none, or only `env` and `timeouts`.
    #[serde(default)]
    pub run: Option<Run>,
    #[serde(default)]
    pub update: Option<Update>,
    #[serde(default)]
    pub install: Option<Install>,
    /// The program, one per machine.
    #[serde(default, rename = "binary")]
    pub binaries: Vec<Binary>,
    #[serde(default)]
    pub python: Option<Python>,
    /// Data files besides the program (a model), each chosen by a setting.
    #[serde(default, rename = "download")]
    pub downloads: Vec<DataFile>,
    #[serde(default)]
    pub settings: Vec<Setting>,
    #[serde(default)]
    pub permissions: Permissions,
    #[serde(default)]
    pub client: Option<Client>,
    #[serde(default)]
    pub migration: Option<Migration>,
}

/// `[plugin]`: who the plugin is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plugin {
    /// a-z, 0-9 and dashes, at most 40 characters: the folder under `<config>/plugins/`
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// one of [`KINDS`]
    pub kind: String,
    /// [`PROTOCOL`]
    pub protocol: u32,
    #[serde(default)]
    pub homepage: String,
    #[serde(default)]
    pub license: String,
    /// the commands it answers ([`COMMANDS`])
    #[serde(default)]
    pub commands: Vec<String>,
    /// a source's request kinds ([`REQUEST_KINDS`])
    #[serde(default)]
    pub kinds: Vec<String>,
    /// a source that can deliver lossless files
    #[serde(default)]
    pub lossless: bool,
    /// it needs an account before it works
    #[serde(default)]
    pub needs_sign_in: bool,
    /// Extension: the oldest Encore it runs on.
    #[serde(default)]
    pub min_encore: Option<String>,
    /// Extension (catalog only): Encore's own code runs it in-process; only the programs it starts are installed.
    #[serde(default)]
    pub builtin: bool,
    /// Extension (catalog only): the Settings page (a section of settings.toml) that keeps this plugin's fields;
    /// the plugin's card links there instead of drawing them.
    #[serde(default)]
    pub settings_on: Option<String>,
}

/// `[[sources]]`: one id of a source plugin in `source_order` and the picker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceEntry {
    pub id: String,
    pub name: String,
    /// the picker's tab id when it differs from `id` (yt-dlp: `ytmusic`)
    #[serde(default)]
    pub picker: Option<String>,
}

/// `[run]`: how a process plugin is started (`<interpreter> <script> <command>`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Run {
    /// the program, relative to the plugin's folder
    #[serde(default)]
    pub script: Option<String>,
    /// [`INTERPRETERS`]; none: the script is a program itself
    #[serde(default)]
    pub interpreter: Option<String>,
    /// Extension: must be absent or false (nothing runs through a shell).
    #[serde(default)]
    pub shell: bool,
    /// Extension: fixed environment variables for the program ({bin}, {bin_dir}, {data} and {plugin} filled in).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// `[run.timeouts]`: seconds per command ([`default_timeout`] for the others)
    #[serde(default)]
    pub timeouts: BTreeMap<String, u64>,
}

/// `[update]`: where new versions come from.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Update {
    /// Extension: "github" (releases of `github`), "pypi" (the project `pypi`) or "none". Left out: github when
    /// `github` is set, pypi when `pypi` is set, else none.
    #[serde(default)]
    pub from: Option<String>,
    /// owner/repo
    #[serde(default)]
    pub github: Option<String>,
    /// a release's tag with `{version}` for the version (default `v{version}`)
    #[serde(default)]
    pub tag: Option<String>,
    /// a process plugin's zip in that release (`{version}` and `{tag}` filled in)
    #[serde(default)]
    pub asset: Option<String>,
    /// the release asset listing the sha256 of the others (sha256sum format), "github" for the digest GitHub lists
    /// for each asset, or an https URL
    #[serde(default)]
    pub sums: Option<String>,
    /// the auto-update switch's starting value
    #[serde(default)]
    pub auto: bool,
    /// Extension: the PyPI project whose newest version is installed (`from = "pypi"`)
    #[serde(default)]
    pub pypi: Option<String>,
    /// Extension: pre-releases count (llama.cpp marks its builds as pre-releases at times)
    #[serde(default)]
    pub prerelease: bool,
    /// Extension: with no matching release, the highest version tag counts
    #[serde(default)]
    pub tags: bool,
    /// Extension: the tag Install puts in (the one this Encore was tested with); Update moves to the newest
    #[serde(default)]
    pub pin: Option<String>,
    /// Extension: the version the pinned release's `verify` names (the streamrip fork's v2.4.10 tag says 2.4.9)
    #[serde(default)]
    pub pin_reports: Option<String>,
}

/// `[install]` (extension): how a new version is checked before Encore switches to it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Install {
    /// the version command: an argv, `{bin}` first; it must exit 0 (default `["{bin}", "--version"]`)
    #[serde(default)]
    pub verify: Option<Vec<String>>,
    /// where the version is read: "output" (the first version number it prints, the default) or "json:<field>"
    #[serde(default)]
    pub version_from: Option<String>,
    /// seconds the version command may take ([`DEFAULT_VERIFY_SECS`])
    #[serde(default)]
    pub timeout: Option<u64>,
    /// the largest download, in MB ([`DEFAULT_MAX_MB`])
    #[serde(default)]
    pub max_mb: Option<u64>,
    /// paths removed from what was unpacked (slskd's web UI)
    #[serde(default)]
    pub remove: Vec<String>,
    /// `[[install.check]]`: more that must hold before the new version is used
    #[serde(default, rename = "check")]
    pub checks: Vec<Check>,
}

/// `[[install.check]]`: a file of the installed program contains a text, or a field of the version command's
/// JSON has a value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    /// relative to the venv (`[python]`) or the unpacked folder (`[[binary]]`); `*` matches within one name
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub contains: Option<String>,
    /// a field of the JSON the version command prints
    #[serde(default)]
    pub json: Option<String>,
    #[serde(default)]
    pub equals: Option<Value>,
    /// what the install error says when it does not hold
    pub message: String,
}

/// `[[binary]]` (extension): the program for one machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binary {
    /// [`ARCHES`]
    pub arch: String,
    /// the release asset of `[update] github` (`{version}` and `{tag}` filled in); a list: the first one the
    /// release has
    #[serde(default)]
    pub asset: Option<Names>,
    /// a direct https URL instead
    #[serde(default)]
    pub url: Option<String>,
    /// the file's sha256, for a URL that names one exact file
    #[serde(default)]
    pub sha256: Option<String>,
    /// where the sha256 is read instead (as `[update] sums`); left out: `[update] sums`
    #[serde(default)]
    pub sums: Option<String>,
    /// "zip", "tar.gz" or "file" (the download is the program); left out: from the name
    #[serde(default)]
    pub unpack: Option<String>,
    /// the program: a path inside the archive, or a file name looked for in its first four folder levels
    pub bin: String,
}

/// `[python]` (extension): a venv made with uv and the image's Python.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Python {
    /// the Python the venv is made with (`uv venv --python`): the image's, under /opt/python
    pub python: String,
    /// what `uv pip install` puts in (`{version}` and `{tag}` filled in)
    pub packages: Vec<String>,
    /// the program in the venv's bin folder; a list: the first one that is there
    pub bin: Names,
    /// requirements that replace what the packages name for the same project (`uv pip install --override`):
    /// zotify names librespot by a git URL, and the image has no git, so its archive goes in instead
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<String>,
}

/// `[[download]]` (extension): a data file the plugin needs besides its program.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataFile {
    /// fetched while this rule holds (`key=value` of one of the plugin's settings); the files of the other values
    /// are removed
    #[serde(default)]
    pub when: Option<String>,
    pub url: String,
    #[serde(default)]
    pub sha256: Option<String>,
    /// "huggingface": the sha256 Hugging Face lists for the file; or as `[update] sums`
    #[serde(default)]
    pub sums: Option<String>,
    /// where it goes, relative to the plugin's data folder
    pub path: String,
    #[serde(default)]
    pub max_mb: Option<u64>,
    /// the file's first bytes (a GGUF model starts with "GGUF")
    #[serde(default)]
    pub magic: Option<String>,
}

/// `[[settings]]`: one field of the plugin's card.
///
/// A builtin plugin's `key` is the name of an Encore setting (settings.toml), kept verbatim so saved values and
/// environment variables keep working; its words come from settings.toml when the setting has a field there.
/// Any other plugin's `key` is its own, stored as `plugin.<id>.<key>` ([`Manifest::store_key`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Setting {
    pub key: String,
    #[serde(default)]
    pub label: Option<String>,
    /// [`SETTING_TYPES`]
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    /// [value, label] pairs of a select or checks
    #[serde(default)]
    pub options: Vec<(String, String)>,
    #[serde(default)]
    pub default: Option<Value>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub help: Option<String>,
    /// Extension: shown while another of the plugin's selects or switches has a value (`key=value`, `key!=value`)
    #[serde(default)]
    pub when: Option<String>,
    /// Extension: a number's unit ("seconds")
    #[serde(default)]
    pub unit: Option<String>,
    /// Extension: a number's step
    #[serde(default)]
    pub step: Option<f64>,
    /// Extension: the field takes the card's whole width
    #[serde(default)]
    pub full: bool,
    /// Extension: for people who know what they are doing
    #[serde(default)]
    pub advanced: bool,
    /// Extension: a heading over this field and the ones after it
    #[serde(default)]
    pub part: Option<String>,
}

/// `[permissions]`: what the plugin's processes may reach.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Permissions {
    /// true / false, or the hosts it talks to (any list means the network is on; the hosts are shown on the card)
    #[serde(default)]
    pub network: Network,
    /// "folder": its own folder, plus the request's download folder while it downloads (the only value)
    #[serde(default)]
    pub filesystem: Option<String>,
    /// Extension: more folders, each named by a setting (or, catalog only, a folder under the config folder)
    #[serde(default)]
    pub paths: Vec<PathGrant>,
    /// Extension: address space in MB (0: no limit)
    #[serde(default)]
    pub memory: Option<u64>,
    /// Extension: processor seconds per run (0: no limit)
    #[serde(default)]
    pub cpu: Option<u64>,
    /// Extension: processes it may start (0: no limit)
    #[serde(default)]
    pub processes: Option<u64>,
    /// Extension: open files (0: no limit)
    #[serde(default)]
    pub open_files: Option<u64>,
}

/// One more folder a plugin's processes see.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathGrant {
    /// the setting that names the folder (`library_path`, `slskd_download_path`)
    #[serde(default)]
    pub setting: Option<String>,
    /// a folder under the config folder (catalog only)
    #[serde(default)]
    pub config: Option<String>,
    /// "read" or "write"
    pub access: String,
}

/// `[client]` (extension): a download client a source hands its downloads to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Client {
    /// the source plugin that uses it
    pub source: String,
    /// "torrent" or "usenet"
    pub protocol: String,
    /// the setting value that makes it the one in use (`torrent_client=qbittorrent`)
    pub chosen_by: String,
}

/// `[migration]` (extension, catalog only): what the plugin takes over from Encore before plugins.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Migration {
    /// its id on the old Download tools card (`/api/system/tools/{id}` answers from this plugin)
    #[serde(default)]
    pub from_tool: Option<String>,
    /// the old auto-update switch (`tools_auto_<id>`): its value becomes the plugin's auto-update
    #[serde(default)]
    pub auto_key: Option<String>,
    /// folders and files under the config folder it uses in place (nothing moves)
    #[serde(default)]
    pub adopt: Vec<String>,
    /// when it was in use before the upgrade: any of these lists whose conditions all hold ([`Condition`])
    #[serde(default)]
    pub in_use: Vec<Vec<String>>,
}

/// `network`: true / false, or a host list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Value", into = "Value")]
pub enum Network {
    #[default]
    Off,
    On,
    Hosts(Vec<String>),
}

impl Network {
    /// The plugin's processes get the network.
    pub fn allowed(&self) -> bool {
        !matches!(self, Network::Off)
    }

    pub fn hosts(&self) -> &[String] {
        match self {
            Network::Hosts(h) => h,
            _ => &[],
        }
    }
}

impl TryFrom<Value> for Network {
    type Error = String;
    fn try_from(v: Value) -> Result<Network, String> {
        match v {
            Value::Bool(true) => Ok(Network::On),
            Value::Bool(false) => Ok(Network::Off),
            Value::Array(a) => {
                let hosts: Option<Vec<String>> = a.into_iter().map(|h| h.as_str().map(str::to_string)).collect();
                hosts.map(Network::Hosts).ok_or_else(|| "network: a list of host names (text), or true / false".to_string())
            }
            _ => Err("network: a list of host names, or true / false".into()),
        }
    }
}

impl From<Network> for Value {
    fn from(n: Network) -> Value {
        match n {
            Network::Off => Value::Bool(false),
            Network::On => Value::Bool(true),
            Network::Hosts(h) => Value::from(h),
        }
    }
}

/// A name, or a list of names (the first one that is there wins).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Value", into = "Value")]
pub struct Names(pub Vec<String>);

impl TryFrom<Value> for Names {
    type Error = String;
    fn try_from(v: Value) -> Result<Names, String> {
        match v {
            Value::String(s) => Ok(Names(vec![s])),
            Value::Array(a) => {
                let names: Option<Vec<String>> = a.into_iter().map(|h| h.as_str().map(str::to_string)).collect();
                names.map(Names).ok_or_else(|| "expected a name or a list of names".to_string())
            }
            _ => Err("expected a name or a list of names".into()),
        }
    }
}

impl From<Names> for Value {
    fn from(n: Names) -> Value {
        Value::from(n.0)
    }
}

// ---- errors -------------------------------------------------------------------------------------

/// Why a manifest was refused: every problem found, each naming the table and key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestError {
    pub problems: Vec<String>,
}

impl ManifestError {
    fn one(p: impl Into<String>) -> ManifestError {
        ManifestError { problems: vec![p.into()] }
    }
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.problems.as_slice() {
            [one] => write!(f, "plugin.toml: {one}"),
            many => write!(f, "plugin.toml has {} problems: {}", many.len(), many.join("; ")),
        }
    }
}

impl std::error::Error for ManifestError {}

// ---- reading ------------------------------------------------------------------------------------

/// Read and check a manifest ([`Manifest::problems`]); the origin's rules are [`Manifest::check_origin`].
pub fn parse(text: &str) -> Result<Manifest, ManifestError> {
    if text.len() > MAX_MANIFEST_BYTES {
        return Err(ManifestError::one(format!("larger than {} KB", MAX_MANIFEST_BYTES / 1024)));
    }
    let m: Manifest = toml::from_str(text).map_err(|e| ManifestError::one(toml_error(text, &e)))?;
    let problems = m.problems();
    if problems.is_empty() { Ok(m) } else { Err(ManifestError { problems }) }
}

/// [`parse`], then the rules of where it came from.
pub fn parse_for(text: &str, origin: Origin) -> Result<Manifest, ManifestError> {
    let m = parse(text)?;
    let problems = m.check_origin(origin);
    if problems.is_empty() { Ok(m) } else { Err(ManifestError { problems }) }
}

/// A TOML or serde error on one line: "line 12: unknown field `shel`, expected one of ...".
fn toml_error(text: &str, e: &toml::de::Error) -> String {
    let msg = e.message().trim().replace('\n', " ");
    match e.span() {
        Some(span) => {
            let line = text.get(..span.start).map(|t| t.matches('\n').count() + 1).unwrap_or(1);
            format!("line {line}: {msg}")
        }
        None => msg,
    }
}

// ---- small checks -------------------------------------------------------------------------------

/// A plugin id: a-z, 0-9 and dashes, at most 40, starting with a letter or digit, not ending with a dash.
pub fn is_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 40
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !s.starts_with('-')
        && !s.ends_with('-')
}

/// A setting key: a-z, 0-9 and underscores, starting with a letter, at most 64.
pub fn is_key(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b.len() <= 64 && b[0].is_ascii_lowercase() && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}

/// A version: letters, digits, dots, dashes and plus signs, at most 40, with a digit, usable as a folder name.
pub fn is_version(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 40
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b".+-_".contains(&b))
        && s.as_bytes()[0].is_ascii_alphanumeric()
        && s.bytes().any(|b| b.is_ascii_digit())
}

/// A path inside a folder: relative, `/`-separated, no empty, `.` or `..` parts, no backslash or NUL. With
/// `glob`, `*` may stand in a name.
pub fn is_rel_path(s: &str, glob: bool) -> bool {
    !s.is_empty()
        && s.len() <= 255
        && !s.starts_with('/')
        && !s.contains(['\\', '\0'])
        && (glob || !s.contains('*'))
        && s.split('/').all(|p| !p.is_empty() && p != "." && p != "..")
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn is_https(s: &str) -> bool {
    s.strip_prefix("https://").is_some_and(|rest| !rest.is_empty() && !rest.contains(char::is_whitespace))
}

fn is_github_repo(s: &str) -> bool {
    let ok = |p: &str| !p.is_empty() && p != "." && p != ".." && p.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b));
    matches!(s.split('/').collect::<Vec<_>>().as_slice(), [o, r] if ok(o) && ok(r))
}

/// An environment variable's name: letters, digits and underscores, not starting with a digit (.NET's
/// `DOTNET_gcServer` has lower-case letters).
fn is_env_name(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b.len() <= 100 && (b[0].is_ascii_alphabetic() || b[0] == b'_') && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')
}

fn is_host_pattern(s: &str) -> bool {
    let h = s.strip_prefix("*.").unwrap_or(s);
    !h.is_empty() && h.len() <= 253 && h.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-.".contains(&b)) && !h.starts_with(['.', '-'])
}

/// A sums source: "github", "huggingface", a file name in the release, or an https URL.
fn is_sums(s: &str, huggingface: bool) -> bool {
    s == "github" || (huggingface && s == "huggingface") || is_https(s) || (is_rel_path(s, false) && !s.contains('/'))
}

/// Text a person reads: no control characters, at most `max` characters.
fn is_text(s: &str, max: usize) -> bool {
    s.chars().count() <= max && !s.chars().any(char::is_control)
}

/// `{version}` and `{tag}` filled in (an asset, a URL, a package spec).
pub fn fill(template: &str, version: &str, tag: &str) -> String {
    template.replace("{version}", version).replace("{tag}", tag)
}

// ---- conditions ---------------------------------------------------------------------------------

/// One condition of `[migration] in_use` (and, as `key=value` / `key!=value`, of `when` and `chosen_by`):
/// - `key`: the setting is on (true, a number other than 0, text that is not empty);
/// - `!key`: it is not;
/// - `key=value`, `key!=value`: it has (not) that value (case does not matter);
/// - `key~item`: its comma-separated list names `item` (`source_order~deezer`);
/// - `saved:key`: the owner set it (a saved value or an environment variable) and it is on;
/// - `file:path`: the file or folder exists under the config folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    On(String),
    Off(String),
    Is(String, String),
    IsNot(String, String),
    Lists(String, String),
    Saved(String),
    File(String),
}

/// What a [`Condition`] is checked against: the settings and the config folder.
pub trait Facts {
    /// A setting's value (saved, else from the environment, else its default); None when Encore has no such key.
    fn value(&self, key: &str) -> Option<Value>;
    /// The owner set it: a saved value or an environment variable, not the default.
    fn saved(&self, key: &str) -> bool;
    /// A path relative to the config folder exists.
    fn exists(&self, rel: &str) -> bool;
}

fn word(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", f as i64),
            _ => n.to_string(),
        },
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// A setting is on: true, a number other than 0, text that is not empty.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.trim().is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
        Value::Null => false,
    }
}

impl Condition {
    pub fn parse(s: &str) -> Result<Condition, String> {
        let bad_key = |k: &str| format!("{s:?}: {k:?} is not a setting name");
        let value_ok = |v: &str| !v.is_empty() && v.trim() == v && !v.contains(['=', '~']);
        if let Some(p) = s.strip_prefix("file:") {
            return if is_rel_path(p, false) { Ok(Condition::File(p.into())) } else { Err(format!("{s:?}: the path must stay inside the config folder")) };
        }
        if let Some(k) = s.strip_prefix("saved:") {
            return if is_key(k) { Ok(Condition::Saved(k.into())) } else { Err(bad_key(k)) };
        }
        let pair = |k: &str, v: &str| -> Result<(String, String), String> {
            if !is_key(k) {
                return Err(bad_key(k));
            }
            if !value_ok(v) {
                return Err(format!("{s:?}: the value must be one word with no = or ~ and no spaces around it"));
            }
            Ok((k.into(), v.into()))
        };
        if let Some((k, v)) = s.split_once("!=") {
            return pair(k, v).map(|(k, v)| Condition::IsNot(k, v));
        }
        if let Some((k, v)) = s.split_once('~') {
            return pair(k, v).map(|(k, v)| Condition::Lists(k, v));
        }
        if let Some((k, v)) = s.split_once('=') {
            return pair(k, v).map(|(k, v)| Condition::Is(k, v));
        }
        if let Some(k) = s.strip_prefix('!') {
            return if is_key(k) { Ok(Condition::Off(k.into())) } else { Err(bad_key(k)) };
        }
        if is_key(s) { Ok(Condition::On(s.into())) } else { Err(bad_key(s)) }
    }

    /// The setting it reads, if any.
    pub fn key(&self) -> Option<&str> {
        match self {
            Condition::On(k) | Condition::Off(k) | Condition::Is(k, _) | Condition::IsNot(k, _) | Condition::Lists(k, _) | Condition::Saved(k) => Some(k),
            Condition::File(_) => None,
        }
    }

    pub fn holds(&self, facts: &impl Facts) -> bool {
        let val = |k: &str| facts.value(k).unwrap_or(Value::Null);
        match self {
            Condition::On(k) => truthy(&val(k)),
            Condition::Off(k) => !truthy(&val(k)),
            Condition::Is(k, v) => word(&val(k)).eq_ignore_ascii_case(v),
            Condition::IsNot(k, v) => !word(&val(k)).eq_ignore_ascii_case(v),
            Condition::Lists(k, item) => word(&val(k)).split(',').any(|x| x.trim().eq_ignore_ascii_case(item)),
            Condition::Saved(k) => facts.saved(k) && truthy(&val(k)),
            Condition::File(p) => facts.exists(p),
        }
    }
}

/// A `key=value` / `key!=value` rule (`when`, `chosen_by`).
fn select_rule(s: &str) -> Result<Condition, String> {
    match Condition::parse(s)? {
        c @ (Condition::Is(..) | Condition::IsNot(..)) => Ok(c),
        _ => Err(format!("{s:?}: expected key=value or key!=value")),
    }
}

// ---- what a manifest says -----------------------------------------------------------------------

impl Update {
    /// "github", "pypi" or "none".
    pub fn source(&self) -> &str {
        match self.from.as_deref() {
            Some(f) => f,
            None if self.github.is_some() => "github",
            None if self.pypi.is_some() => "pypi",
            None => "none",
        }
    }

    pub fn tag_template(&self) -> &str {
        self.tag.as_deref().unwrap_or("v{version}")
    }

    /// The tag of a version.
    pub fn tag_for(&self, version: &str) -> String {
        self.tag_template().replace("{version}", version)
    }

    /// The version a tag stands for, when the tag fits the template (`internet-archive-v0.2.0` → `0.2.0`).
    pub fn version_in_tag(&self, tag: &str) -> Option<String> {
        let (pre, post) = self.tag_template().split_once("{version}")?;
        let v = tag.strip_prefix(pre)?.strip_suffix(post)?;
        is_version(v).then(|| v.to_string())
    }
}

impl Binary {
    /// How the download named `name` (the asset found, or the URL) is unpacked: "zip", "tar.gz" or "file" – as
    /// `unpack` says, else from the name (None: it can't be told).
    pub fn unpack_for<'a>(&'a self, name: &str) -> Option<&'a str> {
        if let Some(u) = self.unpack.as_deref() {
            return Some(u);
        }
        let name = name.split(['?', '#']).next().unwrap_or(name);
        if name.ends_with(".zip") {
            Some("zip")
        } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
            Some("tar.gz")
        } else {
            None
        }
    }

    /// The names a download may have: the asset names, or the URL.
    pub fn names(&self) -> Vec<&str> {
        match (&self.asset, &self.url) {
            (Some(a), _) => a.0.iter().map(String::as_str).collect(),
            (None, Some(u)) => vec![u.as_str()],
            (None, None) => vec![],
        }
    }
}

impl Manifest {
    pub fn id(&self) -> &str {
        &self.plugin.id
    }

    /// A program Encore runs as a process speaking protocol 1 (an upload, a URL), not Encore's own code.
    pub fn is_process(&self) -> bool {
        !self.plugin.builtin
    }

    pub fn answers(&self, command: &str) -> bool {
        self.plugin.commands.iter().any(|c| c == command)
    }

    /// Seconds a command may run.
    pub fn timeout(&self, command: &str) -> u64 {
        self.run.as_ref().and_then(|r| r.timeouts.get(command).copied()).unwrap_or_else(|| default_timeout(command))
    }

    /// The source ids it answers for (a source with none listed: its own id and name).
    pub fn source_ids(&self) -> Vec<SourceEntry> {
        if self.plugin.kind != "source" {
            return vec![];
        }
        if self.sources.is_empty() {
            return vec![SourceEntry { id: self.plugin.id.clone(), name: self.plugin.name.clone(), picker: None }];
        }
        self.sources.clone()
    }

    /// Its program for a machine (`std::env::consts::ARCH`).
    pub fn binary_for(&self, arch: &str) -> Option<&Binary> {
        self.binaries.iter().find(|b| b.arch == arch)
    }

    /// It installs a program (a download or a venv), not only its own files.
    pub fn installs_program(&self) -> bool {
        !self.binaries.is_empty() || self.python.is_some()
    }

    pub fn setting(&self, key: &str) -> Option<&Setting> {
        self.settings.iter().find(|s| s.key == key)
    }

    /// Where a setting's value is kept in Encore's settings: a builtin plugin's keys are Encore's own,
    /// verbatim; any other plugin's are `plugin.<id>.<key>`.
    pub fn store_key(&self, s: &Setting) -> String {
        if self.plugin.builtin { s.key.clone() } else { format!("plugin.{}.{}", self.plugin.id, s.key) }
    }

    /// The version command (`[install] verify`, or `{bin} --version`).
    pub fn verify_argv(&self) -> Vec<String> {
        self.install.as_ref().and_then(|i| i.verify.clone()).unwrap_or_else(|| vec!["{bin}".into(), "--version".into()])
    }

    pub fn max_bytes(&self) -> u64 {
        self.install.as_ref().and_then(|i| i.max_mb).unwrap_or(DEFAULT_MAX_MB) * 1024 * 1024
    }

    /// What this manifest (a new release) asks for that `old` (the installed one) did not: the network, more
    /// hosts, the download folder, more folders or write access, another place its updates come from, or higher
    /// limits. Empty: it asks for no more. An update that asks for more is installed only by hand.
    pub fn asks_for_more(&self, old: &Manifest) -> Vec<String> {
        let mut out = vec![];
        let (n, o) = (&self.permissions, &old.permissions);
        match (&o.network, &n.network) {
            (Network::Off, Network::On) => out.push("the network".to_string()),
            (Network::Off, Network::Hosts(h)) => out.push(format!("the network ({})", h.join(", "))),
            (Network::Hosts(_), Network::On) => out.push("the whole network, not only its hosts".to_string()),
            (Network::Hosts(oh), Network::Hosts(nh)) => {
                let more: Vec<&str> = nh.iter().filter(|h| !oh.contains(h)).map(String::as_str).collect();
                if !more.is_empty() {
                    out.push(format!("more hosts ({})", more.join(", ")));
                }
            }
            _ => {}
        }
        if o.filesystem.is_none() && n.filesystem.is_some() {
            out.push("the download folder".to_string());
        }
        for g in &n.paths {
            let name = g.setting.as_deref().or(g.config.as_deref()).unwrap_or("?");
            let had = o.paths.iter().any(|h| h.setting == g.setting && h.config == g.config && (h.access == "write" || g.access == "read"));
            if !had {
                out.push(format!("{} access to {name}", g.access));
            }
        }
        let github = |m: &Manifest| m.update.as_ref().and_then(|u| u.github.clone());
        if github(self) != github(old) {
            if let Some(g) = github(self) {
                out.push(format!("updates from {g}"));
            }
        }
        // 0 is no limit; nothing set is the default
        let eff = |v: Option<u64>, default: u64| match v.unwrap_or(default) {
            0 => u64::MAX,
            x => x,
        };
        let limits = [
            ("memory", n.memory, o.memory, crate::process::DEFAULT_MEMORY_MB),
            ("processes", n.processes, o.processes, crate::process::DEFAULT_PROCESSES),
            ("open files", n.open_files, o.open_files, crate::process::DEFAULT_OPEN_FILES),
        ];
        for (name, new, was, default) in limits {
            if eff(new, default) > eff(was, default) {
                out.push(format!("a higher {name} limit"));
            }
        }
        if n.cpu.is_some_and(|c| c == 0 || o.cpu.is_some_and(|w| w != 0 && c > w)) && n.cpu != o.cpu {
            out.push("a higher processor time limit".to_string());
        }
        out
    }

    /// Whether this Encore is new enough (`min_encore`).
    pub fn fits(&self, encore_version: &str) -> Result<(), String> {
        match self.plugin.min_encore.as_deref() {
            Some(min) if encore_core::version::compare(encore_version, min) == std::cmp::Ordering::Less => {
                Err(format!("{} needs Encore {min} or newer (this is {encore_version})", self.plugin.name))
            }
            _ => Ok(()),
        }
    }

    /// Whether it was in use before the upgrade (`[migration] in_use`).
    pub fn in_use(&self, facts: &impl Facts) -> bool {
        let Some(m) = &self.migration else { return false };
        m.in_use.iter().any(|all| !all.is_empty() && all.iter().all(|c| Condition::parse(c).is_ok_and(|c| c.holds(facts))))
    }

    /// Every setting key the manifest names outside `[[settings]]` (rules, the auto switch, path grants):
    /// the catalog test checks that Encore has each one.
    pub fn named_keys(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let mut add_rule = |r: &str| {
            if let Some(k) = Condition::parse(r).ok().as_ref().and_then(Condition::key) {
                out.insert(k.to_string());
            }
        };
        for s in &self.settings {
            if let Some(w) = &s.when {
                add_rule(w);
            }
        }
        for d in &self.downloads {
            if let Some(w) = &d.when {
                add_rule(w);
            }
        }
        if let Some(c) = &self.client {
            add_rule(&c.chosen_by);
        }
        if let Some(m) = &self.migration {
            for c in m.in_use.iter().flatten() {
                add_rule(c);
            }
        }
        if let Some(k) = self.migration.as_ref().and_then(|m| m.auto_key.clone()) {
            out.insert(k);
        }
        for p in &self.permissions.paths {
            if let Some(k) = &p.setting {
                out.insert(k.clone());
            }
        }
        out
    }

    // ---- checks ---------------------------------------------------------------------------------

    /// Everything wrong with the manifest on its own (empty: nothing).
    pub fn problems(&self) -> Vec<String> {
        let mut p = Vec::new();
        self.check_plugin(&mut p);
        self.check_sources(&mut p);
        self.check_run(&mut p);
        self.check_update(&mut p);
        self.check_program(&mut p);
        self.check_downloads(&mut p);
        self.check_settings(&mut p);
        self.check_permissions(&mut p);
        self.check_client(&mut p);
        self.check_migration(&mut p);
        p
    }

    fn check_plugin(&self, p: &mut Vec<String>) {
        let pl = &self.plugin;
        if !is_id(&pl.id) {
            p.push(format!("[plugin] id {:?}: use a-z, 0-9 and dashes (at most 40), starting and ending with a letter or digit", pl.id));
        }
        if pl.name.trim().is_empty() || !is_text(&pl.name, 60) {
            p.push("[plugin] name: one line of at most 60 characters".into());
        }
        if !is_version(&pl.version) {
            p.push(format!("[plugin] version {:?}: a version number such as 0.1.0", pl.version));
        }
        if !is_text(&pl.description, 1000) {
            p.push("[plugin] description: at most 1000 characters on one line".into());
        }
        if !KINDS.contains(&pl.kind.as_str()) {
            p.push(format!("[plugin] kind {:?}: one of {}", pl.kind, KINDS.join(", ")));
        }
        if pl.protocol != PROTOCOL {
            p.push(format!("[plugin] protocol {}: this Encore speaks protocol {PROTOCOL}", pl.protocol));
        }
        if !pl.homepage.is_empty() && !(is_https(&pl.homepage) || pl.homepage.starts_with("http://")) {
            p.push("[plugin] homepage: an http(s) address".into());
        }
        if !is_text(&pl.license, 80) {
            p.push("[plugin] license: at most 80 characters".into());
        }
        let mut seen = BTreeSet::new();
        for c in &pl.commands {
            if !COMMANDS.contains(&c.as_str()) {
                p.push(format!("[plugin] commands: {c:?} is not a command of protocol {PROTOCOL} ({})", COMMANDS.join(", ")));
            }
            if !seen.insert(c) {
                p.push(format!("[plugin] commands: {c:?} is listed twice"));
            }
        }
        if !pl.builtin && !self.answers("describe") {
            p.push("[plugin] commands: a plugin program answers describe (Encore runs it at install)".into());
        }
        if pl.kind == "source" {
            for c in ["search", "download"] {
                if !self.answers(c) {
                    p.push(format!("[plugin] commands: a source answers {c}"));
                }
            }
            if pl.kinds.is_empty() {
                p.push(format!("[plugin] kinds: a source lists the requests it takes ({})", REQUEST_KINDS.join(", ")));
            }
        } else if !pl.kinds.is_empty() {
            p.push("[plugin] kinds: only a source takes requests".into());
        }
        let mut seen = BTreeSet::new();
        for k in &pl.kinds {
            if !REQUEST_KINDS.contains(&k.as_str()) {
                p.push(format!("[plugin] kinds: {k:?} is not one of {}", REQUEST_KINDS.join(", ")));
            }
            if !seen.insert(k) {
                p.push(format!("[plugin] kinds: {k:?} is listed twice"));
            }
        }
        if pl.min_encore.as_deref().is_some_and(|v| encore_core::version::parts(v).is_empty()) {
            p.push("[plugin] min_encore: a version such as 0.32.0".into());
        }
        if let Some(s) = &pl.settings_on {
            if !pl.builtin {
                p.push("[plugin] settings_on: only a builtin plugin keeps its fields on a Settings page".into());
            }
            if !is_key(s) {
                p.push(format!("[plugin] settings_on {s:?}: a section id of the settings table"));
            }
        }
    }

    fn check_sources(&self, p: &mut Vec<String>) {
        if !self.sources.is_empty() && self.plugin.kind != "source" {
            p.push("[[sources]]: only a source has source ids".into());
        }
        let mut seen = BTreeSet::new();
        for s in &self.sources {
            let ok = |id: &str| !id.is_empty() && id.len() <= 40 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
            if !ok(&s.id) {
                p.push(format!("[[sources]] id {:?}: a-z, 0-9, dashes and underscores", s.id));
            }
            if s.picker.as_deref().is_some_and(|x| !ok(x)) {
                p.push(format!("[[sources]] {}: picker is an id like the source's", s.id));
            }
            if s.name.trim().is_empty() || !is_text(&s.name, 60) {
                p.push(format!("[[sources]] {}: a name of at most 60 characters", s.id));
            }
            if !seen.insert(&s.id) {
                p.push(format!("[[sources]] {}: listed twice", s.id));
            }
        }
    }

    fn check_run(&self, p: &mut Vec<String>) {
        let run = self.run.clone().unwrap_or_default();
        if self.plugin.builtin {
            if run.script.is_some() || run.interpreter.is_some() {
                p.push("[run] script: a builtin plugin is Encore's own code and has no script".into());
            }
        } else {
            match run.script.as_deref() {
                None => p.push("[run] script: the program Encore runs, relative to the plugin's folder".into()),
                Some(s) if !is_rel_path(s, false) => p.push(format!("[run] script {s:?}: a path inside the plugin's folder")),
                _ => {}
            }
        }
        if let Some(i) = run.interpreter.as_deref() {
            if !INTERPRETERS.contains(&i) {
                p.push(format!("[run] interpreter {i:?}: one of {} (or none: the script is a program itself)", INTERPRETERS.join(", ")));
            }
        }
        if run.shell {
            p.push("[run] shell: must be absent or false – Encore starts programs without a shell".into());
        }
        for (k, v) in &run.env {
            if !is_env_name(k) {
                p.push(format!("[run] env {k:?}: A-Z, 0-9 and underscores"));
            }
            if v.contains('\0') || v.len() > 4096 {
                p.push(format!("[run] env {k}: at most 4096 characters"));
            }
        }
        for (c, secs) in &run.timeouts {
            if !COMMANDS.contains(&c.as_str()) {
                p.push(format!("[run.timeouts] {c:?} is not a command"));
            }
            if !(1..=86_400).contains(secs) {
                p.push(format!("[run.timeouts] {c}: 1 to 86400 seconds"));
            }
        }
    }

    fn check_update(&self, p: &mut Vec<String>) {
        let Some(u) = &self.update else {
            if self.installs_program() {
                p.push("[update]: a plugin that installs a program says where its versions come from".into());
            }
            return;
        };
        match u.source() {
            "github" => {
                match u.github.as_deref() {
                    Some(g) if is_github_repo(g) => {}
                    Some(g) => p.push(format!("[update] github {g:?}: owner/repo")),
                    None => p.push("[update] github: the owner/repo whose releases carry new versions".into()),
                }
                if u.pypi.is_some() {
                    p.push("[update] pypi: only with from = \"pypi\"".into());
                }
            }
            "pypi" => {
                match u.pypi.as_deref() {
                    Some(n) if !n.is_empty() && n.len() <= 100 && n.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)) => {}
                    _ => p.push("[update] pypi: the PyPI project's name".into()),
                }
                if u.github.is_some() || u.asset.is_some() || u.prerelease || u.tags {
                    p.push("[update]: github, asset, prerelease and tags are for from = \"github\"".into());
                }
                if self.python.is_none() {
                    p.push("[update] from = \"pypi\": the plugin installs a [python] venv".into());
                }
            }
            "none" => {
                if u.github.is_some() || u.pypi.is_some() || u.pin.is_some() {
                    p.push("[update] from = \"none\": no github, pypi or pin".into());
                }
                if self.installs_program() {
                    p.push("[update] from = \"none\": a plugin that installs a program needs a place to find its versions".into());
                }
            }
            other => p.push(format!("[update] from {other:?}: github, pypi or none")),
        }
        if let Some(t) = &u.tag {
            if t.matches("{version}").count() != 1 || !is_text(t, 80) || t.contains(['/', ' ']) {
                p.push(format!("[update] tag {t:?}: a tag with {{version}} once (\"v{{version}}\")"));
            }
        }
        if let Some(a) = &u.asset {
            if !is_rel_path(a, false) || a.contains('/') {
                p.push(format!("[update] asset {a:?}: a file name in the release"));
            }
        }
        if let Some(s) = &u.sums {
            if !is_sums(s, false) {
                p.push(format!("[update] sums {s:?}: \"github\", a file name in the release, or an https address"));
            }
        }
        if self.is_process() && u.source() == "github" {
            if u.asset.is_none() && self.binaries.is_empty() {
                p.push("[update] asset: the plugin's zip in each release".into());
            }
            if u.sums.is_none() && u.asset.is_some() {
                p.push("[update] sums: the zip's sha256 must be checked before it is installed (SHA256SUMS or \"github\")".into());
            }
        }
        if u.pin.is_some() && u.source() == "none" {
            p.push("[update] pin: needs a place versions come from".into());
        }
        if u.pin_reports.is_some() && u.pin.is_none() {
            p.push("[update] pin_reports: only with pin".into());
        }
        if let Some(pin) = &u.pin {
            if u.source() == "github" && u.version_in_tag(pin).is_none() {
                p.push(format!("[update] pin {pin:?}: a tag that fits {:?}", u.tag_template()));
            }
            if u.source() == "pypi" && !is_version(pin) {
                p.push(format!("[update] pin {pin:?}: a version"));
            }
        }
    }

    fn check_program(&self, p: &mut Vec<String>) {
        if !self.binaries.is_empty() && self.python.is_some() {
            p.push("[[binary]] and [python]: one program per plugin".into());
        }
        let github = self.update.as_ref().is_some_and(|u| u.source() == "github");
        let update_sums = self.update.as_ref().and_then(|u| u.sums.clone());
        let mut arches = BTreeSet::new();
        for b in &self.binaries {
            let at = format!("[[binary]] {}", b.arch);
            if !ARCHES.contains(&b.arch.as_str()) {
                p.push(format!("[[binary]] arch {:?}: one of {}", b.arch, ARCHES.join(", ")));
            }
            if !arches.insert(&b.arch) {
                p.push(format!("{at}: listed twice"));
            }
            match (&b.asset, &b.url) {
                (Some(a), None) => {
                    if a.0.is_empty() || a.0.iter().any(|n| !is_rel_path(n, false) || n.contains('/')) {
                        p.push(format!("{at}: asset is a file name in the release (or a list of them)"));
                    }
                    if !github {
                        p.push(format!("{at}: asset needs [update] github"));
                    }
                }
                (None, Some(u)) => {
                    if !is_https(u) {
                        p.push(format!("{at}: url must be https"));
                    }
                }
                _ => p.push(format!("{at}: either asset or url")),
            }
            match (&b.sha256, &b.sums) {
                (Some(h), None) if !is_sha256(h) => p.push(format!("{at}: sha256 is 64 hex digits")),
                (Some(_), None) => {}
                (None, Some(s)) if !is_sums(s, false) => p.push(format!("{at}: sums {s:?}: \"github\", a file name in the release, or an https address")),
                (None, Some(_)) => {}
                (None, None) if update_sums.is_some() => {}
                (None, None) => p.push(format!("{at}: sha256 or sums – a program is checked before it runs")),
                (Some(_), Some(_)) => p.push(format!("{at}: sha256 or sums, not both")),
            }
            let github_sums = b.sums.as_deref() == Some("github") || (b.sha256.is_none() && b.sums.is_none() && update_sums.as_deref() == Some("github"));
            if github_sums && b.url.is_some() {
                p.push(format!("{at}: sums = \"github\" is for a release asset"));
            }
            if let Some(u) = b.unpack.as_deref().filter(|u| !matches!(*u, "zip" | "tar.gz" | "file")) {
                p.push(format!("{at}: unpack {u:?}: zip, tar.gz or file"));
            }
            for n in b.names() {
                if b.unpack_for(n).is_none() {
                    p.push(format!("{at}: unpack: zip, tar.gz or file ({n:?} does not tell)"));
                }
            }
            if !is_rel_path(&b.bin, false) {
                p.push(format!("{at}: bin {:?}: a path inside the download", b.bin));
            }
        }
        if let Some(py) = &self.python {
            let ok = py.python.split('.').count() == 2 && py.python.split('.').all(|x| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit()));
            if !ok {
                p.push(format!("[python] python {:?}: a version such as 3.14", py.python));
            }
            if py.packages.is_empty() {
                p.push("[python] packages: what uv installs".into());
            }
            for pkg in &py.packages {
                if pkg.trim().is_empty() || pkg.trim_start().starts_with('-') || pkg.contains(['\n', '\0']) {
                    p.push(format!("[python] packages {pkg:?}: a requirement, not an option"));
                }
            }
            for o in &py.overrides {
                if o.trim().is_empty() || o.trim_start().starts_with('-') || o.contains(['\n', '\0']) {
                    p.push(format!("[python] overrides {o:?}: a requirement, not an option"));
                }
            }
            if py.bin.0.is_empty() || py.bin.0.iter().any(|n| n.is_empty() || n.contains(['/', '\\', '\0'])) {
                p.push("[python] bin: the program's name in the venv's bin folder".into());
            }
        }
        if let Some(i) = &self.install {
            if !self.installs_program() && (i.verify.is_some() || i.version_from.is_some() || !i.remove.is_empty() || !i.checks.is_empty()) {
                p.push("[install]: verify, version_from, remove and check are for a plugin that installs a program".into());
            }
            if let Some(v) = &i.verify {
                if v.first().map(String::as_str) != Some("{bin}") || v.iter().any(|a| a.contains('\0')) {
                    p.push("[install] verify: an argv that starts with \"{bin}\"".into());
                }
            }
            match i.version_from.as_deref() {
                None | Some("output") => {}
                Some(j) if j.strip_prefix("json:").is_some_and(|f| !f.is_empty() && is_text(f, 60)) => {}
                Some(o) => p.push(format!("[install] version_from {o:?}: \"output\" or \"json:<field>\"")),
            }
            if i.timeout.is_some_and(|t| !(1..=600).contains(&t)) {
                p.push("[install] timeout: 1 to 600 seconds".into());
            }
            if i.max_mb.is_some_and(|m| !(1..=16_384).contains(&m)) {
                p.push("[install] max_mb: 1 to 16384".into());
            }
            for r in &i.remove {
                if !is_rel_path(r, false) {
                    p.push(format!("[install] remove {r:?}: a path inside the download"));
                }
            }
            for c in &i.checks {
                match (&c.file, &c.contains, &c.json, &c.equals) {
                    (Some(f), Some(t), None, None) => {
                        if !is_rel_path(f, true) {
                            p.push(format!("[[install.check]] file {f:?}: a path inside the install"));
                        }
                        if t.is_empty() {
                            p.push("[[install.check]] contains: the text to look for".into());
                        }
                    }
                    (None, None, Some(j), Some(_)) => {
                        if j.is_empty() {
                            p.push("[[install.check]] json: the field to read".into());
                        }
                    }
                    _ => p.push("[[install.check]]: either file and contains, or json and equals".into()),
                }
                if c.message.trim().is_empty() || !is_text(&c.message, 400) {
                    p.push("[[install.check]] message: what the install error says (one line)".into());
                }
            }
        }
    }

    fn check_downloads(&self, p: &mut Vec<String>) {
        for d in &self.downloads {
            let at = format!("[[download]] {}", d.path);
            if !is_https(&d.url) {
                p.push(format!("{at}: url must be https"));
            }
            match (&d.sha256, &d.sums) {
                (Some(h), None) if !is_sha256(h) => p.push(format!("{at}: sha256 is 64 hex digits")),
                (None, Some(s)) if !is_sums(s, true) => p.push(format!("{at}: sums {s:?}: \"huggingface\", \"github\", a file name or an https address")),
                (Some(_), None) | (None, Some(_)) => {}
                _ => p.push(format!("{at}: sha256 or sums")),
            }
            if !is_rel_path(&d.path, false) {
                p.push(format!("{at}: path must stay inside the plugin's data folder"));
            }
            if d.max_mb.is_some_and(|m| !(1..=65_536).contains(&m)) {
                p.push(format!("{at}: max_mb 1 to 65536"));
            }
            if d.magic.as_deref().is_some_and(|m| m.is_empty() || m.len() > 16) {
                p.push(format!("{at}: magic is 1 to 16 characters"));
            }
            if let Some(w) = &d.when {
                match select_rule(w) {
                    Ok(c) => {
                        if c.key().is_some_and(|k| self.setting(k).is_none()) {
                            p.push(format!("{at}: when names {w:?}, not a setting of this plugin"));
                        }
                    }
                    Err(e) => p.push(format!("{at}: when {e}")),
                }
            }
        }
    }

    fn check_settings(&self, p: &mut Vec<String>) {
        let mut seen = BTreeSet::new();
        for s in &self.settings {
            let at = format!("[[settings]] {}", s.key);
            if !is_key(&s.key) {
                p.push(format!("[[settings]] key {:?}: a-z, 0-9 and underscores, starting with a letter", s.key));
            }
            if !seen.insert(&s.key) {
                p.push(format!("{at}: listed twice"));
            }
            if !self.plugin.builtin {
                if s.label.is_none() {
                    p.push(format!("{at}: label is the field's name on the card"));
                }
                if s.kind.is_none() {
                    p.push(format!("{at}: type ({})", SETTING_TYPES.join(", ")));
                }
            }
            if let Some(l) = &s.label {
                if l.trim().is_empty() || !is_text(l, 160) {
                    p.push(format!("{at}: label is one line of at most 160 characters"));
                }
            }
            if s.help.as_deref().is_some_and(|h| !is_text(h, 1000)) {
                p.push(format!("{at}: help is one line of at most 1000 characters"));
            }
            if s.part.as_deref().is_some_and(|x| x.trim().is_empty() || !is_text(x, 80)) {
                p.push(format!("{at}: part is a heading of at most 80 characters"));
            }
            if s.unit.as_deref().is_some_and(|x| x.trim().is_empty() || !is_text(x, 20)) {
                p.push(format!("{at}: unit is at most 20 characters"));
            }
            if let Some(w) = &s.when {
                match select_rule(w) {
                    Ok(c) => {
                        let k = c.key().unwrap_or_default();
                        if k == s.key {
                            p.push(format!("{at}: when names the field itself"));
                        } else if !self.plugin.builtin {
                            match self.setting(k).and_then(|x| x.kind.as_deref()) {
                                Some("select" | "bool") => {}
                                _ => p.push(format!("{at}: when names {k:?}, which is not a select or switch of this plugin")),
                            }
                        }
                    }
                    Err(e) => p.push(format!("{at}: when {e}")),
                }
            }
            let Some(t) = s.kind.as_deref() else {
                if !s.options.is_empty() || s.default.is_some() || s.min.is_some() || s.max.is_some() {
                    p.push(format!("{at}: options, default, min and max go with a type"));
                }
                continue;
            };
            if !SETTING_TYPES.contains(&t) {
                p.push(format!("{at}: type {t:?} is not one of {}", SETTING_TYPES.join(", ")));
                continue;
            }
            self.check_setting_values(s, t, &at, p);
        }
    }

    fn check_setting_values(&self, s: &Setting, t: &str, at: &str, p: &mut Vec<String>) {
        let choice = matches!(t, "select" | "checks");
        if choice {
            if s.options.is_empty() {
                p.push(format!("{at}: a {t} lists its options as [value, label] pairs"));
            }
            let mut vals = BTreeSet::new();
            for (v, l) in &s.options {
                if v.is_empty() || v.contains(',') || !is_text(v, 80) || l.trim().is_empty() || !is_text(l, 160) {
                    p.push(format!("{at}: option [{v:?}, {l:?}]: a value without commas and a label"));
                }
                if !vals.insert(v) {
                    p.push(format!("{at}: option {v:?} is listed twice"));
                }
            }
        } else if !s.options.is_empty() {
            p.push(format!("{at}: only a select or checks has options"));
        }
        if t != "number" && (s.min.is_some() || s.max.is_some() || s.step.is_some() || s.unit.is_some()) {
            p.push(format!("{at}: min, max, step and unit are for a number"));
        }
        if let (Some(lo), Some(hi)) = (s.min, s.max) {
            if lo > hi {
                p.push(format!("{at}: min is above max"));
            }
        }
        if s.step.is_some_and(|x| x <= 0.0) {
            p.push(format!("{at}: step is above 0"));
        }
        let Some(d) = &s.default else { return };
        let in_options = |v: &str| s.options.iter().any(|(o, _)| o == v);
        match t {
            "number" => match d.as_f64() {
                None => p.push(format!("{at}: default is a number")),
                Some(n) => {
                    if s.min.is_some_and(|lo| n < lo) || s.max.is_some_and(|hi| n > hi) {
                        p.push(format!("{at}: default {n} is outside min and max"));
                    }
                }
            },
            "bool" => {
                if !d.is_boolean() {
                    p.push(format!("{at}: default is true or false"));
                }
            }
            "select" => match d {
                Value::String(v) if in_options(v) => {}
                Value::Number(n) if in_options(&n.to_string()) => {}
                _ => p.push(format!("{at}: default {d} is not one of the options")),
            },
            "checks" => match d.as_str() {
                Some(v) if v.split(',').map(str::trim).filter(|x| !x.is_empty()).all(in_options) => {}
                _ => p.push(format!("{at}: default is a comma-separated list of option values")),
            },
            _ => {
                if !d.is_string() {
                    p.push(format!("{at}: default is text"));
                }
            }
        }
    }

    fn check_permissions(&self, p: &mut Vec<String>) {
        let pm = &self.permissions;
        for h in pm.network.hosts() {
            if !is_host_pattern(h) {
                p.push(format!("[permissions] network {h:?}: a host name, or *.domain"));
            }
        }
        if pm.filesystem.as_deref().is_some_and(|f| f != "folder") {
            p.push("[permissions] filesystem: \"folder\" (its own folder and the request's download folder)".into());
        }
        for g in &pm.paths {
            match (&g.setting, &g.config) {
                (Some(k), None) if !is_key(k) => p.push(format!("[permissions] paths: setting {k:?} is not a setting name")),
                (None, Some(c)) if !is_rel_path(c, false) => p.push(format!("[permissions] paths: config {c:?} must stay inside the config folder")),
                (Some(_), None) | (None, Some(_)) => {}
                _ => p.push("[permissions] paths: each grant names either a setting or a config folder".into()),
            }
            if !matches!(g.access.as_str(), "read" | "write") {
                p.push(format!("[permissions] paths: access {:?}: read or write", g.access));
            }
        }
        let range = |name: &str, v: Option<u64>, lo: u64, hi: u64, p: &mut Vec<String>| {
            if v.is_some_and(|x| x != 0 && !(lo..=hi).contains(&x)) {
                p.push(format!("[permissions] {name}: 0 (no limit) or {lo} to {hi}"));
            }
        };
        range("memory", pm.memory, 64, 65_536, p);
        range("cpu", pm.cpu, 1, 86_400, p);
        range("processes", pm.processes, 1, 4096, p);
        range("open_files", pm.open_files, 16, 65_536, p);
    }

    fn check_client(&self, p: &mut Vec<String>) {
        match (&self.client, self.plugin.kind == "client") {
            (None, true) => p.push("[client]: a download client names its source, protocol and chosen_by".into()),
            (Some(_), false) => p.push("[client]: only for kind = \"client\"".into()),
            (Some(c), true) => {
                if !is_id(&c.source) {
                    p.push(format!("[client] source {:?}: the id of the source plugin that uses it", c.source));
                }
                if !matches!(c.protocol.as_str(), "torrent" | "usenet") {
                    p.push(format!("[client] protocol {:?}: torrent or usenet", c.protocol));
                }
                match select_rule(&c.chosen_by) {
                    Ok(Condition::Is(..)) => {}
                    Ok(_) => p.push("[client] chosen_by: key=value".into()),
                    Err(e) => p.push(format!("[client] chosen_by {e}")),
                }
            }
            (None, false) => {}
        }
    }

    fn check_migration(&self, p: &mut Vec<String>) {
        let Some(m) = &self.migration else { return };
        if let Some(t) = &m.from_tool {
            if !OLD_TOOLS.contains(&t.as_str()) {
                p.push(format!("[migration] from_tool {t:?}: one of {}", OLD_TOOLS.join(", ")));
            }
        }
        if let Some(k) = &m.auto_key {
            if !is_key(k) || !k.starts_with("tools_auto_") {
                p.push(format!("[migration] auto_key {k:?}: an old tools_auto_<id> switch"));
            }
        }
        for a in &m.adopt {
            if !is_rel_path(a, false) {
                p.push(format!("[migration] adopt {a:?}: a path inside the config folder"));
            }
        }
        for all in &m.in_use {
            if all.is_empty() {
                p.push("[migration] in_use: an empty list of conditions".into());
            }
            for c in all {
                if let Err(e) = Condition::parse(c) {
                    p.push(format!("[migration] in_use {e}"));
                }
            }
        }
    }

    /// What the plugin may not ask for, given where it came from (empty: nothing).
    pub fn check_origin(&self, origin: Origin) -> Vec<String> {
        let mut p = Vec::new();
        if origin == Origin::Catalog {
            if !self.binaries.is_empty() {
                for a in ARCHES {
                    if self.binary_for(a).is_none() {
                        p.push(format!("[[binary]]: the catalog lists a program for {a} too"));
                    }
                }
            }
            return p;
        }
        let pl = &self.plugin;
        if pl.builtin {
            p.push("[plugin] builtin: only the catalog's plugins are Encore's own code".into());
        }
        if pl.settings_on.is_some() {
            p.push("[plugin] settings_on: only for the catalog".into());
        }
        if self.migration.is_some() {
            p.push("[migration]: only the catalog's plugins take over Encore's old tools".into());
        }
        if self.python.is_some() {
            p.push("[python]: Python packages are installed for the catalog's plugins only (bring the code in the zip)".into());
        }
        if !self.downloads.is_empty() {
            p.push("[[download]]: data downloads are for the catalog's plugins only".into());
        }
        for b in &self.binaries {
            if b.sha256.is_none() {
                p.push(format!("[[binary]] {}: an uploaded plugin's program has a fixed sha256", b.arch));
            }
        }
        if let Some(run) = &self.run {
            for k in run.env.keys() {
                if k.starts_with("LD_") || k.starts_with("ENCORE_") || matches!(k.as_str(), "PATH" | "HOME" | "TMPDIR" | "PYTHONPATH" | "PYTHONHOME" | "NODE_OPTIONS") {
                    p.push(format!("[run] env {k}: set by Encore, not by a plugin"));
                }
            }
        }
        for g in &self.permissions.paths {
            if g.setting.as_deref() != Some("library_path") || g.access != "read" {
                p.push("[permissions] paths: an uploaded plugin may only read the library ({ setting = \"library_path\", access = \"read\" })".into());
            }
        }
        if self.update.as_ref().is_some_and(|u| u.source() == "pypi") {
            p.push("[update] from = \"pypi\": only for the catalog's plugins".into());
        }
        p
    }
}

/// Problems across several plugins: two with one id, one source id or one setting key, two taking over one old
/// tool or one old switch, two clients chosen by one value (empty: none).
pub fn check_together(manifests: &[&Manifest]) -> Vec<String> {
    let mut p = Vec::new();
    let mut ids: BTreeMap<&str, usize> = BTreeMap::new();
    for m in manifests {
        *ids.entry(m.id()).or_default() += 1;
    }
    for (id, n) in &ids {
        if *n > 1 {
            p.push(format!("plugin id {id:?} is used {n} times"));
        }
    }
    let mut sources = Owners::default();
    let mut keys = Owners::default();
    let mut tools = Owners::default();
    let mut chosen = Owners::default();
    for m in manifests {
        let id = m.id();
        for s in m.source_ids() {
            sources.claim("source id", &s.id, id, &mut p);
            if let Some(pk) = s.picker.filter(|pk| *pk != s.id) {
                sources.claim("source id", &pk, id, &mut p);
            }
        }
        for s in &m.settings {
            keys.claim("setting key", &m.store_key(s), id, &mut p);
        }
        if let Some(mg) = &m.migration {
            for t in mg.from_tool.iter().chain(mg.auto_key.iter()) {
                tools.claim("old tool", t, id, &mut p);
            }
        }
        if let Some(c) = &m.client {
            chosen.claim("client choice", &c.chosen_by, id, &mut p);
        }
    }
    p
}

/// Which plugin claimed each name first.
#[derive(Default)]
struct Owners(BTreeMap<String, String>);

impl Owners {
    fn claim(&mut self, what: &str, name: &str, id: &str, p: &mut Vec<String>) {
        match self.0.get(name) {
            Some(other) if other != id => p.push(format!("{what} {name:?} belongs to both {other} and {id}")),
            Some(_) => {}
            None => {
                self.0.insert(name.to_string(), id.to_string());
            }
        }
    }
}
