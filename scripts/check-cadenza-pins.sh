#!/usr/bin/env bash
# Checks that every Cargo.toml in the repo that pins Cadenza (the `cadenza` and `cadenza-dsp` crates from
# github.com/Krifmaz/cadenza) names the same rev. Today that is the server (server/Cargo.toml, cadenza-dsp), the
# desktop app (clients/desktop/Cargo.toml, cadenza and cadenza-dsp) and the phone's JNI crate
# (clients/android/cadenza/rust/Cargo.toml, cadenza-dsp): all three build the same engine, so they move together.
#
#   scripts/check-cadenza-pins.sh        exit 0: one rev everywhere; 1: the pins differ, or one is not a rev;
#                                        2: no pin found or a file could not be read
#
# Revs are compared as commit ids, so a short one ("93c2621") and the full id of the same commit are the same
# pin. A pin by branch or tag, or a git dependency without a rev, fails: only a rev builds the same code every
# time. Both ways of writing a dependency are read: inline (`cadenza = { git = "…", rev = "…" }`, under
# [dependencies], [workspace.dependencies] or a [target.….dependencies] table) and as a table of its own
# ([dependencies.cadenza] with git = and rev = lines). Run by the test job of .github/workflows/release.yml.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

# every Cargo.toml of the repo, without build output, git internals and agent worktrees
files=()
while IFS= read -r f; do files+=("$f"); done < <(find . \( -name target -o -name 'target-*' -o -name .git -o -name node_modules \
    -o -name .claude \) -prune -o -name Cargo.toml -type f -print | sed 's|^\./||' | sort)
[ "${#files[@]}" -gt 0 ] || { echo "check-cadenza-pins: no Cargo.toml found under $root" >&2; exit 2; }

# One line per Cadenza dependency: file <TAB> line <TAB> name <TAB> rev ("-" when none) <TAB> branch|tag|"".
# A comment after a value is cut off first (the pins carry no '#' inside their strings).
pins=$(awk '
    function attr(s, k,    t) {
        t = " " s
        if (t !~ ("[{, \t]" k "[ \t]*=[ \t]*\"")) return ""
        sub("^.*[{, \t]" k "[ \t]*=[ \t]*\"", "", t)
        sub(/".*$/, "", t)
        return t
    }
    function is_cadenza(url) { return url ~ /\/[Cc]adenza(\.git)?\/?$/ }
    function emit(file, line, name, rev, other) {
        printf "%s\t%s\t%s\t%s\t%s\n", file, line, name, (rev == "" ? "-" : rev), other
    }
    function flush() {
        if (sec_name != "" && is_cadenza(sec_git)) emit(sec_file, sec_line, sec_name, sec_rev, sec_other)
        sec_name = ""; sec_git = ""; sec_rev = ""; sec_other = ""
    }
    FNR == 1 { flush() }
    {
        line = $0
        sub(/^[ \t]*#.*$/, "", line)
        sub(/[ \t]+#.*$/, "", line)
    }
    line ~ /^[ \t]*\[/ {
        flush()
        # [dependencies.cadenza], [workspace.dependencies.cadenza-dsp], [target.'…'.dependencies.cadenza]
        if (line ~ /dependencies\.[A-Za-z0-9_-]+[ \t]*\]/) {
            n = line
            sub(/^.*dependencies\./, "", n)
            sub(/[ \t]*\].*$/, "", n)
            sec_name = n; sec_line = FNR; sec_file = FILENAME
        }
        next
    }
    sec_name != "" {
        if (line ~ /^[ \t]*git[ \t]*=/) sec_git = attr(line, "git")
        if (line ~ /^[ \t]*rev[ \t]*=/) sec_rev = attr(line, "rev")
        if (line ~ /^[ \t]*branch[ \t]*=/) sec_other = "branch"
        if (line ~ /^[ \t]*tag[ \t]*=/) sec_other = "tag"
        next
    }
    line ~ /^[ \t]*"?[A-Za-z0-9_-]+"?[ \t]*=[ \t]*\{/ {
        url = attr(line, "git")
        if (!is_cadenza(url)) next
        name = line
        sub(/^[ \t]*"?/, "", name)
        sub(/"?[ \t]*=.*$/, "", name)
        other = ""
        if (attr(line, "branch") != "") other = "branch"
        if (attr(line, "tag") != "") other = "tag"
        emit(FILENAME, FNR, name, attr(line, "rev"), other)
    }
    END { flush() }
' "${files[@]}") || { echo "check-cadenza-pins: could not read the Cargo.toml files" >&2; exit 2; }

[ -n "$pins" ] || { echo "check-cadenza-pins: no Cadenza pin found in ${#files[@]} Cargo.toml files" >&2; exit 2; }

annotate() { # level file line message: a GitHub annotation in Actions, nothing elsewhere
    [ "${GITHUB_ACTIONS:-}" = true ] && echo "::$1 file=$2,line=$3::$4"
    return 0
}

bad=0
revs=()
echo "Cadenza pins:"
while IFS=$'\t' read -r file line name rev other; do
    printf '  %-48s %-12s %s\n' "$file:$line" "$name" "$rev${other:+ (by $other)}"
    if [ -n "$other" ]; then
        echo "    $name is pinned by $other, not by rev" >&2
        annotate error "$file" "$line" "$name is pinned by $other; pin it by rev like the other Cadenza dependencies"
        bad=1
    elif ! [[ "$rev" =~ ^[0-9a-f]{7,40}$ ]]; then
        echo "    $name has no rev (or not a commit id): $rev" >&2
        annotate error "$file" "$line" "$name needs rev = \"<commit>\" like the other Cadenza dependencies"
        bad=1
    else
        revs+=("$rev")
    fi
done <<< "$pins"

# the longest rev is the reference: every other one must be the start of it
ref=""
for r in ${revs[@]+"${revs[@]}"}; do [ "${#r}" -gt "${#ref}" ] && ref=$r; done
differ=0
for r in ${revs[@]+"${revs[@]}"}; do [[ "$ref" == "$r"* ]] || differ=1; done

if [ "$differ" = 1 ]; then
    echo >&2
    echo "The Cadenza pins differ:" >&2
    for r in $(printf '%s\n' ${revs[@]+"${revs[@]}"} | sort -u); do
        where=$(awk -F'\t' -v r="$r" '$4 == r { printf "%s%s:%s (%s)", (n++ ? ", " : ""), $1, $2, $3 }' <<< "$pins")
        echo "  $r: $where" >&2
    done
    echo "Move them to one rev together (the server's and the phone's cadenza-dsp, the desktop app's cadenza and" >&2
    echo "cadenza-dsp), then run cargo check in each of those folders so its Cargo.lock follows." >&2
    while IFS=$'\t' read -r file line name rev other; do
        [ -z "$other" ] && [[ "$rev" =~ ^[0-9a-f]{7,40}$ ]] || continue
        annotate error "$file" "$line" "$name is at $rev; the Cadenza pins of the server, the desktop app and the phone differ"
    done <<< "$pins"
    bad=1
fi

if [ "$bad" = 0 ]; then
    echo "All ${#revs[@]} Cadenza pins are at $ref."
fi
exit "$bad"
