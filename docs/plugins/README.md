# Writing a plugin for Encore

A plugin adds something to Encore without a new Encore release: most often a **source**, a service Encore can
search and download music from, both in the download picker and for requests. Encore's own sources, download
clients, tools and media-server connections are plugins too (the catalog on Settings → Plugins); the ones you
write are installed with **Upload plugin** or **Install from URL** on the same page. Written for Encore 0.32.0.

| File | What it is |
|---|---|
| This page | How to write, check, package and publish one, with the example plugin |
| [CONTRACT.md](CONTRACT.md) | Protocol 1: how Encore starts a plugin and what it sends and expects back |
| [docs/contracts/plugin-manifest.md](../contracts/plugin-manifest.md) | Every key of `plugin.toml`, the catalog's included |
| [SANDBOX.md](SANDBOX.md) | What a plugin can see and reach, and what a Docker host needs for full isolation |
| [example-tone/](example-tone/) | The example plugin: a source that "searches" a short list of tones and "downloads" a sine tone as FLAC |
| [plugins/sdk/encore_plugin.py](../../plugins/sdk/encore_plugin.py) | The SDK: one Python file, no other packages |
| [plugins/tools/plugin-check.py](../../plugins/tools/plugin-check.py) | Runs a plugin folder the way Encore does and checks every answer |

## How a plugin works

A plugin is a folder (a zip when you install it) with a manifest, `plugin.toml`, and a program. For every
command (`describe`, `status`, `test`, `search`, `download`) Encore starts the program once, writes the request
as one JSON object on its stdin, and reads one JSON object back from its stdout. Lines on stderr go to the
plugin's log, and `progress 42 text` lines drive the download's progress bar. Settings, secrets included,
arrive on stdin only. Encore checks, tags and imports what the plugin downloads, exactly as for its own
sources, so a plugin only finds and fetches files.

Any language works when it speaks the protocol and the image has its interpreter (`python3`, `node`, `sh`, or
a program of its own). The SDK makes it short in Python:

```python
import encore_plugin as ep

def search(req):
    return {"results": [ep.result(key="item-1", title="Song", artist="Artist", format="flac", payload={"id": 1})]}

def download(req):
    path = fetch_into(req["folder"], req["payload"]["id"])   # your code
    return {"files": [ep.file_entry(path, title="Song", artist="Artist")]}

ep.run("my-source", "0.1.0", kind="source", kinds=("track", "album"),
       handlers={"search": search, "download": download})
```

## The example plugin

[example-tone/](example-tone/) is a complete source to copy. It has no network and no account: it "searches"
two albums of tones (*Tuning Fork* and *Test Signals* by *The Example Tones*) and "downloads" the one picked by
writing a sine tone as a FLAC file. Its files:

| File | What it holds |
|---|---|
| `plugin.toml` | The manifest: id, name, version, kind, the commands it answers, the kinds of request it takes, its time limits, two settings (tone length, sample rate) and its permissions (no network) |
| `plugin.py` | The program: `status`, `test`, `search` and `download` handlers, then `ep.run(...)` |
| `encore_plugin.py` | A copy of the SDK, beside the program, as in every plugin's zip |
| `README.md` | A short note for people who find the folder on GitHub |

What to look at in `plugin.py`:

- **Settings with a fallback.** `tone_seconds()` and `sample_rate()` read a setting with a default and a range,
  so a value that does not fit is logged and replaced, never a crash.
- **The request's fields.** `search` uses `query` (typed words) when it is there, else `artist` and `title`
  (or `album`), and `length` for the tone's length: a song Encore knows the length of gets a tone exactly that
  long, so Encore's length check passes.
- **Scores and names.** Each result says how well it fits (0 to 100) and carries the real title and artist:
  for a request, Encore takes the best result whose names match what was asked.
- **The payload.** Each result carries what `download` needs (`{"album", "track", "seconds"}`); Encore gives it
  back unchanged. `download` checks it anyway and answers `not_found` for an item that is gone.
- **Writing files.** Each file is written as `<name>.part` and renamed when whole; a stop (SIGTERM, which the SDK
  turns into `ep.Cancelled`) removes the part file. Progress is reported about once a second.
- **Errors in plain words.** `ep.PluginError("that tone is not in the list any more", "not_found")`.

The FLAC writer at the end of `plugin.py` stores the samples as they are (verbatim frames): the files are as
large as WAV files, and every player reads them. A real source downloads its files instead.

## Check it on your computer

```bash
python3 plugins/tools/plugin-check.py docs/plugins/example-tone --data /tmp/tone-data
python3 plugins/tools/plugin-check.py docs/plugins/example-tone --data /tmp/tone-data \
    --track "The Example Tones|Concert A" --download 1 --folder /tmp/tone-files
python3 plugins/tools/plugin-check.py docs/plugins/example-tone --data /tmp/tone-data \
    --album "The Example Tones|Test Signals" --download 1 --set rate=48000 --set seconds=5
```

plugin-check reads the manifest with Encore's rules for uploads, runs each command the way Encore does (a
cleared environment, the request on stdin, the manifest's time limits, SIGTERM and then SIGKILL when one runs
over) and checks every answer: the describe fields, the shape of each result, the downloaded files. It exits
with 0 when all is well. `--data` keeps the plugin's data folder out of the plugin's own folder, so it does not
end up in the zip. plugin-check does not sandbox the plugin: run plugins you did not write in a throwaway
container or virtual machine.

## Make your own

1. Copy `example-tone/` to a folder of your own and give the plugin its own `id` (a-z, 0-9 and dashes; not an
   id of the catalog: `ytdlp`, `streamrip`, `slskd`, …), `name`, `version` and `description`, in `plugin.toml`
   and in the `ep.run(...)` line (describe must answer the manifest's id and version).
2. Replace the list and the tone with your service: `search` returns results, `download` writes files into
   `req["folder"]` and returns them. Keep the SDK file beside your program.
3. Ask for what you need in `[permissions]`: `network = ["api.example.org"]` (the hosts are shown on the card;
   any list turns the network on), `paths = [{ setting = "library_path", access = "read" }]` to read the
   library. Reading the library is the only folder of the server a plugin you add can ask for.
4. Set `[run.timeouts]` longer than your own network timeouts and retries together (the SDK's `http()` tries 3
   times).
5. Run plugin-check until it passes, with a search and a download.
6. Zip the folder and install it with **Upload plugin**:

   ```bash
   cd docs/plugins && python3 -m zipfile -c example-tone.zip example-tone/
   ```

   (a zip of at most 200 MB, with `plugin.toml` at its root or in its one top folder).

### The manifest of a plugin you add

[docs/contracts/plugin-manifest.md](../contracts/plugin-manifest.md) describes every key. A plugin you add uses
these (the catalog's extensions, such as `builtin`, `[python]` or `[migration]`, are refused in an upload):

| Table | Keys |
|---|---|
| `[plugin]` | `id`, `name`, `version`, `description`, `kind` (`source` for a source), `protocol = 1`, `commands`, `kinds` (`track`, `album`; `url` is in the protocol, but Encore 0.32 sends pasted links to yt-dlp and streamrip, not to plugins), `lossless`, `needs_sign_in`, `homepage`, `license`, `min_encore` |
| `[[sources]]` | `id`, `name`, `picker`: only when the plugin answers for more than one source id (default: one source with the plugin's id and name) |
| `[run]` | `script`, `interpreter` (`python3`, `node`, `sh`, or none), `env` (fixed variables, not `PATH`, `HOME`, `TMPDIR`, `PYTHONPATH`, `PYTHONHOME`, `NODE_OPTIONS`, `LD_*`, `ENCORE_*`) |
| `[run.timeouts]` | seconds per command |
| `[update]` | `github`, `tag`, `asset`, `sums`, `auto`, `prerelease`, `tags` |
| `[[binary]]` | a program to download per machine (`arch`, `url` or `asset`, a fixed `sha256`, `unpack`, `bin`) |
| `[install]` | `verify`, `version_from`, `timeout`, `max_mb`, `remove` and `[[install.check]]`, for a `[[binary]]` |
| `[[settings]]` | `key`, `label`, `type` (`text`, `password`, `number`, `bool`, `select`, `checks`, `textarea`, `path`, `url`), `options`, `default`, `min`, `max`, `help`, `when`, `unit`, `step`, `full`, `advanced`, `part` |
| `[permissions]` | `network`, `filesystem = "folder"`, `paths` (reading the library only), `memory`, `cpu`, `processes`, `open_files` |

A key Encore does not know is refused, so a typo is an error, not a silent default. Ids, source ids and
settings must not clash with another plugin Encore knows.

### Settings

`[[settings]]` draws the plugin's card. The values are Encore's to keep (as `plugin.<id>.<key>`), checked
against the field when they are saved, and sent to every command but `describe` as `req["settings"]`. A
`password` is a secret: it is never shown again after it is saved, never in argv or the environment, and
must never be written to the log. Read a value with `ep.setting(req, "key", default)`.

## Publish updates

Put the plugin in a GitHub repository and publish each version as a release:

1. A tag that fits `[update] tag` (`my-source-v{version}` gives `my-source-v0.2.0`).
2. The zip as the `[update] asset` (`my-source-{version}.zip`) and a `SHA256SUMS` file listing it
   (`sha256sum my-source-0.2.0.zip > SHA256SUMS`).
3. `[update] github = "you/your-repo"`, `sums = "SHA256SUMS"`; `auto = true` lets the "Update plugins" task
   install new versions by itself when the card's switch is on – as long as a new version asks for no more
   than the installed one (the network, hosts, folders, limits, the `[update] github`). One that asks for
   more waits for the admin's Update on its card.

Encore checks the zip's sha256 before it unpacks anything, runs `describe` from the new version, and switches
only when that passes; the plugin's settings and data folder are kept. Encore reads the repository's 50 newest
releases. If the repository also publishes an app whose updater reads GitHub's latest release, publish the
plugin's releases with `gh release create … --latest=false`.

People can then install it from a URL: the raw address of its `plugin.toml` (Encore installs the newest
release), the zip's address, or `you/your-repo@v0.2.0` (`you/your-repo/folder/of/plugin@main` for a plugin in a
folder of a repository).

## Good manners

- Plain error words: what failed and what to do ("sign in again on the card"), with the right code
  (`auth`, `not_found`, `network`, `rate_limited`, `unsupported`, `internal`).
- Stable keys: the same item gets the same `key` in every search; Encore remembers picks and blocks by it.
- Honest results: `lossless` only for lossless files, the real names, a `score` that means something.
- Small payloads (at most 64 KB) and quick `status` answers, off the network when possible.
- Write only into the download folder and the data folder, and through `.part` files.
- Respect the services you use: their terms, their rate limits (`rate_limited` with `retry_after` makes
  Encore wait), and the people whose music it is. Encore's catalog holds no plugin whose purpose is removing
  DRM, and these pages are not a guide to writing one.
