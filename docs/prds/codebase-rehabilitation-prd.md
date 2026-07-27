# Artisan Editor Codebase Rehabilitation PRD

Status: In progress

Owner: Architecture

Last updated: 2026-07-26

## Summary

Artisan Editor needs a bounded architecture-rehabilitation milestone before more
backend surface area is added. The milestone will:

1. replace handwritten frontend-to-Forge request/result plumbing with one
   authoritative typed RPC contract;
2. move application byte streams onto native binary WebSocket frames and select
   a control-frame codec from measurements, not preference;
3. reduce root-level documentation and script clutter without deleting active
   project state;
4. remove the dormant native bounded file-store package and its build, packaging,
   adapter, test, and dependency surface atomically; and
5. decompose the giant files and duplicated invariants already identified by the
   maintainability audit.

This PRD records desired outcomes. It does not mark any rehabilitation slice as
implemented without repository evidence.

## Migration Status

Verified as of 2026-07-26:

- Root product, design, research, engineering, and status documents have moved
  under the indexed `docs/` hierarchy.
- Active root scripts have moved under `.scripts/{build,dev,package}` and their
  repository references have been updated in the current working tree.
- The dormant `@artisan/bounded-file-store-native` package, adapter, build,
  packaging, dependency, and dedicated test surface have been removed in the
  current working tree. The generic shared-checkout bounded-store contract
  remains intentionally in scope for a separate audit.
- Root `MEMORY.md` has been replaced atomically by the capped
  `.memory/active-handoff.md`; durable verified product truth remains in
  `docs/status/backend-completion-matrix.md`.
- Control frames use Effect RPC MessagePack serialization, byte streams remain
  native `Uint8Array` WebSocket frames, and one `ControlRpcGroup` derives
  request/success/error associations. Thread, workspace-inspection, and
  Marketplace read domains plus Guidance/Model Behaviour mutations now use
  extracted typed Effect handlers.
- Orchestration outbox ownership and workspace-change persisted-row codecs have
  moved into dedicated modules while retaining the canonical transaction-local
  journal append invariant.
- The integrated `pnpm run validate` gate passes 1,301 tests across 183 files
  with 4 explicit skips.

Still open:

- Continue decomposing the remaining mutation routing and giant persistence
  ownership files as normal maintainability work; the typed wire contract and
  required rehabilitation boundaries are complete.

## Why Now

The current branch contains strong individual capabilities but weak ownership
boundaries:

- The frontend/Forge protocol is runtime-typed with Effect Schema, but request
  and result associations are duplicated manually across a very large protocol
  and client facade.
- The portable transport already models binary stream chunks, but its WebSocket
  adapter converts those bytes to base64 inside JSON text frames.
- Root documentation mixes current product truth, historical build notes,
  research, design language, and a 200 KB append-only agent handoff.
- Active repository scripts and installer source are split between `scripts/`
  and `build/` with no purpose-based structure.
- The Windows bounded file-store is a serious security kernel, not a trivial
  helper, but normal production composition does not currently instantiate its
  registry.
- The thermo-nuclear maintainability review found giant ownership surfaces and
  duplicated transaction invariants that make further work increasingly risky.

## Product Principles

- Preserve verified behavior before changing representation.
- One schema declaration owns each application request, result, error, and
  streaming shape.
- Prefer Effect capabilities already present in the repository before adding a
  parallel framework.
- Binary is a wire property, not proof of a typed contract.
- Generated code is acceptable only when its source of truth and evolution
  policy are explicit.
- Repository organization must distinguish current truth, historical context,
  generated output, and transient agent state.
- Native complexity must purchase a stated product guarantee and must be reached
  by production composition.
- No migration earns completion status from unit tests alone.

## Scope

### In scope

- All application control RPCs and application byte streams between the
  immutable frontend and Artisan Forge.
- Transport framing, codec selection, protocol evolution, malformed-input
  handling, request/result derivation, reconnect, replay, ACK, cancellation,
  subscriptions, and backpressure.
- Root Markdown and repository-owned script organization.
- The root `MEMORY.md` agent handoff contract present at the verified baseline.
- Product disposition and maintainability of
  `@artisan/bounded-file-store-native`.
- Previously identified blocker-level giant files and duplicated persistence
  invariants where they directly affect these boundaries.

### Out of scope

- Replacing WebSocket solely because another transport is fashionable.
- Encoding static frontend assets, health endpoints, or installer payloads as
  RPC.
- Moving canonical tool configuration merely to make the root visually empty.
- Weakening current replay, crash-recovery, filesystem-confinement, or
  authentication guarantees without an explicit product decision.
- Adding remote, network-filesystem, removable-media, or power-loss guarantees
  to the bounded file-store.

## Verified Baseline

### Frontend-to-Forge transport

The current transport is typed at runtime, but not generated from one RPC
registry:

- `modules/protocol/src/control.ts` declares the large control envelope with
  Effect Schema.
- `modules/transport/src/wire.ts` adds versioning, connection fencing, control
  and stream channels, stream sequencing, and `Uint8Array` chunks.
- `modules/transport/src/message-port.ts` owns the portable bounded-queue and
  scoped-lifetime semantics.
- `modules/transport/src/websocket/protocol.ts` accepts text frames only,
  serializes with JSON, and recursively wraps `Uint8Array` as base64.
- `modules/transport/src/internal/client-common.ts` manually maintains parallel
  request and result unions.
- Forge applies a 16 MiB WebSocket payload cap plus loopback, origin, session,
  and authentication protections.

The first problem is contract duplication. JSON/base64 is a second, separable
problem.

### Repository root

The root currently contains:

- active product and completion documents;
- historical frontend and agent-engine notes;
- design-language research;
- a very large append-only `MEMORY.md`;
- active formatter and editor configuration; and
- active script/config entrypoints.

`.gitignore`, `.gitattributes`, `.editorconfig`, `.oxfmtignore`, and
`.oxfmtrc.json` are normal, active repository metadata. They are not cleanup
targets merely because they appear at root.

### Native bounded file-store

`modules/bounded-file-store-native` is a private pnpm workspace package whose
implementation is a Rust `cdylib` compiled with napi-rs into a Windows Node-API
addon. In this repository, "module" generally means a workspace package with an
owned capability and dependency boundary; it does not imply that the package is
small or JavaScript-only.

The native package pins a fixed local NTFS root by kernel handle and provides:

- bounded, stable reads of regular, single-link files;
- handle-relative traversal that rejects reparse traversal;
- compare-and-swap-like regular-file replacement;
- authenticated NTFS extended-attribute receipts;
- recovery across tested process-crash windows; and
- explicit replacement finalization.

Its complexity exists to defend against pathname races, root/path renames,
symlinks/reparse points, hard links, concurrent mutation, and ambiguous
replacement recovery. Ordinary Node pathname APIs do not provide the same
contract.

The capability is nevertheless dormant in ordinary production composition:
the backend falls back to an empty bounded-store registry, and nonempty registry
construction is currently found only in tests. The desktop build still compiles
and stages the addon.

## Requirement 1: One Typed Frontend-to-Forge Contract

All frontend-to-Forge application communication must be declared in one
authoritative contract registry. From that registry, the implementation must
derive:

- client method names and input types;
- success and domain-error types;
- server handler requirements;
- streaming result types;
- runtime input/output validation;
- protocol documentation or inventory; and
- compatibility fixtures.

Adding a request must not require editing a separate request union, result
union, switch statement, and facade signature by hand.

The contract must remain renderer-safe. The frontend must not import Forge,
backend, Node, Electron, or native implementation packages.

### Effect-first spike

The spike must first evaluate the installed Effect 4
`effect/unstable/rpc` capability. It already provides schema-aware RPC groups,
typed clients and handlers, streaming results, socket protocols, middleware,
and pluggable serialization, including JSON and MessagePack.

Because that API is explicitly unstable, adoption requires compatibility
fixtures and an isolation boundary owned by `@artisan/transport`; application
code must not spread unstable Effect RPC types throughout the repository.

The spike must prove whether Effect RPC can preserve or cleanly host Artisan's
existing:

- semantic command idempotency;
- connection fencing;
- durable replay and ACK behavior;
- cursor resume and gap detection;
- subscription snapshots and patches;
- stream tickets, ordering, cancellation, and close-on-overflow behavior;
- trace/authentication metadata; and
- bounded queues and scoped cleanup.

If it cannot, the contract registry may remain Artisan-owned while still
deriving both client and server surfaces from one declaration.

## Requirement 2: Native Binary Frames and Evidence-Gated Codec Selection

Application byte streams must use native binary WebSocket frames. Terminal
output, asset content, and other `Uint8Array` payloads must not be base64-wrapped
inside JSON in the final architecture.

The codec boundary must be explicit and independently testable. JSON may remain
available for diagnostics, migration, or control frames if measurements justify
it.

### Required comparison

Use a captured, anonymized corpus containing:

- small and large control requests;
- success and domain-error responses;
- conversation snapshots and patches;
- terminal traffic;
- representative asset chunks; and
- payloads at and beyond the configured maximum.

The redacted corpus, capture/replay instructions, expected schema version, and
benchmark command must be checked in under `.tests/fixtures/transport/` and
`.scripts/bench/`. Every result must record hardware, operating system,
Node/browser/runtime versions, warmup and sample counts, raw output, and the
decision thresholds selected before the comparison. The codec decision belongs
in an architecture decision record linked from this PRD.

Compare at least:

1. the current JSON/base64 adapter;
2. JSON control frames plus native binary stream frames;
3. Effect RPC JSON and Effect RPC MessagePack where the RPC spike is viable;
4. CBOR with the existing Effect Schema source of truth; and
5. Protobuf using Buf Protobuf-ES when a generated cross-language contract is a
   real product goal.

Measure:

- encoded bytes by corpus category;
- encode/decode p50 and p99;
- allocation and garbage-collection pressure;
- browser startup and bundle impact;
- peak memory at the 16 MiB boundary;
- malformed and adversarial input rejection;
- current/previous client compatibility;
- reconnect, replay, cancellation, and backpressure behavior; and
- developer steps required to add and evolve one RPC.

The accepted option must improve byte-stream handling and must not regress
protocol semantics.

The binary framing specification must define:

- encoded and decoded payload limits before allocation;
- whether compression is permitted and the decompressed-size limit;
- stable close/error behavior for invalid kind, version, length, or payload;
- control-versus-stream frame discrimination without trial decoding;
- browser `binaryType = "arraybuffer"` handling without a `Blob` conversion
  fallback; and
- a rollout plan that prevents an unnegotiated client/server flag day, using
  explicit version negotiation and a bounded dual-codec migration window when
  compatibility is required.

### Codec disposition

- **CBOR** is a compact, browser-suitable binary codec with a native byte-string
  type. It can preserve Effect Schema as the source of truth, but it does not
  create a typed request/result contract by itself. If deterministic encoding
  matters, the protocol must require and test the RFC's deterministic profile.
- **MessagePack** is the lowest-friction all-binary candidate if Effect RPC is
  adopted because Effect ships the serializer. Its evolution and canonical
  representation still require Artisan policy.
- **Protobuf-ES** is preferred only if Artisan needs a durable generated
  contract consumable outside TypeScript. The team must own field-number
  evolution and decide whether `.proto` or Effect Schema is authoritative;
  maintaining two handwritten schema systems is not acceptable.
- **Cap'n Proto** is rejected for the browser path unless a separate spike
  overturns its current toolchain and ecosystem mismatch. Its official
  JavaScript support is Node-oriented, and the available TypeScript browser
  implementation is not presented as tested browser support.

The implementation decision is therefore:

> Artisan frontend-to-Forge application transport will eliminate base64 byte
> streams and derive client/server behavior from one typed RPC contract. The
> control codec will be selected from measured results; this PRD does not
> preselect CBOR, MessagePack, or Protobuf.

### Research sources

- [Effect RPC API](https://effect-ts.github.io/effect/docs/rpc)
- [Effect RPC serialization API](https://effect-ts.github.io/effect/docs/rpc/RpcSerialization.ts.html)
- [WebSocket binaryType](https://developer.mozilla.org/en-US/docs/Web/API/WebSocket/binaryType)
- [RFC 8949: CBOR](https://www.rfc-editor.org/rfc/rfc8949.html)
- [Protocol Buffers overview](https://protobuf.dev/overview/)
- [Protocol Buffers evolution rules](https://protobuf.dev/programming-guides/proto3/#updating)
- [Buf Protobuf-ES](https://github.com/bufbuild/protobuf-es)
- [Cap'n Proto language support](https://capnproto.org/otherlang.html)

## Requirement 3: Documentation and Handoff Structure

The final root should contain only documents that function as repository entry
points. At minimum, `AGENTS.md` remains at root. A future `README.md`,
`CONTRIBUTING.md`, `LICENSE`, or security policy may also belong there.

The future root `README.md` must be a polished product entry point modeled on
the presentation of the
[Orca README](https://github.com/stablyai/orca/blob/main/README.md?plain=1).
It must include a neat, scannable feature table that pairs each major Artisan
capability with concise benefit-led copy and an accurate screenshot, animation,
or other purposeful visual. The showcase must be followed by a compact inventory
covering all remaining shipped features, supported engines and platforms, with
links to deeper documentation where available. README claims and visuals must
describe verified product behavior rather than planned or aspirational scope,
and the inventory must be updated whenever a milestone adds, removes, or
materially changes a user-facing capability.

Target document layout:

```text
docs/
  README.md
  architecture/
  design/
  engineering/
  history/
  prds/
  release/
  research/
  status/
```

Migration candidates:

| Current file                   | Target                                     |
| ------------------------------ | ------------------------------------------ |
| `artisan-editor-prd.md`        | `docs/prds/artisan-editor-v1.md`           |
| `backend-completion-matrix.md` | `docs/status/backend-completion-matrix.md` |
| `agent-engine-io-research.md`  | `docs/research/agent-engine-io.md`         |
| `barekey-design-language.md`   | `docs/design/barekey-design-language.md`   |
| `frontend-checkpoints.md`      | `docs/engineering/frontend-checkpoints.md` |

All links, scripts, tests, and agent instructions must be updated in the same
change as each move. Files move with Git history; they are not copied and left
duplicated.

`frontend-checkpoints.md` currently mixes active completion rules with
historical checkpoints. The migration must preserve the active rules under
`docs/engineering/` and may split genuinely historical narrative into
`docs/history/`; it must not relabel the whole document as historical without an
audit. `docs/README.md` must index the canonical PRDs, current status, active
engineering rules, architecture decisions, and research.

### Retiring `MEMORY.md`

At the verified baseline, root `MEMORY.md` could not be deleted first because
`AGENTS.md` required every session to read and update it. Deletion without
replacement would have broken the repository's operating contract and discarded
active state.

Retirement requires:

1. extract durable product truth into the relevant PRD, architecture, status,
   or decision document;
2. extract the current branch handoff into a concise, committed
   `.memory/active-handoff.md` that is not ignored;
3. archive only historically useful narrative under `docs/history/`; discard
   redundant transcript-like accumulation;
4. cap the active handoff and rotate completed milestone detail;
5. change `AGENTS.md` and every link atomically;
6. redact user-machine paths, process incident details, credentials, tokens, and
   other sensitive local context before preserving any material under
   `docs/history/`; and
7. delete root `MEMORY.md` only after the replacement is readable in a fresh
   checkout and contains the unresolved work needed by the next session.

Durable implementation truth belongs in committed `docs/status/`; the committed
`.memory/active-handoff.md` contains only concise branch/session continuity.
`AGENTS.md` must require both at the appropriate times and must cap the active
handoff so it cannot become another append-only history.

## Requirement 4: Script Ownership

Repository-owned scripts must move into a hidden, purpose-based root:

```text
.scripts/
  build/
  dev/
  package/
```

Initial mapping:

| Current path                        | Target area              |
| ----------------------------------- | ------------------------ |
| `scripts/electron-before-build.cjs` | `.scripts/build/`        |
| `scripts/start-browser-forge.ps1`   | `.scripts/dev/`          |
| `scripts/update-user-path.ps1`      | `.scripts/package/`      |
| `build/nsis/artisan-path.nsh`       | `.scripts/package/nsis/` |

The NSIS include is hand-maintained installer source, not generated build
output. It must move with its `desktop-builder.yml` reference.

Module-private build and verification scripts may stay next to the module they
own. In particular, native-addon scripts are not root script clutter.

Every move must update `package.json`, `desktop-builder.yml`, tests, and internal
references in one change. Prefer TypeScript for new or substantially rewritten
Node scripts; a mechanical move does not require an unrelated language rewrite.

Windows acceptance must specifically prove that:

- root `package.json` can launch the moved Forge development supervisor;
- electron-builder resolves the moved `beforeBuild` hook;
- NSIS resolves the moved `artisan-path.nsh` include; and
- `forge.vite.config.ts` copies the moved `update-user-path.ps1` into the
  packaged Forge output.

Root build-tool configuration is a separate decision. Active files such as
Vite, TypeScript, Vitest, formatter, editor, package-manager, and desktop-builder
configuration may remain at root when their tools expect or conventionally
discover them there.

## Requirement 5: Remove the Dormant Native Bounded Store

`@artisan/bounded-file-store-native` has no non-test production construction
site. Normal backend composition installs
`EmptyWorkspaceBoundedRegularFileStoreRegistryLive`, while desktop builds still
compile and package the Windows addon. Cowork's selected API-native vault
architecture does not require a protected ordinary NTFS checkout.

Remove the dormant native capability atomically:

- `modules/bounded-file-store-native/`;
- `.tests/bounded-file-store-native/`;
- the backend native adapter and native initialization errors;
- the backend workspace dependency and lockfile importer;
- root desktop-build compilation;
- desktop Vite staging and missing-artifact checks;
- native-specific desktop configuration and release assertions;
- backend exports that construct or expose the native adapter; and
- documentation, completion claims, and verification instructions that imply the
  addon is a production capability.

The change must prove that no production or packaging path loads, builds, stages,
or references the addon.

### Separate shared-checkout audit

Do not conflate deletion of the native addon with immediate deletion of every
generic workspace-file abstraction. `BoundedRegularFileStore`, the
non-adversarial Node implementation, workspace mutation protocols, snapshots,
payloads, review, rollback, and reconciliation form a broader shared-checkout
architecture exercised by tests even though normal production currently
installs an empty bounded-store registry.

After the addon deletion, audit that broader stack against default
harness-native behavior and the Cowork PRD. Remove dead shared-checkout mutation
in a separate coherent change, preserving any generic file browsing, diff,
history, or other behavior still required outside Cowork.

## Requirement 6: Close the Existing Maintainability Blockers

This rehabilitation milestone also owns the directly related findings from the
2026-07-26 thermo-nuclear code-quality review:

1. Centralize transaction-local journal/event append and projection invariants
   in `JournalStore`; repositories must not carry drifting private copies.
2. Decompose the 4,900-line `ProtocolServer` by capability and registration
   ownership while the typed RPC contract is introduced.
3. Split the orchestration command/observation repository and workspace
   reconciliation repository along transaction and domain boundaries.
4. Remove or lazily acquire the unused frontend `LiveWorkspaceStore`; the
   production frontend must not eagerly allocate a giant unused capability.
5. Remove pervasive `any` and cast-based decoding at the conversation
   projection transaction/schema boundary.

These are architecture migrations. Each requires characterization tests before
movement and full validation afterward.

## Delivery Sequence

### Phase 0: Baseline and freeze

- Capture current transport corpus, payload limits, behavior tests, package
  sizes, and representative performance.
- Approve the benchmark thresholds and reproducibility metadata before running
  codec comparisons.
- Record all current imports and references for documents/scripts before moves.
- Record the exact native-addon deletion inventory and the boundary of the
  separate shared-checkout mutation audit.

### Phase 1: Contract registry

- Build the Effect RPC compatibility spike.
- Introduce one request-to-result registry behind the existing public client.
- Derive the client/server inventory without changing the wire codec.
- Characterize reconnect, replay, subscription, stream, and cancellation
  behavior.

### Phase 2: Binary wire

- Move raw byte streams to binary WebSocket frames.
- Run the required codec comparison.
- Adopt the winning codec behind the codec boundary.
- Add current/previous version-skew and malformed-input fixtures.

### Phase 3: Native-addon removal

- Remove the native package, adapter, dependency, build, staging, smoke-test,
  export, documentation, and release surface atomically.
- Verify desktop development/build/package behavior without any native addon
  artifact.
- Inventory the broader shared-checkout mutation stack for a separate deletion
  or preservation decision.

### Phase 4: Repository organization

- Establish `docs/`, `.memory/`, and `.scripts/` conventions.
- Move documents and scripts in reviewable batches with references updated.
- Retire root `MEMORY.md` only in the atomic handoff migration.

### Phase 5: Structural debt

- Centralize persistence invariants.
- Decompose the remaining giant files.
- Remove unused eager frontend capability acquisition and unsafe schema casts.

## Completion Gates

The milestone is complete only when:

- adding one frontend-to-Forge RPC requires one authoritative contract change;
- application byte streams cross WebSocket as binary without base64;
- the codec decision is recorded with reproducible corpus measurements;
- the selected framing enforces encoded/decompressed limits and stable protocol
  close behavior before unbounded allocation;
- all existing replay, ACK, subscription, stream, reconnect, cancellation,
  authentication, and malformed-input tests pass;
- a negotiated rollout supports the approved previous-client/current-Forge and
  current-client/previous-Forge window without a flag day, or the product
  explicitly chooses and surfaces a hard version boundary;
- root Markdown and scripts match the approved structure with no broken links or
  command references;
- a repository check proves old document/script paths have no references outside
  explicit migration-history notes, a Markdown link checker passes, and the
  package/Forge Vite/electron-builder/NSIS command-reference smoke passes;
- `docs/README.md` makes this rehabilitation PRD and every canonical current
  document discoverable;
- root `MEMORY.md` is either still the valid handoff or has been atomically
  replaced; it is never simply absent;
- no source, dependency, build, package, test, documentation, or release path
  references or stages `@artisan/bounded-file-store-native`;
- desktop build and package verification pass without a native addon artifact;
- the broader shared-checkout mutation stack has a separately recorded,
  evidence-backed disposition;
- the completion matrix reflects only verified production state;
- `pnpm run validate` passes from the reviewed worktree; and
- release-only packaging gates pass for every affected platform boundary.

## Risks

- A codec rewrite can consume substantial effort while leaving the duplicated
  RPC contract untouched.
- Effect RPC's unstable API can leak churn if it is not isolated.
- Protobuf can create two competing schemas if generation ownership is not
  explicit.
- Moving active documents/scripts without exhaustive reference updates can
  silently break development and packaging.
- Deleting the current handoff early can erase the only record of unresolved
  branch state.
- Removing the addon and the broader shared-checkout mutation stack in one
  undifferentiated deletion can accidentally remove file browsing, diff, history,
  review, or rollback behavior still needed outside Cowork.
