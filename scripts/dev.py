#!/usr/bin/env python3
"""Build, install, and launch the development installation.

A thin wrapper over `cargo dev` (the Rust runner in scripts/native_dev);
every argument is passed through, for example `python3 scripts/dev.py stage
--profile performance`. See `cargo dev --help`.
"""
import sys

from build_support import run

run('cargo', 'dev', *sys.argv[1:])
