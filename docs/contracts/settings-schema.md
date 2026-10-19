# Settings schema

What `GET /api/admin/settings/schema` answers, where it comes from, and how a client draws the
Settings → Server pages from it. Written for 0.29.0: the server has one settings table
(`server/crates/config/settings.toml`); the web app's settings pages render from this answer from 0.29 on,
and the phone app's full Settings (0.30) reads the same answer.

## Where it comes from

`server/crates/config/settings.toml` is the one table of server settings. Every setting has its
environment variable, its default and whether it is secret; the ones shown on the generic settings form
also have their card, label, help, type, options and visibility rule. The words were copied from the
web app's static table (`SECTIONS` and `GROUPS` in the old `web-rs/src/pages/settings.rs`), which was the
source of truth for them until 0.29. Since 0.29 the web app draws the Server pages from this answer
(`web-rs/src/settings/`) and the static table is gone; this file is the one place the words live.

`server/crates/config/build.rs` turns the file into `settings_gen.rs` when the server is built:

- `Keys`: every setting's name as a constant (`Keys::APP_NAME` is `"app_name"`);
- `TABLE`: every setting with its environment variable, default, secret flag, type, card, label and help.
  The loader (`Config::load`) reads it; there is no `defaults.json` any more;
- `SCHEMA`: the pages, their cards and the fields on each card;
- a typed accessor per setting on `Config`: `cfg.app_name()` (a `String`), `cfg.session_days()` (an `i64`),
  `cfg.auto_approve()` (a `bool`), `cfg.musicbrainz_rate()` (an `f64`). They read the value in effect
  (environment, then saved, then default), like `cfg.str("app_name")`.

A file that does not fit stops the build with the reason: an unknown attribute, a type that does not match
the default, a select without options, a field on a card with no label, a card or page that does not exist,
a `when` rule that does not parse or names a missing setting or option, an accessor name that a `Config`
method or a Rust keyword already takes.

## The request

`GET /api/admin/settings/schema`, as an admin (a cookie session, an app token or the API key, like every
`/api/admin` route). 401 without a session, 403 for a member who is not an admin. The answer holds no
values: `GET /api/admin/settings` has them (with `_meta` saying where each one comes from), and
`PUT /api/admin/settings` saves them, both unchanged. Every field in the schema is a key of that answer (the
old static table drew a field only when the answer had its key).

## The answer

```json
{
  "sections": [
    {
      "id": "apps",
      "title": "Apps & sources",
      "aliases": ["downloads", "tagging", "services"],
      "groups": [
        {
          "id": "torznab",
          "title": "Torrents & Usenet (Prowlarr)",
          "part": "Download sources",
          "desc": "Searches your torrent and Usenet indexers (through Prowlarr) for whole albums …",
          "test": "torznab",
          "test_label": "Test connection",
          "fields": [
            {
              "key": "qbit_password",
              "label": "qBittorrent password",
              "help": null,
              "type": "password",
              "options": [],
              "when": "torrent_client=qbittorrent",
              "secret": true,
              "env": "QBIT_PASSWORD",
              "env_locked": false,
              "default": "",
              "unit": null,
              "min": null,
              "max": null,
              "step": null,
              "full": false,
              "advanced": false
            }
          ]
        }
      ]
    }
  ]
}
```

Every attribute is always there; one that does not apply is `null` (or `[]`, `false`, `""` as listed).

### Section (a page under Settings → Server)

| Field | Meaning |
|---|---|
| `id` | the page's id in links (`#/settings?s=<id>`) |
| `title` | the nav entry and the page heading |
| `aliases` | older `?s=` names that open this page (`[]` for none) |
| `groups` | the cards on the page, in order. **Empty** for a page the web app draws with its own code (Auto-download, Contribute, Blocked): the section is listed so the nav order comes from one place |

Sections come in nav order: General, Apps & sources, Email & notifications, Auto-download, Contribute,
Blocked, Scheduled tasks.

### Group (a card)

| Field | Meaning |
|---|---|
| `id` | the card's id (also the old `?s=` name of the card, which opens its page) |
| `title`, `desc` | the card's heading and the line under it |
| `part` | the heading over a run of cards on the page ("Download sources", "Library & tagging", "Services" on Apps & sources); `""` for none. A new heading starts where `part` changes |
| `test` | the card's Test button calls `POST /api/system/test/<test>`; `null`: no button |
| `test_label` | the button's words; set whenever `test` is (the web app's old default was "Test connection") |
| `fields` | the card's fields, in order |

The Scheduled tasks page has one card (`sync`, Playlists & sync) under the task list, which the web app
draws itself.

### Field

| Field | Meaning |
|---|---|
| `key` | the setting's name in `GET/PUT /api/admin/settings` |
| `label` | the field's label |
| `help` | the line under the field; `null` for none |
| `type` | how to draw it, below |
| `options` | `[{"value", "label"}]` for `select` and `checks`, in order; `[]` otherwise. Values are always strings, even when the setting is a number (`streamrip_quality` is `2`, its options are `"1"` … `"4"`): compare the setting's value as text |
| `when` | visibility rule, below; `null`: always shown |
| `secret` | the server never sends the value: `GET /api/admin/settings` says `"•••• (saved)"` or `""`. A password field is always secret; a secret textarea (the WireGuard config, pasted cookies) too |
| `env` | the environment variable that sets it, `null` for none |
| `env_locked` | that variable is set on this server: the value comes from it, saving it does nothing (the field is shown disabled). Same as `_meta.<key>.source == "env"` in `GET /api/admin/settings` |
| `default` | the value when nothing is saved and no environment variable is set; its JSON type is the setting's type |
| `unit` | what a number counts (`"minutes"`, `"days"`, `"seconds"`, `"MB"`, `"kbps"`, `"requests per second"`); the label already says it in words, so the web form does not show it again |
| `min`, `max` | the sensible range of a number (a hint; the server does not refuse other values) |
| `step` | the number input's step (`0.1`, `0.05`); `null`: whole numbers |
| `full` | the field takes the card's whole width |
| `advanced` | a field for people who know what they are doing (its label says so too) |

### Types

| `type` | Drawn as | Value |
|---|---|---|
| `text` | text input | string |
| `path` | text input; the value is a folder inside the container | string |
| `url` | text input; the value is an address | string |
| `password` | password input that starts empty (empty keeps the saved value); placeholder "saved – paste a new value to replace" / "not set" / "set by environment" | string |
| `textarea` | text area (always full width) | string |
| `number` | number input with `step` | number |
| `bool` | switch | boolean |
| `select` | drop-down of `options` | string (or a number, see `options`) |
| `checks` | a row of check boxes, one per option | comma-separated option values (`"Album,EP"`), compared without case and punctuation |

`path` and `url` are drawn exactly like `text` (an `<input type="text">`): the type tells what the value is,
for clients that want to offer a folder picker or check an address later.

### Visibility rules (`when`)

`"key=value"`: the field is shown only while the select `key` has `value`; `"key!=value"`: only while it
has any other value. `key` is always a `select` on the same page and `value` one of its options (the build
checks both). Today every rule is of the first kind and names `torrent_client` or `nzb_client`: the torrent
and Usenet clients' fields show only for the client chosen. A field hidden by a rule is still saved with
its card.

## What is not in the schema

Settings that no generic card shows are in the table but not in the answer: the scheduled tasks' switches
and intervals (drawn by the tasks page), the download tools' auto-update switches, the Translator's model,
the Cadenza engine's own settings (drawn from the engine's description, `GET /api/playback/dsp`), the
Auto-download page, what a new member gets (People → Users & permissions), the Contribute page, and
values the server keeps for itself (the API key, the Roon and Plex ids, the managed slskd's key, the
MusicBrainz sign-in). `settings.toml` says for each where it is edited.

## Changing the table

- A new setting is a `[keys.<name>]` entry in `settings.toml`; with `group` it shows on that card, after
  the card's other fields in file order. Nothing else is needed: the loader, `GET/PUT /api/admin/settings`,
  the schema and the accessor follow from the file.
- The diagnostics page lists changed settings in the table's order: A to Z, then the keys in `listed_last`
  (settings added after the first table, kept in the order they always had). A new key goes A to Z; do not
  add it to `listed_last`.
- The tests in `server/crates/config/src/settings_tests.rs` check that every key is listed once and every
  field is a key, that `when` rules parse and name a select option, that defaults fit their types and
  options, that the release-type options equal `release_types::KINDS` / `EXTRAS`, the shape of the schema
  answer, and that every key the server's code reads with `cfg.str("…")` and its siblings is in the table.
- web-rs keeps the HTML the old static table drew (`web-rs/src/settings/fixtures/settings-cards.html`) and
  its tests compare the schema renderer with it, using a recorded schema answer
  (`web-rs/src/settings/fixtures/settings-schema.json`). After an intended change to the table, record
  both again: the schema answer from `GET /api/admin/settings/schema`, then
  `ENCORE_WRITE_SETTINGS_CARDS=1 cargo test settings::` in `web-rs/`.
