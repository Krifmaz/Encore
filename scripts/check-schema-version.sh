#!/usr/bin/env bash
# The release guard for the database schema: a release must never write an older schema than one already
# released. Encore's server refuses a database written by a newer Encore (exit code 3), so a release with a
# lower SCHEMA_VERSION than its predecessors cannot start on any server that ran them – 0.42.17 and 0.42.18,
# built from a stale branch, shipped schema 41 after 0.42.11–0.42.16 had moved databases to 42.
#
#   scripts/check-schema-version.sh            compare HEAD with the newest 20 vX.Y.Z release tags
#   scripts/check-schema-version.sh v0.42.16   compare HEAD with that tag only
#
# Exits 1 (with a GitHub Actions error line) when HEAD's schema is lower than the highest one found; needs the
# tags fetched (actions/checkout with fetch-depth: 0). scripts/test-check-schema-version.sh tests it.
set -euo pipefail

# where SCHEMA_VERSION lives (encore_db)
FILE=server/crates/db/src/lib.rs
# how many of the newest releases are compared
NEWEST=20

# The number in `pub const SCHEMA_VERSION: i64 = N;` of the file on stdin (empty when there is none).
schema() { sed -n 's/^pub const SCHEMA_VERSION: i64 = \([0-9][0-9]*\);.*/\1/p' | head -1; }

NEW=$(schema < "$FILE")
[ -n "$NEW" ] || { echo "::error::no SCHEMA_VERSION in $FILE"; exit 1; }

if [ $# -gt 0 ]; then
  TAGS="$1"
else
  TAGS=$(git tag -l 'v[0-9]*.[0-9]*.[0-9]*' | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | sort -V | tail -n "$NEWEST" || true)
fi
if [ -z "$TAGS" ]; then
  echo "No earlier release to compare with: schema $NEW"
  exit 0
fi

HIGHEST=0
HIGHEST_TAG=""
for t in $TAGS; do
  # a release before the file existed (or without the constant) counts as schema 0
  S=$(git show "$t:$FILE" 2>/dev/null | schema || true)
  if [ -n "$S" ] && [ "$S" -gt "$HIGHEST" ]; then HIGHEST=$S; HIGHEST_TAG=$t; fi
done

if [ "$NEW" -lt "$HIGHEST" ]; then
  echo "::error::This build writes database schema $NEW, but $HIGHEST_TAG already shipped schema $HIGHEST: servers that ran it could not open their database. Release from the branch that has the newer migrations (main)."
  exit 1
fi
echo "Database schema $NEW (the highest released so far: ${HIGHEST_TAG:-none} with $HIGHEST)"
