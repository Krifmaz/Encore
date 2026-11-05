#!/usr/bin/env bash
# Tests scripts/check-schema-version.sh in a throwaway git repository: a release whose schema is lower than an
# earlier release's fails (the 0.42.18 case), the same or a higher one passes, and so does a first release.
set -euo pipefail

SCRIPT="$(cd "$(dirname "$0")" && pwd)/check-schema-version.sh"
DIR=$(mktemp -d)
OUT=$(mktemp)
trap 'rm -rf "$DIR" "$OUT"' EXIT
cd "$DIR"
git init -q
git config user.email test@example.org
git config user.name test
mkdir -p server/crates/db/src

# commit the schema file at schema $1
at() { printf 'pub const SCHEMA_VERSION: i64 = %s;\n' "$1" > server/crates/db/src/lib.rs; git add -A; git commit -qm "schema $1"; }
# expect $1 (pass/fail) from the check with arguments $2…
expect() {
  local want=$1; shift
  if bash "$SCRIPT" "$@" > "$OUT" 2>&1; then got=pass; else got=fail; fi
  [ "$got" = "$want" ] || { echo "expected $want, got $got: $(cat "$OUT")"; exit 1; }
}

at 41
expect pass                       # a first release: nothing to compare with
git tag v0.42.10
at 42
git tag v0.42.16
expect pass                       # the same schema as the newest release
at 43
expect pass                       # a higher one
git checkout -q v0.42.10          # a stale branch (schema 41) after 42 shipped
expect fail
grep -q "v0.42.16 already shipped schema 42" "$OUT" || { echo "the error names the release: $(cat "$OUT")"; exit 1; }
git tag v0.42.18                  # the broken release itself does not lower the bar
git checkout -q -
expect pass
expect pass v0.42.16              # one tag named: 43 over 42
git checkout -q v0.42.18
expect fail v0.42.16              # one tag named
echo "check-schema-version: all cases pass"
