#!/usr/bin/env python3
"""Run the Cargo quality gates, including child fixtures and packaging proofs."""
import argparse
import os
import sys
import tempfile
from pathlib import Path
from build_support import build, executable, run
from package import package

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--tests-only', action='store_true', help='run tests with already-built fixtures')
parser.add_argument('--bin-dir', type=Path)
parser.add_argument('--runner', choices=['cargo', 'nextest'], default='cargo')
parser.add_argument('--package', help='restrict test execution to one Cargo package')
args = parser.parse_args()
scope = ['-p', args.package] if args.package else ['--workspace']
features = ['--features', 'artisan-frontend/visual-proof'] if args.package in (None, 'artisan-frontend') else []
if not args.tests_only:
    run('cargo', 'fmt', '--all', '--', '--check')
    run(sys.executable, 'scripts/audit_rust_target_registration.py')
    run(sys.executable, 'scripts/file_size_ratchet.py')
    run('cargo', 'clippy', '--locked', '--workspace', '--all-targets', '--features', 'artisan-frontend/visual-proof')
bin_dir = args.bin_dir.resolve() if args.bin_dir else build(examples=True)
env = os.environ.copy()
for variable, name in [
    ('ARTISAN_ENGINE_OWNER_FIXTURE', 'engine-owner-fixture'),
    ('ARTISAN_CODEX_WIRE_FIXTURE', 'codex-wire-fixture'),
    ('ARTISAN_DIRECTORY_CONTROLLER_FIXTURE', 'directory-controller-fixture'),
]:
    env[variable] = str(executable(bin_dir / 'examples', name))
with tempfile.TemporaryDirectory() as tmp:
    # Exercise the exact producer with bounded inputs. Debug UI binaries can
    # exceed a gigabyte and obscure these structural and tampering proofs.
    payload_dir = Path(tmp) / 'binaries'
    payload_dir.mkdir()
    for name in ('ae', 'editor', 'forge', 'installer'):
        executable(payload_dir, name).write_bytes(('archive fixture: ' + name).encode())
    for variable, filename in [
        ('ARTISAN_VERSIONED_PAYLOAD_ARCHIVE', 'payload.zip'),
        ('ARTISAN_VERSIONED_PAYLOAD_ARCHIVE_REPRODUCIBILITY', 'repeat.zip'),
    ]:
        env[variable] = str(package(payload_dir, Path(tmp) / filename,
                                    executable(bin_dir, 'payload-manifest-generator')))
    for name in ('ae', 'editor', 'forge', 'installer'):
        env[f'ARTISAN_VERSIONED_PAYLOAD_{name.upper()}_BINARY'] = str(executable(payload_dir, name))
    if args.runner == 'nextest':
        run('cargo', 'nextest', 'run', '--locked', '--offline', '--no-fail-fast', *scope,
            '--all-targets', *features, '--test-threads=1', env=env)
    else:
        run('cargo', 'test', '--locked', '--no-fail-fast', *scope, '--all-targets',
            *features, '--', '--test-threads=1', env=env)
    if not args.tests_only:
        run('cargo', 'test', '--locked', *scope, '--doc', env=env)
