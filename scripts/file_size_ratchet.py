#!/usr/bin/env python3
"""Enforce frozen Rust file sizes; --update only shrinks existing allowances."""
import argparse
from pathlib import Path
from build_support import ROOT

GENERATED = {'modules/protocol/src/artisan_capnp.rs',
             'modules/protocol/src/phase1_proof_capnp.rs',
             'modules/protocol/src/composer_state_capnp.rs'}


def inspect(root, limit):
    allowed = {}
    for line in (root / 'scripts/file-size-allowlist.txt').read_text().splitlines():
        if line.strip() and not line.lstrip().startswith('#'):
            count, path = line.split(maxsplit=1)
            allowed[path] = int(count)
    sizes = {}
    for path in (root / 'modules').rglob('*.rs'):
        relative = path.relative_to(root).as_posix()
        if relative in GENERATED or 'target' in path.relative_to(root).parts:
            continue
        count = len(path.read_text().splitlines())
        if count > limit:
            sizes[relative] = count
    findings = [f'{path}: {count} lines, allowed {allowed.get(path, limit)}'
                for path, count in sorted(sizes.items())
                if count > allowed.get(path, limit)]
    return sizes, findings


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=ROOT)
    parser.add_argument('--limit', type=int, default=800)
    parser.add_argument('--update', action='store_true')
    args = parser.parse_args()
    sizes, findings = inspect(args.root, args.limit)
    if findings:
        raise SystemExit('\n'.join(findings))
    if args.update:
        (args.root / 'scripts/file-size-allowlist.txt').write_text(
            ''.join(f'{count} {path}\n' for path, count in sorted(sizes.items())))
    print(f'File-size ratchet clean: {len(sizes)} frozen files; none grew.')
