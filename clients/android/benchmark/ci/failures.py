#!/usr/bin/env python3
"""Prints why instrumentation tests failed, short enough to read in a CI log (PERFORMANCE.md, "In CI").

Usage: failures.py <dir with TEST-*.xml results> [<dir with journey-failure-*.xml screen dumps>]

For each failed test: its name and the first lines of the failure. For each screen dump the journeys saved when a
step found nothing (journeys/Journeys.kt `fail`): the texts, descriptions and resource ids on screen at that moment.
"""
import glob
import os
import sys
import xml.etree.ElementTree as ET


def results(folder):
    """Failed test cases with the head of their message."""
    for f in sorted(glob.glob(os.path.join(folder, "**", "TEST-*.xml"), recursive=True)):
        for case in ET.parse(f).getroot().iter("testcase"):
            for bad in list(case.findall("failure")) + list(case.findall("error")):
                text = (bad.text or bad.get("message") or "").strip().splitlines()
                print(f"FAILED {case.get('classname', '')}.{case.get('name', '')}")
                for line in text[:6]:
                    print(f"    {line}")


def screens(folder):
    """What was on screen in each failure dump."""
    for f in sorted(glob.glob(os.path.join(folder, "**", "journey-failure-*.xml"), recursive=True)):
        seen = []
        for node in ET.parse(f).getroot().iter("node"):
            for attr in ("text", "content-desc", "resource-id"):
                v = node.get(attr, "")
                if v and v not in seen:
                    seen.append(v)
        print(f"SCREEN {os.path.basename(f)}: " + " | ".join(seen[:60]))


def main():
    results(sys.argv[1])
    if len(sys.argv) > 2:
        screens(sys.argv[2])
    return 0


if __name__ == "__main__":
    sys.exit(main())
