#!/usr/bin/env python3
"""Measure a private Editor child without managing any existing Editor or Forge."""

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import time

from x11_window import X11Window


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("executable", type=Path)
    parser.add_argument("output_directory", type=Path)
    parser.add_argument("--fixture-editor-data", type=Path)
    parser.add_argument("--link")
    parser.add_argument("--scenario", default="unpaired-home")
    parser.add_argument("--build", default="unspecified")
    parser.add_argument("--seconds", type=float, default=60)
    parser.add_argument("--warmup", type=float, default=20)
    parser.add_argument("--renderer-diagnostics", action="store_true")
    parser.add_argument("--require-completed-conversation", action="store_true")
    parser.add_argument("--xdotool", type=Path, help="Use X11 and independently sample the owned window")
    parser.add_argument("--x11-scale", type=float, default=1.0)
    parser.add_argument("--child-env", action="append", default=[], metavar="NAME=VALUE",
                        help="Set a diagnostic variable only in the owned Editor child")
    parser.add_argument("--capture-with", type=Path,
                        help="ImageMagick import executable; capture the owned X11 window after sampling")
    parser.add_argument("--window-origin", type=int, nargs=2, metavar=("X", "Y"),
                        help="Place only the owned X11 child before and after warmup, without resizing or focusing it")
    args = parser.parse_args()
    if sys.platform != "linux":
        parser.error("requires Linux")
    if not math.isfinite(args.seconds) or not math.isfinite(args.warmup) or args.seconds <= 0 or args.warmup < 0:
        parser.error("seconds must be positive and warmup must be non-negative")
    if not math.isfinite(args.x11_scale) or args.x11_scale <= 0:
        parser.error("x11-scale must be finite and positive")
    if (args.capture_with or args.window_origin) and not args.xdotool:
        parser.error("capture and placement require --xdotool for window ownership checks")
    child_environment = {}
    for entry in args.child_env:
        name, separator, value = entry.partition("=")
        if not separator or not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name):
            parser.error("child environment must use NAME=VALUE")
        if name in {
            "ARTISAN_EDITOR_DATA", "ARTISAN_EDITOR_SCRIPT", "ARTISAN_EDITOR_CAPTURE",
            "ARTISAN_EDITOR_CAPTURE_DELAY", "ARTISAN_EDITOR_WINDOW_ORIGIN",
        }:
            parser.error(f"{name} is reserved by the isolated launcher")
        child_environment[name] = value
    output = args.output_directory.resolve()
    output.mkdir(parents=True, exist_ok=True)
    if any(output.iterdir()):
        parser.error("output directory must be empty")
    run_directory = Path(tempfile.mkdtemp(prefix="artisan-idle-"))
    data = run_directory / "editor"
    data.mkdir()
    if args.fixture_editor_data:
        for name in ("identity.json", "hosts.json", "settings.json"):
            source = args.fixture_editor_data / name
            if source.is_file():
                shutil.copyfile(source, data / name)
    binary = run_directory / "artisan-editor"
    shutil.copy2(args.executable, binary)
    with binary.open("rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    environment = os.environ.copy()
    for name in (
        "ARTISAN_EDITOR_SCRIPT", "ARTISAN_EDITOR_CAPTURE",
        "ARTISAN_EDITOR_CAPTURE_DELAY", "ARTISAN_EDITOR_WINDOW_ORIGIN",
    ):
        environment.pop(name, None)
    environment.update(ARTISAN_EDITOR_DATA=str(data), RUST_LOG="info,gpui=warn,gpui::renderer_memory=info")
    if args.xdotool:
        environment.pop("WAYLAND_DISPLAY", None)
        environment["GPUI_X11_SCALE_FACTOR"] = str(args.x11_scale)
    environment.pop("GPUI_RENDERER_DIAGNOSTICS", None)
    if args.renderer_diagnostics:
        environment["GPUI_RENDERER_DIAGNOSTICS"] = "1"
    environment.update(child_environment)
    arguments = [str(binary)]
    if args.link:
        arguments.append(args.link)
    child = None
    try:
        with (output / "stderr.log").open("wb") as log:
            child = subprocess.Popen(arguments, env=environment, stdout=log, stderr=log)
            print(f"Measuring isolated Editor PID {child.pid}, artifacts {output}", flush=True)
            if args.window_origin:
                deadline = time.monotonic() + 30
                while True:
                    if child.poll() is not None:
                        raise RuntimeError("Editor exited before placement")
                    try:
                        owned_window = X11Window.find(args.xdotool, child.pid)
                        owned_window.read()
                        break
                    except RuntimeError:
                        if time.monotonic() >= deadline:
                            raise
                        time.sleep(0.1)
                subprocess.run([
                    str(args.xdotool), "windowmove", "--sync", str(owned_window.window),
                    *map(str, args.window_origin),
                ], check=True, timeout=10)
            deadline = time.monotonic() + args.warmup
            while time.monotonic() < deadline:
                if child.poll() is not None:
                    raise RuntimeError("Editor exited during warmup")
                time.sleep(min(0.1, max(0, deadline - time.monotonic())))
            if args.require_completed_conversation:
                editor_log = (data / "editor.log").read_text()
                if not any("thread opened thread=idle-conversation " in line and
                           "items=40 rows=40 settled=true" in line for line in editor_log.splitlines()):
                    raise RuntimeError("completed 40-item conversation did not settle")
            window = X11Window.find(args.xdotool, child.pid) if args.xdotool else None
            placement = None
            if args.window_origin:
                # Some window managers override startup placement after mapping.
                # Reapply it before sampling, then allow the move to settle.
                before = window.read()
                subprocess.run([
                    str(args.xdotool), "windowmove", "--sync", str(window.window),
                    *map(str, args.window_origin),
                ], check=True, timeout=10)
                time.sleep(1)
                placement = {"before": before, "after": window.read(), "settle_seconds": 1}
            context = {
                "binary_sha256": digest,
                "binary_bytes": binary.stat().st_size,
                "source_executable": str(args.executable.resolve()),
                "executable_copy": str(binary),
                "editor_data": str(data),
                "pid": child.pid,
                "scenario": args.scenario,
                "build": args.build,
                "warmup_seconds": args.warmup,
                "completed_conversation_verified": args.require_completed_conversation,
                "visible_conversation_verified": False,
                "child_environment_overrides": child_environment,
                "requested_window_origin": args.window_origin,
                "placement_after_warmup": placement,
                "renderer_diagnostics": args.renderer_diagnostics,
                "window_state_verified": False,
                "x11_window_at_start": window.read() if window else None,
                "graphics_environment": {key: environment[key] for key in (
                    "WAYLAND_DISPLAY", "DISPLAY", "VK_DRIVER_FILES", "WGPU_BACKEND", "GALLIUM_DRIVER",
                    "MESA_LOADER_DRIVER_OVERRIDE", "LIBGL_DRIVERS_PATH", "MIMALLOC_ALLOW_THP",
                    "GPUI_X11_SCALE_FACTOR"
                ) if key in environment},
            }
            (output / "context.json").write_text(json.dumps(context, indent=2) + "\n")
            sampler = [
                sys.executable, str(Path(__file__).with_name("measure-idle.py")), str(child.pid),
                "--seconds", str(args.seconds), "--scenario", args.scenario, "--build", args.build,
                "--output", str(output / "measurement.json"),
            ]
            if window:
                sampler.extend(["--x11-window", str(window.window), "--xdotool", str(args.xdotool)])
            subprocess.run(sampler, check=True)
            (output / "smaps.txt").write_text(Path(f"/proc/{child.pid}/smaps").read_text())
            if args.capture_with:
                window.read()
                capture_environment = os.environ.copy()
                # A system capture tool must use its own libraries, not the
                # Editor's Nix development-shell search paths.
                capture_environment.pop("LD_LIBRARY_PATH", None)
                capture_environment.pop("LD_PRELOAD", None)
                subprocess.run([
                    str(args.capture_with), "-silent", "-window", str(window.window),
                    "-depth", "8", str(output / "window-after-sample.png"),
                ], env=capture_environment, check=True, timeout=30)
                png_header = (output / "window-after-sample.png").read_bytes()[:24]
                if png_header[:8] != b"\x89PNG\r\n\x1a\n":
                    raise RuntimeError("capture tool did not produce a PNG")
                capture_width, capture_height = struct.unpack(">II", png_header[16:24])
                captured_window = window.read()
                context["capture_after_sample"] = {
                    "path": str(output / "window-after-sample.png"),
                    "tool": str(args.capture_with),
                    "window": captured_window,
                    "width": capture_width,
                    "height": capture_height,
                    "covers_client": (capture_width, capture_height) == (
                        captured_window["width"], captured_window["height"]),
                    "visually_verified": False,
                }
                (output / "context.json").write_text(json.dumps(context, indent=2) + "\n")
            result = json.loads((output / "measurement.json").read_text())
            print(json.dumps(result["summary"], indent=2))
    finally:
        if child is not None:
            if child.poll() is None:
                child.terminate()
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()
            print(f"Closed isolated Editor PID {child.pid}", flush=True)
        if (data / "editor.log").is_file():
            shutil.copyfile(data / "editor.log", output / "editor.log")


if __name__ == "__main__":
    main()
