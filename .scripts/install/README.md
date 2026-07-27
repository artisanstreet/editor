# Artisan landing transport

These scripts are the small, inspectable transport layer behind the Artisan
landing-page install commands. They do not install Artisan themselves. They
select a native Rust bootstrap from GitHub Releases, download its SHA-256
sidecar, verify the executable, run it from a unique temporary directory, and
remove that directory on exit.

Windows:

```powershell
irm https://sonstabo.com/editor/windows | iex
```

macOS and Linux:

```sh
curl -fsSL https://sonstabo.com/editor/unix | sh
```

The landing page must link directly to the corresponding script so a user can
inspect it before running it. `ARTISAN_VERSION=v1.2.3` pins a release; omitting
it resolves GitHub's latest release. `ARTISAN_GITHUB_REPOSITORY` exists for
mirrors and release qualification. Remaining arguments are forwarded unchanged
when the scripts are saved and invoked directly.

Release assets use these names:

```text
artisan-bootstrap-windows-x64.exe
artisan-bootstrap-windows-arm64.exe
artisan-bootstrap-macos-x64
artisan-bootstrap-macos-arm64
artisan-bootstrap-linux-x64-gnu
artisan-bootstrap-linux-arm64-gnu
artisan-bootstrap-linux-x64-musl
artisan-bootstrap-linux-arm64-musl
```

Every asset has a sibling `<asset>.sha256` containing its lowercase SHA-256
digest. The scripts reject malformed or mismatched sidecars before execution.
