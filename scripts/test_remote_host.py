#!/usr/bin/env python3
"""Exercise remote Forge authentication, persisted reconnect, restart, and identity rejection."""
import argparse
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import time


def wait_file(path, child):
    deadline = time.monotonic() + 30
    while not path.exists():
        if child.poll() is not None:
            raise RuntimeError('Forge exited before readiness')
        if time.monotonic() >= deadline:
            raise TimeoutError('Forge readiness')
        time.sleep(0.05)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bin-dir', type=Path, required=True)
    parser.add_argument('--helper', type=Path)
    parser.add_argument('--address', default='127.0.0.1')
    args = parser.parse_args()
    binaries = args.bin_dir.resolve()
    with tempfile.TemporaryDirectory(prefix='artisan-host-smoke-') as temporary:
        root = Path(temporary)
        env = dict(os.environ, XDG_DATA_HOME=str(root / 'client'))
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as reservation:
            reservation.bind((args.address, 0))
            port = reservation.getsockname()[1]
        endpoint = f'{args.address}:{port}'
        invitation = root / 'forge/host.json'
        command = [str(args.helper.resolve() if args.helper else binaries / 'forge-host'), '--home', str(root / 'forge'),
                   '--forge', str(binaries / 'forge'), '--listen', endpoint, '--advertise', endpoint,
                   '--name', 'Remote integration host']
        with (root / 'forge.log').open('w') as log:
            def start():
                process = subprocess.Popen(command, env=env, stdout=log, stderr=log, start_new_session=True)
                try:
                    wait_file(root / 'forge/readiness.json', process)
                except Exception:
                    stop(process)
                    print((root / 'forge.log').read_text()[-4000:])
                    raise
                deadline = time.monotonic() + 30
                while time.monotonic() < deadline:
                    if invitation.exists() and json.loads(invitation.read_text())['pid'] != previous_pid:
                        return process
                    time.sleep(.05)
                raise TimeoutError('new invitation')

            def stop(process):
                if process.poll() is None:
                    os.killpg(process.pid, signal.SIGINT)
                    try:
                        process.wait(timeout=15)
                    except subprocess.TimeoutExpired:
                        os.killpg(process.pid, signal.SIGKILL)
                        process.wait()
                deadline = time.monotonic() + 5
                while (root / 'forge/readiness.json').exists() and time.monotonic() < deadline:
                    time.sleep(.05)

            def probe(home, succeeds=True):
                result = subprocess.run([str(binaries / 'editor'), '--host-home', home, '--probe-host'],
                                        env=env, text=True, capture_output=True, timeout=50)
                if (result.returncode == 0) != succeeds:
                    raise AssertionError(result.stdout + result.stderr)

            previous_pid = None
            process = start()
            try:
                imported = subprocess.run([str(binaries / 'editor'), '--import-host', str(invitation)],
                                          env=env, check=True, text=True, capture_output=True).stdout.strip()
                probe(imported)
                probe(imported)
                assert process.poll() is None, 'client disconnect stopped remote daemon'
                previous_pid = json.loads(invitation.read_text())['pid']
                stop(process)
                process = start()
                # Reuse the original registration, refreshing its trusted source after restart.
                probe(imported)
                # An invitation source may update routing but must never replace the trusted identity.
                original = invitation.read_bytes()
                changed = json.loads(original)
                changed['certificate'] = 'AQIDBA=='
                invitation.write_text(json.dumps(changed))
                try:
                    probe(imported, succeeds=False)
                finally:
                    invitation.write_bytes(original)
                probe(imported)
                print('Remote host: initial query, reconnect, daemon restart, identity rejection, and disconnect passed.')
            finally:
                stop(process)


if __name__ == '__main__':
    main()
