#!/usr/bin/env python3
import hashlib
import json
import shutil
import subprocess
import tempfile
from pathlib import Path

root = Path(__file__).resolve().parent
provenance = json.loads((root / 'PROVENANCE.json').read_text())
records = {entry['path']: entry for entry in provenance['retained_files']}
own = {'PROVENANCE.json', 'fork.patch', 'verify.py'}
present = {
    str(path.relative_to(root))
    for path in root.rglob('*')
    if path.is_file() and str(path.relative_to(root)) not in own
}


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


failures = sorted(f'File inventory mismatch: {name}' for name in present ^ records.keys())
failures += sorted(
    f'Checksum mismatch: {name}'
    for name in present & records.keys()
    if digest(root / name) != records[name]['sha256']
)
with tempfile.TemporaryDirectory() as scratch:
    for name in records.keys() & present:
        target = Path(scratch) / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(root / name, target)
    reverted = subprocess.run(
        ['patch', '-R', '-p1', '-s', '-i', str(root / 'fork.patch')], cwd=scratch
    )
    if reverted.returncode != 0:
        failures.append('fork.patch does not reverse-apply to the vendored files')
    failures += sorted(
        f'Upstream checksum mismatch after reverting fork.patch: {name}'
        for name in records.keys() & present
        if (Path(scratch) / name).exists()
        and digest(Path(scratch) / name) != records[name]['upstream_sha256']
    )
if failures:
    print('\n'.join(failures))
    raise SystemExit(1)
print(f'Verified {len(records)} vendored Mermaid files against upstream through fork.patch.')
