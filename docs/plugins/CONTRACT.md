# Plugin contract (protocol 1)

How Encore talks to a plugin program: what it starts, what it writes on stdin, what it reads back on stdout
and stderr, and how it stops it. This is protocol 1, as the plugin SDK implements it
([plugins/sdk/encore_plugin.py](../../plugins/sdk/encore_plugin.py), one file, Python 3.11 or newer, no other
packages). Written for Encore 0.32.0.

- Start here if you are writing a plugin: [README.md](README.md) (the walk-through with the example plugin).
- The manifest every plugin carries, `plugin.toml`: [docs/contracts/plugin-manifest.md](../contracts/plugin-manifest.md).
- What a plugin can see and reach: [SANDBOX.md](SANDBOX.md).
- A local runner that speaks this contract to a plugin folder:
  [plugins/tools/plugin-check.py](../../plugins/tools/plugin-check.py) ([below](#checking-a-plugin-locally)).
- An example plugin written with the SDK: [example-tone/](example-tone/).

The catalog's plugins (yt-dlp, streamrip, slskd, the torrent and Usenet clients, Cadenza, the Translator and
the media servers) are Encore's own code and answer the same commands in-process; this contract is what an
uploaded plugin, or one installed from a URL, speaks as a separate program.

## One command, one process

Encore starts the plugin once per command and waits for it to end:

```
<interpreter> <script> <command>          python3 plugin.py search
```

`interpreter` and `script` come from `[run]` in the manifest (no interpreter: the script is started itself,
and Encore makes it executable at install). `python3` is the image's Python 3.14 under `/opt/python` (else
`python3` on `PATH`); `node` and `sh` are the system's. Nothing else is on the command line: no settings, no
secrets, no paths. Nothing goes through a shell.

| Channel | What goes there |
|---|---|
| stdin | One JSON object (UTF-8) with the request, then end of file. `describe` gets an empty stdin |
| stdout | One JSON object on one line: the answer. The last line of stdout that is a JSON object counts; anything a plugin prints before it is ignored (print to stderr instead). At most 16 MB |
| stderr | Lines for the plugin's log and the progress bar ([Logs and progress](#logs-and-progress)) |
| exit status | 0 after an answer. An answer with `"ok": false` is the plugin's error whatever the status (the SDK exits 2 for an unsupported command, 130 when cancelled, 1 after an unexpected exception). No answer at all: Encore reports an `internal` error with the last line of stderr |

The working folder is the plugin's data folder. Encore may run several commands of one plugin at the same
time (a picker search while a download runs), each in its own process: keep writes to the data folder atomic
(write a temporary file, then rename it).

**Time.** Every command has a time limit, `[run.timeouts]` in the manifest, else: `describe` 10 s, `status`
15 s, `test` 30 s, `search` 60 s, `download` 3600 s, `update` 600 s. A plugin that runs over is stopped as
below and the error says "did not answer within 60 seconds". Keep each limit longer than the plugin's own
network timeouts and retries together, so a slow service ends in the plugin's own `network` error rather than
in Encore's time limit. The processor time limit is the same number of seconds unless `[permissions] cpu`
says otherwise.

**Stopping.** The plugin runs in a session (process group) of its own. To stop it (a cancelled request, a
time limit, Encore shutting down, Disable or Remove) Encore sends SIGTERM to the plugin's processes (at full
isolation the ones inside bubblewrap, so bubblewrap itself stays to pass on their exit), waits up to 10
seconds, then sends SIGKILL to the whole group. Processes a plugin leaves behind after it answers are
killed too. The SDK turns SIGTERM into its `Cancelled` exception: a running `download_file` removes its
`.part` file, the plugin answers `{"ok": false, "error": "cancelled", "code": "cancelled"}` and exits with
130. Encore removes the request's download folder after a cancelled or failed download.

## Environment

The environment is cleared. Encore sets:

| Variable | Value |
|---|---|
| `ENCORE_PLUGIN_DATA` | The plugin's data folder, `<config>/plugins/<id>/data`: caches, tokens, cookies. Kept across updates, removed with Remove. The SDK's `data_dir()` reads it (and falls back to `data/` beside the script when the plugin runs on its own) |
| `ENCORE_PLUGIN_ID`, `ENCORE_PLUGIN_VERSION` | The manifest's `id` and `version` |
| `ENCORE_VERSION`, `ENCORE_PROTOCOL` | This Encore's version and `1` |
| `ENCORE_PLUGIN_BIN` | The installed program, when the manifest installs one (`[[binary]]`) |
| `HOME`, `TMPDIR`, `XDG_CONFIG_HOME`, `XDG_CACHE_HOME`, `XDG_DATA_HOME` | Private folders under the plugin's folder (`home/`, `tmp/`) |
| `PATH` | `/usr/local/bin:/usr/bin:/bin` |
| `LANG`, `TZ` | `C.UTF-8` and Encore's time zone (`UTC` when none is set) |
| `PYTHONUNBUFFERED`, `PYTHONDONTWRITEBYTECODE`, `PYTHONIOENCODING` | `1`, `1`, `utf-8` |
| `HTTP_PROXY`, `HTTPS_PROXY`, `NO_PROXY` (and lower case), `SSL_CERT_FILE`, `SSL_CERT_DIR` | Encore's, when it has them and the plugin has the network |

and the manifest's `[run] env` (`{bin}`, `{bin_dir}`, `{data}` and `{plugin}` filled in; an uploaded plugin
may not set `PATH`, `HOME`, `TMPDIR`, `PYTHONPATH`, `PYTHONHOME`, `NODE_OPTIONS` or any `LD_*` or `ENCORE_*`
variable). Nothing else of Encore's environment reaches a plugin: no settings, no secrets, no other variables.

## The sandbox

Encore finds out once after each start which level the host allows and shows it on every plugin's card:

- **Full isolation** (bubblewrap works here): the plugin runs in namespaces of its own, sees the system
  folders read-only, its own folders, the download folder while it downloads, the library read-only only when
  its manifest asks, and the network only when its manifest asks. Not the config folder, the database or the
  other plugins.
- **Limits only** (bubblewrap is refused, as in a Docker container without the compose option): resource
  limits, no new privileges, a session of its own, low priority, the cleared environment above. This level
  does not hide files: the plugin runs as Encore's user and can read what Encore can.

[SANDBOX.md](SANDBOX.md) has the details: what each level hides, how the level is found, and what a Docker
host needs for full isolation. An uploaded plugin, or one installed from a URL, is third-party: its card
carries a warning that says what its level means. ffmpeg, the tagger and the import stay Encore's: a plugin
only delivers files.

## Commands

| Command | When Encore runs it | Request | Answer |
|---|---|---|---|
| `describe` | at install and after every update | none | who the plugin is |
| `status` | the card and the picker's source line | settings | a line about its state |
| `test` | the card's Test button | settings | a line about what was checked |
| `search` | the picker and the request worker (a source) | the request | results |
| `download` | a pick, or the worker's best result (a source) | the request and the result picked | files |
| `update` | Update on the card, the scheduled "Update plugins" task | settings, version | a line |

A plugin lists the commands it answers in its manifest (`commands`) and in `describe`. The SDK answers a
command it has no handler for with `unsupported`.

### describe

No request. The answer (the example plugin's):

```json
{"ok": true, "name": "example-tone", "version": "0.1.0", "protocol": 1, "kind": "source",
 "kinds": ["track", "album"], "commands": ["describe", "status", "test", "search", "download"], "needs_sign_in": false}
```

`name` is the plugin's **id** (the SDK's `run("example-tone", ...)`). Encore checks it against the manifest
at install and after every update, running it from the new version's folder in the sandbox before anything
switches: `protocol` is 1, `name` is the manifest's `id`, `version` its `version`, `kind` its `kind`, and every
command the manifest lists is in `commands` (a manifest that promises more than the program answers is
refused). A plugin that fails `describe` is not installed, and an update that fails it leaves the version in
use as it was.

### status

Request: `{"settings": {...}}`. Answer: `{"ok": true, "message": "Ready · 7 tones in 2 albums",
"signed_in": true | false | null}`. Shown on the card and as the picker's source state (`signed_in: false`
shows "sign in"). Keep it quick and, when possible, off the network.

### test

Request: `{"settings": {...}}`. Answer: `{"ok": true, "message": "Signed in as … · the service answered in
120 ms"}`, or an error (`auth` when the sign-in is refused, `network` when the service does not answer). The
message is shown as it is.

### search

The request:

| Field | Type | Meaning |
|---|---|---|
| `kind` | text | `track` or `album` (one of the manifest's `kinds`; Encore sends no other). `url` (a pasted link) is part of the protocol, but Encore 0.32 sends links to yt-dlp and streamrip, not to plugins |
| `source` | text | The source id it is asked as (the plugin's id, or one of its `[[sources]]`) |
| `artist`, `title`, `album` | text | The request's names; left out when unknown |
| `query` | text | What someone typed in the picker; when set, search by it as typed (the names may be absent) |
| `length` | number | A song's length in seconds, when known |
| `track_count` | number | An album's number of songs, when known |
| `lossless_only` | bool | Only lossless results are wanted |
| `limit` | number | The most results wanted (25 when not given, at most 200) |
| `isrc` | text | A song's ISRC, when known |
| `url` | text | `kind = "url"`: the pasted link (not sent by Encore 0.32) |
| `settings` | object | The plugin's settings |

The answer is `{"ok": true, "results": [...]}`, best first; no results is `{"ok": true, "results": []}`, not
an error. Each result has the shape of the SDK's `result()`:

| Field | Type | Meaning |
|---|---|---|
| `key` | text | Unique and stable for this result (`tuning-fork/1`): Encore keeps picks and blocks by it |
| `title`, `artist`, `album` | text | What the result is |
| `subtitle` | text | The line under the title (the SDK joins artist and album) |
| `format` | text or null | The file type in lower case (`flac`, `mp3`, `m4a`, `opus`) |
| `bitrate` | number | kbps, 0 when unknown |
| `bit_depth`, `sample_rate` | number | Bits and Hz, 0 when unknown |
| `quality` | text | The label shown ("FLAC 24-bit/96kHz", "MP3 320k"; the SDK's `quality()`) |
| `lossless` | bool | Every file is lossless |
| `tracks` | number or null | The number of files a pick downloads (1 for a song) |
| `size` | number or null | Bytes in all |
| `duration` | number or null | Seconds in all |
| `isrc` | text or null | A song's ISRC |
| `cover` | text or null | A cover image's address (shown only when it is http or https) |
| `info` | text | One more line (licence, collection, downloads) |
| `score` | number or null | 0 to 100: how well it fits the request; Encore weighs it beside its own checks |
| `url` | text or null | A page the person can open (only http or https) |
| `payload` | object | Anything the plugin needs to download it; given back as it is in `download` (at most 64 KB) |

Encore leaves out a result without a `key` or `title`, a second result with the same `key`, a score outside 0
to 100 and a payload over 64 KB, and keeps at most `limit`. Titles and other text are shown as text, never as
HTML. For a request (not a pick), the worker takes the best result whose title and artist match what was asked
(and length, when both know it), so a result's names should be the real ones.

### download

The request: the search request's fields (`kind`, `artist`, `title`, `album`, `length` and the others, as the
request has them), `key` and `payload` of the result picked, its `url` when it had one, `folder` (an absolute
path: an empty folder Encore made for this download, `<staging>/request-<n>/plugin-<source>/`, the only place
the plugin may write besides its own folders) and `settings`.

The answer, in the shape of the SDK's `file_entry()`:

```json
{"ok": true,
 "files": [{"path": "/downloads/encore/request-12/plugin-example-tone/01 Concert A.flac", "title": "Concert A",
            "artist": "The Example Tones", "album": "Tuning Fork", "track": 1, "disc": 1, "duration": 30.0,
            "format": "flac", "size": 2650039, "isrc": null}],
 "message": "Wrote 1 tone of 30 s at 44.1 kHz"}
```

- `path` is absolute (the SDK makes it so) or relative to `folder`. Encore checks every file: inside
  `folder` after following links (a link out of it is refused), a regular file, an audio file by its
  extension (the SDK's `AUDIO_EXT`: flac, mp3, m4a, alac, ogg, opus, wav, aiff, aif, ape, wv, aac, webm), not a
  `.part` file. A download with no file is an error.
- The tags are hints: Encore tags, checks (AcoustID, the audio check: fake lossless, upsampled, wrong length)
  and imports the files as it does for every source.
- `cover_url`, `source_url` and `message` are optional: a cover when the files have none, the page the files
  came from (kept with the request), a line for the request's history.
- Write each file as `<name>.part` and rename it when it is whole (the SDK's `download_file` does, and checks
  the size and an MD5 when it knows them), so a stopped download leaves no half file under a real name.
- Report progress while it works ([below](#logs-and-progress)).

### update

Optional. Request: `{"settings": {...}, "version": "0.1.0"}`. Answer: `{"ok": true, "message": "...",
"changed": true | false}`. For a plugin that keeps something of its own up to date in its data folder (a list,
a helper it downloads): Encore runs it after each install or update of the plugin (Install, Update on the
card, the scheduled task). New versions of the plugin itself come from `[update]` in its manifest, and Encore
installs those, never the plugin.

## Errors

```json
{"ok": false, "error": "the service is rate-limiting this server (HTTP 429)", "code": "rate_limited", "retry_after": 30}
```

`error` is shown as it is: plain words, no stack trace (that goes to stderr). `code` is one of:

| Code | Means | What Encore does |
|---|---|---|
| `auth` | A sign-in is missing or refused | The card and the picker say to sign in; a request moves on to the next source |
| `not_found` | The item is not there (any more) | A request moves on; a pick says the result is gone |
| `network` | The service could not be reached or answered badly | A request moves on and tries again on its next attempt |
| `rate_limited` | The service asks to slow down | No new command but `describe` and `status` for `retry_after` seconds (60 when not given, at most a day); a request moves on |
| `unsupported` | This command or request kind is not supported | Not asked again for that kind until another version is installed or Encore restarts |
| `cancelled` | Stopped by Encore | Nothing more |
| `internal` | Anything else (a bug) | Shown with the plugin's log; a request moves on |

An unknown code counts as `internal`. The SDK's `PluginError(message, code, retry_after)` makes this answer,
and its `http()` turns HTTP 401/403, 404, 429 and connection errors into `auth`, `not_found`,
`rate_limited` and `network`.

## Logs and progress

stderr is read line by line:

| Line | Meaning |
|---|---|
| `progress <0-100> [text]` | The progress bar of the download (`progress 42.0 03 Song.flac`) and what is happening |
| `log <text>` | A line for the plugin's log |
| anything else | A line for the log as it is (a traceback) |

Encore keeps the log in `<config>/plugins/<id>/logs/plugin.log` (rotated at 1 MB, three old files kept,
control characters dropped, lines cut at 4000 characters), shows the last lines in the card's Logs drawer,
and uses the last line as the reason when a plugin ends without an answer. Report progress about once a
second (the SDK's `download_file` does). Never write a setting's value to the log.

## Settings and secrets

Every request but `describe` carries `settings`: the values saved on the plugin's card, the manifest's
`default` for a field never saved. Encore keeps a plugin's values in its own settings, as
`plugin.<id>.<key>`, checks each against its field when it is saved (a number within `min` and `max`, a
select's value one of its options), and deletes them with Remove. A `password` field's value is in there too:
secrets reach a plugin on stdin only, never in its argv or environment, and the card shows them only as saved
after they are saved. The SDK's `setting(req, key, default)` reads one (empty text counts as unset).

## Packaging and releases

A plugin is a zip of at most 200 MB: `plugin.toml` (at most 64 KB) at its root or in its one top folder, the
script, and `encore_plugin.py` beside it (a copy of the SDK file). Names are checked when it is unpacked: no
absolute names, no `..`, no links that point outside the plugin, no devices. Install it with **Upload plugin**
on the Plugins page, or **Install from URL** with:

- the https address of the zip;
- the https address of a raw `plugin.toml` whose `[update]` names a GitHub release with the zip and its sums
  (Encore installs the newest one);
- `owner/repo@ref`, which Encore fetches as GitHub's archive zip of that branch, tag or commit (no git needed),
  or `owner/repo/sub/folder@ref` for a plugin in a folder of a repository:
  `Krifmaz/Encore/docs/plugins/example-tone@main`.

Refused: an id of a catalog plugin (`ytdlp`, `slskd`, …), the ids `check`, `upload` and `install-url` (words of
the Plugins page), an id, source id or setting that another plugin already has, a `min_encore` newer than this
Encore, and anything the manifest rules for uploads refuse. Installing a plugin with the id of one already
installed is an update of it: its settings and its on/off switch are kept.

For updates, publish each version as a GitHub release whose tag fits `[update] tag`, with the zip as the
`[update] asset` and a `SHA256SUMS` file listing it (`sha256sum *.zip > SHA256SUMS`). Encore lists the
repository's 50 newest releases, keeps the ones whose tag fits, checks the zip's sha256 before it unpacks
anything, runs `describe`, and switches to the new version only when that passes. A repository that also
publishes an app (as Encore's own does) should publish plugin releases with `gh release create --latest=false`:
apps that ask GitHub for the repository's latest release would otherwise take the plugin's release for theirs.

## Checking a plugin locally

```bash
python3 plugins/tools/plugin-check.py path/to/plugin
python3 plugins/tools/plugin-check.py path/to/plugin --track "Artist|Song|Album"
python3 plugins/tools/plugin-check.py path/to/plugin --album "Artist|Album" --download 1 --folder /tmp/encore-check
python3 plugins/tools/plugin-check.py path/to/plugin --query "typed words" --set format=mp3 --only search
```

It reads the manifest (and says what Encore would refuse in it), then runs the plugin the way Encore does:
the same argv, a cleared environment with `ENCORE_PLUGIN_DATA`, the request on stdin, the time limits of
the manifest, SIGTERM and then SIGKILL to its process group when one runs over. It runs `describe` (and
compares it with the manifest), `status` and `test`; `--track`, `--album`, `--query` or `--url` add a search
and print the results; `--download N` downloads result N into `--folder` (a new temporary folder when not
given) and checks the files as Encore does. `--set key=value` sets a setting (the manifest's defaults
otherwise), `--data DIR` puts the data folder somewhere else than `data/` in the plugin's folder, `--only`
picks the commands, `--json` prints the raw answers. It exits with 0 when every command it ran answered ok
and as the contract says, 1 otherwise (an error answer, a broken answer, a manifest Encore would refuse). It
needs Python 3.11 or newer and nothing else. It does not use the sandbox: run plugins you did not write in a
throwaway container or virtual machine.

## The SDK

`plugins/sdk/encore_plugin.py` is the owner's SDK file, unchanged:

| Name | What it does |
|---|---|
| `run(name, version, handlers=..., kind=..., kinds=..., needs_sign_in=...)` | The entry point: reads the command and the request, calls the handler, writes the answer, answers `describe` by itself |
| `PluginError(message, code, retry_after)`, `Cancelled` | An error the person reads; the cancel |
| `progress(percent, text)`, `log(text)` | The stderr lines above |
| `setting(req, key, default)`, `data_dir()` | A setting's value; the data folder |
| `http()`, `http_json()`, `http_text()` | Fetches with retries, errors turned into codes |
| `download_file(url, dest, expected_size, md5, on_progress)` | A download through `.part` with size and MD5 checks |
| `result(...)`, `file_entry(...)` | A search result and a downloaded file in the shapes above |
| `safe_name()`, `quality()`, `norm()`, `contains()`, `is_lossless()` | File names, quality labels, matching |

Encore keeps protocol 1 compatible with this file: a plugin built with it keeps working on later Encore
releases that speak protocol 1.
