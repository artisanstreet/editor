#!/usr/bin/env python3
"""Read-only Linux process sampler. CPU 100% means one occupied logical core."""

import argparse
from datetime import datetime, timezone
import json
import math
import os
from pathlib import Path
import sys
import time

from x11_window import X11Window


def positive_seconds(value):
    number = float(value)
    if not math.isfinite(number) or number <= 0:
        raise argparse.ArgumentTypeError("must be finite and positive")
    return number


def read_stat(directory):
    # The parenthesized command name may itself contain spaces and parentheses.
    fields = (directory / "stat").read_text().rpartition(")")[2].split()
    return {
        "cpu_seconds": (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK"),
        "start_ticks": int(fields[19]),
        "threads": int(fields[17]),
        "state": fields[0],
    }


def read_memory(directory):
    fields = {}
    for line in (directory / "smaps_rollup").read_text().splitlines():
        key, _, value = line.partition(":")
        if value.strip().endswith("kB"):
            fields[key] = int(value.split()[0]) * 1024
    return {
        "working_set_bytes": fields["Rss"],
        "proportional_set_bytes": fields["Pss"],
        "private_resident_bytes": fields["Private_Clean"] + fields["Private_Dirty"],
        "swap_bytes": fields["Swap"],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pid", type=int)
    parser.add_argument("--seconds", type=positive_seconds, default=60)
    parser.add_argument("--interval", type=positive_seconds, default=1)
    parser.add_argument("--warmup", type=float, default=0)
    parser.add_argument("--scenario", default="unspecified")
    parser.add_argument("--build", default="unspecified")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--x11-window", type=int)
    parser.add_argument("--xdotool", type=Path)
    args = parser.parse_args()
    if sys.platform != "linux":
        parser.error("requires Linux /proc; on Windows use measure-idle.ps1")
    if args.pid <= 0:
        parser.error("pid must be positive")
    if not math.isfinite(args.warmup) or args.warmup < 0:
        parser.error("warmup must be finite and non-negative")
    if bool(args.x11_window) != bool(args.xdotool):
        parser.error("--x11-window and --xdotool must be provided together")
    window = X11Window(args.xdotool, args.x11_window, args.pid) if args.x11_window else None

    directory = Path("/proc") / str(args.pid)
    initial = read_stat(directory)
    executable = str((directory / "exe").readlink())
    time.sleep(args.warmup)
    measured_at = datetime.now(timezone.utc).isoformat()
    started = time.monotonic()
    samples = []
    while True:
        stat = read_stat(directory)
        if stat["start_ticks"] != initial["start_ticks"] or stat["state"] in {"Z", "X"}:
            raise RuntimeError("process exited or PID was reused during measurement")
        samples.append({
            "elapsed_seconds": time.monotonic() - started,
            "cpu_seconds": stat["cpu_seconds"],
            "threads": stat["threads"],
            **read_memory(directory),
            **({"window": window.read()} if window else {}),
        })
        remaining = args.seconds - (time.monotonic() - started)
        if remaining <= 0:
            break
        time.sleep(min(args.interval, remaining))

    elapsed = samples[-1]["elapsed_seconds"] - samples[0]["elapsed_seconds"]
    cpu = samples[-1]["cpu_seconds"] - samples[0]["cpu_seconds"]
    summary = {
        "elapsed_seconds": elapsed,
        "cpu_seconds": cpu,
        "cpu_core_percent": 100 * cpu / elapsed,
        "cpu_machine_percent": 100 * cpu / elapsed / (os.cpu_count() or 1),
    }
    for name in ("working_set", "proportional_set", "private_resident", "swap"):
        values = [sample[f"{name}_bytes"] for sample in samples]
        summary.update({
            f"{name}_min_bytes": min(values),
            f"{name}_max_bytes": max(values),
            f"{name}_mean_bytes": sum(values) / len(values),
        })
    report = {
        "schema_version": 1,
        "platform": "linux",
        "measured_at_utc": measured_at,
        "scenario": args.scenario,
        "build": args.build,
        "pid": args.pid,
        "process_start_ticks": initial["start_ticks"],
        "executable": executable,
        "logical_processors": os.cpu_count(),
        "warmup_seconds": args.warmup,
        "qualification": {
            "acceptance": "not_established",
            "window_state_verified": False,
            "remaining_checks": "Verify window dimensions, scale, focus and visibility remained constant, plus the intended workload, lack of user input and build identity separately.",
        },
        "summary": summary,
        "samples": samples,
        "scope": "One process; includes its CPU-resident mappings, but does not measure dedicated GPU memory or other processes. Window size, display scale and focus must be recorded separately. Scenario is supplied by the caller, not verified by this sampler.",
    }
    if window:
        stable = all(sample["window"] == samples[0]["window"] for sample in samples)
        visible = all(sample["window"]["visible"] for sample in samples)
        report["qualification"].update({
            "window_state_verified": stable and visible,
            "window_state_stable": stable,
            "window_visible_throughout": visible,
            "remaining_checks": "Scale, build identity, intended workload and lack of input must be verified separately. X11 viewability does not establish absence of occlusion.",
        })
    text = json.dumps(report, indent=2) + "\n"
    if args.output:
        args.output.write_text(text)
    else:
        print(text, end="")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError) as error:
        print(f"measure-idle: {error}", file=sys.stderr)
        sys.exit(1)
