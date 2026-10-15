#!/usr/bin/env python3
"""Verify the committed package files against their reviewed checksums."""
import hashlib
import json
from pathlib import Path

root = Path(__file__).resolve().parent
provenance = json.loads((root / 'PROVENANCE.json').read_text())
expected = {entry['path']: entry['sha256'] for entry in provenance['retained_files']}
actual = {
    str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
    for package in ('syntect', 'onig')
    for path in (root / package).rglob('*')
    if path.is_file()
}
if actual != expected:
    for name in sorted(actual.keys() | expected.keys()):
        if actual.get(name) != expected.get(name):
            print(f'Checksum or file inventory mismatch: {name}')
    raise SystemExit(1)
print(f'Verified {len(actual)} retained dependency files.')
