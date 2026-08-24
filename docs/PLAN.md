# Artisan Editor Rust port plan

## Purpose

Build a new native Artisan Editor repository in Rust. The repository will contain a GPUI desktop frontend and a standalone Rust backend (Forge), connected through an Artisan application protocol encoded with Cap'n Proto and carried directly over QUIC with Quinn. The backend owns SQLite through SeaORM.

This is a new implementation, not a line-for-line translation of the TypeScript repository. For every capability selected for the native product, the existing repository is the reference for meaningful behavior, product detail, and visual intent. It is not the Rust architecture blueprint, an exact-parity specification, or a checklist requiring every TypeScript capability to be reproduced.

## Global port mindset

Preserve the product, not the TypeScript implementation.

When a capability is selected for the native product, its meaningful details and user-visible behavior should be preserved by default. The TypeScript code, tests, and UI are valuable references for understanding those details. They do not dictate Rust module boundaries, ownership, concurrency, error handling, state management, or control flow.

The port must be idiomatic Rust and GPUI rather than a one-to-one TypeScript or Effect translation:

- use Rust types and enums instead of recreating runtime schema/tag machinery;
- use explicit constructors and application assembly instead of recreating Effect layers;
- use `async` functions, typed `Result` values, owned tasks, and channels instead of recreating Effect programs and fibers;
- use GPUI entities and focused state ownership instead of translating frontend reactivity mechanically;
- introduce traits only at real polymorphic or testing boundaries, not as a direct copy of every TypeScript service;
- redesign an awkward subsystem when a simpler, safer, faster, or more coherent native design is available.

Improvement is part of the port. If implementation work exposes an opportunity to make a system better, take it rather than preserving a bad boundary for parity's sake. Intentional behavior changes should be explicit in the work packet and tests so an improvement is not confused with an accidental regression.

For UI that is selected for the native product, copy the existing visual result as closely as practical: layout, spacing, typography, color, icons, hierarchy, states, and interaction details. This is a visual reference target, not a requirement to reproduce Svelte component structure or browser rendering internals.

Do not infer UI behavior from screenshots, component names, or a top-level Svelte file. Trace each surface through its product call sites, the local `$lib/components/ui` wrappers, theme and animation styles, and the exact pinned Bits UI implementation beneath those wrappers. Record what the code actually does before choosing a native API. The shared GPUI framework is derived from that evidence; it is not a guessed approximation or a mechanical port of Bits UI.

Shader-backed effects are explicitly deferred. Do not build a custom shader or renderer extension during the current port. Shader-dependent surfaces may use ordinary GPUI styling where a basic surface is necessary, but shader fidelity does not block the native architecture or the rest of the UI. Shader work requires a later, separately approved plan.

## Confirmed decisions

- The desktop frontend is a native GPUI application.
- The browser frontend is deprecated and is not part of the new repository.
- Forge is rewritten in Rust as a separate backend process.
- Frontend and backend share owned domain types and an application protocol.
- Cap'n Proto is the wire format.
- Quinn provides direct QUIC transport. WebSocket and WebTransport are not target transports.
- SQLite remains the database.
- SeaORM owns SQLite access, entities, repositories, and migrations.
- Bazel is the authoritative build, test, code-generation, packaging, and CI graph.
- Cargo manifests and `Cargo.lock` remain because Rust dependencies and Rust tooling understand them; Bazel consumes them through `rules_rust`/Crate Universe rather than invoking `cargo build`.
- Tests live under root `tests/`, not beside production source and not under `.tests/`.
- A root `scripts/` package is created only if project-specific executable tooling becomes necessary. It is not created merely to wrap ordinary Bazel commands.
- `thiserror` is used for explicit error types.
- `anyhow` is prohibited in first-party packages owned by this repository.
- Every static SVG source referenced by the old frontend is vendored into a first-party Rust assets crate.
- Reusable native visual and interaction primitives live in a first-party shared GPUI framework crate under `modules/ui/`.
- Bits UI and the local Svelte wrappers are behavioral references for that framework, not dependencies or architectural templates for the native product.
- Streaming Markdown uses `pulldown-cmark` for parsing and `syntect` for fenced-code syntax highlighting; Artisan does not implement a Markdown grammar or programming-language grammars.
- Artisan owns the streaming coordinator, owned Markdown document model, bounded highlight cache, theme mapping, and native GPUI renderer rather than adopting a third-party styled Markdown component.
- Shader implementation is deferred and is not part of the current port.
- Mobile applications are not in the current scope.
- Nix is not part of the initial toolchain.

## Deliberate non-requirements

This plan does not assume:

- one-to-one coverage of every TypeScript feature or retention of TypeScript/Effect implementation structure;
- retention of the TypeScript module graph or Effect-based architecture;
- continued compatibility with the old WebSocket protocol;
- a compatibility matrix where old frontends communicate with the new backend or vice versa;
- migration of every existing database or every historical migration;
- a specific mobile product;
- a specific remote-cache vendor;
- a specific installer format, updater, signing system, or release channel before those choices are made;
- macOS or Linux release support before those platforms are explicitly selected;
- provider-by-provider ports, performance targets, feature flags, rollback systems, or product capabilities that have not been requested.
- custom shader implementation during the current port.

When one of these becomes a product requirement, it receives its own decision and implementation scope. It is not smuggled into the port as implied parity work. This does not weaken the preservation rule: once a capability is selected, its meaningful behavior and details remain the starting point unless the work deliberately improves them.

## Technology change

| Current direction | Native repository direction |
| --- | --- |
| Electron/browser UI | GPUI desktop frontend |
| TypeScript/Node Forge | Rust backend process |
| WebSocket transport | Direct QUIC through Quinn |
| TypeScript wire contracts | Cap'n Proto schemas and Rust adapters |
| Drizzle/Effect SQLite access | SeaORM with SQLite |
| Tabler, SVGL, custom files, and inline SVG components | Vendored SVG source exposed by a first-party Rust assets crate |
| Svelte/shadcn wrappers over Bits UI | First-party shared GPUI framework based on audited behavior and visual details |
| Browser/custom shader effects | Deferred; ordinary GPUI styling only for the current port |
| Bazel wrapping pnpm scripts and host Cargo | Bazel-native `rules_rust` targets |
| Vitest/TypeScript tests | Bazel `rust_test` targets under `tests/` |
| Node and PowerShell orchestration | Bazel targets and, only when necessary, a Rust tool under `scripts/` |
| Electron packaging | Packaging of Bazel-built native binaries and assets |

## Repository layout

The initial repository layout is:

```text
editor/
|-- MODULE.bazel
|-- BUILD.bazel
|-- .bazelrc
|-- .bazelversion
|-- Cargo.toml
|-- Cargo.lock
|-- rust-toolchain.toml
|-- docs/
|   |-- PLAN.md
|   `-- ui/
|       `-- INVENTORY.md         # produced by the UI archaeology phase
|-- modules/
|   |-- domain/
|   |   |-- BUILD.bazel
|   |   |-- Cargo.toml
|   |   `-- src/
|   |-- assets/
|   |   |-- BUILD.bazel
|   |   |-- Cargo.toml
|   |   |-- manifest.toml
|   |   |-- svg/
|   |   `-- src/
|   |-- ui/
|   |   |-- BUILD.bazel
|   |   |-- Cargo.toml
|   |   `-- src/
|   |-- protocol/
|   |   |-- BUILD.bazel
|   |   |-- Cargo.toml
|   |   |-- schema/
|   |   `-- src/
|   |-- transport/
|   |   |-- BUILD.bazel
|   |   |-- Cargo.toml
|   |   `-- src/
|   |-- database/
|   |   |-- BUILD.bazel
|   |   |-- Cargo.toml
|   |   `-- src/
|   |-- migrations/
|   |   |-- BUILD.bazel
|   |   |-- Cargo.toml
|   |   `-- src/
|   |-- backend/
|   |   |-- BUILD.bazel
|   |   |-- Cargo.toml
|   |   `-- src/
|   `-- frontend/
|       |-- BUILD.bazel
|       |-- Cargo.toml
|       `-- src/
|-- tests/
|   |-- domain/
|   |-- assets/
|   |-- ui/
|   |-- protocol/
|   |-- transport/
|   |-- database/
|   |-- backend/
|   |-- frontend/
|   `-- fixtures/
`-- scripts/                    # absent until a real script/tool is needed
```

Folder names carry the concern, so source files do not repeat `frontend`, `backend`, or another parent name unnecessarily.

The frontend and backend packages should each expose a library target plus a thin binary entry point. Application assembly and exit-code mapping live at the binary boundary; behavior remains testable through the library target.

## Dependency architecture

```text
frontend ----> transport ----> protocol ----> domain
frontend ----> ui ---------> assets
frontend ------------------> assets
backend  ----> transport ----> protocol ----> domain
backend  ----> database --------------------> domain
backend  ----> migrations
```

### `domain`

Owns application concepts independent of networking, storage, GPUI, and process runtime. This includes typed identifiers, commands, events, state, and validation that genuinely belong to the native product.

Initial external dependencies:

- `thiserror` for domain-specific failures;
- `time` when a domain value actually contains time.

Additional serialization or URL crates are added only when a concrete domain type requires them.

### `assets`

Owns every static SVG source referenced by the old frontend. The migration inventory must find:

- direct and barrel imports from Tabler;
- SVGL component imports;
- checked-in `.svg` files;
- custom Svelte logo/mark components containing inline SVG;
- other inline `<svg>` usage in frontend source.

Each discovery is classified as either a static asset or a data-driven drawing. Static assets are extracted and checked in. Data-driven SVG widgets are recorded in the inventory and reimplemented using appropriate GPUI drawing/layout APIs; they are not flattened into incorrect static files.

The crate layout is:

```text
modules/assets/
|-- BUILD.bazel
|-- Cargo.toml
|-- manifest.toml
|-- licenses/
|-- svg/
|   |-- tabler/
|   |-- svgl/
|   |-- lobe/
|   |-- simple-icons/
|   |-- jetbrains/
|   `-- artisan/
`-- src/
    `-- lib.rs
```

`manifest.toml` records a stable asset ID, original name, source family, upstream source/version or local origin, license file, view box, and whether the asset is monochrome or multicolor. The source SVG and applicable license/attribution material are preserved rather than redrawn by hand.

Bazel validates the manifest and SVG files deterministically and generates or verifies a typed Rust API such as `AssetId` plus access to the embedded SVG source. Frontend code uses that API rather than npm packages, Svelte components, or arbitrary filesystem paths. Every static SVG actually referenced by the old frontend is vendored, including references outside the first native workflow. Genuinely unreferenced files are excluded only after checking that they are not packaging resources. The inventory still records data-driven SVG use so it can be reimplemented natively rather than disappearing accidentally.

An extraction utility may be added under `scripts/` if repeatable automation is necessary. It is a migration/build tool, not a Node or npm runtime dependency of the native product.

### `protocol`

Owns Cap'n Proto schemas, generated Rust bindings, message envelopes, and conversions between wire values and owned `domain` values. Whether explicit protocol version negotiation is needed is decided with the first message design.

Initial external dependencies:

- `capnp`;
- `thiserror`.

Generated Cap'n Proto reader/builder types do not become the application's domain model. They remain at the protocol boundary. Bazel owns schema generation as a declared, cacheable action. The Cap'n Proto compiler is a pinned Bazel tool, not an ambient host executable or hidden Cargo `build.rs` dependency. Generated output may be checked in if that is required for reliable IDE support, but a Bazel verification target must then prove that it matches the schemas.

### `transport`

Owns Quinn endpoints, QUIC connection lifecycle, TLS configuration, stream framing, bounded messages, request correlation, event delivery, reconnection behavior chosen for the native product, and transport-level errors.

Initial external dependencies:

- `quinn`;
- `rustls` and `rustls-pki-types`;
- `tokio` and `tokio-util`;
- `bytes`;
- `futures-util`;
- `async-channel` for runtime-neutral handoff to GPUI;
- `tracing`;
- `thiserror`.

`rcgen` may be used by tests or local development after the certificate/trust model is selected. The production trust and authentication design is an open decision; it must be written down before remote connections are enabled.

QUIC is the transport, not the application protocol. Quinn therefore does not belong in `protocol`.

### `database`

Owns the SeaORM connection, current entities, repository operations, transactions, and SQLite-specific connection policy. Only the backend depends on it; the frontend never opens the database.

Initial external dependencies:

- `sea-orm` with only the SQLite, Tokio, macros, and explicitly used value-type features;
- `tokio`;
- `tracing`;
- `thiserror`;
- `time` and `serde_json` only when selected schema columns require them.

### `migrations`

Owns immutable, ordered SeaORM migrations. It stays separate from current entities so editing an entity cannot silently rewrite migration history.

Initial external dependency:

- `sea-orm-migration` with SQLite/Tokio support only.

The first native schema is designed for the selected native product scope. Importing or upgrading an existing TypeScript-era database is separate work unless it is explicitly chosen as a requirement.

### `backend`

Owns Forge process assembly and native backend capabilities selected for implementation. It hosts the QUIC server, owns database startup and migrations, and coordinates backend services.

Initial external dependencies:

- `tokio`;
- `clap` if a command-line interface is needed;
- `tracing` and `tracing-subscriber`;
- `thiserror`;
- `serde` and `toml` only after a configuration format is chosen.

The binary maps typed startup/runtime failures to diagnostics and `ExitCode`. It does not erase them behind a catch-all application error type.

### `ui`

Owns Artisan's first-party shared GPUI framework. It contains reusable visual foundations and interaction primitives proven necessary by the legacy UI audit: theme and typography values, spacing and shape conventions, icons and asset presentation, focus treatment, input semantics, motion and reduced-motion behavior, overlays, anchored surfaces, menus, selection controls, and other reusable components as selected workflows require them.

The framework does not own product, transport, database, or Forge state. Product-specific screens and workflow composition remain in `frontend`. It is also not a general-purpose reimplementation of Bits UI: a primitive enters this crate only when a concrete Artisan call site requires it, and its Rust API follows GPUI ownership and event semantics rather than copying Svelte props.

Feature-level behavior such as transcript scroll anchoring, model-selector preview state, command-menu ranking, and screen-specific moving indicators stays in `frontend` until multiple native consumers prove a shared abstraction. A generic scroll container, for example, must not absorb the transcript's history-prepend and follow-tail policy.

Initial dependencies:

- `gpui`, pinned exactly after the Bazel/native-host proof succeeds;
- `pulldown-cmark` for CommonMark/GFM event parsing;
- `syntect` for fenced-code syntax parsing and highlighting;
- the first-party `assets` crate.

No browser DOM, CSS runtime, Svelte runtime, or Bits UI dependency is introduced. The exact legacy sources remain evidence for behavior and appearance, while the implementation is native GPUI.

#### Streaming Markdown and text highlighting

Markdown is split at an engine boundary. Third-party crates own grammar correctness: `pulldown-cmark` emits CommonMark/GFM events and `syntect` produces syntax-highlight ranges for fenced source. First-party code under `ui` converts those events into an owned `MarkdownDocument`, maps semantic and syntax styles through Artisan theme tokens, and renders native GPUI elements. It owns headings, paragraphs, lists, tables, quotes, links, inline code, code-block chrome, selection, copy actions, safe link and image behavior, layout, accessibility, and stable element identity. Raw HTML is inert and is never introduced into a browser or HTML renderer.

The transcript's stream lifecycle remains in `frontend`, because pacing and correction behavior are product policy rather than a generic visual primitive. QUIC supplies application text deltas only. The frontend maintains the canonical message, detects append-only growth versus correction/replacement, applies the audited reveal cadence, bypasses pacing for reduced motion, and submits visible-prefix snapshots to the Markdown worker. Work is coalesced rather than spawned for every network chunk, runs away from the GPUI thread, and carries a monotonically increasing generation so stale parse or highlight results cannot overwrite newer text.

The first implementation reparses the currently visible prefix with `pulldown-cmark` at most once per scheduled presentation update. Optimize only after measurement. If profiling proves this insufficient, retain completed block nodes with stable IDs and reparse only the live tail; do not replace the parser with a first-party grammar. A settled message always receives a final full parse because later reference definitions and similar Markdown constructs can affect earlier content.

Streaming preserves the useful behavior established by the current frontend:

- plain text can paint before the first parsed document is ready;
- an open fenced-code block remains plain while its language definition may be prepared in the background;
- a fence is highlighted and cached as soon as it closes, even while later message content continues streaming;
- non-append corrections replace the visible content immediately instead of replaying old pacing;
- math and Mermaid do not perform expensive rich rendering on a live partial construct; their settled renderers are selected and proven separately when those capabilities enter a native workflow.

The code-fence cache is bounded and keyed by source body, normalized language, and syntax-theme revision. A theme change invalidates or remaps affected entries. `syntect` output is converted into GPUI styled-text ranges; ordinary Markdown colors and typography always come from the same first-party theme tokens as the rest of the application.

Zed's Markdown implementation and `gpui-component` may be audited as native reference implementations, but neither is adopted wholesale or used to bypass the first-party Artisan framework. Tree-sitter remains a separate later decision for real editor buffers, where incremental syntax trees and semantic-token integration may justify it; the editor language stack is not pulled into transcript rendering merely to color fenced examples.

External tests under `tests/ui/` cover partial constructs, append/correction transitions, reduced motion, stale-generation rejection, stable completed blocks, open and closed fences, language fallback, theme invalidation, raw HTML inertness, links, selection, copy behavior, and final-settle correctness.

### `frontend`

Owns the GPUI application, windows, native product state, actions, screen composition, and product-specific presentation. It consumes reusable primitives through `ui` and static SVGs through the typed `assets` crate. Network work runs away from the UI thread and hands owned state/events into GPUI through an explicit boundary.

Initial dependencies:

- `gpui`, pinned exactly after the Bazel/native-host proof succeeds;
- the first-party `ui` and `assets` crates;
- `tokio` for the Quinn client runtime;
- `async-channel` for the transport-to-GPUI boundary;
- `tracing` and `tracing-subscriber`;
- `thiserror`.

No WebView, browser DOM, Electron runtime, Svelte runtime, or browser compatibility layer is introduced.

### Test-only dependencies

Start with:

- `tempfile` for isolated files and SQLite databases;
- `proptest` for protocol/domain invariants where generated cases add value;
- `rcgen` for isolated QUIC tests if the selected trust design uses test certificates;
- GPUI's test support for shared primitive behavior, frontend actions, and view state.

Snapshot or mocking libraries are not baseline dependencies. Add them only for a demonstrated testing need.

## Error policy

- `anyhow` is forbidden in manifests and source owned by this repository.
- Third-party dependencies may use `anyhow` internally. Their implementation choices do not require a fork, patch, or policy exception, and the dependency lockfiles are not checked for transitive `anyhow` packages.
- First-party libraries expose narrow typed errors at meaningful boundaries rather than adding or re-exporting `anyhow` themselves.
- Libraries use `thiserror` for explicit error enums where it improves the boundary.
- Errors retain their sources when a lower-level cause is useful.
- External input, protocol decoding, and database data return errors rather than panicking.
- Binary entry points log typed failures and return explicit exit codes.

## Bazel and Cargo responsibilities

### Bazel is authoritative

Bazel owns:

- Rust compilation through `rules_rust`;
- Cap'n Proto generation;
- tests and test data;
- rustfmt and Clippy checks;
- dependency-policy checks;
- native resources;
- assembly of frontend and backend artifacts;
- packaging actions;
- CI entry points;
- local and remote action caching.

Production targets must be native `rust_library`, `rust_binary`, `rust_test`, and focused custom rules. A Bazel target must not call `cargo build`, `cargo test`, pnpm, Node, or an undeclared host script to do its real work.

Each first-party module is a useful compilation/cache boundary. Tests should be small, hermetic Bazel targets so unchanged successful results can be reused. Code generation declares schemas, compiler tools, configuration, and outputs explicitly. Broad source globs and inherited host environments should be avoided because they reduce cache correctness and hit rate.

### Cargo has a support role

The root Cargo workspace and per-module manifests provide:

- one Rust dependency/version vocabulary;
- `Cargo.lock` input for Crate Universe;
- rust-analyzer and ecosystem metadata;
- a conventional way to inspect the workspace.

They do not create a second release build. When Cargo metadata and Bazel targets disagree, the discrepancy is fixed rather than normalized as an accepted hybrid.

The Rust version is pinned once and used consistently by `rust-toolchain.toml` and the Bazel Rust toolchain. Bazel itself is pinned in `.bazelversion`.

Dependency updates follow one explicit flow:

1. update the relevant Cargo manifests;
2. resolve and commit `Cargo.lock`;
3. repin Crate Universe and commit its required metadata;
4. run the complete Bazel build and test graph;
5. fail CI when Cargo dependency state and Bazel dependency state are stale or disagree.

### Cache plan

1. Make local actions deterministic and hermetic first.
2. Confirm repeat builds and tests produce local cache hits.
3. Add a shared remote cache after its provider and credential model are selected.
4. Keep cache credentials outside the repository.
5. Restrict remote writes appropriately in untrusted CI contexts.
6. Measure cache hit rates and investigate misses rather than assuming the cache is effective.

Remote execution is not required initially. It can be added independently if build volume later justifies it.

## Tests

All authored tests live under root `tests/` and are grouped by module or by a purpose-built behavior harness:

```text
modules/transport/src/connection.rs
tests/transport/connection.rs
```

Bazel maps each meaningful test source to a `rust_test`, or uses a small `rust_test_suite` where that preserves useful cache granularity.

The normal Rust harness still uses `#[test]`; async and GPUI tests can use `#[tokio::test]` and `#[gpui::test]`. Tests stored outside a production crate normally exercise its public API. If a private invariant truly needs a white-box test, its source still belongs under `tests/` and is explicitly wired into the test build instead of being hidden beside production code. Because Cargo does not automatically discover this repository layout, the Bazel/rust-analyzer project metadata must include the test targets rather than leaving them unindexed.

The initial test layers are:

1. domain validation and state transitions;
2. asset manifest/source validation and typed-ID coverage;
3. shared GPUI primitive tests for state, keyboard and pointer input, focus, dismissal, positioning, motion policy, and composition required by audited behavior;
4. Cap'n Proto encode/decode and rejection of unsupported or malformed messages;
5. in-process QUIC connection, stream, cancellation, and malformed-input behavior;
6. SQLite migration and repository behavior using temporary databases;
7. backend service tests without GPUI;
8. GPUI action/state tests without a real backend where possible;
9. a packaged frontend/backend smoke test after packaging exists.

Tests use the selected legacy behavior as their default baseline and catch accidental loss of meaningful detail in capabilities being ported. They do not exist to prove global parity with every TypeScript capability, and they may encode an intentional improvement once that change is explicit.

## Scripts and developer commands

Ordinary work uses Bazel directly:

```text
bazel build //...
bazel test //...
bazel run //modules/frontend
bazel run //modules/backend
```

Do not add a script for these commands.

If a workflow needs real project-specific logic—such as checking native prerequisites, verifying generated sources, assembling release inputs, or coordinating two long-running development processes—create one Rust tooling package under `scripts/`, build it with Bazel, and give it typed subcommands. It may orchestrate declared Bazel-built artifacts; it may not become another build graph.

## Packaging

Packaging consumes frontend, backend, and resource artifacts built by Bazel. It does not rebuild them through Cargo.

The packaging implementation remains open until a proof is scoped. Whatever tool or focused Bazel rules are selected must consume Bazel-built artifacts rather than creating a second build. Installer format, signing, updating, and distribution are separate decisions.

## Nix decision boundary

Nix is deferred because the initial proof should use the native desktop host toolchain directly. On Windows, Nix runs through WSL and is not the authority for a native Windows GPUI build using the Windows SDK and MSVC.

Reconsider Nix when at least one concrete need exists:

- Forge receives a Linux deployment target;
- Linux/macOS contributors need a reproducible native dependency shell;
- CI and local non-Windows environments repeatedly drift;
- a deployment artifact benefits from a Nix derivation.

If adopted, Nix describes the surrounding environment. Bazel remains the build graph.

## Execution phases

The phases prove the architecture in dependency order. A phase may be split into smaller reviewed changes, but later phases do not bypass an unproven foundation.

### Phase 1: repository and Bazel foundation

Create the root workspace, Bzlmod configuration, pinned Bazel/Rust toolchains, Crate Universe dependency import, module skeletons, and baseline CI commands.

Prove separately that Bazel can:

- build and run a minimal GPUI window on the native development host;
- build a Quinn client/server loopback target;
- run Cap'n Proto generation as a declared action;
- open an in-memory SQLite database through SeaORM;
- run one external `rust_test` from `tests/`;
- run rustfmt, Clippy, and dependency-policy targets.

The proof must exercise GPUI's proc macros, native platform dependencies, and build-script behavior, plus SeaORM's derive macros. A trivial dependency fetch or unused library target is not sufficient evidence that Bazel can own these crates.

Completion evidence:

- `bazel build //...` succeeds without invoking host Cargo or Node;
- `bazel test //...` succeeds;
- a clean repeat demonstrates cache reuse;
- rust-analyzer can understand first-party crates, root-level test targets, and the chosen generated-code arrangement;
- dependency checks show that no first-party manifest declares `anyhow`.

### Phase 2: domain and application protocol

Choose the first native end-to-end workflow before defining its messages. Add only the domain types, commands, events, queries, and errors required by that workflow.

Define:

- the message envelope and whether explicit protocol version negotiation is required;
- request, response, and event correlation required by the selected workflow;
- bounds and validation for every external value;
- Cap'n Proto schemas;
- conversions between generated wire values and owned domain values;
- failure behavior for incompatible or malformed messages.

Completion evidence:

- protocol and domain tests pass through Bazel;
- generated-code verification is deterministic;
- malformed and unsupported messages return typed errors;
- neither Quinn nor GPUI leaks into the domain crate.

### Phase 3: QUIC transport

Implement the minimum transport required by the selected workflow:

- Quinn endpoint construction;
- connection establishment and shutdown;
- the selected TLS trust/authentication model for the intended connection scope;
- application handshake and protocol-version check;
- framing over QUIC streams;
- bounded input before allocation;
- typed connection, framing, timeout, cancellation, and peer errors;
- frontend-friendly event delivery without UI-thread network work.

Do not reproduce the WebSocket transport or add WebTransport abstraction layers.

Completion evidence:

- client/server loopback tests cover success, rejection, malformed input, cancellation, and reconnect behavior that has actually been selected;
- tests are hermetic and cacheable;
- protocol logic can still be tested without a network;
- transport contains no database or GPUI state.

### Phase 4: SQLite and SeaORM

Design the initial native schema for the selected workflow. Add current entities, repositories, and forward migrations. Forge is the only process that opens the production database.

Decide explicitly whether the first release starts with a new database or imports legacy data. If legacy import is selected later, plan it as a bounded migration with its own fixtures; do not turn it into an unstated whole-schema parity requirement.

Completion evidence:

- a new temporary database migrates from empty to current;
- migrations are ordered and repeat startup safely;
- repository tests cover the transactions required by the selected workflow;
- frontend targets have no dependency path to SeaORM or SQLite.

### Phase 5: Rust backend vertical slice

Build the backend process around the proven transport and database boundaries. Implement only the backend capability chosen for the first workflow.

Add:

- typed startup configuration required for the selected connection/database setup;
- database migration at startup;
- QUIC server lifecycle;
- request dispatch into the selected backend service;
- structured tracing;
- graceful shutdown of resources created by this slice;
- explicit exit-code mapping.

Completion evidence:

- the backend runs independently;
- a test client completes the selected request/event flow;
- restart and shutdown leave the database usable;
- failures remain typed through the service and process boundary;
- no TypeScript or Node runtime is required.

### Phase 6: UI archaeology and SVG asset foundation

Audit the old frontend's UI system before designing native components. The audit covers the whole reachable component stack so shared behavior is not guessed from the first selected screen:

1. product routes, feature components, debug galleries, fixtures, and their actual call sites;
2. every directory under `modules/frontend/src/lib/components/ui/`, including wrapper composition, defaults, variants, and dormant wrappers;
3. `components.json`—currently Maia style, neutral base color, and Tabler icons—plus theme, font, utility, animation, prose, and vendor styles that affect the rendered result;
4. direct imports that bypass local wrappers;
5. the installed and locked Bits UI 2.18.1 package, including the implementation beneath each reachable primitive rather than only its public type declarations.

Also inspect the exact pinned GPUI/Zed implementation and any usable component facilities for focus, accessibility, events, input, overlays, and placement. The first-party framework should build on proven native facilities where they fit; it must not guess that GPUI provides a behavior or reimplement one without checking.

The current source contains 32 local UI wrapper directories. Baseline evidence includes Accordion, AlertDialog, Avatar, Collapsible, Command/`useId`, ContextMenu, Dialog, DropdownMenu, LinkPreview, Popover, Progress, ScrollArea, Select, Separator, Slider, Switch, Tabs, Toggle, ToggleGroup, and Tooltip. Dialog is also wrapped as Sheet, while LinkPreview and Dialog have direct product call sites. This list is a starting census, not permission to skip a full reachability scan. Unused wrappers are recorded as dormant and are not ported automatically.

For each reachable primitive, `docs/ui/INVENTORY.md` records concrete source paths and:

- call sites, composition, defaults, variants, sizes, and visual states;
- controlled and uncontrolled state, transitions, callback ordering, and event consumption;
- keyboard commands, directional or roving focus, typeahead, selection, and activation;
- focus entry, trapping, restoration, and behavior for disabled, invalid, read-only, and loading states;
- pointer, hover, long-hover/grace-area, outside-press, Escape, and dismissal behavior;
- overlay ordering, modality, portals, scroll locking, anchor geometry, collision handling, and transform origins;
- presence, open/close timing, animation, and reduced-motion behavior;
- accessibility intent, labels, descriptions, and announcements, translated to the closest native semantics rather than copied as DOM attributes;
- which behavior comes from Artisan code, the local wrapper, or Bits UI itself.

Inspect the relevant Bits UI internals such as its state machines, focus and tabbable helpers, roving focus, arrow navigation, typeahead, floating positioning, presence management, body scroll lock, and safe-polygon/grace-area handling. A screenshot comparison alone is not acceptable evidence for a behavior-heavy primitive.

In parallel, perform a complete SVG-use inventory across the old frontend:

- enumerate Tabler imports and extract the corresponding upstream SVG source;
- enumerate SVGL imports and extract the corresponding upstream SVG source;
- copy checked-in first-party and brand `.svg` files;
- extract static paths and markup from custom/inline Svelte SVG components;
- classify data-driven SVG widgets for idiomatic GPUI reimplementation;
- record provenance and attribution in the assets manifest;
- expose the vendored static assets through the typed `assets` crate.

Do not design shared native primitives until the relevant inventory entries have been reviewed. Do not translate the Svelte component tree mechanically and do not start shader implementation.

Completion evidence:

- every local UI wrapper and direct Bits UI import has an inventory entry classified as reachable or dormant;
- every primitive needed by the first workflow has a behavior contract traced through call site, wrapper, styles, and the exact Bits UI implementation where applicable;
- the pinned GPUI capabilities relevant to those contracts are documented as usable, insufficient, or absent with source/API evidence;
- visual tokens, typography, spacing, shapes, interaction states, timing, and reduced-motion rules used by the first workflow have concrete source references;
- every old SVG use has an inventory entry and classification;
- every static SVG referenced by the old frontend exists under `modules/assets/svg/` with manifest metadata;
- Bazel validates the assets without Tabler, SVGL, Svelte, npm, or a browser runtime;
- shader-backed effects are identified as deferred and have no custom shader implementation in the current graph.

### Phase 7: shared GPUI framework

Create `modules/ui/` as Artisan's shared GPUI framework using the approved archaeology records. A controller approves the crate boundary and behavioral contracts before a framework-builder subagent writes the implementation.

Build the reusable foundations demanded by the first workflow:

- theme values for color, typography, spacing, shape, elevation, density, and interaction states;
- asset and icon presentation through `assets`;
- consistent keyboard, pointer, focus, disabled, invalid, and reduced-motion behavior;
- overlay stacking, dismissal, focus restoration, and anchored-surface positioning where audited components require them;
- the smallest coherent set of controls and composed primitives needed by the selected workflow;
- a native component gallery or focused harness that exposes every implemented state and variant for side-by-side review.

Shared machinery is extracted when multiple audited primitives need the same invariant. A component-specific behavior stays component-specific until evidence supports a reusable abstraction. Do not clone Bits UI's public API, rebuild DOM concepts inside GPUI, or add all legacy wrappers merely because they exist.

Product screens, navigation, backend state, and workflow policy remain outside this crate. The framework exposes idiomatic Rust state and actions suitable for GPUI consumers.

Completion evidence:

- Bazel builds the `ui` crate and runs its external tests under `tests/ui/`;
- every implemented primitive links back to reviewed inventory entries and concrete legacy source paths;
- tests cover the audited state, keyboard/pointer, focus, selection, dismissal, positioning, and motion behavior that applies to each implemented primitive;
- native accessibility inspection covers names, roles or platform equivalents, selected/checked/expanded state, descriptions, and status announcements supported by the pinned GPUI/platform stack;
- the component gallery demonstrates defaults, variants, edge states, and composition without a browser runtime;
- side-by-side review confirms close non-shader visual treatment for the audited states;
- an independent reviewer checks both idiomatic GPUI design and fidelity to the recorded behavior before frontend screens depend on it.

### Phase 8: GPUI frontend vertical slice

Build the native frontend shell and the complete UI required by the same selected workflow.

Add:

- GPUI application/window startup;
- frontend application state;
- a Quinn client service outside the UI thread;
- explicit conversion from transport events into GPUI state updates;
- transcript stream coordination with canonical text, append/correction handling, reveal pacing, and stale-work rejection;
- native Markdown presentation through the shared `ui` document and renderer, including fenced-code highlighting;
- loading, connected, failed, and disconnected states required by the workflow;
- GPUI tests for the actions and state transitions introduced.

Implement against the approved UI inventory and its cited legacy sources rather than rediscovering behavior from screenshots. If a screen exposes an unrecorded detail, return to archaeology before choosing behavior. Preserve meaningful detail while using idiomatic GPUI ownership and composition unless an improvement is deliberate and recorded.

Completion evidence:

- the frontend connects to the Rust backend and completes the selected workflow;
- transport work does not block the GPUI thread;
- frontend behavior is testable without a live backend where appropriate;
- screens use the shared `ui` crate rather than introducing private, behaviorally divergent copies of its primitives;
- streaming Markdown remains responsive, never commits stale parse/highlight work, and reaches the same final document as a full settled parse;
- a side-by-side review confirms close layout, typography, color, icon, state, and interaction treatment for the selected non-shader UI;
- shader fidelity is not claimed and does not block the slice;
- the implementation contains no browser surface or compatibility layer.

### Phase 9: packaged end-to-end proof

Package the Bazel-built frontend and backend together with the resources needed by the first workflow. The package proof must answer how the two processes are located and started without prematurely choosing updater or distribution requirements.

Completion evidence:

- a clean native environment can launch both binaries and complete the selected workflow;
- packaged resources resolve correctly;
- the package contains no Electron, browser bundle, Node runtime, or TypeScript Forge;
- the package action is deterministic apart from explicitly declared release metadata;
- installer/signing/update questions discovered by the proof are recorded as decisions, not silently guessed.

### Phase 10: capability-by-capability native development

After the first end-to-end proof, select additional product capabilities by priority. Each capability is implemented as a vertical slice through only the layers it needs:

1. domain changes;
2. protocol changes;
3. transport behavior if genuinely new;
4. database/migration work if state is durable;
5. backend behavior;
6. asset and UI archaeology when the capability adds or changes a surface;
7. shared `ui` framework changes required by the audited behavior;
8. frontend behavior and composition;
9. focused tests and package impact.

The TypeScript codebase informs the capability's details and behavior, but it does not dictate the Rust/GPUI design. Preserve the meaningful product behavior, improve the system when there is a better native design, and make intentional differences explicit. Work is complete when the selected native behavior is implemented and verified—not when every historical TypeScript implementation has been copied.

### Phase 11: browser and legacy retirement

Retire old components when the explicitly selected native product scope is sufficient to replace them. This is a product-scope decision, not an exact-parity gate.

Before deletion, identify the data and operational knowledge intentionally retained. Preserve the completed UI behavior inventory, SVG inventory, and vendored sources independently of legacy frontend deletion, then remove obsolete browser/TypeScript build and runtime dependencies from the shipping path.

Completion evidence:

- the supported native workflows require only the new repository's artifacts;
- release and support documentation describe the native system;
- no deprecated browser runtime is shipped as a fallback.

## Work decomposition and controlled delegation

One controlling agent owns the active phase, architecture, shared manifests, integration, and the claim that the phase is complete. Subagents accelerate bounded work; they do not collectively design the system by accident.

### Work packet contract

Every delegated packet contains:

1. one concrete objective and observable result;
2. the approved design decisions it must follow;
3. explicit non-goals, including whether the task is inventory, implementation, or review;
4. relevant TypeScript references and which behavior/details should be retained;
5. owned files or directories and files it must not edit;
6. allowed dependency or schema changes;
7. expected Bazel targets and tests;
8. the evidence the agent must return: changed files, commands run, results, and unresolved questions;
9. for UI work, the approved inventory records and exact call-site, wrapper, style, Bits UI, and GPUI references that define the expected behavior.

A packet may improve the implementation inside its approved boundary. It may not choose new product scope, require global TypeScript parity, mechanically copy Effect architecture, change a shared interface without approval, or hide a behavior change.

### Delegation sequence

1. **Reconnaissance:** a read-only agent inventories the relevant TypeScript behavior, UI details, assets, and existing failure cases. UI reconnaissance follows the complete call-site-to-wrapper-to-Bits chain and checks native GPUI facilities. It reports evidence; it does not propose a bulk translation.
2. **Design:** the controlling agent chooses the native boundary and records intentional improvements before writers begin.
3. **Foundation:** agents may implement independent proofs or leaf crates in parallel after their interfaces and file ownership are fixed.
4. **Implementation:** each writer owns a disjoint packet, preferably in an isolated Git worktree once the repository is initialized.
5. **Integration:** the controlling agent reads every diff, resolves interface changes, updates shared Bazel/Cargo state, and runs targeted checks followed by the authoritative broader graph.
6. **Independent review:** agents that did not implement the packet review failure/security behavior and code quality/idiomatic Rust separately. Overlapping findings receive extra weight.
7. **Closure:** the controlling agent fixes or delegates findings, reruns the required Bazel targets, and closes the packet only when its evidence matches the objective.

### Shared-framework subagent assignment

After the relevant archaeology records and public boundary are approved, the controller assigns one lead framework-builder subagent coherent ownership of `modules/ui/` and `tests/ui/` in an isolated worktree. Giving the framework one primary writer prevents competing focus, overlay, event, and styling abstractions from appearing in parallel.

The framework builder receives the first workflow, reviewed behavior contracts, approved public API, allowed dependencies, and explicit non-goals. It may improve the internal native design, but it may not port dormant wrappers, copy the Bits UI API, absorb product-specific behavior, alter root Bazel/Cargo state, or change `frontend` and `assets` ownership without controller approval.

Separate read-only agents audit the legacy behavior, review native accessibility and interaction behavior, and perform visual comparison. The controller reads the implementation, owns shared interface decisions, integrates the worktree, and runs the authoritative Bazel graph. Later primitive families can become separate packets only after the foundational focus, overlay, navigation, theme, and event interfaces have stabilized and file ownership can remain disjoint.

### Parallelization boundaries

Parallelize work that has genuinely disjoint ownership. Serialize work that defines an interface used by the other packets.

- Root `MODULE.bazel`, `Cargo.toml`, lockfiles, toolchain configuration, and shared Bazel macros have one writer at a time.
- Domain and protocol decisions land before frontend/backend agents implement against them.
- Migration ordering and database interfaces land before multiple backend writers depend on them.
- Theme/assets work and read-only UI archaeology may proceed in parallel.
- The controller approves the `ui` API; the framework builder is its sole writer while the foundation is taking shape.
- Overlay/focus behavior lands before floating-component packets, and shared keyboard navigation lands before menu/select/command packets fan out.
- The asset manifest/API and shared GPUI framework land before screen-level UI packets fan out.
- A writer does not edit another writer's files to make its own packet pass; it reports the required interface change to the controller.
- Keep one concurrency slot for the controlling agent. Use the remaining slots for two or three bounded agents only when their dependency edges allow it.

### Phase delegation map

| Phase | Suitable delegated packets | Controller-owned work |
| --- | --- | --- |
| Bazel foundation | GPUI feasibility proof; Quinn/Cap'n Proto proof; SeaORM/macro proof; cache audit | Root toolchains, dependency policy, shared Bazel design, final integration |
| Domain/protocol | Read-only TS behavior inventory; approved schema/codegen implementation; protocol test corpus | Native domain boundaries, message semantics, intentional redesigns |
| Transport | Quinn loopback implementation; malformed/cancellation tests; independent security review | Trust/authentication decision, public transport API |
| Database/backend | Schema inventory; isolated migration/repository packets; failure-path review | Native schema decision, transaction/service boundaries, backend assembly |
| UI archaeology/assets | Call-site and wrapper census; Bits UI behavior trace; GPUI capability audit; Tabler/SVGL/custom SVG extraction; asset validation | Reachability decisions, behavior-contract approval, asset API, visual direction, shader deferral |
| Shared GPUI framework | Approved theme/state-light controls; overlay/focus proof; navigation/selection proof; component gallery; behavior tests | Public `ui` API, framework boundary, product-versus-shared decisions, final integration |
| GPUI frontend | Independent screens/components after the framework stabilizes; product-state tests | Application state model, navigation/composition, visual integration review |
| Packaging | Tool feasibility investigation; clean-package smoke harness | Packaging choice, artifact contract, release decisions |

The controlling agent always retains final integration and verification. Subagent summaries are leads, not proof; the controller inspects the resulting files and command evidence directly.

## Open decisions

These are intentionally unresolved and should be decided when their phase needs them:

1. The first native end-to-end product workflow.
2. The QUIC certificate, peer-trust, pairing, and authentication model for local and any future remote use.
3. Whether legacy SQLite data is imported, selectively migrated, or left with the legacy product.
4. The remote-cache provider and trust/credential policy.
5. Installer format, signing, updater, and distribution.
6. Desktop platforms after the initial native-host proof.
7. Whether and when Nix is useful for Forge deployment or non-Windows development.

Mobile remains outside this plan unless it becomes a separately approved product target.

## Plan completion

The port plan has been executed when:

- Bazel natively builds, tests, checks, generates, and packages the selected Rust product without delegating the build to Cargo, Node, or pnpm;
- the selected native product scope runs through GPUI frontend, Quinn/QUIC, Cap'n Proto, Rust backend, and SQLite/SeaORM end to end;
- selected capabilities preserve meaningful TypeScript product detail and behavior except where an improvement is intentional and documented;
- the Rust/GPUI implementation is idiomatic and does not recreate TypeScript or Effect architecture mechanically;
- the reachable legacy UI stack has been traced through product call sites, local wrappers, styles, and Bits UI internals, with dormant wrappers distinguished from behavior that is actually used;
- a first-party `ui` crate provides the shared GPUI visual and interaction framework, and its implemented primitives have evidence-linked behavior tests rather than screenshot-based guesses;
- selected UI surfaces closely retain their non-shader layout, typography, colors, icons, states, and interactions;
- every old frontend SVG use has been inventoried, every referenced static SVG is vendored with provenance through the `assets` crate, and data-driven SVGs selected for native UI are reimplemented natively;
- shaders remain explicitly deferred rather than being partially implemented or treated as a hidden completion gate;
- tests are organized under `tests/` and run as cacheable Bazel targets;
- any project-specific tooling lives under `scripts/` and does not duplicate the build graph;
- first-party packages satisfy the `anyhow` prohibition;
- the shipping native product contains no browser frontend or TypeScript Forge;
- intentionally unresolved product choices have been decided only when their work is approved;
- completion is based on the selected native scope, not exact TypeScript parity.

## Primary technology references

- [Bazel `rules_rust`](https://bazelbuild.github.io/rules_rust/)
- [Bazel Crate Universe](https://bazelbuild.github.io/rules_rust/crate_universe_bzlmod.html)
- [Bazel remote caching](https://bazel.build/remote/caching)
- [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui)
- [Quinn](https://github.com/quinn-rs/quinn)
- [Cap'n Proto for Rust](https://github.com/capnproto/capnproto-rust)
- [SeaORM](https://www.sea-ql.org/SeaORM/)
- [SeaORM migrations](https://www.sea-ql.org/SeaORM/docs/migration/setting-up-migration/)
