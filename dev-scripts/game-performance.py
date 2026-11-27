#!/usr/bin/env python3
"""Capture one game or compare two bundled emulator executables on one host."""

import argparse
import csv
import json
import os
from pathlib import Path
import re
import shutil
import statistics
import subprocess
import sys
import tempfile

TAP_SCRIPT = Path(__file__).with_name("ai-tap-sequence.py")
RESOURCES = ("touchHLE_dylibs", "touchHLE_fonts", "touchHLE_default_options.txt", "touchHLE_wallpaper.png")


def capture(args, bundle, name, profile=False):
    output = args.out / name
    output.mkdir(parents=True)
    with tempfile.TemporaryDirectory(prefix="game-perf-") as temp:
        work = Path(temp)
        for name_in_bundle in RESOURCES:
            src = bundle / name_in_bundle
            dst = work / name_in_bundle
            if src.is_dir():
                shutil.copytree(src, dst)
            else:
                shutil.copy2(src, dst)
        cache = work / "cache"
        cache.mkdir()
        env = dict(os.environ, XDG_CACHE_HOME=str(cache), MESA_SHADER_CACHE_DIR=str(cache / "mesa"),
                   TOUCHHLE_FRAME_TIMES="1")
        if profile:
            env["TOUCHHLE_PROFILE_RUNTIME"] = "1"
        else:
            env.pop("TOUCHHLE_PROFILE_RUNTIME", None)
        command = [sys.executable, str(TAP_SCRIPT), str(args.app), "--exe", str(bundle / "touchHLE"),
                   "--cwd", str(work), "--out", str(output), "--final-wait", "20",
                   "--require-nonblack-frame", "--require-frame-content", "--require-eagl-frames"]
        for step in args.step:
            command.extend(["--step", step])
        command.extend(["--", *args.emulator_arg, "--overlay=off", "--device-family=iphone-4s", "--no-error-popup"])
        status = subprocess.run(command, env=env).returncode
        detailed = work / "HyperHLE_log.txt"
        if detailed.is_file():
            shutil.copy2(detailed, output / "detailed.log")
    log = output / "emulator.log"
    if args.require_metal and log.is_file():
        with log.open(errors="replace") as lines:
            rendered = [line.strip() for line in lines if re.search(r"Driver info:.*ANGLE.*Metal", line, re.I)]
        (output / "active-renderer.txt").write_text("\n".join(rendered) + "\n")
        if not rendered:
            print(f"{name}: no ANGLE Metal renderer reported", flush=True)
            status = 1
    return status


def percentile(sorted_values, percentage):
    pos = (len(sorted_values) - 1) * percentage
    lower = int(pos)
    return sorted_values[lower] + (sorted_values[min(lower + 1, len(sorted_values) - 1)] - sorted_values[lower]) * (pos - lower)


def summarize(output, build, name, status):
    path = output / name / "metrics.json"
    metrics = json.loads(path.read_text()) if path.is_file() else {}
    milestones = metrics.get("startup_elapsed_seconds", {})
    row = dict(build=build, launch=name, exit_status=status, guest_cpu_seconds=milestones.get("guest_cpu_begin"),
               first_present_seconds=milestones.get("first_present_call"))
    # Contexts can present independently. Keep separate distributions.
    intervals = {}
    hitches = []
    path = output / name / "frame-times.csv"
    if path.is_file():
        with path.open() as stream:
            for frame in csv.DictReader(stream):
                ms = float(frame["interval_ms"])
                context = frame["context"]
                intervals.setdefault(context, []).append(ms)
                if ms > 100:
                    hitches.append(dict(build=build, launch=name, context=context,
                                        end_seconds=float(frame["estimated_end_elapsed_seconds"]), interval_ms=ms))
    rows = []
    for context, values in intervals.items():
        values.sort()
        mean_ms = statistics.mean(values)
        rows.append(dict(row, context=context, samples=len(values), mean_fps=1000 / mean_ms if mean_ms > 0 else None,
                         p95_ms=percentile(values, 0.95), p99_ms=percentile(values, 0.99), max_ms=values[-1],
                         hitches_over_100_ms=sum(ms > 100 for ms in values)))
    if not rows:
        rows.append(dict(row, context=None, samples=0, mean_fps=None, p95_ms=None, p99_ms=None,
                         max_ms=None, hitches_over_100_ms=None))
    return rows, hitches


def save_reports(output, rows, hitches):
    (output / "comparison.json").write_text(json.dumps(rows, indent=2) + "\n")
    for filename, fields, entries in [
        ("comparison.csv", list(rows[0]), rows),
        ("hitches.csv", ["build", "launch", "context", "end_seconds", "interval_ms"], hitches),
    ]:
        with (output / filename).open("w", newline="") as file:
            writer = csv.DictWriter(file, fieldnames=fields)
            writer.writeheader()
            writer.writerows(entries)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", required=True, type=Path)
    parser.add_argument("--candidate", required=True, type=Path)
    parser.add_argument("--before", type=Path)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--step", action="append", default=[])
    parser.add_argument("--emulator-arg", action="append", default=[])
    parser.add_argument("--require-metal", action="store_true")
    args = parser.parse_args()
    args.app = args.app.resolve()
    args.out.mkdir(parents=True, exist_ok=True)
    order = [("after", "timed", args.candidate)]
    if args.before:
        order = [("before", "before-1", args.before), ("after", "after-1", args.candidate),
                 ("after", "after-2", args.candidate), ("before", "before-2", args.before)]
    rows, hitches = [], []
    failed = False
    for build, name, bundle in order:
        print(f"Capturing {name}...", flush=True)
        status = capture(args, bundle, name)
        failed |= status != 0
        measurements, slow_frames = summarize(args.out, build, name, status)
        rows.extend(measurements)
        hitches.extend(slow_frames)
        save_reports(args.out, rows, hitches)
    if not args.before and not failed:
        # Coroutine-level tracing changes performance; keep this separate from timing.
        failed = capture(args, args.candidate, "profile", profile=True) != 0
    return int(failed)


if __name__ == "__main__":
    sys.exit(main())
