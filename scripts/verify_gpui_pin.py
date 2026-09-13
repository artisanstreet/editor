#!/usr/bin/env python3
"""Verify the pinned fork commit is reachable from artisan/editor (requires gh)."""
import argparse
import json
from pathlib import Path
import subprocess
import tomllib
from build_support import ROOT

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--root', type=Path, default=ROOT)
args = parser.parse_args()
manifest = tomllib.loads((args.root / 'Cargo.toml').read_text())
deps = manifest['workspace']['dependencies']
rev = deps['gpui']['rev']
if deps['gpui_platform']['rev'] != rev:
    raise SystemExit('GPUI and platform pins differ')

def api(endpoint):
    try:
        return json.loads(subprocess.check_output(['gh', 'api', f'repos/artisanstreet/gpui-ce/{endpoint}']))
    except subprocess.CalledProcessError as error:
        raise SystemExit(f'GitHub verification failed (gh exit {error.returncode}); see the diagnostic above.') from None

if api(f'commits/{rev}')['sha'] != rev:
    raise SystemExit('Pinned commit did not resolve exactly')
if api(f'compare/{rev}...artisan/editor')['status'] not in ('ahead', 'identical'):
    raise SystemExit('Pin is not reachable from artisan/editor')
print(f'GPUI pin {rev} is reachable from artisan/editor.')
