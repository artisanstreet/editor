# Using a Forge on another machine (or in WSL)

The Editor keeps no threads itself; it shows the threads of the Forges it is
paired with. A Forge on the same computer or in a WSL distribution pairs from
the Editor's first screen with one click. Any other machine pairs with a
code.

## On the machine that will run the agents

```sh
ae install --forge here --editor none   # installs ae and Forge, starts it at login
ae forge start                          # if it isn't running
```

Forge listens on `127.0.0.1:47900` by default. To serve other machines, set
a reachable address in Forge's `config.json` (in its data folder,
`~/.local/share/artisan-street/forge/data` on Linux) and restart it:

```sh
echo '{"listen": "0.0.0.0:47900"}' > ~/.local/share/artisan-street/forge/data/config.json
ae forge restart
```

Then make a one-time pairing code (valid for 10 minutes, once):

```sh
ae pair
```

It prints one line to paste into the Editor: the address, the code and
Forge's certificate fingerprint, and waits until an Editor uses it. Replace
`0.0.0.0` with an address the Editor can reach.

## In the Editor

Settings → Hosts → *Pair a Forge on another machine*, paste
`<host:port> <code> <fingerprint>`. The Editor connects only if Forge's
certificate has that fingerprint, then registers its own certificate with
Forge. From then on every request is mutual TLS; there are no tokens.

## WSL

Install Forge inside the distribution: `ae install` from Windows and pick
*In <distro> (WSL)* for Forge, or `ae install` inside it. On the Editor's
first screen (or Settings → Hosts) choose *Use the Forge in WSL: <distro>*;
the Editor runs `ae pair --json` there and pairs with what it prints.

From Windows, `ae forge start|stop|restart`, `ae pair`, `ae devices` and
`ae engine` act on the Forge in WSL when this PC has none (`--distro <name>`
picks one when several have it).

## Devices

```sh
ae devices                 # paired Editors
ae devices revoke [<id>]   # unpair one (asks which); its next request is refused
```

## Troubleshooting

- `ae status` shows what is installed, whether Forge runs and answers, its
  log, and what to do about problems.
- "unreachable" in the Editor: check the address and that the port is open
  between the machines (TCP 47900).
- A fingerprint mismatch means the Editor is talking to a different Forge
  (or Forge's identity was reset): pair again.
