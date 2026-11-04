#!/usr/bin/env python3
"""Prints the Android lint issues in the files a branch changed (android-perf.yml, "Lint of the files this branch
changed").

Usage: lint.py <lint-results-*.xml> <file listing the changed paths, one per line, relative to the repository>

Lint's XML report gives each issue's file as an absolute path; an issue counts when that path ends with one of the
changed paths. Prints one line per issue (severity, id, file:line, message) and a count; never fails.
"""
import sys
import xml.etree.ElementTree as ET


def main():
    report, changed_list = sys.argv[1], sys.argv[2]
    try:
        root = ET.parse(report).getroot()
    except (OSError, ET.ParseError) as e:
        print(f"no lint report ({e})")
        return 0
    with open(changed_list) as fh:
        changed = [line.strip() for line in fh if line.strip()]
    total = 0
    shown = 0
    for issue in root.iter("issue"):
        total += 1
        loc = issue.find("location")
        path = loc.get("file", "") if loc is not None else ""
        if not any(path.endswith(c) for c in changed):
            continue
        shown += 1
        line = loc.get("line", "?")
        short = next((c for c in changed if path.endswith(c)), path)
        print(f"{issue.get('severity')} {issue.get('id')} {short}:{line}: {issue.get('message')}")
    print(f"lint: {shown} issues in changed files ({total} in the whole app)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
