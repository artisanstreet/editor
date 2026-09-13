#!/usr/bin/env python3
"""Generate unsigned release metadata for an existing payload archive."""
import argparse
import json
from pathlib import Path
from build_support import executable, metadata, run

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--archive', type=Path, required=True)
parser.add_argument('--metadata', type=Path, required=True,
                    help='public artifact metadata JSON; platform must match the archive binaries')
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--tool', type=Path, help='prebuilt release-tool; avoids Cargo')
args = parser.parse_args()
fields = json.loads(args.metadata.read_text())
allowed = {'format-version', 'product-version', 'editor-forge-compatibility-version',
           'channel', 'signing-key-id', 'algorithm', 'minimum-installer-version',
           'minimum-cli-version', 'artifact-id', 'platform', 'architecture', 'libc',
           'archive-format', 'file-name'}
if set(fields) != allowed or not all(isinstance(v, str) for v in fields.values()):
    parser.error('metadata must contain exactly the public release fields as strings')
tool = args.tool
if tool is None:
    run('cargo', 'build', '--locked', '-p', 'artisan-packaging', '--bin', 'release-tool')
    tool = executable(Path(metadata()['target_directory']) / 'debug', 'release-tool')
command = [str(tool), 'generate', '--archive', str(args.archive.resolve()),
           '--output', str(args.output.resolve())]
for key, value in fields.items():
    command.extend(['--' + key, value])
run(*command)
