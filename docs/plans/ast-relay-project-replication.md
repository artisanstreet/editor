# AST accounts, Relay, and intelligent project replication

- Status: design draft; product decisions recorded, protocol details remain open
- Scope: account identity, remote Forge connectivity, local state, and project replication
- Implementation status: this document does not describe shipped functionality

## Settled product decisions

Artisan Editor uses AST accounts. The first-party product will not maintain a separate
accountless mode. AST supplies account identity; it does not store threads, projects,
attachments, project packages, or other durable project/application content. That content
stays on the user's machines, owned by their respective Forges. The account/discovery
metadata needed by AST and Relay still needs a concrete schema.

Editor connects to available Forges, asks each for its threads and projects, and collects
those results into one UI. This is aggregation of independent host state. There is no
account-wide database of conversations and no automatic replication of Forge state.

Offline operation works with whatever hosts the user can reach. A local Forge or a directly
reachable remote Forge remains usable without an AST content service. A Relay-only host
requires a working Relay route. Provider-backed agents also need access to their providers;
reachability of Forge alone cannot supply an unavailable external service.

Pairing grants full trust. Paired participants can use the target Forge's full exposed
surface, including command execution. A Forge can ask another paired Forge to do work.
This is deliberately remote code execution on the user's own computers, under the user's
responsibility. Per-project or read-only permission tiers are not part of this design.
Full Forge access operates with the privileges available to its host process; it does not
magically grant operating-system administrator privileges.

Project replication creates an independent copy at a point in time. Afterward each host
maintains its own copy; users reconcile changes through their VCS. Artisan does not provide
continuous file synchronization, background merging, or a new conflict-resolution system.

The replication button runs an internal agent prompt. That workflow decides what is needed
to make the project work on the destination, including relevant environment files, secrets,
databases, and setup. Nix is an optional tool, not a required environment or platform.

## Current implementation

Each Forge already owns its SQLite database and content. Editor connects directly to paired
Forges using mutual TLS and certificate pinning, maintaining a separate `HostData` view for
each one. The current `artisan_api::Account` is an operating-system username and hostname,
not an AST identity. The current `ae pair` invitation contains an address, code, and
certificate fingerprint and requires a reachable endpoint.

Relevant implementation entry points:

- `crates/artisan-api/src/request.rs`: current account, project, and request contracts.
- `crates/artisan-api/src/thread.rs`: projects, threads, and runs owned by a Forge.
- `crates/editor-core/src/hosts.rs`: device identity and direct host pairing.
- `crates/editor-core/src/host.rs`: per-host application state.
- `crates/forge-store/src/lib.rs`: durable local state and publication after commit.
- `crates/forge-http/src/tls.rs`: current transport authentication.
- `docs/runbooks/remote-forge.md`: current remote access procedure.

The current host-owned storage model is retained. Account identity, Relay connectivity,
Forge-to-Forge calls, and intelligent project replication extend that model. A Forge's
sequence counter remains local to that Forge; it need not become an account-wide order.

## Component responsibilities

| Component | Responsibility |
| --- | --- |
| AST service | Account identity and the account metadata required for host discovery/enrollment |
| Editor | Connect to reachable Forges, aggregate their state, and show host provenance and progress |
| Forge | Own local threads, projects, content, files, runs, and execution bookkeeping |
| Relay | Coordinate pairing and tunnel traffic to otherwise unreachable Forge hosts |
| Internal replication agent | Assess source and destination, decide packaging and setup, and evaluate readiness |
| Transfer primitives | Capture, integrity checks, resumable transport, and destination materialization |

These are logical boundaries; separate deployment units are not yet specified. AST is not
in the durable content path. Forge accepts and records its own commands and results.

## Aggregated projects and threads

Editor requests the thread/project inventory from every connected Forge, then follows
that Forge's updates. Collecting the inventory need not eagerly download every transcript;
full history and content can remain lazy reads from the owning host.

For example:

```text
Editor: combined view of connected hosts
├── Desktop Forge
│   └── Artisan Editor project: /home/sander/projects/editor, its threads and runs
└── Server Forge
    └── Artisan Editor copy: /srv/editor, its threads and runs
```

A project can exist on only one host. Its visibility does not imply that another host has
its files. Editor must preserve host provenance when grouping projects, selecting execution,
and addressing threads. Host-scoped object keys avoid treating unrelated objects as the
same merely because names, paths, or identifiers coincide.

Replicated projects may record their source locally so Editor can explain their relationship.
The exact grouping/lineage contract is open. It must not introduce AST project-content
storage or imply synchronized files. Matching Git remotes alone does not establish that two
checkouts should be treated as one project.

A disconnected host's durable history still lives on that host. The policy for retaining
an Editor-local cache and displaying unavailable hosts is open; any cache remains on the
user's machine and must distinguish stale observations from current host state.

## Remote connectivity through Relay

Forge maintains an outbound connection to Relay. Editor also connects outward. Relay
forwards traffic between them, so the remote router need not accept an unsolicited inbound
connection and the user need not configure port forwarding, a VPN, or a third-party tunnel.

```text
Editor ──outbound connection──> Relay <──outbound connection── Forge
Forge A ──outbound connection──> Relay <──outbound connection── Forge B
```

Resolving Forge identity to an IP address and port does not by itself overcome NAT or
firewalls. Relay must be able to carry application traffic. Direct routes can serve reachable
hosts; NAT traversal with tunnel fallback is a possible later optimization. Losing the
Relay route does not invalidate an independently working direct route.

Only traffic between participating endpoints needs the tunnel. A Forge can contact agent
providers and dependency registries directly. Relay carries a project package when needed
for transfer; it is not a durable project-package store.

### Pairing and trust

1. The remote user runs `ae pair` or its eventual account-aware equivalent.
2. Forge opens its Relay connection and registers a pending request tied to its key.
3. Relay generates a code; the CLI displays it and waits.
4. A signed-in Editor submits that code and its account/device identity.
5. Both parties accept the pairing.
6. The trust association is recorded before pairing is reported complete.

An accepted relationship grants full Forge access. It does not need separate project or
operation scopes. Whether account enrollment automatically trusts additional devices or
hosts remains a protocol decision; full access after pairing does not answer who is paired.

The existing transport uses certificates, not SSH. The pairing protocol should retain
cryptographic identity verification. Code lifetime, replay protection, attempt limits,
identity binding, and revocation mechanics remain to be specified.

Proposed transport boundary: authenticate peers independently of the tunnel and encrypt
sessions end to end. The protocol must explicitly define whether Relay can assert peer
identities or whether initial pairing requires a separately verified key binding. Full trust
in a paired computer does not imply trust in arbitrary tunnel clients or Relay itself.

Forge-to-Forge calls use the accepted trust relationship and full remote execution surface.
The implementation must define whether trust is direct between each pair or delegated by
an account, and prevent forwarded calls from looping or being mistaken for new user work.

## One-action intelligent replication

The user selects a source project and destination Forge, then starts replication. An
internal prompt receives source context, destination capabilities, and the tools needed to
inspect, package, transfer, prepare, and verify. Users do not manage that prompt or choose
among a menu of packaging technologies.

```text
Inspect → Plan → Capture → Transfer → Prepare → Verify → Register destination project
```

The agent decides what to transfer and what to reconstruct for this project. It must not
blindly discard `node_modules`, ignored files, or caches: these can contain modifications
or artifacts that are unavailable elsewhere. Conversely, a Cargo cache need not travel when
its relevant contents can be obtained on the destination.

Project environment files, secrets, local databases, assets, and necessary configuration
can travel when the workflow decides they are needed. There is no blanket exclusion merely
because they are ignored by Git or contain credentials. Content goes to the user's destination
host, not an AST content store. Handling secret values in model context and logs remains an
implementation detail to resolve without changing this transfer policy.

The agent can use native package managers, Nix, containers, direct copies, or project-specific
commands. It assesses the actual destination OS and architecture. It may report that a
project cannot be prepared there; native Windows support must not depend on requiring Nix.

### Reliable execution around the prompt

The agent supplies judgment; deterministic primitives provide reliable mechanics. A proposed
manifest records selected files, reconstruction steps, and readiness checks. The transfer
layer should:

- Capture selected source state, including needed local commits and uncommitted/untracked files.
- Coordinate with writers or detect changes to avoid an inconsistent package.
- Use application-aware capture where necessary for running databases.
- Preserve required metadata where the destination supports it.
- Verify transferred content and resume after interruption.
- Stage a new destination copy before exposing it as ready.
- Preserve existing destination work under a defined collision policy.

Packaging leaves the source intact. Excluding a directory from the package never requires
removing it from the source. Independent-copy semantics remove ongoing merge requirements,
but do not remove the need to capture a coherent initial copy.

Packages and temporary transfer state live on the participating machines. Source or
destination can retain completed chunks to resume; AST does not retain a package for an
offline source. If required data was not transferred before disconnection, wait for a route
to that source rather than pretending the data is centrally available.

Preparation uses the host's granted execution access. Track completed steps and operation
identity so reconnects do not blindly repeat non-idempotent setup. Full pairing removes
application-level permission tiers; commands can still encounter host-level privilege limits.

Choose project-appropriate checks and report what actually passed. A copy, a build, and a
working external integration establish different levels of readiness. Preserve useful work
on partial failure and present the remaining requirement. Normal UI shows progress and
results, while internal prompt details remain out of the main flow.

After completion, both projects evolve independently. VCS handles subsequent collaboration
and reconciliation. Project replication does not itself copy conversation history, migrate
native agent sessions, or move a live run; those semantics remain undecided.

## Questions still requiring decisions

1. **What does the AST account register?** Define host discovery metadata and whether enrolling
   a host makes it usable from all signed-in Editors. Can one Forge belong to multiple accounts?
2. **How is offline identity handled?** Existing trusted, reachable hosts should work offline.
   Specify cached identity, credential expiry, new pairing without AST, and reconnection behavior.
3. **How does trust reach another Forge?** Does every pair approve independently or does account
   membership delegate full access? Define authentication for a forwarded Forge-to-Forge request.
4. **What is retained in Editor?** Only live inventories, or a persistent local cache of history
   from previously connected hosts? Decide how stale/offline content appears.
5. **How are project copies grouped?** Define identity, source lineage, names, and execution
   selection without merging independent state or creating a cloud project database.
6. **Who runs and pays for the internal agent?** Choose source/destination orchestration,
   provider credentials, cancellation, context access, and cost limits. Project content remains
   local to the hosts except for whatever provider requests the workflow actually requires.
7. **What does readiness require?** Define how the workflow chooses checks and reports a partial
   or platform-incompatible result. Decide how needed host-level elevation is handled.
8. **How is initial capture coordinated?** Define behavior when agents or services are writing,
   when the source disappears, and when a destination path already contains work.
9. **Does replication include thread history?** Decide whether history stays on the source,
   is explicitly copied, or supports a separate continuation handoff. Automatic run migration
   is not implied by project copying.
10. **How are retries and cross-host requests identified?** Define durable operation IDs,
    cancellation, duplicate suppression, and recovery without an AST command log.
11. **How is Relay operated?** Choose protocol, encryption/key binding, deployment, regions,
    reconnect behavior, bandwidth limits, and package cleanup on the hosts.
12. **What does revocation mean offline?** Define removal of full-access trust, propagation to
    directly connected peers, and treatment of already running processes. Remote data deletion
    is not an automatic consequence of removing a pairing.
13. **How does account-only rollout adopt existing hosts?** Attach identity and discovery to
    existing local data without uploading it or requiring a historical-content migration.

Previously open questions about cloud thread storage, continuous synchronization, restricted
pairing scopes, and blanket environment-file exclusions are settled by the product decisions
above. They should not be reintroduced as implicit prerequisites.

## Proposed implementation sequence

This is an implementation recommendation, not a commitment to build it in this change.

1. Define account discovery/enrollment and the full-trust pairing and offline identity contracts.
2. Add Relay tunnels alongside reachable direct routes, preserving peer authentication.
3. Extend client aggregation and project provenance where needed; keep Forge storage authoritative.
4. Add authenticated Forge-to-Forge calls and recoverable snapshot/transfer primitives.
5. Add the internal replication prompt, destination preparation, verification, and progress UI.
6. Add local-cache, revocation, and rollout behavior according to the remaining decisions.

Validation should cover unavailable hosts, offline direct operation, incorrect identities,
full-access peer calls, duplicate requests, source edits during capture, interrupted transfer,
partial setup, destination collisions, platform incompatibility, environment transfer, and
independent changes after copying. Verify recoverable user outcomes rather than only that
an agent completed its prompt.
