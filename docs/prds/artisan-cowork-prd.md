# Artisan Cowork PRD

Status: Proposed

Owner: Architecture

Last updated: 2026-07-26

## Product Definition

**Artisan Cowork** is an opt-in, API-native repository runtime.

When a repository enters Cowork, its canonical contents move into Artisan's
custody. The repository is no longer an ordinary host directory that Artisan
attempts to protect. Cowork stores canonical repository state in an encrypted
vault and exposes it only through authenticated application interfaces:

- the Artisan client reads and edits through typed Forge RPC;
- agent harnesses read and mutate through capability-scoped MCP tools; and
- commands, builds, tests, and other pathname-dependent tools run in ephemeral
  authorized execution environments.

Default Artisan remains harness-native. Harnesses use their normal worktree,
container, or VM behavior unless the user explicitly enables Cowork for a
repository.

## Decision

Cowork will not install a custom kernel filesystem driver or attempt to enforce
policy over an ordinary checkout.

Cowork will not expose its canonical repository as a writable host mount.

The canonical repository is an encrypted, service-owned data model. Filesystem
trees are projections created only for authorized execution sessions or
deliberate export.

This decision supersedes the earlier protected-directory/minifilter concept for
Cowork. Kernel-driver research remains useful historical threat-model input, but
it is not the selected product architecture.

## User Promise

When Cowork is enabled:

- canonical repository contents are not stored as an ordinary plaintext
  checkout;
- the Artisan client can browse, search, read, edit, diff, and review the
  repository normally;
- agents receive only the repository operations granted to their run;
- every mutation is revision-checked, durable, attributable, and reversible;
- arbitrary host processes cannot discover the canonical plaintext repository
  by opening a known workspace path;
- pathname-dependent commands execute only in an explicitly authorized
  environment; and
- the user can deliberately export or remove the repository from Cowork.

Cowork is not digital-rights management against the machine's administrator,
kernel compromise, memory inspection of an active authorized process, or copies
created before import.

## Terminology

- **Cowork Vault**: encrypted canonical repository storage plus durable metadata.
- **Cowork Repository**: one repository identity and its revision graph inside
  the vault.
- **Cowork Session**: a bounded authorization context for a client, agent, or
  execution environment.
- **Cowork Mutation**: one optimistic, transactional proposed change set.
- **Cowork MCP**: the capability-scoped MCP surface used by agent harnesses.
- **Cowork Execution**: an ephemeral materialization used for commands requiring
  ordinary pathname-based files.
- **Export**: a deliberate creation of an ordinary plaintext repository outside
  Cowork custody.

## Architecture

```text
Artisan Client                         Agent Harness
      |                                     |
Typed Forge RPC                         Cowork MCP
      |                                     |
      +------------------+------------------+
                         |
                         v
                CoworkRepository
                 Effect Service
                         |
          +--------------+------------------+
          |              |                  |
          v              v                  v
    SQLite WAL      Encrypted CAS      CoworkExecution
 metadata/state     repository blobs   ephemeral container / VM
```

### One domain service

Forge RPC and MCP are adapters over the same `CoworkRepository` capability.
They must not implement separate filesystem semantics, validation, authorization
logic, mutation behavior, or conflict rules.

The core capability must be modeled as Effect Services and Layers. All RPC, MCP,
storage, configuration, and execution inputs must be decoded with Effect Schema
before entering domain logic.

Conceptual operations:

```text
ListDirectory
ReadFile
ReadRevision
Search
SubscribeChanges

BeginMutation
WriteFile
ApplyPatch
DeletePath
MovePath
CommitMutation
AbortMutation

CreateExecution
RunCommand
CaptureExecutionChanges
DestroyExecution

ImportRepository
ExportRepository
RemoveRepository
```

Names are illustrative. The implementation must derive client/server request and
result types from one authoritative contract rather than duplicating unions and
facades.

## Storage Model

### SQLite WAL

SQLite owns transactional metadata and coordination:

- repository identities;
- revisions and parent relationships;
- path trees;
- content identities;
- mutations and lifecycle;
- optimistic concurrency;
- sessions and capability grants;
- execution leases;
- audit records;
- Git references and synchronization state;
- import/export state; and
- garbage-collection reachability.

SQLite does not need to store every large file body in table rows.

### Encrypted content-addressed storage

File contents and large repository objects live in an encrypted
content-addressed store. Plaintext content identity may be used internally only
if its information-leak implications are accepted; otherwise Cowork must use a
keyed or separately blinded storage identifier.

Required properties:

- authenticated encryption;
- per-repository or hierarchically derived keys;
- key versioning and rotation;
- atomic blob publication;
- integrity verification on every read;
- deduplication policy that does not leak content across security boundaries;
- reachability-based garbage collection;
- bounded streaming reads and writes; and
- no plaintext temporary files outside an authorized execution lifecycle.

Vault database files, encrypted blobs, indexes, logs, diagnostics, and backups
must never contain plaintext source content or secrets.

### Revision model

Every committed repository state has an immutable revision identity. A mutation
names its base revision and expected content identities.

Concurrent mutation never silently overwrites newer state. Commit produces one
of:

```text
Committed
AlreadyCommitted
Conflict
Rejected
```

Conflict output must identify affected paths and current/base identities without
discarding either proposed change.

Multi-file mutations commit atomically at the Cowork metadata level.

## Artisan Client Surface

The Artisan client is a trusted Cowork client, not a host-filesystem consumer.
It accesses Cowork through the same typed Forge transport used by the rest of
the application.

"Trusted" does not mean implicitly authorized by loopback location or possession
of a Forge URL. Every client connection must authenticate an audience-bound
session and receive repository, revision, operation, and content-size grants.
Authorization and revocation are enforced inside `CoworkRepository`, not only at
the RPC router. Forge's local endpoint and credential storage must prevent an
unrelated same-user process from replaying a client session.

The client must support:

- virtual directory navigation;
- bounded text and binary reads;
- repository search;
- syntax-highlightable content streams;
- direct editing through Cowork mutations;
- diffs, revisions, history, and undo;
- live mutation and execution updates;
- conflicts and resolution;
- import, export, and removal workflows; and
- explicit indication that the repository is Cowork-managed.

An unauthenticated, expired, revoked, other-user, or other-session client must
not be able to list repository identities, infer paths, read content, subscribe
to changes, or begin mutations.

The UI must never fabricate local paths for vault content. A Cowork URI or opaque
repository/path identity may be used internally, but it is not a promise that an
ordinary host path exists.

## Cowork MCP Surface

Agent harnesses access Cowork through an MCP server owned by Forge.

Initial tool vocabulary:

```text
cowork.list
cowork.read
cowork.search
cowork.diff
cowork.begin_mutation
cowork.apply_patch
cowork.write
cowork.move
cowork.delete
cowork.commit
cowork.abort
cowork.run
```

The exact MCP shape must follow the current MCP specification and the target
harness's native integration model.

### Capability grants

Every MCP connection is bound to a durable Cowork Session containing:

- repository identity;
- base revision;
- agent, run, and thread identities;
- readable path scopes;
- writable path scopes;
- allowed operations;
- content and result-size limits;
- expiration and revocation state;
- execution permissions;
- network and secret policy for command execution; and
- audit correlation.

The model is deny-by-default. Capability checks occur inside
`CoworkRepository`, not only in the MCP adapter.

Possession of a generic MCP endpoint URL is not authorization. Connections
require an unguessable, short-lived, audience-bound credential delivered through
the harness's trusted launch/configuration boundary.

### Read behavior

Reads are revision-aware. A harness can request a stable base revision while
other sessions continue working. Large files use bounded streams rather than
unbounded MCP payloads.

### Mutation behavior

Agent writes enter an explicit mutation. The agent can inspect its proposed diff
before commit. Commit compares the mutation's base and expected identities with
current state.

An agent never receives an unrestricted "write anywhere" host filesystem tool
merely because Cowork is enabled.

## Commands, Builds, and Tests

MCP file tools are sufficient for repository inspection and mutation, but
compilers, package managers, Git implementations, language servers, and test
runners require pathname-based files.

Cowork provides `cowork.run` through `CoworkExecution`:

1. authorize an exact repository revision and execution policy;
2. create an ephemeral worktree, container, or VM;
3. materialize only the required repository state;
4. run the command with explicit time, CPU, memory, disk, process, network,
   secret, and output limits;
5. stream structured stdout, stderr, lifecycle, and artifact events;
6. capture resulting repository changes as a proposed Cowork mutation;
7. return the diff and execution evidence; and
8. revoke the execution lease, destroy its encryption key, and tear down the
   execution environment.

Managed Artisan harnesses and rented VMs are the strongest Cowork execution
profile. Managed execution must place repository plaintext, temporary files,
caches, swap, snapshots, and command artifacts only on storage governed by the
execution's encryption and snapshot policy. Teardown claims cryptographic
inaccessibility by destroying the execution key; it does not claim physical
erasure from SSDs or provider media.

A local process-backed profile may be offered later, but it must state that
plaintext exists in a temporary local execution area and cannot claim the same
isolation or teardown guarantee as a managed VM. It cannot satisfy the strong
Cowork completion gate.

An expired, revoked, or destroyed execution lease can never reopen or reuse its
materialized projection.

Cowork must not pretend that an arbitrary local shell can operate on an
API-only repository without materialization.

## Git Model

Git is a Cowork capability, not a requirement that the client can open a host
`.git` directory.

Cowork is the canonical storage authority. The Git object graph is a versioned
domain inside the same vault, not a second canonical filesystem:

- immutable Git objects are encrypted and published to the CAS before reference
  updates;
- SQLite owns Git references and their mapping to Cowork revisions;
- Cowork revision commits and Git reference changes that belong to one operation
  commit in one SQLite transaction;
- a crash may leave unreachable immutable objects for garbage collection, but
  must never expose a Git reference without its object or split a committed
  reference from its Cowork revision; and
- Cowork working revisions may exist without a Git commit, but their relationship
  to the current Git tree is explicit and queryable.

Conceptual operations:

```text
Status
Diff
CreateBranch
SwitchRevision
Commit
Fetch
Push
Merge
Rebase
ResolveConflict
```

The implementation may:

- store and operate Git objects within Cowork-controlled storage;
- execute Git inside an ephemeral authorized environment; or
- use an existing safe Git library after capability research.

The selected implementation must preserve Git object/ref semantics, remote
authentication boundaries, atomic ref updates, cancellation, progress, and
recovery. Crash-injection tests must cover every boundary between immutable
object publication, SQLite revision commit, and Git reference update.

## Import

Import is an explicit custody transition:

1. enter `Importing` and inspect and validate the source repository;
2. establish repository and encryption identities;
3. ingest files, Git objects, references, and relevant metadata;
4. verify a complete Cowork revision against the source;
5. enter `ImportedUnprotected` while the original source remains;
6. present an exact deletion or retained-unprotected-export choice;
7. complete the selected source disposition and enter `CoworkManaged`; and
8. record the custody transition durably.

`ImportedUnprotected` is a verified staging state, not enabled Cowork protection.
The product must not claim protected custody while the original checkout remains
an unresolved second authority. If the user retains it, it becomes an explicitly
labelled unprotected export snapshot; changes to it do not mutate Cowork unless
the user deliberately imports them through a new mutation.

Every state transition is crash-safe and resumable. At no point may both the
source checkout and Cowork Vault be presented as canonical.

Cowork cannot guarantee secure deletion of the original repository, especially
on SSDs, snapshots, backups, or synchronized storage. Product language must say
that Cowork protects canonical state after import, not that all historical
plaintext copies have been erased.

Import never destroys the source without a separate explicit user confirmation
after successful verification.

## Export and Removal

Export deliberately creates plaintext outside Cowork:

- choose exact revision and destination;
- fail on a nonempty or conflicting destination;
- materialize atomically where the platform permits;
- verify resulting content and Git state;
- report that the export is outside Cowork protection; and
- retain Cowork canonical state unless removal is separately confirmed.

Removing a repository from Cowork requires:

1. no active mutation or execution sessions;
2. a verified export or explicit destructive confirmation;
3. remote synchronization status disclosure;
4. key/blob/database cleanup scheduling; and
5. a durable audit record.

Deletion of encryption keys is the primary cryptographic erasure mechanism.
Physical media erasure is not guaranteed.

## Security Model

### Cowork protects against

- casual or unauthorized local processes discovering canonical plaintext by
  opening an ordinary repository path;
- accidental writes outside Cowork's mutation protocol;
- conflicting agent mutations;
- partial multi-file metadata commits;
- tampering with encrypted blobs;
- unbounded agent file operations;
- stale or replayed mutation commits;
- MCP clients operating outside granted path and operation scopes; and
- plaintext persistence after an authorized execution is successfully
  destroyed, subject to the execution platform's guarantees.

### Cowork does not protect against

- machine administrators or kernel compromise;
- memory inspection of an active authorized client, Forge, MCP session, or
  execution environment;
- an authorized agent intentionally exfiltrating readable content through
  network access, tool output, prompts, or artifacts;
- copies, backups, logs, or snapshots created before Cowork import;
- compromise of Cowork's encryption keys;
- weaknesses in a local execution profile that are absent from managed VMs; or
- third-party services to which the user deliberately exports or pushes data.

Network and secret policy are therefore part of an execution capability. File
authorization alone does not prevent an authorized agent from exfiltrating
content it may read.

## Availability and Recovery

The vault is canonical, so recovery is a product-critical boundary.

Required:

- SQLite WAL recovery and integrity checks;
- authenticated blob verification;
- backup and restore with key recovery;
- schema and encryption-version migrations;
- mutation and execution lease recovery after Forge restart;
- incomplete import/export recovery;
- corruption detection with fail-closed writes;
- explicit degraded read-only mode where safe;
- observable garbage collection; and
- disaster-recovery documentation verified on real backups.

No single local database file may be the user's only recoverable copy of a
repository.

## Relationship to Default Artisan

Default mode:

```text
HarnessNative
```

- harness owns its worktree/container/VM;
- ordinary repositories remain ordinary;
- Artisan observes and orchestrates;
- Cowork services are not composed.

Cowork mode:

```text
CoworkManaged
```

- vault owns canonical repository state;
- client uses Forge RPC;
- harness uses Cowork MCP;
- commands use Cowork Execution;
- import/export are explicit custody boundaries.

The mode is a tagged repository policy, not a loose boolean spread through the
application. A repository cannot be simultaneously canonical in an ordinary
checkout and Cowork Vault.

## Relationship to Existing Native File-Store Work

`@artisan/bounded-file-store-native` is not the foundation of Cowork.

That package attempts to make conditional mutations safe inside an ordinary
Windows NTFS checkout. Cowork removes the ordinary checkout from the canonical
storage boundary, so symlink, hard-link, reparse, and pathname publication races
are no longer the primary mutation architecture.

The code-quality rehabilitation milestone removes the dormant native package,
adapter, build, packaging, and native smoke-test surface. The broader
shared-checkout mutation architecture is audited separately so useful generic
file behavior is not deleted accidentally. The native package must not be
retained or completed under the claim that Cowork requires it.

## Product Experience

Enabling Cowork must use explicit custody language:

> Import this repository into Artisan Cowork?
>
> Cowork will store the canonical repository in its encrypted vault. Artisan can
> read and edit it normally. Agents access it through scoped MCP tools, and
> commands run in authorized temporary environments. External applications will
> not receive an ordinary repository folder unless you export one.

The repository surface must always show:

- Cowork status;
- canonical revision;
- synchronization state;
- active sessions and executions;
- pending mutations;
- vault backup health; and
- export/remove actions.

## Delivery Phases

### Phase 0: Decision records and threat model

- Approve custody, encryption, key-recovery, and administrator threat-model
  decisions.
- Define the MCP capability and execution isolation contracts.
- Specify the Cowork-authoritative Git object/ref mapping, atomic update
  protocol, and remote synchronization semantics.

### Phase 1: Read-only vault

- Import without deleting the source.
- Expose the result as `ImportedUnprotected`, never as protected
  `CoworkManaged`.
- Encrypted CAS plus SQLite revision/path metadata.
- Client browse, read, search, diff, and integrity verification.
- Backup and restore proof.

### Phase 2: Transactional mutation

- Client mutations and optimistic conflicts.
- Revision subscriptions, history, undo, and multi-file commits.
- Crash/restart and corruption recovery.

### Phase 3: Cowork MCP

- Read-only capability sessions.
- Mutation tools with scoped paths and explicit commits.
- Revocation, expiry, size limits, and complete audit evidence.

### Phase 4: Cowork Execution

- Managed VM/container materialization.
- Structured command lifecycle.
- Change capture back into proposed mutations.
- Destruction and plaintext-residue verification.

### Phase 5: Git and custody lifecycle

- Fetch, push, branch, commit, merge, and conflict behavior.
- Verified export and removal.
- Key rotation, garbage collection, backup recovery, and operational tooling.

## Completion Gates

Cowork is not complete until:

- the canonical repository can be reconstructed from a verified vault backup;
- canonical plaintext is absent from ordinary host repository paths;
- `CoworkManaged` cannot be entered until import verification and explicit source
  disposition complete, and crash injection never produces dual canonical
  authority;
- the client can browse, read, search, edit, diff, and resolve conflicts without
  a host checkout;
- client RPC sessions enforce authentication, audience, repository, revision,
  operation, size, expiry, and revocation scopes inside the domain service, and
  negative tests prove unrelated clients cannot enumerate or read vault state;
- MCP grants enforce repository, revision, path, operation, size, expiry, and
  execution scopes inside the domain service;
- concurrent mutations cannot silently overwrite each other;
- every mutation and execution is attributable and auditable;
- managed commands execute on per-execution encrypted storage, return changes as
  proposed Cowork mutations, and become cryptographically inaccessible after
  lease revocation and key destruction;
- an expired or destroyed execution cannot reopen its materialized projection;
- Git and Cowork revision authority cannot silently diverge;
- import, export, removal, restart, backup, restore, key rotation, and corruption
  scenarios pass destructive integration tests;
- default harness-native startup never initializes vault keys/storage, Cowork
  MCP routes, client Cowork routes, or execution infrastructure; and
- no kernel driver or writable canonical filesystem mount is required.

## Open Decisions

- Per-repository versus hierarchical vault encryption keys.
- Key recovery and user-account recovery model.
- Whether encrypted deduplication may cross repository boundaries.
- Git library or execution implementation beneath the Cowork-authoritative
  object/ref model.
- Local execution profile and its exact reduced security promise.
- Managed VM provider and snapshot/destruction guarantees.
- MCP transport, credential delivery, and compatibility per harness.
- Maximum repository, blob, mutation, and execution sizes.
- Binary, large-file, submodule, symlink, executable-bit, and sparse-checkout
  semantics.
- Backup destinations and whether remote Cowork storage is part of the initial
  product.
