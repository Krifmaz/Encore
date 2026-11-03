#!/usr/bin/env python3
"""Turns Macrobenchmark's result files into the tables of PERFORMANCE.md.

Usage: summarize.py <dir with *-benchmarkData.json files> [label]

Prints a Markdown table per kind of result: startup (time to initial display, median / min / max) and frame timing
(frame duration P50 / P90 / P99, janky frames per iteration: frames with a frame overrun above 0 ms, i.e. frames that
missed their deadline). Values Macrobenchmark didn't report show as "–".
"""
import glob
import json
import os
import sys


def stat(metric, key):
    """One summary value (P50, median ...) of a metric dict, or None."""
    if not metric:
        return None
    return metric.get(key)


def fmt(v, unit="ms"):
    """A number as the tables show it."""
    return "–" if v is None else f"{v:.1f} {unit}".strip()


def janky_per_iteration(overrun):
    """Mean count of frames with an overrun > 0 ms per iteration, from the per-frame samples of each run."""
    runs = (overrun or {}).get("runs") or []
    if not runs:
        return None
    return sum(sum(1 for x in run if x > 0) for run in runs) / len(runs)


def main():
    folder = sys.argv[1] if len(sys.argv) > 1 else "."
    label = sys.argv[2] if len(sys.argv) > 2 else ""
    files = sorted(glob.glob(os.path.join(folder, "**", "*benchmarkData.json"), recursive=True))
    if not files:
        print(f"no *benchmarkData.json under {folder}")
        return 1
    startup, frames = [], []
    for f in files:
        with open(f) as fh:
            data = json.load(fh)
        device = data.get("context", {}).get("build", {})
        for b in data.get("benchmarks", []):
            m = {**b.get("metrics", {}), **b.get("sampledMetrics", {})}
            name = b.get("name", "?")
            if "timeToInitialDisplayMs" in m:
                t = m["timeToInitialDisplayMs"]
                startup.append((name, t.get("median"), t.get("minimum"), t.get("maximum"), len(t.get("runs") or [])))
            if "frameDurationCpuMs" in m:
                d = m["frameDurationCpuMs"]
                count = stat(m.get("frameCount"), "median")
                frames.append((name, d.get("P50"), d.get("P90"), d.get("P99"), janky_per_iteration(m.get("frameOverrunMs")), count))
    title = f" – {label}" if label else ""
    print(f"### Startup{title}\n")
    print("| Journey | Median | Min | Max | Iterations |\n|---|---|---|---|---|")
    for n, med, lo, hi, it in startup:
        print(f"| {n} | {fmt(med)} | {fmt(lo)} | {fmt(hi)} | {it} |")
    print(f"\n### Frame timing{title}\n")
    print("| Journey | P50 | P90 | P99 | Janky frames / iteration | Frames / iteration |\n|---|---|---|---|---|---|")
    for n, p50, p90, p99, jank, count in frames:
        print(f"| {n} | {fmt(p50)} | {fmt(p90)} | {fmt(p99)} | {fmt(jank, '')} | {fmt(count, '')} |")
    if device:
        print(f"\nDevice: {device.get('brand', '')} {device.get('model', '')}, Android API {device.get('version', {}).get('sdk', '?')}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
