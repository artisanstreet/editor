# Installer release trust

The installer has exactly one trust root per build. There is no runtime path
that lets a release build choose a different one.

## Build modes

| Build | Mode marker | Embedded anchor | Runtime `--public-key` |
| --- | --- | --- | --- |
| Cargo dev / `cargo test` | debug profile (`debug_assertions`) | none | required, unpinned |
| `cargo build --release` | `debug_assertions` off | `ARTISAN_RELEASE_KEY_ID` + `ARTISAN_RELEASE_PUBLIC_KEY_HEX` | refused (typed error) |

A release build whose anchor is missing is a compile error (`manifest.rs`), not
a runtime prompt: shipping an anchorless release must be impossible. A
development build with no key fails at startup with
`MissingDevelopmentTrustKey`; there is no silent fallback.

The embedded anchor binds both the Ed25519 public key and the release `key_id`.
A manifest signed by any other key id is rejected with
`UntrustedSigningKey` even when its signature verifies.

## Release procedure

1. Generate the release signing key offline and store the 32-byte seed in the
   release secret store. Keep the seed out of the repository.
2. Replace the two lines in `modules/installer/release/trust_anchor.env`:
   `ARTISAN_RELEASE_KEY_ID` (a stable identifier recorded in every installed
   `installation.json`) and `ARTISAN_RELEASE_PUBLIC_KEY_HEX` (the raw 32-byte
   Ed25519 public key as lowercase hex; the RFC 8410 SubjectPublicKeyInfo
   encoding is also accepted).
3. Sign the release manifest with `cargo run --locked -p artisan-packaging --bin release-tool -- sign` using the
   same key id.
4. Build the release installer through `cargo build --locked -p ae-installer --release` with
   `ARTISAN_RELEASE_KEY_ID` and `ARTISAN_RELEASE_PUBLIC_KEY_HEX` exported.
5. Verify the produced installer only accepts manifests signed by the pinned
   key and refuses `--public-key`/`ARTISAN_INSTALLER_PUBLIC_KEY`.

The checked-in anchor currently names the pre-release `development` key used by
`packaging/release` fixtures. It must be rotated by the procedure above before
the first public release; until then a release build validates only pre-release
manifests.

## Existing version directories

`versions/<v>` on disk has no signature of its own. Before activating an
existing version directory the installer re-downloads the signed artifact,
verifies its size and SHA-256, extracts it, and compares every signed archive
entry with the tree on disk (including the tree's `payload-manifest.json`
entries). A dev-staged or tampered tree is refused with `TamperedRelease`; a
tree whose payload manifest is missing, unsafe, or does not cover exactly the
signed entries is refused with `UnverifiedRelease`. Nothing from the existing
tree is executed, copied to the stable launcher, or registered as an
integration before that verification succeeds.

Nix release outputs read the two public anchor values from `release/trust_anchor.env` as explicit derivation inputs and set the release marker. `nix build .#unsigned-release` generates metadata for the exact Nix-linked Linux archive; `nix run .#sign-release -- ...` signs at runtime using a caller-supplied key file. Private signing material must never be a Nix source or derivation input.
