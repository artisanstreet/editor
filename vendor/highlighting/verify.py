#!/usr/bin/env python3
import hashlib
import json
from pathlib import Path

root = Path(__file__).resolve().parent
repository = root.parents[1]
provenance = json.loads((root / 'PROVENANCE.json').read_text())
expected = {entry['path']: entry['sha256'] for entry in provenance['retained_files']}
actual = {
    str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
    for package in ('syntect', 'onig', 'onig_sys')
    for path in (root / package).rglob('*')
    if path.is_file()
}
failures = [
    f'Checksum or file inventory mismatch: {name}'
    for name in sorted(actual.keys() | expected.keys())
    if actual.get(name) != expected.get(name)
]
for entry in provenance['integration_inputs']:
    path = repository / entry['path']
    if not path.is_file():
        failures.append(f'Integration input is missing: {entry["path"]}')
    elif entry['wires'] not in path.read_text():
        failures.append(
            f'Integration input no longer wires the vendored packages: {entry["path"]} lacks {entry["wires"]}'
        )
if failures:
    print('\n'.join(failures))
    raise SystemExit(1)
print(f'Verified {len(actual)} retained dependency files and {len(provenance["integration_inputs"])} integration inputs.')
