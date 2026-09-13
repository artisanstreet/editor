"""Shared Cargo invocation and output discovery for repository commands."""
import json
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parent.parent
# Native UI test linking is memory intensive on WSL. Callers can override this.
os.environ.setdefault("CARGO_BUILD_JOBS", "2")


def run(*args, **kwargs):
    subprocess.run(args, cwd=ROOT, check=True, **kwargs)


def metadata():
    return json.loads(subprocess.check_output(
        ['cargo', 'metadata', '--locked', '--no-deps', '--format-version', '1'], cwd=ROOT))


def build(profile='dev', examples=False):
    args = ['cargo', 'build', '--locked', '--workspace', '--bins', '--profile', profile]
    if examples:
        args.append('--examples')
    run(*args)
    return Path(metadata()['target_directory']) / ('debug' if profile == 'dev' else profile)


def executable(directory, name):
    return directory / (name + ('.exe' if os.name == 'nt' else ''))
