#!/usr/bin/env python3
"""Capture only the X11 window owned by a newly launched visual harness."""
import argparse
import os
from pathlib import Path
import signal
import struct
import subprocess
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--program', type=Path, required=True)
parser.add_argument('--output', type=Path, default=Path('evidence/visual/screen-demo.png'))
parser.add_argument('--software', action='store_true')
parser.add_argument('--software-icd', type=Path, required=True)
args = parser.parse_args()
args.output = args.output.resolve()
args.output.parent.mkdir(parents=True, exist_ok=True)
env = os.environ.copy()
env.pop('WAYLAND_DISPLAY', None)  # Capture this child through X11/XWayland.
if args.software:
    env['VK_DRIVER_FILES'] = str(args.software_icd)
process = subprocess.Popen([str(args.program)], env=env, start_new_session=True)
try:
    deadline = time.monotonic() + 45
    window = None
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise SystemExit(f'Visual harness exited before opening a window: {process.returncode}')
        result = subprocess.run(['xdotool', 'search', '--onlyvisible', '--pid', str(process.pid)],
                                capture_output=True, text=True, timeout=2, check=False)
        if result.returncode == 0 and result.stdout.splitlines():
            window = result.stdout.splitlines()[0]
            break
        time.sleep(0.25)
    if window is None:
        raise SystemExit('No visible harness window appeared within 45 seconds')
    time.sleep(1)  # Let the compositor present the initial frame.
    subprocess.run(['magick', 'import', '-window', window, 'png:' + str(args.output)],
                   check=True, timeout=10)
    header = args.output.read_bytes()[:24]
    if header[:8] != b'\x89PNG\r\n\x1a\n' or len(header) != 24:
        raise SystemExit('Capture did not produce a PNG')
    width, height = struct.unpack('>II', header[16:24])
    if min(width, height) < 64:
        raise SystemExit(f'Harness window was unexpectedly small: {width}x{height}')
    print(f'Captured harness PID {process.pid}, X11 window {window}: {args.output} ({width}x{height})')
finally:
    if process.poll() is None:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
