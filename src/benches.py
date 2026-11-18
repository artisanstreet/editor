import argparse
import json
import os
from pathlib import Path
import subprocess
import sys

BASELINE = Path(__file__).resolve().parent / "benches.json"
GROSS = 3.0
UNITS = {"ps": 1e-3, "ns": 1.0, "µs": 1e3, "us": 1e3, "ms": 1e6, "s": 1e9}
BRANCHES = ("├─ ", "╰─ ")
INDENTS = ("│  ", "   ")


def nanoseconds(value, unit):
    if unit not in UNITS:
        raise ValueError(f"unknown divan time unit {unit!r}")
    return float(value) * UNITS[unit]


def medians(output):
    results = {}
    path = []
    for line in output.splitlines():
        if not line.strip() or line.startswith("Timer precision"):
            continue
        head = line
        depth = 0
        while head.startswith(INDENTS):
            head = head[3:]
            depth += 1
        if head.startswith(BRANCHES):
            head = head[3:]
            depth += 1
        columns = [column.strip() for column in head.split("│")]
        head = columns[0]
        words = head.split()
        if not words:
            continue
        name = words[0]
        path = path[:depth] + [name]
        if len(columns) < 3 or depth == 0 or len(words) < 3:
            continue
        median = columns[2].split()
        results["/".join(path)] = nanoseconds(*median)
    return results


def judge(baseline, measured, factor=GROSS):
    rows = []
    failures = []
    for name in sorted(set(baseline) | set(measured)):
        before = baseline.get(name)
        now = measured.get(name)
        if before is None:
            failures.append(f"{name} has no baseline; run python3 src/benches.py --write")
        elif now is None:
            failures.append(f"{name} has a baseline but did not run")
        else:
            ratio = now / before
            rows.append((name, before, now, ratio))
            if ratio > factor:
                failures.append(f"{name} took {now / before:.1f}x its baseline ({format_time(now)} against {format_time(before)})")
    return rows, failures


def format_time(nanos):
    for unit, scale in (("s", 1e9), ("ms", 1e6), ("µs", 1e3)):
        if nanos >= scale:
            return f"{nanos / scale:.3g} {unit}"
    return f"{nanos:.3g} ns"


def summary(rows, failures):
    lines = ["| Benchmark | Baseline | Median | Ratio |", "| --- | ---: | ---: | ---: |"]
    lines += [f"| `{name}` | {format_time(before)} | {format_time(now)} | {ratio:.2f}x |" for name, before, now, ratio in rows]
    lines += [f"\n**Regressed:** {failure}" for failure in failures]
    return "\n".join(lines) + "\n"


def benches(cargo):
    metadata = json.loads(
        subprocess.run([*cargo, "metadata", "--locked", "--no-deps", "--format-version", "1"], check=True, stdout=subprocess.PIPE, text=True).stdout
    )
    owners = {
        package["manifest_path"]: package["name"]
        for package in metadata["packages"]
        if any("bench" in target["kind"] for target in package["targets"])
    }
    selected = [argument for manifest in sorted(owners) for argument in ("-p", owners[manifest])]
    built = subprocess.run(
        [*cargo, "bench", "--locked", "--profile", "dev", "--no-run", "--message-format=json", *selected, "--bench", "*"],
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    ).stdout
    for line in built.splitlines():
        message = json.loads(line)
        if message.get("reason") == "compiler-artifact" and message["target"]["kind"] == ["bench"] and message.get("executable"):
            yield owners[message["manifest_path"]], Path(message["manifest_path"]).parent, message["executable"]


def run(cargo):
    measured = {}
    for package, directory, executable in benches(cargo):
        output = subprocess.run([executable, "--bench"], cwd=directory, check=True, stdout=subprocess.PIPE, text=True).stdout
        measured.update({f"{package}/{name}": nanos for name, nanos in medians(output).items()})
    return measured


def main():
    parser = argparse.ArgumentParser(description="Run the divan benches and compare their medians with the checked-in baseline.")
    parser.add_argument("--write", action="store_true", help="replace the baseline with this run's medians")
    parser.add_argument("--cargo", default="cargo", help="the cargo command, split on spaces")
    arguments = parser.parse_args()
    cargo = arguments.cargo.split()
    measured = run(cargo)
    if arguments.write:
        BASELINE.write_text(json.dumps({name: round(nanos) for name, nanos in sorted(measured.items())}, indent=2) + "\n")
        return 0
    rows, failures = judge(json.loads(BASELINE.read_text()), measured)
    report = summary(rows, failures)
    sys.stdout.write(report)
    if "GITHUB_STEP_SUMMARY" in os.environ:
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as handle:
            handle.write(report)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
