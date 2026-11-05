# Decisions

Calls made while working hands-off, with why. Newest first.

## 2026-11-05 – Self-update: why 0.42.15 → 0.42.18 came back on 0.42.15, and what changed

**What happened.** The repository's default branch was a stale branch (`ccr-a4585b2e-d7xdtf`, stopped around
0.42.6). Two pull requests were merged into it and the release-on-merge workflow shipped v0.42.17 and v0.42.18
from it. Those images write database schema 41; 0.42.11–0.42.16 had already moved databases to 42. On "Update
now" the helper pulled the right tag and swapped the container correctly, the new 0.42.18 server refused the
newer database (exit code 3) and the helper, as designed, rolled back to the old container, which still runs
0.42.15 on an untouched schema-42 database. Not a pinned compose tag, not a wrong pull, not a reused image id.
The helper's log was lost because Docker removed its container (`AutoRemove`), so the page could only say
"did not go through".

**Decisions.**
- *Releases only from main, and the schema never goes down.* release.yml fails unless the commit is on main,
  and `scripts/check-schema-version.sh` fails a release whose `SCHEMA_VERSION` is lower than any of the newest
  20 releases'. A script, so the kit-based workflows can call it as they are.
- *The updater refuses an older schema twice.* Before pulling, the server reads the release's
  `SCHEMA_VERSION` from GitHub (tag `v<version>`) and refuses with the reason. Then the helper, which runs inside
  the new image, compares its own schema with the database's (`--schema`) and its own version with the tag,
  before anything is stopped. The GitHub check covers images that predate the helper check; the helper check
  covers a missing tag or an unreachable GitHub.
- *"Migrate only once the new image is confirmed" is done as: confirm before stopping, back up right before
  migrating.* The server cannot be healthy before it has opened (and so migrated) its database, so waiting for
  health before migrating is impossible. Instead the new image is confirmed before the old container is
  touched (version and schema, above), and the server already backs up to `/config/backups` right before it
  migrates (`Db::open`, `backup::before_upgrade`).
- *Forward only once a migration began.* The server logs `database schema N → M: backing up first` before it
  migrates. If the new container stops and its log has that line, the helper no longer rolls back (the old
  Encore may not read the database): it starts the new container again (3 times), keeps the old one stopped
  under its `-old-` name, and reports how to go back with the backup.
- *The helper's log outlives it.* The helper container is no longer `AutoRemove`; every line names its step
  (`check`, `pull`, `helper`, `inspect`, `schema`, `stop`, `rename`, `create`, `start`, `verify`, `cleanup`).
  The server that comes back reads it, merges it with its own steps into one record (`update_report.rs`),
  checks its own version against the target (`verify`), removes the helper and shows the failing step and
  error with Copy in Diagnostics → Updates.
