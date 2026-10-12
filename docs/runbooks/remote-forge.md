# Using a Forge on another machine (or in WSL)

The Editor keeps no threads itself; it shows the threads of the Forges it is
paired with. A Forge on the same computer or in a WSL distribution pairs from
the Editor's first screen with one click. Any other machine pairs with a
code.

## On the machine that will run the agents

```sh
ae install forge                 # installs ae and Forge, starts the service
ae forge start                   # if it isn't running
```

Forge listens on `127.0.0.1:47900` by default. To serve other machines, run
it on a reachable address, for example in the systemd unit or by hand:

```sh
ae forge serve --listen 0.0.0.0:47900
```

Then make a one-time pairing code (valid for 10 minutes, once):

```sh
ae forge pair --listen <address-the-editor-can-reach>:47900
```

It prints the address, the code and Forge's certificate fingerprint.

## In the Editor

Settings → Hosts → *Pair a Forge on another machine*, paste
`<host:port> <code> <fingerprint>`. The Editor connects only if Forge's
certificate has that fingerprint, then registers its own certificate with
Forge. From then on every request is mutual TLS; there are no tokens.

## WSL

Install Forge inside the distribution (`ae install forge --wsl <distro>` from
Windows, or `ae install forge` inside it). On the Editor's first screen (or
Settings → Hosts) choose *Use the Forge in WSL: <distro>*; the Editor runs
`wsl.exe -d <distro> -- ae forge pair --json` and pairs with what it prints.

## Devices

```sh
ae forge devices                 # paired Editors
ae forge devices revoke <id>     # unpair one; its next request is refused
```

## Troubleshooting

- `ae forge status`, `ae forge logs -f`, `ae doctor`.
- "unreachable" in the Editor: check the address and that the port is open
  between the machines (TCP 47900).
- A fingerprint mismatch means the Editor is talking to a different Forge
  (or Forge's identity was reset): pair again.
