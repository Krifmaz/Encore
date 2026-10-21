# Plugin manifest (`plugin.toml`)

Every plugin carries a `plugin.toml`: the plugins of the catalog Encore ships (`server/crates/plugins/catalog/`)
and every plugin that is uploaded or installed from a URL. Written for 0.32.0. The format is the one of the
plugin SDK, protocol 1 (`plugins/sdk/encore_plugin.py`, the owner's Internet Archive plugin was written with
it): `[plugin]`, `[run]`, `[run.timeouts]`, `[update]`, `[[settings]]` and `[permissions]`. Encore adds the
tables and keys the catalog needs to install and take over the tools it used to manage by itself (yt-dlp,
streamrip, slskd, Cadenza, the Translator); those are marked **extension** below. A manifest written for
the SDK reads unchanged.

`server/crates/plugins/src/manifest.rs` reads the file (serde and toml) and checks it. **A key it does not
know is refused**, in every table, so a typo (`shel = true`) is an error and not a silent default. The
process a plugin speaks is in [docs/plugins/CONTRACT.md](../plugins/CONTRACT.md).

Three checks run, in the order an install runs them:

1. **The file**: it reads as TOML, every key is known, every value fits (the rules under each table).
   Errors name the line or the table and key: `plugin.toml: [[settings]] format: default "ogg" is not one
   of the options`; several problems are listed together.
2. **Where it came from**: the catalog (first-party, Encore's own code) may ask for more than an upload or a
   URL (third-party, the strictest sandbox and a warning on the card). See [Origins](#origins).
3. **Beside the others**: plugin ids, source ids and setting keys are unique across every plugin Encore
   knows (the catalog and what is installed). See [Unique across plugins](#unique-across-plugins).

## The tables

| Table | From | What it says |
|---|---|---|
| `[plugin]` | SDK | Who the plugin is: id, name, version, kind, protocol, commands |
| `[[sources]]` | extension | The source ids a source answers for (`source_order`, the picker) |
| `[run]`, `[run.timeouts]` | SDK | How the program is started; seconds per command |
| `[update]` | SDK | Where new versions come from |
| `[install]`, `[[install.check]]` | extension | The version command and the checks a new version must pass |
| `[[binary]]` | extension | The program to download, one per machine |
| `[python]` | extension | A venv made with uv and the image's Python |
| `[[download]]` | extension | Data files besides the program (a model), chosen by a setting |
| `[[settings]]` | SDK | The fields of the plugin's card |
| `[permissions]` | SDK | Network, folders and limits of its processes |
| `[client]` | extension | A download client: the source it serves and the setting value that picks it |
| `[migration]` | extension | What it takes over from Encore before plugins (catalog only) |

## `[plugin]`

| Key | Type | Required | Meaning |
|---|---|---|---|
| `id` | text | yes | a-z, 0-9 and dashes, at most 40, starting and ending with a letter or digit. The folder under `<config>/plugins/`, the id in the API (`/api/plugins/{id}`) and what `describe` answers as `name` |
| `name` | text | yes | What the card shows, one line, at most 60 characters |
| `version` | text | yes | The plugin's version (letters, digits, `.`, `-`, `+`; with a digit; a folder name). A catalog entry's version is the Encore release that ships it (`0.32.0`); its card shows the program's version (what `[install] verify` reports) |
| `description` | text | no | One paragraph, at most 1000 characters |
| `kind` | text | yes | `source` (searched for requests and in the picker), `client` (a download client a source hands its downloads to), `tool` (used by other parts of Encore: the Translator), `playback` (server processing: Cadenza), `connection` (another music server: Jellyfin, Plex, Navidrome) |
| `protocol` | number | yes | `1`, the only protocol |
| `homepage` | text | no | An http(s) address |
| `license` | text | no | SPDX name; for a catalog plugin, the license of the program it installs (Encore's own code is MIT) |
| `commands` | list | yes | What it answers, from `describe`, `status`, `test`, `search`, `download`, `update`. A plugin program must answer `describe`; a source must answer `search` and `download` |
| `kinds` | list | a source | The requests a source takes: `track`, `album`, `url` (a pasted link). Only a source has kinds |
| `lossless` | bool | no | A source that can deliver lossless files |
| `needs_sign_in` | bool | no | It needs an account before it works |
| `min_encore` | text | no | **Extension.** The oldest Encore it runs on (`0.32.0`); a newer one is refused with "needs Encore 0.33.0 or newer (this is 0.32.0)". Every build carries its real version from 0.32 on |
| `builtin` | bool | no | **Extension, catalog only.** Encore's own code runs it in-process; only the programs it starts are installed. Its setting keys are Encore's own (see `[[settings]]`) |
| `settings_on` | text | no | **Extension, catalog only.** The Settings page (a section of `settings.toml`) that keeps this plugin's fields; its card links there instead of drawing them (Cadenza: `processing`; Jellyfin, Plex, Navidrome: `integrations`) |

## `[[sources]]` (extension)

A source answers for its own id in `source_order` and the picker, unless it lists others. streamrip answers
for three (`qobuz`, `tidal`, `deezer`); yt-dlp answers for `ytdlp` in `source_order` and has the picker tab
`ytmusic`.

| Key | Type | Required | Meaning |
|---|---|---|---|
| `id` | text | yes | The id in `source_order` (a-z, 0-9, `-`, `_`) |
| `name` | text | yes | The picker tab's name ("YouTube Music") |
| `picker` | text | no | The picker's tab id when it differs from `id` |

The `source_order` ids of 0.31 stay as they are: `qobuz`, `tidal`, `deezer`, `slskd`, `torznab`, `ytdlp`. 0.34 adds
`zotify` (the Spotify tab) and `gamdl` (the Apple Music tab): their searches and downloads go through the registry as
a plugin program's do (`catalog::THROUGH_REGISTRY`), answered by Encore's own code in the SDK's shapes.

## `[run]` and `[run.timeouts]`

How a plugin program is started: `<interpreter> <script> <command>`, one JSON request on stdin, one JSON
answer on stdout ([the contract](../plugins/CONTRACT.md)).

| Key | Type | Required | Meaning |
|---|---|---|---|
| `script` | text | a program | The program, relative to the plugin's folder (no `..`, not absolute). A builtin plugin has none |
| `interpreter` | text | no | `python3` (the image's Python: 3.14 under /opt/python, else `python3` on PATH), `node`, `sh`. Left out: the script is a program itself (its mode must allow running it) |
| `shell` | bool | no | **Extension.** Must be absent or `false`: nothing is started through a shell, every program gets an argv. (The old `streamrip_command` template, run through `sh -c` until 0.31, is split into an argv by the streamrip plugin) |
| `env` | table | no | **Extension.** Fixed environment variables for the program, on top of the ones Encore sets ([the contract](../plugins/CONTRACT.md#environment)). `{bin}` (the installed program), `{bin_dir}` (its folder), `{data}` (the plugin's data folder) and `{plugin}` (its folder) are filled in. An uploaded plugin may not set `PATH`, `HOME`, `TMPDIR`, `PYTHONPATH`, `PYTHONHOME`, `NODE_OPTIONS` or any `LD_*` or `ENCORE_*` variable |
| `[run.timeouts]` | table | no | Seconds per command, 1 to 86400. The defaults: `describe` 10, `status` 15, `test` 30, `search` 60, `download` 3600, `update` 600 |

A builtin plugin has no `script`; its `[run]` may hold `env` (for the programs it starts) and `timeouts`.

## `[update]`

Where new versions come from. Update on the card and the scheduled task ("Update plugins", the old "Update
download tools" with the same `task_tools_*` settings) ask it; with the plugin's auto-update switch on, the
task installs what it finds. For an uploaded or URL plugin the task installs a new release by itself only when
it asks for no more than the installed version: when its `[permissions]` turn the network on, add hosts, the
download folder, more `paths` or write access, raise a limit, or its `[update] github` changes, the task
refuses it (409, "… asks for more than the installed version: …") and the card shows it; Update on the card
installs it.

| Key | Type | Required | Meaning |
|---|---|---|---|
| `github` | text | from = github | `owner/repo` whose releases carry the versions |
| `tag` | text | no | A release's tag, with `{version}` once (default `v{version}`): `internet-archive-v{version}`, `{version}` (slskd), `b{version}` (llama.cpp) |
| `asset` | text | a plugin program | The plugin's zip in each release (`{version}` and `{tag}` filled in) |
| `sums` | text | see below | Where the sha256 of a download is read: a file in the same release in `sha256sum` format (`SHA256SUMS`), `"github"` (the `digest` GitHub's API lists for each release asset), or an https address |
| `auto` | bool | no | The auto-update switch's starting value (default false) |
| `from` | text | no | **Extension.** `github`, `pypi` or `none`. Left out: `github` when `github` is set, `pypi` when `pypi` is set, else `none` |
| `pypi` | text | from = pypi | **Extension.** The PyPI project (yt-dlp) whose newest version is installed into the `[python]` venv |
| `prerelease` | bool | no | **Extension.** Pre-releases count (llama.cpp marks its builds as pre-releases at times) |
| `tags` | bool | no | **Extension.** With no release that fits, the highest tag that fits counts (the streamrip fork tags versions without releases at times) |
| `pin` | text | no | **Extension.** The tag (or PyPI version) Install puts in: the one this Encore was tested with. Update and the scheduled task move to the newest. A tag must fit `tag` |
| `pin_reports` | text | no | **Extension.** The version the pinned release's version command names, when it differs (the streamrip fork's v2.4.10 tag still says 2.4.9) |

**Finding the newest version** (`from = "github"`): Encore lists the repository's releases (drafts skipped,
pre-releases only with `prerelease`), keeps the ones whose tag fits `tag` with a version in place of
`{version}`, and takes the highest version that has the asset this machine needs (the plugin's zip, or the
`[[binary]]` asset). So a plugin can live in a repository with other releases: the owner's plugin's
`internet-archive-v{version}` tags sit beside Encore's own `v0.31.0`. With `tags` and no fitting release, the
tags are read the same way. `from = "pypi"` reads the project's current version from PyPI.

**Sums** are required for every download Encore unpacks and runs: a plugin program's zip (`[update] sums`),
each `[[binary]]` (its own `sha256` or `sums`, else `[update] sums`). The sha256 is checked before anything is
unpacked; a download that does not match is deleted and the plugin stays as it was. A `[python]` venv (catalog
only) is the exception, as it was in the image: uv fetches the packages from PyPI or the GitHub archive over
https, and the version command and `[[install.check]]` decide whether the result is used.

## `[install]` and `[[install.check]]` (extension)

How a new version of a program is checked before Encore switches to it. For a plugin program without
`[[binary]]` or `[python]`, Encore runs `describe` instead ([the contract](../plugins/CONTRACT.md#describe)).

| Key | Type | Default | Meaning |
|---|---|---|---|
| `verify` | list | `["{bin}", "--version"]` | The version command: an argv starting with `{bin}`; it must exit 0. Cadenza: `["{bin}", "render", "--describe"]` |
| `version_from` | text | `output` | Where the version is read: `output` (the first version number in what it prints) or `json:<field>` (a field of the JSON it prints: Cadenza's `json:version`) |
| `timeout` | number | 60 | Seconds the version command may take (1 to 600) |
| `max_mb` | number | 400 | The largest download, in MB (1 to 16384) |
| `remove` | list | none | Paths removed from what was unpacked (slskd's `wwwroot`: Encore runs it headless) |

`[[install.check]]`: more that must hold. Either `file` and `contains` (a file of the install contains a
text; `file` is relative to the venv for `[python]`, to the unpacked folder for `[[binary]]`; `*` matches
within one name) or `json` and `equals` (a field of the version command's JSON has a value). `message` is
what the install error says when it does not hold. streamrip's check is the image's old one: the fork's
`config.py` must read `[cli] no_update_check` (streamrip.rs reads the same file at run time to decide whether
to write that switch, so the venv keeps the usual `bin/` and `lib/python3.*/site-packages/` layout). Cadenza's
checks that `render --describe` speaks protocol 1.

When a check or the version command fails, the new version is removed and the plugin keeps the version it had
("streamrip v2.5.0 was downloaded but does not work here: … – Encore keeps using 2.4.10").

## `[[binary]]` (extension)

The program for one machine (`std::env::consts::ARCH`): `x86_64` or `aarch64`. The catalog lists both.

| Key | Type | Required | Meaning |
|---|---|---|---|
| `arch` | text | yes | `x86_64` or `aarch64` |
| `asset` | text or list | one of asset, url | A release asset of `[update] github` (`{version}` and `{tag}` filled in). A list: the first one the release has (llama.cpp moved from `.zip` to `.tar.gz`) |
| `url` | text | one of asset, url | A direct https address instead |
| `sha256` | text | one of sha256, sums | The file's sha256, for a URL that names one exact file |
| `sums` | text | one of sha256, sums | As `[update] sums`; left out, `[update] sums` is used |
| `unpack` | text | no | `zip`, `tar.gz` or `file` (the download is the program). Left out: from the name (`.zip`, `.tar.gz`, `.tgz`) |
| `bin` | text | yes | The program: a path inside the archive, or a file name looked for in its first four folder levels (`llama-server`) |

Archives are unpacked with path checks: no absolute names, no `..`, links only to places inside the archive.

## `[python]` (extension)

A venv made with uv and the image's Python, for programs published as Python packages (yt-dlp, streamrip).

| Key | Type | Required | Meaning |
|---|---|---|---|
| `python` | text | yes | The Python the venv is made with (`3.14`): the image's, under /opt/python, which uv finds with `uv venv --python 3.14` (no download) |
| `packages` | list | yes | What `uv pip install` puts in, `{version}` and `{tag}` filled in: `yt-dlp[default]=={version}`, `streamrip @ https://github.com/Stensel8/streamrip/archive/refs/tags/{tag}.zip`. A requirement, never an option (nothing starting with `-`) |
| `bin` | text or list | yes | The program in the venv's `bin` folder; a list: the first one that is there (`["rip", "streamrip"]`) |
| `overrides` | list | no | Requirements that replace what the packages ask for the same project (`uv pip install --override`), `{version}` and `{tag}` filled in. zotify names librespot by a git URL and the image has no git: `librespot @ https://github.com/…/archive/<commit>.zip`. A requirement, never an option |

The venv is made where it stays (`<plugin>/<version>/venv`): a venv's scripts name their own path, so it is
never moved after it is made. The image keeps uv (pinned) and /opt/python at the same path, so an adopted
venv (`tools/venv`, made by 0.31's image) keeps running. A Python plugin is catalog only in 0.32.

## `[[download]]` (extension)

A data file besides the program: the Translator's model.

| Key | Type | Required | Meaning |
|---|---|---|---|
| `url` | text | yes | An https address |
| `sha256` / `sums` | text | one | The file's sha256, or `"huggingface"` (the sha256 Hugging Face lists for the file, its LFS id), `"github"`, a file name or an https address. When Hugging Face's API does not answer, the file is kept only when its first bytes are `magic` |
| `path` | text | yes | Where it goes, relative to the plugin's data folder (`models/Qwen3-8B-Q4_K_M.gguf`) |
| `when` | text | no | Fetched while this rule holds (`translator_model=best`, a setting of the plugin); the files of the other values are removed |
| `max_mb` | number | no | The largest size (1 to 65536) |
| `magic` | text | no | The file's first bytes (`GGUF`) |

Catalog only in 0.32.

## `[[settings]]`

The fields of the plugin's card, in order. The SDK's keys are `key`, `label`, `type` (`select`, `text`,
`number`, `bool`, `password`), `options` (`[value, label]` pairs), `default`, `min`, `max` and `help`.

| Key | Type | Meaning |
|---|---|---|
| `key` | text | a-z, 0-9 and underscores, starting with a letter, at most 64; unique in the plugin |
| `label` | text | The field's name (one line, at most 160 characters). Required for a plugin program |
| `type` | text | `text`, `password`, `number`, `bool`, `select`, `checks`, `textarea`, `path`, `url` (the SDK's five and the settings table's others). Required for a plugin program |
| `options` | list | `[value, label]` pairs of a `select` or `checks`; values without commas, each once |
| `default` | any | Text, a number or true/false, by type; a select's default is one of its values, a number's lies within `min` and `max` |
| `min`, `max` | number | A number's range |
| `help` | text | The line under the field (at most 1000 characters) |
| `when` | text | **Extension.** Shown while another select or switch of the plugin has a value: `key=value` or `key!=value` |
| `unit`, `step` | text, number | **Extension.** A number's unit ("seconds") and step |
| `full` | bool | **Extension.** The field takes the card's whole width |
| `advanced` | bool | **Extension.** For people who know what they are doing (the field says so) |
| `part` | text | **Extension.** A heading over this field and the ones after it ("YouTube sign-in" on the yt-dlp card) |

A `password` is a secret: never sent to a browser (only "set" or "not set"), sent to the plugin on stdin with
the other settings, never in its argv or environment.

**Where the values live.** A plugin program's keys are its own: its `format` is kept in Encore's settings as
`plugin.<id>.format` and sent to it as `settings.format`, so two plugins may both have a `format`.
**A builtin plugin's keys are Encore's**: its `key` is the name of a setting in
`server/crates/config/settings.toml`, kept verbatim (`streamrip_deezer_arl`, `slskd_download_path`), so every
saved value and every environment variable (`STREAMRIP_DEEZER_ARL`, which then locks the field) keeps
working, and nothing is renamed. For such a key:

- when the settings table gives it a field (it has a `group`), the manifest names only the key (and maybe a
  `part`): the label, help, type, options, `when` rule, environment lock and secret flag come from the
  settings table, the one place the words live (docs/contracts/settings-schema.md);
- when it has no field there (the Translator's `translator_model` and `translate_background`, which the old
  Download tools card drew), the manifest gives the field's words, and its default is the table's;
- a key with neither is internal (`streamrip_sources`, `slskd_managed_api_key`, `plex_client_id`,
  `cadenza_engine_settings`): the plugin owns it, the card does not draw it.

A setting of the old torrent and Usenet card keeps its `when` rule (`torrent_client=qbittorrent`) in the
settings table for the clients' cards; on a client's own card the rule names a select of another plugin
(torznab's), so the card is shown as a whole for the chosen client rather than field by field.

## `[permissions]`

What the plugin's processes may reach. Encore runs them in the sandbox it finds at start (full isolation with
bubblewrap, or limits only: [the contract](../plugins/CONTRACT.md#the-sandbox)).

| Key | Type | Meaning |
|---|---|---|
| `network` | list or bool | The hosts it talks to (`["archive.org", "*.archive.org"]`), `true` or `false` (default). Any list means the network is on: the hosts are shown on the card, Encore does not filter by host |
| `filesystem` | text | `"folder"` (the only value, and the default): its own folder, plus the request's download folder while it downloads |
| `paths` | list | **Extension.** More folders: `{ setting = "library_path", access = "read" }` (a folder a setting names) or, catalog only, `{ config = "slskd", access = "write" }` (a folder under the config folder). An upload or a URL plugin may only ask to read the library, and its card says so |
| `memory` | number | **Extension.** Address space in MB (64 to 65536; 0: no limit; default 2048 for a plugin program). .NET programs reserve far more than they use: slskd has 0 |
| `cpu` | number | **Extension.** Processor seconds per run (0: no limit; default the command's timeout) |
| `processes` | number | **Extension.** Processes it may start (0: no limit; default 64) |
| `open_files` | number | **Extension.** Open files (0: no limit; default 1024) |

## `[client]` (extension)

A download client (`kind = "client"`): the source that hands it downloads and the setting value that makes
it the one in use. The clients of the torznab source answer `status` and `test` (the client's own check, as
`dl::Client::status` does today).

| Key | Meaning |
|---|---|
| `source` | The source plugin's id (`torznab`) |
| `protocol` | `torrent` or `usenet` |
| `chosen_by` | `key=value` of a select of that source: `torrent_client=qbittorrent`, `nzb_client=blackhole` |

## `[migration]` (extension, catalog only)

What a catalog plugin takes over from Encore before plugins, read once on the first start of 0.32 (schema 28).

| Key | Meaning |
|---|---|
| `from_tool` | Its id on the old Download tools card: `ytdlp`, `streamrip`, `cadenza`, `slskd`, `translator`. `/api/system/tools*` answers from this plugin for one more release, in the old shape |
| `auto_key` | The old auto-update switch (`tools_auto_<id>`): its value becomes the plugin's auto-update |
| `adopt` | Folders and files under the config folder it uses where they are: nothing moves, so the previous image still runs on them (`tools/venv`, `tools/slskd`, `slskd`, `tools/cadenza`, `tools/translator`, `streamrip`) |
| `in_use` | When it was in use: a list of condition lists; the plugin was in use when every condition of any one list holds |

On an upgrade, every catalog plugin that was in use is adopted, enabled and, where its program is missing,
installed, so downloads keep working with no new setup; on a fresh install every plugin is "available, not
installed".

**Conditions** (`in_use`; `when` and `chosen_by` use the `key=value` and `key!=value` forms):

| Form | Holds when |
|---|---|
| `key` | The setting is on: true, a number other than 0, text that is not empty |
| `!key` | It is not |
| `key=value`, `key!=value` | It has (not) that value; case does not matter |
| `key~item` | Its comma-separated list names `item` (`source_order~deezer`) |
| `saved:key` | The owner set it (a saved value or an environment variable, not the default) and it is on |
| `file:path` | The file or folder exists under the config folder |

What the catalog counts as in use (the contract's rule: a tool whose program or venv is there, a source set
up with its settings, or an auto-update switch the owner turned on):

| Plugin | In use when |
|---|---|
| ytdlp | always: every image before plugins shipped yt-dlp and every pasted link used it, whatever `ytdlp_enabled` said (`tools/venv/bin/yt-dlp`, `ytdlp_enabled`, `!ytdlp_enabled`) |
| streamrip | `tools/venv/bin/rip` is there, or `streamrip_enabled` with a Deezer ARL, a Qobuz token or a streamrip config (the Tidal sign-in), or `saved:tools_auto_streamrip` |
| slskd | `tools/slskd/current` is there, or `slskd_enabled` with a username and password, or `saved:tools_auto_slskd` |
| torznab | `torznab_enabled` with the Prowlarr address and key |
| a torrent or Usenet client | `torznab_enabled`, the client chosen as Encore picks it (`torrent_client` / `nzb_client`: any value that names no other client is qBittorrent / SABnzbd, `rutorrent` is rTorrent), and its address set (a watch folder has a default) |
| cadenza | `tools/cadenza/current` is there, or `cadenza_server_enabled`, or `saved:tools_auto_cadenza` |
| translator | its program (`tools/translator/llama/current`) and its models folder are there: a model is gigabytes, so the switch alone fetches none |
| jellyfin, plex, navidrome | the server's address is set |

## Origins

| | Catalog (first-party) | Upload or URL (third-party) |
|---|---|---|
| `builtin`, `settings_on`, `[migration]` | yes | refused |
| `[python]`, `[[download]]`, `from = "pypi"` | yes | refused in 0.32 (bring the code in the zip) |
| `[[binary]]` | both machines, `sha256` or `sums` | a fixed `sha256` each |
| `[permissions] paths` | any setting or config folder | reading `library_path` only |
| `[run] env` | any | not `PATH`, `HOME`, `TMPDIR`, `PYTHONPATH`, `PYTHONHOME`, `NODE_OPTIONS`, `LD_*`, `ENCORE_*` |
| sandbox | the level found at start | the level found at start, the strictest settings, a warning on the card |

An upload is a zip of at most 200 MB with `plugin.toml` at its root (or in its one top folder); a URL install
takes a zip, a raw `plugin.toml` whose `[update]` names the zip, or `owner/repo@ref` (GitHub's archive zip of
that ref, so the image needs no git).

## Unique across plugins

Across the catalog and every installed plugin: plugin ids; source ids and picker ids (an upload cannot
answer for `deezer`); setting keys as they are kept (a builtin plugin's key, `plugin.<id>.<key>` for the
others); the old tool and auto-update switch each plugin takes over; the select value each client is chosen
by. A setting key belongs to one plugin.

## On disk

```
<config>/plugins/<id>/
  plugin.toml       the manifest of the installed version (the plugins table keeps it too, with its sha256)
  current           the installed version: a text file naming the version folder (switched in one rename)
  <version>/        what an install put in: the unpacked zip, the program, the venv (made in place)
  data/             ENCORE_PLUGIN_DATA: kept across updates, removed with Remove
  logs/             plugin.log and its rotated copies
  home/, tmp/       the private HOME and TMPDIR of its processes
```

A version that does not pass its checks is removed before `current` moves; after a switch the older versions
are removed. An adopted plugin keeps its old folder (`[migration] adopt`) until an update or Remove: an update
installs into `<config>/plugins/<id>/`, and the old folder is left for the previous image.

## From the plan's field list

The plan (docs/backlog/2026-10-plan.md, 0.32) listed `[runtime]`, `[[binary]]`, `[python]`, `[updates]`,
`[[setting]]`, `[commands]`, `[permissions]` and `[migration]`. With the owner's SDK as the format they are:

| Plan | Here |
|---|---|
| `[plugin]` id, name, version, description, kind, min_encore | `[plugin]` (kind adds `client`; `min_encore` is an extension) |
| `[runtime]` builtin, binary, python or script; argv; env allow-list; no shell | `[plugin] builtin`; `[[binary]]`; `[python]`; `[run]` script and interpreter, `[run] env`; `[run] shell` absent or false |
| `[[binary]]` per arch: url, sha256 or sums_url, unpack, bin, verify argv | `[[binary]]` (`asset` or `url`, `sha256` or `sums`, `unpack`, `bin`) and `[install] verify` |
| `[python]` pip spec | `[python] packages`, installed with uv |
| `[updates]` github_releases, github_tags, pypi, huggingface | `[update]` `github` (+ `tags`, `prerelease`), `pypi`; Hugging Face files are `[[download]]` with `sums = "huggingface"` |
| `[[setting]]` keeping today's keys | `[[settings]]` (the SDK's name); a builtin plugin's keys are Encore's |
| `[commands]` | `[plugin] commands`, `[run.timeouts]` |
| `[permissions]` network, paths, memory, cpu, timeout, secrets on stdin | `[permissions]` network, filesystem, paths, memory, cpu, processes, open_files; timeouts in `[run.timeouts]`; secrets always on stdin |
| `[migration]` from_tool | `[migration]` from_tool, auto_key, adopt, in_use |

## The catalog

`server/crates/plugins/catalog/<id>.toml`, embedded in the server. All are Encore's own code
(`builtin = true`); seven install a program (zotify and gamdl since 0.34).

| id | kind | Program it installs | From | Sum | Adopts |
|---|---|---|---|---|---|
| `ytdlp` | source | yt-dlp, venv | PyPI `yt-dlp` | none of Encore's (https; the version command) | `tools/venv` |
| `streamrip` | source (qobuz, tidal, deezer) | streamrip fork, venv; Install pins v2.4.10 | GitHub `Stensel8/streamrip` releases or tags | none of Encore's (https; the version command and the `no_update_check` check) | `tools/venv`, `streamrip` |
| `slskd` | source | slskd | GitHub `slskd/slskd` releases | GitHub's asset digest | `tools/slskd`, `slskd` |
| `zotify` | source (Spotify) | zotify fork, venv (librespot through `[python] overrides`); Install pins v0.21.0 | GitHub `Googolplexed0/zotify` tags | none of Encore's (https; the version command) | |
| `gamdl` | source (Apple Music) | gamdl, venv; Install pins 3.9.1 | PyPI `gamdl` | none of Encore's (https; the version command) | |
| `torznab` | source | none | | | |
| `qbittorrent`, `transmission`, `deluge`, `rtorrent`, `torrent-blackhole` | client (torrent) | none | | | |
| `sabnzbd`, `nzbget`, `usenet-blackhole` | client (usenet) | none | | | |
| `cadenza` | playback | `cadenza` | GitHub `Krifmaz/Cadenza` releases | `SHA256SUMS` | `tools/cadenza` |
| `translator` | tool | llama.cpp CPU build and a model | GitHub `ggml-org/llama.cpp` releases (pre-releases count); Hugging Face | GitHub's asset digest; Hugging Face's sha256 | `tools/translator` |
| `jellyfin`, `plex`, `navidrome` | connection | none | | | |

The settings each one owns (every key of the old cards streamrip, torznab, slskd, ytdlp, youtube, media and
cadenza belongs to exactly one plugin; the catalog test checks it):

- `ytdlp`: `ytdlp_enabled`, `ytdlp_last_resort`, `ytdlp_format`, `ytdlp_quality`, `ytdlp_cookies`,
  `youtube_light_refresh` (the YouTube sign-in card becomes a part of the yt-dlp card);
- `streamrip`: `streamrip_enabled`, `streamrip_quality`, `streamrip_deezer_arl`, `streamrip_qobuz_user`,
  `streamrip_qobuz_token`, `streamrip_bin`, `streamrip_command`, `streamrip_sources` (internal);
- `zotify`: `zotify_client_id`, `zotify_sign_in_link`, `zotify_format`, `zotify_quality`, `zotify_lyrics_file`,
  `zotify_bulk_wait`, `zotify_real_time`, `zotify_language` (in the settings table without a card: their words
  are in the manifest, as the Translator's);
- `gamdl`: `gamdl_media_user_token`, `gamdl_codec`, `gamdl_wrapper_url`, `gamdl_wrapper_decrypt`, `gamdl_storefront`,
  `gamdl_lyrics`, `gamdl_language` (the same way);
- `slskd`: `slskd_enabled`, `soulseek_username`, `soulseek_password`, `soulseek_port`, `soulseek_share`,
  `soulseek_upload_slots`, `slskd_download_path`, `slskd_preferred_format`, `slskd_min_bitrate`,
  `slskd_search_timeout`, `slskd_stall_minutes`, `slskd_max_candidates`, `vpn_enabled`,
  `vpn_wireguard_config`, `vpn_port_forwarding`, `vpn_kill_switch`, `slskd_managed_api_key` (internal);
- `torznab`: `torznab_enabled`, `prowlarr_url`, `prowlarr_api_key`, `torznab_min_seeders`,
  `torznab_timeout_minutes`, `torrent_client`, `nzb_client`;
- the clients: `qbit_*`, `transmission_*`, `deluge_*`, `rtorrent_*`, `torrent_blackhole_*`, `sab_*`,
  `nzbget_*`, `nzb_blackhole_*`, each with its client;
- `cadenza`: `cadenza_server_enabled`, `cadenza_max_streams`, `cadenza_max_rate`, `cadenza_max_taps`,
  `cadenza_cache_mb`, `cadenza_engine_settings` (internal; the fields stay on Server processing);
- `translator`: `translator_model`, `translate_background` (their words are in the manifest);
- `jellyfin`: `jellyfin_url`, `jellyfin_api_key`; `plex`: `plex_url`, `plex_token`, `plex_client_id`
  (internal); `navidrome`: `navidrome_url`, `navidrome_user`, `navidrome_password` (the fields stay on
  Integrations → Media servers).

The auto-update switches `tools_auto_ytdlp`, `tools_auto_streamrip`, `tools_auto_slskd`, `tools_auto_cadenza`
and `tools_auto_translator` are taken over by `[migration] auto_key`. The Internet Archive plugin is not in the
catalog: it installs through Upload plugin.

## New setting keys

Keys a catalog plugin adds that `settings.toml` does not have. Each one is listed here as `- `key`` with what
it does, and its manifest entry has a label and a type; the catalog test checks both ways.

None: every catalog setting is in `settings.toml` (zotify's and gamdl's since 0.34, without a card).

## Example: yt-dlp (catalog)

`server/crates/plugins/catalog/ytdlp.toml` as it is (the catalog test compares the two):

```toml
# yt-dlp: YouTube Music for requests, and links from YouTube, SoundCloud, Bandcamp and about 1,800 other sites.
# Encore's own code (sources/ytdlp.rs) runs it; the plugin installs yt-dlp into a venv of its own.
# docs/contracts/plugin-manifest.md describes every key.

[plugin]
id = "ytdlp"
name = "yt-dlp"
version = "0.32.0"
description = "Last-resort source: finds the song on YouTube Music. yt-dlp also downloads SoundCloud links you paste in the Downloader, along with YouTube, Bandcamp, Mixcloud and about 1,800 other sites."
kind = "source"
protocol = 1
homepage = "https://github.com/yt-dlp/yt-dlp"
license = "Unlicense"
commands = ["status", "test", "search", "download", "update"]
kinds = ["track", "album", "url"]
lossless = false
needs_sign_in = false
builtin = true

[[sources]]
id = "ytdlp"
name = "YouTube Music"
picker = "ytmusic"

[run.timeouts]
search = 60
download = 3600
test = 60

# The newest yt-dlp on PyPI; sites change often, so it updates by itself unless switched off.
[update]
from = "pypi"
pypi = "yt-dlp"
auto = true

[python]
python = "3.14"
packages = ["yt-dlp[default]=={version}"]
bin = "yt-dlp"

[install]
verify = ["{bin}", "--version"]

[[settings]]
key = "ytdlp_enabled"

[[settings]]
key = "ytdlp_last_resort"

[[settings]]
key = "ytdlp_format"

[[settings]]
key = "ytdlp_quality"

[[settings]]
key = "ytdlp_cookies"
part = "YouTube sign-in"

[[settings]]
key = "youtube_light_refresh"

[permissions]
network = true
filesystem = "folder"

[migration]
from_tool = "ytdlp"
auto_key = "tools_auto_ytdlp"
adopt = ["tools/venv"]
# Every image before plugins shipped yt-dlp and every pasted link went through it, whatever YouTube Music's
# switch said: an upgrade always adopts it (the rule holds with ytdlp_enabled on and with it off).
in_use = [["file:tools/venv/bin/yt-dlp"], ["ytdlp_enabled"], ["!ytdlp_enabled"]]
```

## Example: the example tone plugin (a plugin program)

The example plugin of docs/plugins/ (`docs/plugins/example-tone/plugin.toml` as it is): a harmless source
written with the SDK that searches a short built-in list and downloads a generated sine tone as FLAC. It reads
as an upload; [docs/plugins/README.md](../plugins/README.md) walks through it.

```toml
# Example tone: a source for plugin authors to copy. docs/contracts/plugin-manifest.md describes every key.

[plugin]
id = "example-tone"
name = "Example tone"
version = "0.1.0"
description = "An example source for plugin authors: searches a short built-in list of tones and downloads the one picked as a generated sine-tone FLAC. No network, no account."
kind = "source"
protocol = 1
homepage = "https://github.com/Krifmaz/Encore/tree/main/docs/plugins/example-tone"
license = "MIT"
commands = ["describe", "status", "test", "search", "download"]
kinds = ["track", "album"]
lossless = true
needs_sign_in = false
min_encore = "0.32.0"

[run]
script = "plugin.py"
interpreter = "python3"

# Seconds per command. Writing a 10-minute tone at 96 kHz takes Python a while on a small server.
[run.timeouts]
search = 10
download = 1800
test = 10

[[settings]]
key = "seconds"
label = "Tone length"
type = "number"
default = 30
min = 1
max = 600
unit = "seconds"
help = "How long each downloaded tone lasts. A song Encore knows the length of gets a tone of that length."

[[settings]]
key = "rate"
label = "Sample rate"
type = "select"
options = [["44100", "44.1 kHz"], ["48000", "48 kHz"], ["96000", "96 kHz"]]
default = "44100"
help = "The sample rate of the FLAC files it makes."

[permissions]
network = false
filesystem = "folder"
```
