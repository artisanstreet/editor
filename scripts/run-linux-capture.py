#!/usr/bin/env python3
"""Capture one isolated Editor child; never manage an installed Editor or Forge."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("executable", type=Path)
    parser.add_argument("fixture", type=Path, help="fixture.json from a fresh idle_fixture")
    parser.add_argument("output_directory", type=Path)
    parser.add_argument("--script", type=Path, help="script whose {output} tokens become the output directory")
    parser.add_argument("--x11-scale", type=float, default=1.25)
    parser.add_argument("--interactivity-diagnostics", action="store_true")
    args = parser.parse_args()
    if not 0 < args.x11_scale <= 8:
        parser.error("X11 scale must be greater than zero and at most eight")
    binary = args.executable.resolve(strict=True)
    fixture = json.loads(args.fixture.read_text())
    output = args.output_directory.resolve()
    output.mkdir(parents=True, exist_ok=True)
    if any(output.iterdir()):
        parser.error("output directory must be empty")
    data = Path(tempfile.mkdtemp(prefix="artisan-capture-"))
    for name in ("identity.json", "hosts.json", "settings.json"):
        source = Path(fixture["editor_data"]) / name
        if source.is_file():
            shutil.copyfile(source, data / name)
    script = args.script.read_text() if args.script else """wait 5000
capture {output}/01-initial.png
wait 500
capture {output}/02-initial-repeat.png
scroll 750 400 1500
wait 500
capture {output}/03-scrolled-up.png
scroll 750 400 -1500
wait 500
capture {output}/04-scrolled-back.png
quit
"""
    script_path = output / "script.txt"
    script_path.write_text(script.replace("{output}", str(output)))
    environment = os.environ.copy()
    environment.pop("WAYLAND_DISPLAY", None)
    environment.update({
        "ARTISAN_EDITOR_DATA": str(data),
        "ARTISAN_EDITOR_SCRIPT": str(script_path),
        "GPUI_X11_SCALE_FACTOR": str(args.x11_scale),
        "GPUI_RENDERER_DIAGNOSTICS": "1",
        "LEAK_BACKTRACE": "1",
        "RUST_BACKTRACE": "1",
        "RUST_LOG": "info,gpui=warn,gpui::renderer_memory=info",
    })
    environment.pop("GPUI_INTERACTIVITY_DIAGNOSTICS", None)
    if args.interactivity_diagnostics:
        environment["GPUI_INTERACTIVITY_DIAGNOSTICS"] = "1"
    context = {
        "binary": str(binary),
        "binary_sha256": hashlib.file_digest(binary.open("rb"), "sha256").hexdigest(),
        "fixture": fixture,
        "editor_data": str(data),
        "platform": "X11",
        "scale": args.x11_scale,
        "interactivity_diagnostics": args.interactivity_diagnostics,
        "scope": "Scripted visual diagnostic, not memory or idle CPU acceptance",
    }
    child = None
    try:
        with (output / "stderr.log").open("wb") as log:
            child = subprocess.Popen([str(binary), fixture["link"]], env=environment, stdout=log, stderr=log)
            context["pid"] = child.pid
            (output / "context.json").write_text(json.dumps(context, indent=2) + "\n")
            print(f"Capturing isolated Editor PID {child.pid}, artifacts {output}", flush=True)
            child.wait(timeout=120)
    finally:
        if child is not None:
            if child.poll() is None:
                child.terminate()
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()
            context["exit_code"] = child.returncode
            (output / "context.json").write_text(json.dumps(context, indent=2) + "\n")
        if (data / "editor.log").exists():
            shutil.copyfile(data / "editor.log", output / "editor.log")
    if child.returncode:
        raise SystemExit(f"Capture Editor exited {child.returncode}; inspect stderr.log")
    if "script step failed" in (output / "editor.log").read_text():
        raise SystemExit("A script step failed; inspect editor.log")
    print("Captured " + ", ".join(path.name for path in sorted(output.glob("*.png"))))


if __name__ == "__main__":
    main()
