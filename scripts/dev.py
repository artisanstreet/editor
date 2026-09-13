#!/usr/bin/env python3
"""Build and launch the isolated development installation."""
import argparse
from build_support import ROOT, build, executable, run

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--profile', choices=['dev', 'performance', 'release'], default='dev')
args, launcher_args = parser.parse_known_args()
bin_dir = build(args.profile)
run(str(executable(bin_dir, 'dev')), '--bin-dir', str(bin_dir),
    '--dev-dir', str(ROOT / '.dist/dev'), *launcher_args)
