#!/usr/bin/env python3
"""Generate bindings, or check them without changing the checkout."""
import argparse
from pathlib import Path
import tempfile
import subprocess
from build_support import ROOT, executable, metadata, run


def generate(plugin, compiler, output):
    output.mkdir(parents=True, exist_ok=True)
    subprocess.run([compiler, 'compile', '--no-standard-import', '--src-prefix=schema',
                    f'-o{plugin}:{output.resolve()}', 'schema/phase1_proof.capnp',
                    'schema/artisan.capnp', 'schema/composer_state.capnp'], cwd=ROOT / 'modules/protocol', check=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--plugin', type=Path)
    parser.add_argument('--compiler', default='capnp')
    parser.add_argument('--output', type=Path, default=ROOT / 'modules/protocol/src')
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    if args.plugin is None:
        run('cargo', 'build', '--locked', '-p', 'artisan-capnp-codegen')
        args.plugin = executable(Path(metadata()['target_directory']) / 'debug', 'artisan-capnp-codegen')
    if args.check:
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp)
            generate(args.plugin.resolve(), args.compiler, output)
            drift = [p.name for p in output.glob('*.rs')
                     if not (args.output / p.name).exists()
                     or p.read_bytes() != (args.output / p.name).read_bytes()]
            if drift:
                raise SystemExit('Generated bindings differ: ' + ', '.join(drift))
    else:
        generate(args.plugin.resolve(), args.compiler, args.output)
