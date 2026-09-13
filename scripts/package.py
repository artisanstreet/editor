#!/usr/bin/env python3
"""Assemble a deterministic payload ZIP using Cargo's manifest generator."""
import argparse
from pathlib import Path
import tempfile
import zipfile
from build_support import ROOT, build, executable, run


def package(bin_dir, output, generator=None, layout=None):
    output = Path(output).resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    entries = [(f'bin/{executable(bin_dir, name).name}', executable(bin_dir, name))
               for name in ('ae', 'editor', 'forge', 'installer')]
    with tempfile.TemporaryDirectory() as tmp:
        manifest = Path(tmp) / 'payload-manifest.json'
        args = [str(generator or executable(bin_dir, 'payload-manifest-generator')),
                '--layout', str(layout or ROOT / 'packaging/portable/versioned_layout.txt'),
                '--output', str(manifest)]
        for member, source in entries:
            args += ['--file', member, str(source)]
        run(*args)
        entries.append(('payload-manifest.json', manifest))
        with zipfile.ZipFile(output, 'w', compression=zipfile.ZIP_STORED) as archive:
            for member, source in sorted(entries):
                info = zipfile.ZipInfo(member, (2010, 1, 1, 0, 0, 0))
                info.create_system = 3
                info.create_version = 0
                info.extract_version = 10
                info.external_attr = 0o100777 << 16
                archive.writestr(info, source.read_bytes())
    return output


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--profile', choices=['dev', 'performance', 'release'], default='dev')
    parser.add_argument('--output', type=Path, default=ROOT / '.dist/artisan-editor-versioned-payload.zip')
    parser.add_argument('--bin-dir', type=Path, help='consume prebuilt binaries without Cargo')
    parser.add_argument('--generator', type=Path)
    parser.add_argument('--layout', type=Path)
    args = parser.parse_args()
    print(package(args.bin_dir or build(args.profile), args.output, args.generator, args.layout))
