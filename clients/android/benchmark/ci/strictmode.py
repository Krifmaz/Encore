#!/usr/bin/env python3
"""Summarises what the debug build logged during the journeys (PERFORMANCE.md, "StrictMode and JankStats").

Usage: strictmode.py <logcat dump, `adb logcat -d -v brief`>

StrictMode: each violation (disk read, disk write, network on the main thread) is counted by its kind and the first
stack frame in Encore's own code (app.encore.android), which is the line to fix. JankStats: the janky frames
core/DevPerf.kt logged (tag EncoreJank), counted per screen with their median duration.
"""
import collections
import re
import statistics
import sys


def main():
    path = sys.argv[1]
    with open(path, errors="replace") as fh:
        lines = fh.read().splitlines()
    violations = collections.Counter()
    current = None
    for line in lines:
        if "StrictMode" not in line:
            if current is not None:
                current = None
            continue
        m = re.search(r"android\.os\.strictmode\.(\w+)", line)
        if m and "policy violation" in line:
            current = [m.group(1), None]
            continue
        if current is not None and current[1] is None:
            f = re.search(r"at (app\.encore\.android\.[\w.$]+\([^)]*\))", line)
            if f:
                current[1] = f.group(1)
                violations[(current[0], current[1])] += 1
    jank = collections.defaultdict(list)
    for line in lines:
        m = re.search(r"EncoreJank.*janky frame ([\d.]+) ms \[(.*)\]", line)
        if m:
            screen = re.search(r"screen=(\w+)", m.group(2))
            jank[screen.group(1) if screen else "?"].append(float(m.group(1)))
    print("### StrictMode (debug build, main thread)\n")
    if not violations:
        print("No violations from Encore's own code.\n")
    else:
        print("| Count | Kind | First frame in Encore's code |\n|---|---|---|")
        for (kind, frame), n in violations.most_common():
            print(f"| {n} | {kind} | `{frame}` |")
        print()
    print("### JankStats (debug build: slower than release, compare screens with each other)\n")
    if not jank:
        print("No janky frames logged.")
    else:
        print("| Screen | Janky frames | Median duration |\n|---|---|---|")
        for screen, ds in sorted(jank.items(), key=lambda kv: -len(kv[1])):
            print(f"| {screen} | {len(ds)} | {statistics.median(ds):.1f} ms |")
    return 0


if __name__ == "__main__":
    sys.exit(main())
