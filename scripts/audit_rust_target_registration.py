#!/usr/bin/env python3
"""Check that external Rust tests are reachable from Cargo targets."""
import re
from build_support import ROOT, metadata

seen = set()


def visit(path):
    path = path.resolve()
    if path in seen or not path.is_file():
        return
    seen.add(path)
    text = path.read_text()
    for relative in re.findall(r'#\[path\s*=\s*"([^"]+)"\]', text):
        visit(path.parent / relative)
    # Ordinary modules may be children of lib.rs, main.rs, mod.rs or foo.rs.
    base = path.parent if path.name in ('lib.rs', 'main.rs', 'mod.rs') else path.with_suffix('')
    for name in re.findall(r'\bmod\s+(\w+)\s*;', text):
        visit(base / (name + '.rs'))
        visit(path.parent / (name + '.rs'))
        visit(base / name / 'mod.rs')


for pkg in metadata()['packages']:
    for target in pkg['targets']:
        visit(__import__('pathlib').Path(target['src_path']))
missing = sorted(str(p.relative_to(ROOT)) for p in (ROOT / 'tests').rglob('*.rs')
                 if p.resolve() not in seen)
if missing:
    raise SystemExit('Rust test sources not reachable from Cargo:\n' + '\n'.join(missing))
print('All external Rust test sources are reachable from Cargo.')
