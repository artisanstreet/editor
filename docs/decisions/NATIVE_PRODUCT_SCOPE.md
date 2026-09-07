# Native product scope and first vertical workflow

Status: approved for implementation on 2026-08-24.

This decision unblocks Phases 2 through 11. It is a product boundary, not a
request to reproduce the TypeScript, Effect, Svelte, Electron, or browser
architecture in Rust. Meaningful product behavior and detail remain evidence;
the native design remains idiomatic Rust, Quinn, SeaORM, and GPUI.

## Shipping target

The shipping target is a native desktop Artisan product built and packaged by
Bazel:

- a GPUI editor process;
- a Rust Forge process;
- direct Quinn/QUIC communication using Cap'n Proto application messages;
- a Forge-owned file-backed SQLite database through SeaORM;
- no Electron, WebView, browser bundle, Node runtime, TypeScript Forge,
  WebSocket, or WebTransport compatibility path in the shipped artifact.

The complete native port covers the current runtime-reachable core product:

- project attachment and project identity;
- thread creation, listing, navigation, retention, and titles;
- composer drafts, bounded text and attachments, message submission, steering,
  cancellation, and durable receipts;
- real assistant execution and streamed conversation presentation;
- Markdown, fenced-code highlighting, approvals, questions, errors, activity,
  changes, usage interruptions, and durable run interaction state;
- orchestration and agent inspection;
- terminal sessions and output;
- engine installation/setup and model selection required by supported engines;
- settings, notifications, privacy/telemetry consent, attention, diagnostics,
  recovery, and graceful lifecycle behavior;
- the reachable read/open editor surface and workspace file presentation.

Capabilities that are only contracts, tests, debug galleries, drafts, dormant
wrappers, or otherwise not reachable from the shipping product are not copied
automatically. They enter scope when their product capability is deliberately
approved. The current editor save/conflict contract is therefore not treated as
shipping behavior until runtime reachability proves otherwise.

Mobile remains outside this port. Shader-backed effects remain deferred, but
their absence does not relax non-shader layout, typography, color, icon, state,
motion, focus, or interaction fidelity.

## First vertical workflow

The first workflow is implemented in reviewable milestones but ends as a real
assistant turn:

1. attach an opaque local project directory;
2. list and create project-assigned threads;
3. accept a bounded first user text message idempotently and persist a queued
   receipt;
4. dispatch that accepted message through one supported real engine adapter;
5. stream assistant events through Forge and QUIC;
6. render the final transcript natively through GPUI and the shared Markdown
   document/highlighting seam;
7. restart both processes and recover the durable project, thread, user message,
   final assistant message, and selected observable run state.

The queued receipt is an intermediate stack milestone, not the definition of a
finished product workflow. A deterministic event source is allowed only in
tests and component harnesses so streaming coordination and rendering can be
developed before the real engine adapter lands. Phase 8 and Phase 9 completion
require the real engine path; no fake engine ships as the product proof.

## Identity, validation, and event decisions

- Forge mints durable project, thread, message, and run identifiers.
- The client mints retry-stable request and command identifiers.
- External string and payload bounds are expressed in UTF-8 bytes, with a
  separate scalar-count rule only where product presentation needs one.
- Commands are idempotent at the durable transaction boundary.
- Request/response correlation is single-use. Late or duplicate completions are
  rejected without reusing a retired correlation identifier.
- Event streams carry a monotonically increasing stream revision/cursor. A
  replay gap or invalid cursor produces an explicit resnapshot requirement.
- Forge may use a durable journal/outbox internally; its storage layout does not
  become the public protocol.
- The protocol has an explicit application hello/welcome exchange and one
  initially supported protocol version. Unsupported versions fail closed with
  a typed error.

## QUIC scope and trust

The initial supported connection scope is local desktop only. Remote access and
mobile pairing are disabled until separately designed and approved.

- Forge binds a loopback QUIC endpoint by default.
- The packaged editor starts its sibling Forge by default; Forge also remains
  independently runnable for tests, diagnostics, and controlled deployments.
- The launch handoff gives the editor the exact endpoint, a pinned certificate
  identity/fingerprint, and a high-entropy one-time bootstrap capability through
  an authenticated, access-restricted local handoff.
- TLS certificate pinning authenticates the launched Forge endpoint. The
  application hello consumes the bootstrap capability and establishes the
  bounded session.
- Discovery by scanning ports, trusting arbitrary local certificates, or
  accepting a connection solely because it originated from loopback is not
  allowed.
- Remote trust, pairing, durable peer identity, and certificate rotation are
  later security decisions, not hidden compatibility behavior.

The transport exposes owned Artisan session/events APIs. Quinn handles do not
become the public frontend or domain interface, and network work never runs on
the GPUI thread.

## Database disposition

The native product starts with a new native SQLite schema and database file.
Forge is the only production process allowed to open it.

- Migrations are forward-only, ordered, repeatable at startup, and owned by the
  native repository.
- The legacy database is never silently opened or mutated in place.
- Legacy import is a separate, optional, explicit migration product with
  sanitized fixtures and rollback based on an untouched clone of the old data.
- Runtime leases, transient handoff state, pending process-owned work, and other
  rows that cannot be safely resumed are not imported implicitly.

This permits the native architecture to improve the schema instead of freezing
the historical Drizzle layout into a parity requirement.

## Process and package decisions

- The normal packaged lifecycle is editor-owned sibling Forge with explicit
  readiness, authenticated handoff, bounded shutdown, and orphan containment.
- Forge remains a separately executable Bazel binary; the UI is not its process
  supervisor abstraction.
- Phase 9 first proves a relocatable deterministic portable archive. Installer,
  signing, updater, release-channel, and distribution-service choices remain
  deferred until that artifact and clean-host workflow are proven.
- Runtime resources come only from declared Bazel inputs and the typed assets
  crate. Production code does not fall back to source-tree paths or depend on a
  Bazel runfiles layout.

## Completion meaning

The port is complete only when the approved native scope works through the real
GPUI editor, Rust Forge, direct QUIC, Cap'n Proto protocol, SeaORM/SQLite, and at
least one real supported engine; the deterministic package passes a clean-host
workflow; the browser/Node shipping path is removed; preserved data and
operational behavior have explicit disposition; and every required stack entry
has passed its authoritative tests. The agents never merge the stack.
