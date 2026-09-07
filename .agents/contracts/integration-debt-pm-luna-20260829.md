# Integration-first native port PM/architect lane

Act as the external PM/system architect for the native-product assembly lane
from exact clean local base `b74a5fef81ed25ffac28278afa6f84d1e789a48d`.

Provider/session policy is external GPT-5.6 Luna Max, Standard/non-fast, with
both multi-agent features disabled. Do not spawn descendants. You are a PM,
not an implementation worker: do not edit or commit any product, control,
handoff, manifest, or test file; do not publish, merge, launch processes, or
run Cargo/Bazel/native graphs. Your durable output is the final PM report
captured by the launcher.

## Mission

Correct the port conveyor from isolated TypeScript-parity leaves toward a
coherent native executable without discarding accepted work. Treat actual
current source and build files as authority. Read root `AGENTS.md`,
`.agents/PLAN.md`, `.agents/MODELS.md`, `docs/SESSION_HANDOFF.md`,
`docs/STACK.md`, `docs/plans/NATIVE_ENGINE_FIRST_WORKFLOW.md`, the packaging
task-force contract, and the existing E6/P5a/P5b contracts.

The already-audited high-risk facts to independently validate are:

- `editor` still enters `frontend::proof::run` rather than the native shell;
- `forge` still returns success without owning `ForgeApp`;
- frontend declares transport but does not compose a production client;
- P1-P4 package `bin/{ae,installer,editor,forge}` while live CLI/installer
  launch and retirement paths retain Broker/Node/Electron-era layout;
- root format/Clippy registration is manually drifting and four committed
  frontend tests currently fail Rust 1.98 rustfmt;
- 138 source-copy `#[path]` tests are useful leaf oracles but weak composition
  evidence;
- 238 registered worktrees require a preservation-aware retirement audit.

## Live worker supervision

The VP is launching the already-frozen E6 process-custody worker in a separate
clean worktree. Review its frozen contract and the current `ForgeApp`, binary
entrypoint, CLI coordination-lock precedent, Cargo/Bazel dependency surfaces,
and Windows/Unix filesystem semantics. Report any concrete contract defect
that must be corrected before candidate integration. Do not modify its files.

## Required architecture output

Return a compact implementation-ready dependency graph for the next native
assembly wave, prioritizing real production consumers. It must contain:

1. a smallest genuine gate-integrity PR that formats the four proven failures,
   makes package/root format and Clippy aggregates exhaustive, and adds an
   automated audit rejecting unregistered Rust source/test targets;
2. E6 process custody registration followed by the smallest honest Forge
   executable assembly packet that owns explicit configuration, custody,
   `ForgeApp::start`, readiness/service lifetime, graceful shutdown, and
   failure exit codes without inventing source-tree/default paths;
3. existing P5a/P5b native launch/retirement convergence in exact dependency
   order;
4. the first E7 production Editor transport-client composition that replaces
   the proof-only entry without pretending the whole shell is complete;
5. one E8 Editor -> authenticated transport -> Forge -> SQLite -> engine ->
   transcript workflow and failure/restart proof as the primary acceptance
   gate;
6. a bounded duplicate/type-state cleanup queue only where each cleanup is
   consumed by one of those production verticals;
7. a read-only classification strategy for the registered inactive worktrees,
   with exact evidence required before any retirement.

For every packet provide dependency, exact owned paths, no-touch paths,
public interface/invariants, meaningful tests, native-gate needs, conflict
surfaces reserved to the VP, and completion evidence. Reject policy-only or
presentation-only packets without a named production consumer. Separate what
is READY now from what is BLOCKED and name the exact unblocker.

End with: current stage counts, the three smallest executable next actions,
and concrete review criteria the VP should apply to the live E6 worker result.
