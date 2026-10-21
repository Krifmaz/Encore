# Notes on the Internet Archive plugin 0.1.0

For the plugin's author. Thank you for the plugin and the SDK: protocol 1 and your `plugin.toml` became the
format of Encore's plugins, and the SDK file is in the repository unchanged
(`plugins/sdk/encore_plugin.py`). As you asked, the plugin is not built in and not in the catalog; nothing in
its files was changed. These are the problems an earlier read found, checked again against your zip
(`owner-internet-archive-0.1.0.zip`, sha256 `79dee207…988a3b`), plus a few more. Each was confirmed by calling
the plugin's own functions with made-up archive.org answers (archive.org cannot be reached from the machine
these were checked on). What to do about them is your call.

**What works.** Uploaded through Settings → Plugins → Upload plugin, it installs as a third-party plugin, passes
Encore's `describe` check in full isolation, reports "Ready · public files from archive.org, no account
needed", and its card shows the three settings with their defaults and saves them. Its manifest reads under
Encore's rules unchanged.

## The problems

| # | What happens | Where | How much it matters |
|---|---|---|---|
| 1 | **Duplicate tracks from derivatives in subfolders.** A file's group is its file name without the folder (`01 Opener`), but a derivative's group is the whole path of its original (`disc1/01 Opener`). For an item whose files sit in a folder, the FLAC original and its MP3 derivative land in two groups, so both are offered and an album download gets every song twice. | `audio_files()`, `"group"` | High for items with folders (multi-disc sets, many Live Music Archive shows) |
| 2 | **Two files with the same name in different folders become one.** The other side of 1: `disc1/01 Intro.flac` and `disc2/01 Intro.flac` share the group `01 Intro`, so one of them is dropped and the album comes out a song short. (New in this read.) | `audio_files()`, `"group"` | High for multi-disc items |
| 3 | **Apple Lossless counted as lossy.** `"apple lossless audio"` maps to the extension `m4a`, and `lossless` is decided from the extension, so ALAC files are scored, labelled and filtered (`lossless_only`) as lossy. | `FORMATS`, `audio_files()` | Medium |
| 4 | **One collection read letter by letter.** archive.org gives a one-value field as a plain string (the reason `first()` exists); `info_line()` iterates `collection` as a list, so an item in one collection, `etree`, shows `e` on its info line. | `info_line()` | Low (only the info line) |
| 5 | **Restricted items offered.** Nothing leaves out items that cannot be downloaded: lending-library and stream-only items (the `stream_only` collection, `access-restricted-item`, files marked private). They show up as results, and the download then fails with the SDK's "refused the request (HTTP 403): check the sign-in", which points at a sign-in the plugin does not have. | `build_query()`, `audio_files()` | Medium |
| 6 | **A search without words, with Collections set, searches everything.** The empty-search check compares the finished query with `mediatype:audio`; with Collections set it is `mediatype:audio AND (collection:("etree"))`, so the check misses and the most downloaded items of the collection come back as results for nothing in particular. | `search()`, `build_query()` | Low (needs a search without words) |
| 7 | **Typed words searched as one exact phrase.** What someone types in the picker is quoted whole: `grateful dead cornell 1977` becomes `("grateful dead cornell 1977")`, which finds only items with those four words in that order in one field. | `build_query()` | Medium (the picker's search box) |
| 8 | **Leading numbers taken off real titles.** When archive.org has no title for a file, the title comes from the file name with any leading number removed: `99 Luftballons` becomes `Luftballons`, `7 Rings` becomes `Rings`, `01 - 99 Problems` becomes `Problems`, `2 + 2 = 5` becomes `+ 2 = 5`. Encore then matches and tags by the wrong title. | `strip_track_prefix()` | Medium for files without title tags |
| 9 | **Time limits shorter than the plugin's own retries.** `[run.timeouts] test = 30`, but `test` calls `http_json(..., timeout=20)` and the SDK tries 3 times with pauses (up to about 65 s). `search = 60`, but one search makes the advanced search (30 s, 3 tries) and then up to 25 metadata requests (30 s, 3 tries each, 4 at a time). On a slow network Encore stops the plugin at its limit ("did not answer within 30 seconds") before the plugin can give its own clear `network` error. | `plugin.toml`, `test()`, `search()` | Low (slow networks) |
| 10 | **`max_items` read without a guard.** `int(ep.setting(req, "max_items", 8))` ends in an `internal` error for a value like `8.5` sent as text. Encore checks numbers when the card saves them, so this is unlikely through Encore. | `search()` | Very low |

## Ideas, if you want them

1, 2: give every file the same kind of group, the path without the extension:
`os.path.splitext(name)[0]` for originals and `os.path.splitext(original)[0]` for derivatives.

3: decide `lossless` from archive.org's format as well, for example a set of lossless formats
(`flac`, `24bit flac`, `apple lossless audio`, `wave`, `aiff`) checked before the extension.

4: read `collection` through a helper that turns a string into a one-item list.

5: add `AND -collection:(stream_only) AND -access-restricted-item:(true)` to the query, and skip files whose
metadata says `"private": "true"`.

6: return no results when the request has no words, before the collections are added.

7: send typed words as separate terms (each required, or each quoted on its own), not as one phrase.

8: strip a leading number only when it looks like a track number (followed by ` - `, `.` or `_`, or equal to
the file's `track` field), and keep the name as it is otherwise.

9: raise `test` to about 70 and `search` to 180 or more, or lower the per-request timeouts and retries.

10: read it with a `try`/`except` and the default.

## About releases

The manifest's `[update]` names releases of `Krifmaz/Encore` tagged `internet-archive-v{version}`, made by
`.github/workflows/plugin-release.yml`. Two things to know before the first one:

- That workflow is not in the repository, so no such release exists yet; Update and the "Update plugins" task
  find nothing until one does.
- Encore's server, the desktop app, the phone app and `scripts/release-apps.sh` all read the repository's
  **latest** release to find Encore's own newest version. A plugin release published the usual way becomes
  the latest release until the next Encore release, and is taken for Encore's: the server's Updates card
  shows "internet-archive-v0.1.0" with the plugin's notes as the newest Encore, the desktop and phone apps
  find no installers in it, and `scripts/release-apps.sh` run without a version would attach the apps to it.
  Publish plugin releases with
  `gh release create … --latest=false` (or `make_latest: "false"` in the API), not as pre-releases (Encore's
  plugin update check skips pre-releases unless `[update] prerelease = true`). A repository of its own for
  plugins avoids the question, and also keeps the plugin's releases among the 50 newest that Encore's update
  check reads.

Two comments in the files point at old names: `docs/CONTRACT.md` is `docs/plugins/CONTRACT.md` in the
repository, and the scheduled task "Update download tools" is called "Update plugins" from 0.32 on.
