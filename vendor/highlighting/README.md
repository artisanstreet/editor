# Bounded highlighting dependencies

These are runtime copies of Syntect 5.3.0 and Onig 6.5.3, patched for Artisan's
highlighting memory use. All packaged grammar/theme assets and runtime source
modules remain present. The editor selects `regex-onig`; `regex-fancy` remains
available for a rollback. No grammar or highlighting theme is changed.

Syntect gives each native search a 1,000,000-retry budget using its owned
`MatchParam`. A stack/match/search-limit error lazily selects the original
fancy-regex 0.16.2 implementation, with its unchanged Oniguruma mode and default
budget. That regex then continues using fancy. Capture offsets and optional
captures are copied into reusable storage. Metadata `is_match` deliberately uses
fancy's original unanchored semantics. No process-global limits are changed.
A linked engine that cannot configure the search limit also falls back to fancy.

The runtime modifications are confined to Syntect's `src/parsing/regex.rs` and
Onig's `src/match_param.rs`. Manifest changes enable the fallback dependency, pin
Onig/fancy/onig_sys, prohibit publishing these copies, and remove upstream
example/bench/test targets and dev dependencies. Original manifests are retained.
Upstream Syntect's published crate already omits the testdata required by its
full internal suite; the standalone `validation` package exercises the changed
module and bundled grammars without pretending that suite is available.

`PROVENANCE.json` records upstream archive SHA-256 values, package VCS revisions,
licenses, and the upstream/current checksum of every retained package file.
`NOTICE.txt` contains the upstream notices, including the bundled C engine's
notice. Release archives include it as `HIGHLIGHTING-NOTICES.txt`.

Run from the repository root:

```sh
python3 vendor/highlighting/verify.py
cargo test --manifest-path vendor/highlighting/validation/Cargo.toml --locked --release
cargo run --manifest-path vendor/highlighting/validation/Cargo.toml --locked --release --bin check_patterns
```

The seven module tests exercise fallback, captures, Unicode, truncation, metadata,
serialization, concurrent searches and invalid ranges. The adapter rejects a start
beyond end before native pointer arithmetic; invalid end offsets and UTF-8 end
boundaries still fail in Rust slicing, matching the original backend. Two parser tests compare original-engine
scope digests for 71 samples/all bundled syntaxes and an unfinished C++ template
line that exposed unrestricted native search. Digests use the repository's pinned
Rust 1.98 toolchain. `check_patterns` walks all 8,156 stored match entries and
metadata patterns in 75 syntaxes, including representative empty/ASCII/Unicode/
metacharacter expansions of 116 dynamic patterns. It compiles 3,732 unique
patterns in both engines. These checks do not prove equivalence for every input
or every possible dynamic capture value.

`onig_sys = 69.9.3` stays a registry dependency and compiles its bundled C sources
statically through `cc`. Existing Linux/MinGW C toolchains suffice; the `generate`
feature is disabled, so this path does not require bindgen. Keep
`RUSTONIG_SYSTEM_LIBONIG` and dynamic-library overrides unset for the qualified
build. The root Cargo.lock records all transitives. The isolated validation
package has its own lockfile and patches Onig to this same local package.

An upgrade must refresh the upstream archives/checksums, reapply the two runtime
changes, regenerate both lockfiles, and run these tests plus Editor Markdown,
Linux/Windows build and visual validation. Do not swap to unrestricted Onig.
A future upstream MatchParam API and maintained Syntect fork could replace these
copies once immutable, publicly resolvable revisions are available.
