#!/usr/bin/env python3
"""Install a per-user Forge service on Linux, including Ubuntu WSL."""
import argparse
import ipaddress
import os
import socket
from pathlib import Path
import subprocess
import sys
import time


def systemd_quote(value):
    """Quote one literal systemd ExecStart argument, without shell expansion."""
    if any(c in value for c in '\n\r\0'):
        raise ValueError('control characters are not supported in service arguments')
    return '"' + value.replace('\\', '\\\\').replace('"', '\\"').replace('%', '%%').replace('$', '$$') + '"'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--home', type=Path, default=Path.home() / '.local/state/artisan-forge')
    parser.add_argument('--port', type=int, default=4433)
    parser.add_argument('--address', help='Reachable IPv4 address; defaults to the IPv4 address selected by the default route')
    parser.add_argument('--bin-dir', type=Path, help='Use already built forge and forge-host binaries')
    parser.add_argument('--name', default=os.environ.get('WSL_DISTRO_NAME', 'Linux Forge'))
    args = parser.parse_args()
    if sys.platform != 'linux':
        parser.error('run this installer inside the Linux host / WSL distro')
    if not 1 <= args.port <= 65535:
        parser.error('--port must be between 1 and 65535')
    if args.address:
        address = ipaddress.IPv4Address(args.address)
    else:
        # UDP connect selects a local route without sending application data.
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as route:
            route.connect(('192.0.2.1', 9))
            address = ipaddress.IPv4Address(route.getsockname()[0])
    if address.is_unspecified or address.is_multicast:
        parser.error('--address must identify a reachable interface')
    repo = Path(__file__).resolve().parents[1]
    home = args.home.expanduser().absolute()
    if args.bin_dir:
        forge = args.bin_dir.resolve() / 'forge'
        helper = args.bin_dir.resolve() / 'forge-host'
    else:
        paths = subprocess.check_output(['nix', 'build', '--no-link', '--print-out-paths', '.#forge', '.#forge-host'], cwd=repo, text=True).splitlines()
        forge = next(Path(p) / 'bin/forge' for p in paths if (Path(p) / 'bin/forge').exists())
        helper = next(Path(p) / 'bin/forge-host' for p in paths if (Path(p) / 'bin/forge-host').exists())
        # Keep installed service binaries alive across Nix garbage collection.
        roots = Path.home() / '.local/state/artisan-forge-roots'
        roots.mkdir(parents=True, exist_ok=True)
        for name, binary in [('forge', forge), ('forge-host', helper)]:
            subprocess.run(['nix-store', '--add-root', str(roots / name), '--indirect', '--realise', str(binary.parent.parent)], check=True, stdout=subprocess.DEVNULL)
    for binary in [forge, helper]:
        if not binary.is_file() or not os.access(binary, os.X_OK):
            parser.error(f'executable missing: {binary}')
    # Re-resolve the distro address whenever the service starts after a WSL restart.
    bind_address = str(address) if args.address else "auto"
    launcher = [str(helper), '--home', str(home), '--forge', str(forge),
                '--listen', f'{bind_address}:{args.port}', '--advertise', f'{bind_address}:{args.port}', '--name', args.name]
    unit = '[Unit]\nDescription=Artisan Forge host\nAfter=network-online.target\n\n[Service]\nType=simple\n'
    unit += 'ExecStart=' + ' '.join(map(systemd_quote, launcher)) + '\n'
    unit += 'Restart=on-failure\nRestartSec=2\n'
    unit += 'KillSignal=SIGINT\nKillMode=control-group\nTimeoutStopSec=15\nUMask=0077\n'
    unit += 'Environment=' + systemd_quote('PATH=' + os.environ.get('PATH', '/usr/bin:/bin')) + '\n'
    unit += '\n[Install]\nWantedBy=default.target\n'
    directory = Path.home() / '.config/systemd/user'
    directory.mkdir(parents=True, exist_ok=True)
    service = directory / 'artisan-forge.service'
    if service.exists() and service.read_text() != unit:
        parser.error(f'{service} already exists with different settings; review and remove it before reinstalling')
    service.write_text(unit)
    subprocess.run(['systemctl', '--user', 'daemon-reload'], check=True)
    subprocess.run(['systemctl', '--user', 'enable', '--now', 'artisan-forge.service'], check=True)
    deadline = time.monotonic() + 30
    while not (home / 'host.json').exists():
        if time.monotonic() >= deadline:
            raise RuntimeError('Forge did not publish its invitation; inspect journalctl --user -u artisan-forge.service')
        state = subprocess.run(['systemctl', '--user', 'is-failed', '--quiet', 'artisan-forge.service'])
        if state.returncode == 0:
            raise RuntimeError('Forge service failed; inspect journalctl --user -u artisan-forge.service')
        time.sleep(.2)
    print(f'Forge endpoint: {address}:{args.port}')
    print(f'Private host invitation: {home / "host.json"}')
    print('In the Windows editor: Machines → Add host from invitation. Select this file through \\\\wsl.localhost\\<distro>\\…')


if __name__ == '__main__':
    main()
