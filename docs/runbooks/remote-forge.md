# Forge hosts, including WSL

The native editor connects directly to an authenticated Forge over QUIC/UDP.
WSL uses the same connection protocol as another Linux machine. The Editor has
no Forge of its own; every Forge it uses is a registered host.

## A Linux installation as a host

An Artisan installation on Linux runs its Forge as a systemd user service it
owns, and serves other machines once its Forge has host access:

```sh
ae setup <instance values> --listen auto:4433 --host-name Ubuntu --autostart
ae start
```

- `--listen auto:PORT` binds the IPv4 source address of the default route,
  resolved at each start, so a WSL distribution whose address changes keeps
  publishing a current endpoint; `--listen IP:PORT` binds one fixed address.
  Without `--listen` the Forge is loopback-only.
- `--autostart` writes and enables `artisan-forge.service` (the dev channel's
  installation: `artisan-forge-dev.service`), which runs the installation's
  `ae start --foreground`. `ae start` starts it through the user manager.
- Each start publishes a private invitation at `<installation>/host.json`
  (`~/.local/share/Artisan Street Dev/host.json` for the dev installation).

For development, `nix run .#dev` does all of this from the checkout inside the
distro and registers the invitation with the Windows Editor it installs; see
[native development](native-dev.md). It runs as your Linux user, with access to
that user's projects, Git, and engine tools. A running WSL distro and user
manager are required; adding a saved host does not start a stopped distro.

The invitation contains the endpoint, public certificate, initial capability,
and daemon incarnation. Transfer it through a trusted channel. The private key
stays on the Linux machine; neither secret is placed in the Nix store or passed
as a command line argument. Files and directories use the existing private
credential storage boundary (Unix permissions or Windows ACLs).

## Add and select the machine

`nix run .#dev` registers the dev Forge with the dev Editor itself. To add a
host by hand, open the bottom-left profile menu, then its avatar/name/host
header (or use Ctrl+K), and select **Add host from invitation…**. For the dev
installation in this Ubuntu distro, select:

```text
\\wsl.localhost\Ubuntu\home\sander\.local\share\Artisan Street Dev\host.json
```

Import trusts the certificate in the selected invitation. Only import invitations
from a host you intend to control. Use the entire profile header as the ghost host selector, or press Ctrl+Shift+M. Arrow keys and Enter select a
host; Escape dismisses the dropdown. Saved hosts also appear in Ctrl+K's
**Machines** group. `editor.exe --machines` opens the dropdown at startup.

The profile name uses the local identity while signed out. When a future Artisan
Street account session supplies an identity, its display name takes precedence.
The avatar, display name, host subtitle, and selector icon form one clickable
header. Usage and its menu action are hidden when Forge is disconnected.
The host subtitle remains separate: the registered host's name or address
("This computer on WSL" for a host whose invitation lives in a local WSL distribution). Account sign-in and cloud sync are not implemented by this UI change.

The Editor has no built-in host: it never starts a Forge of its own. The machine
menu lists registered hosts and **Add new host**. At launch the Editor opens the
host it last connected to (resolved to that host's current registration), else
the first registered host; with none registered the window offers **Add a host**.
`editor --host-home <registration>` opens one registered host explicitly (the
dev runner launches the dev Editor this way), and `ARTISAN_DEV_FORGE_HOME`
attaches a development Editor to a manually started Forge.

Selecting a machine changes the active Forge connection **in the same editor
window**. Switching does not spawn an editor process or disconnect other hosts.
Each connection retains its own project catalog, conversations, events, and
unsent drafts, so switching back restores that workspace. Agent work on another
Forge continues while you view a different machine. The editor remains available
when a Forge is offline; the selector and local UI do not depend on a connection.

Forge currently grants one controlling editor session per daemon. Selecting an
already connected machine reuses its existing session. Quitting the editor closes
all its connections and preserves remote reconnect credentials; it does not stop
an externally managed Forge.

The original invitation path is remembered. On connecting to the host again, the editor
can refresh the address and incarnation from that file **only if its certificate
matches the imported identity**. This supports WSL restarts through the stable UNC
path. If you transferred a copy from another machine, transfer a fresh invitation
after a daemon restart. A changed certificate requires an explicit new import.

## Networking

Forge listens on the selected Linux interface. Windows connects directly to that
IP and UDP port; no localhost relay or WSL communication bridge is involved.
Allow the selected UDP port through applicable host/Hyper-V firewalls. The
installer does not change Windows firewall rules or WSL networking mode.

Mirrored-mode localhost operation is optional and is not assumed or automatically
configured. The implementation also accepts explicit IPv6 remote endpoints.

For direct daemon deployments, Forge now accepts `--listen IP:PORT`. Omitting it
retains the existing `127.0.0.1:0` behavior. The NixOS module exposes
`services.artisan-forge.listenAddress`; its default remains loopback.

## Engines

The Forge installs, updates, and launches its own engine CLIs (Claude Code, Codex, Grok
Build, and Cursor Agent on Linux) under `<installation>/data/toolchain/<engine>/`, beside its
database, verified against the vendor's published checksums, and runs them with their own
homes there; it never uses a `claude` or `codex` found on `PATH` (see
`docs/plans/managed-engines.md`). The Editor's Settings engine pages show each engine's
status, version, and version controls.

A freshly installed engine has no account. Sign in once per host with the installation's
`ae`, which manages its own Forge without flags:

```sh
ae engine list
ae engine login claude
ae engine login codex -- --device-auth
ae engine login grok
ae engine login cursor
```

`ae engine versions|use|rollback|status <engine>` operate on the same install state as the
Forge; `--database PATH` selects another Forge's state.

## Operations and diagnostics

```sh
ae status
ae doctor
systemctl --user status artisan-forge-dev.service
journalctl --user -u artisan-forge-dev.service
ae autostart --disable        # stop the service and remove its unit
```

After a restart, select the host again once its previous connection has stopped
to refresh its invitation in the existing window.
An interrupted credential-rotation handshake is quarantined rather than replayed;
restart Forge and select the host again in that case. A readiness receipt left by
a Forge that was killed is reconciled by the next `ae start`: it is removed only
when the Forge that wrote it is gone and nothing holds the installation's custody.

Headless client verification (also supported by `editor.exe`):

```sh
editor --import-host /trusted/path/host.json
# The import prints the private registration home. Pass it below:
editor --host-home /absolute/registration/home --probe-host
```

The probe uses the real editor transport to authenticate, query projects, and
cleanly disconnect. Running it again verifies the persisted reconnect credential.
It requires exclusive access to that Forge, just like an editor window.

Automated integration check:

```sh
python3 scripts/test_remote_host.py --bin-dir target/debug
nix build .#checks.x86_64-linux.remote-hosts
```

The check covers initial authentication/project query, repeated connection,
daemon restart through the original saved registration, changed-identity
rejection, and keeping the daemon alive when the client disconnects. Linux CI
runs it independently of the broader workspace tests.

## Validation in this workspace

Verified on Ubuntu WSL with a native Windows MSVC editor build:

- Windows UDP round trip to the distro's IPv4 interface.
- Native Windows QUIC authentication, real project query, clean disconnect, and
  a second process reconnecting with the rotated capability.
- The installed Nix Forge service restarted through systemd; the original saved
  Windows registration refreshed its invitation and connected successfully.
- Native Windows profile clicks and ghost-selector keyboard navigation selected Ubuntu,
  completed authenticated initial queries, and switched to another registered
  host and back in the same window (unchanged HWND and process).
  [Machine dropdown](../../evidence/whole-profile-selector-20260913-203140.png).
- GPUI interaction tests cover mouse and keyboard selection, draft retention,
  isolation of commands/events, and shutdown of all retained host views.
  Run `cargo test -p artisan-frontend --lib native_application::workspace::tests`.
  With Ubuntu registered, run `scripts/verify_visual.ps1 -ExePath <editor.exe>
  -VerifyMachineSwitch` for the native Windows interaction check.
- Nix `remote-hosts`, workflow validation, and Nix formatting checks passed.
- Focused transport, pinning, Forge runtime, private-host storage, frontend
  transport, and menu tests passed. Relevant Cargo targets compile and pass Clippy.

The systemd service restarts on failure. After a daemon incarnation change,
select its disconnected machine again to refresh the controlling session. General workspace test failures and existing
file-size ratchet violations remain outside this change.

## Windows project selection for local WSL hosts

A host imported from `\\wsl$\<distribution>\…` or
`\\wsl.localhost\<distribution>\…` is recognized as local WSL independently
of its editable display name. Add project opens the Windows native folder
chooser at that distribution's default user home (for example,
`\\wsl$\Ubuntu\home\sander`). The home comes from the distro's `$HOME`,
resolved off the UI thread independently of the Windows username. The editor translates a selection
from that share into a Linux path; selections from other distributions or
Windows drives are rejected. Forge validates existence, directory type and
canonical path through its isolated helper before issuing a single-use
`DirectoryId`. Project attachment and retry semantics remain unchanged.

One local editor process owns each host's reconnect credential at a time.
If another window holds it, the editor shows “Host is already connected”
and a retry action. Close the other connection, then retry; do not delete
credential records or re-use a consumed bootstrap invitation.
