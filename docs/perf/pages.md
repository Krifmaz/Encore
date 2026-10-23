# Pages that wait on MusicBrainz (Oct 23, 2026)

The owner on Oct 23: "very slow load times when pulling artists, discographies and stuff … the library is very
snappy though". The library pages are the local database (about 2 ms warm, `cargo bench --bench perf`); the
artist, album and search pages wait for MusicBrainz, which Encore asks at most once a second (its rule for the
public server). So what a page costs is how many MusicBrainz calls it waits behind.

    cd server && cargo bench -p encore --bench pages --profile dev

`server/benches/pages.rs` runs each scenario on a fresh server against a stand-in MusicBrainz that takes 250 ms
an answer, at the default one call a second, with an artist of 250 release groups. Cold: nothing cached.

| scenario | before ms | after ms | MusicBrainz calls before → after |
|---|---|---|---|
| web: artist page | 3303 | 3303 | |
| web: artist page and every section | 6269 | 6270 | 7 → 7 |
| **Android: artist page** | **12298** | **3308** | |
| **Android: artist page and every section** | **18285** | **6274** | **19 → 7** |
| album page | 2262 | 2263 | 3 → 3 |
| **album opened 1 s after the artist page** | **4988** | **2984** | 10 → 10 |
| search while typing, earlier searches run on (before: every app) | 5020 | 5029 | 7 → 7 |
| **search while typing, earlier searches cancelled (after: web, desktop, Android)** | | **2882** | 5 |
| **artist page again after a restart** | **3293** | **34** | **4 → 0** |

What changed:

- **One call for each answer at a time** (`encore_http::mb::Mb::get`). The Android artist page asks for the page,
  the popular songs, the other catalogues and the similar artists at once, and each of them asked MusicBrainz
  for the artist and its release groups: the same four calls three times over, one a second. A second caller now
  waits for the first one's answer (when that one is as urgent or more).
- **MusicBrainz's look-ups on disk** (`encore_http::mb_store`, `musicbrainz-cache.db` next to the database). An
  artist, its release groups, a release …: kept 30 days (at most 50 000 answers), never searches. Under an hour
  old they are used as they are, so a restart no longer makes every page ask again; older, a page still gets
  them at once and they are asked again in the background (`Mb::refresh_stale`, at background priority).
  Imports, requests and background work never take an answer older than an hour.
- **A page's slow section goes after the next page** (`Priority::Section`). The other catalogues section of an
  artist page makes a dozen MusicBrainz look-ups (barcodes, ISRCs); they went at a page's priority, so the album
  tapped next waited behind them. They now go after any page and before imports.
- **Search drops the search of the words typed before** (web `api::get_abortable`, desktop `Task::abortable`,
  Android `FeaturesApi.searchCancellable`). The server drops a search whose connection closed, and with it its
  MusicBrainz calls, which went before the newer search's in the line.

What stays: a first visit to an artist costs one MusicBrainz call per 100 release groups plus the artist (about
3 s for 250), and an album page three calls (about 2 s). That is the one-call-a-second rule; a MusicBrainz mirror
(Settings → `musicbrainz_url`, with `musicbrainz_rate` raised) is what makes those faster.
